use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::thread;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PythonInfo {
    pub installed: bool,
    pub version: String,
    pub path: Option<String>,
    pub is_installing: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct PythonState {
    pub is_installing: bool,
    pub last_error: Option<String>,
}

pub static PYTHON_STATE: Mutex<Option<PythonState>> = Mutex::new(None);

/// Create a Command that never flashes a console window on Windows (CREATE_NO_WINDOW).
fn hidden_command<S: AsRef<std::ffi::OsStr>>(program: S) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd
}

/// Locate Python 3 executable path in candidate locations or system PATH.
pub fn find_python_bin() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        // Check Windows standard installation directories
        if let Ok(local_data) = std::env::var("LOCALAPPDATA") {
            let programs_python = PathBuf::from(&local_data).join("Programs").join("Python");
            if programs_python.is_dir()
                && let Ok(entries) = std::fs::read_dir(&programs_python) {
                    for entry in entries.flatten() {
                        let candidate = entry.path().join("python.exe");
                        if candidate.is_file() {
                            return Some(candidate);
                        }
                    }
                }
        }
        if let Ok(prog_files) = std::env::var("ProgramFiles") {
            let pf_python = PathBuf::from(&prog_files).join("Python");
            if pf_python.is_dir()
                && let Ok(entries) = std::fs::read_dir(&pf_python) {
                    for entry in entries.flatten() {
                        let candidate = entry.path().join("python.exe");
                        if candidate.is_file() {
                            return Some(candidate);
                        }
                    }
                }
        }

        // Try finding via `where.exe python`
        if let Ok(output) = hidden_command("where.exe").arg("python").output()
            && output.status.success() {
                let out_str = String::from_utf8_lossy(&output.stdout);
                for line in out_str.lines() {
                    let trimmed = line.trim();
                    // Avoid Windows Store dummy redirects
                    if !trimmed.is_empty() && !trimmed.contains("WindowsApps") {
                        let p = PathBuf::from(trimmed);
                        if p.is_file() {
                            return Some(p);
                        }
                    }
                }
            }

        // Try Python launcher `py.exe`
        if let Ok(output) = hidden_command("where.exe").arg("py").output()
            && output.status.success() {
                let out_str = String::from_utf8_lossy(&output.stdout);
                if let Some(first_line) = out_str.lines().next() {
                    let trimmed = first_line.trim();
                    if !trimmed.is_empty() {
                        let p = PathBuf::from(trimmed);
                        if p.is_file() {
                            return Some(p);
                        }
                    }
                }
            }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let candidates = [
            "/usr/bin/python3",
            "/usr/local/bin/python3",
            "/opt/homebrew/bin/python3",
            "/usr/bin/python",
            "/usr/local/bin/python",
        ];
        for c in &candidates {
            let p = PathBuf::from(c);
            if p.is_file() {
                return Some(p);
            }
        }

        // Try finding via `which python3` or `which python`
        if let Ok(output) = hidden_command("which").arg("python3").output()
            && output.status.success()
        {
            let out_str = String::from_utf8_lossy(&output.stdout);
            if let Some(first_line) = out_str.lines().next() {
                let trimmed = first_line.trim();
                if !trimmed.is_empty() {
                    let p = PathBuf::from(trimmed);
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }

        if let Ok(output) = hidden_command("which").arg("python").output()
            && output.status.success()
        {
            let out_str = String::from_utf8_lossy(&output.stdout);
            if let Some(first_line) = out_str.lines().next() {
                let trimmed = first_line.trim();
                if !trimmed.is_empty() {
                    let p = PathBuf::from(trimmed);
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }

    None
}

/// Query Python status and installed version if available.
pub fn get_python_info() -> PythonInfo {
    let (is_installing, last_error) = {
        let state_lock = PYTHON_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref st) = *state_lock {
            (st.is_installing, st.last_error.clone())
        } else {
            (false, None)
        }
    };

    let py_path = find_python_bin();
    if let Some(ref bin) = py_path {
        let output = hidden_command(bin).arg("--version").output();
        if let Ok(out) = output {
            let stdout_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr_str = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let ver = if !stdout_str.is_empty() { stdout_str } else { stderr_str };

            if !ver.is_empty() {
                return PythonInfo {
                    installed: true,
                    version: ver,
                    path: Some(bin.to_string_lossy().to_string()),
                    is_installing,
                    error: last_error,
                };
            }
        }
    }

    PythonInfo {
        installed: false,
        version: String::new(),
        path: None,
        is_installing,
        error: last_error,
    }
}

/// Trigger non-blocking installer or dependency check for Python in background.
pub fn ensure_python_installed_background() {
    let current = get_python_info();
    if current.installed {
        crate::term_log!("[Python] Python runtime detected: {} ({:?})", current.version, current.path);
        return;
    }

    {
        let mut lock = PYTHON_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let st = lock.get_or_insert_with(PythonState::default);
        if st.is_installing {
            crate::term_log!("[Python] Python check/install is already running in background...");
            return;
        }
        st.is_installing = true;
        st.last_error = None;
    }

    crate::term_log!("[Python] Python runtime not detected. Initiating automated setup...");

    thread::spawn(move || {
        let install_result = run_python_installer();

        let mut lock = PYTHON_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let st = lock.get_or_insert_with(PythonState::default);
        st.is_installing = false;

        match install_result {
            Ok(ver) => {
                st.last_error = None;
                crate::term_log!("[Python] Python successfully setup! Version: {}", ver);
            }
            Err(e) => {
                crate::term_err!("[Python] Installation note: {}", e);
                st.last_error = Some(e);
            }
        }
    });
}

/// Execute package manager or installer per operating system.
pub fn run_python_installer() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        crate::term_log!("[Python] Attempting winget install for Python 3.12...");
        let output = hidden_command("winget")
            .args(["install", "-e", "--id", "Python.Python.3.12", "--accept-package-agreements", "--accept-source-agreements"])
            .output();

        if let Ok(out) = output
            && out.status.success() {
                let info = get_python_info();
                if info.installed {
                    return Ok(info.version);
                }
            }
        Err("Please install Python 3 using 'winget install Python.Python.3.12' or from python.org".to_string())
    }

    #[cfg(target_os = "macos")]
    {
        crate::term_log!("[Python] Attempting brew install python3...");
        let output = hidden_command("brew")
            .args(["install", "python3"])
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                let info = get_python_info();
                if info.installed {
                    return Ok(info.version);
                }
            }
        }
        return Err("Please install Python 3 using 'brew install python3' or from python.org".to_string());
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        // Try dnf (Fedora/RHEL)
        if hidden_command("which").arg("dnf").output().map(|o| o.status.success()).unwrap_or(false) {
            crate::term_log!("[Python] Detected Fedora/RHEL. Attempting dnf install python3...");
            let _ = hidden_command("pkexec").args(["dnf", "install", "-y", "python3"]).output();
        } else if hidden_command("which").arg("apt").output().map(|o| o.status.success()).unwrap_or(false) {
            crate::term_log!("[Python] Detected Debian/Ubuntu. Attempting apt install python3...");
            let _ = hidden_command("pkexec").args(["apt", "install", "-y", "python3"]).output();
        } else if hidden_command("which").arg("pacman").output().map(|o| o.status.success()).unwrap_or(false) {
            crate::term_log!("[Python] Detected Arch Linux. Attempting pacman install python...");
            let _ = hidden_command("pkexec").args(["pacman", "-S", "--noconfirm", "python"]).output();
        }

        let info = get_python_info();
        if info.installed {
            Ok(info.version)
        } else {
            Err("Python 3 not detected. Please install 'python3' using your distribution package manager (e.g. sudo dnf install python3 / sudo apt install python3).".to_string())
        }
    }
}
