use anyhow::{bail, Result};

/// Open native OS folder selection dialog (File Manager)
pub fn pick_folder_dialog(initial_path: Option<&str>) -> Result<Option<String>> {
    let _ = &initial_path;
    #[cfg(target_os = "linux")]
    {
        // 1. Try zenity (standard on GNOME, Fedora, Ubuntu, Sway/Wayland, etc.)
        let mut cmd = std::process::Command::new("zenity");
        crate::agent::engine::configure_subprocess(&mut cmd);
        cmd.arg("--file-selection")
            .arg("--directory")
            .arg("--title=Select Workspace Folder")
            .arg("--modal")
            .arg("--width=700")
            .arg("--height=500");

        if let Some(init) = initial_path {
            let trimmed = init.trim();
            if !trimmed.is_empty() {
                let formatted = if trimmed.ends_with('/') {
                    trimmed.to_string()
                } else {
                    format!("{}/", trimmed)
                };
                cmd.arg(format!("--filename={}", formatted));
            }
        }

        if let Ok(output) = cmd.output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !stdout.is_empty() {
                    return Ok(Some(stdout));
                }
            } else if output.status.code() == Some(1) {
                // Exit code 1 in zenity means the user clicked Cancel or closed the window
                return Ok(None);
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                tracing::warn!("[pick_folder_dialog] Zenity non-zero exit ({:?}): {}", output.status.code(), stderr);
            }
        }

        // 2. Try kdialog (standard on KDE Plasma)
        let mut kcmd = std::process::Command::new("kdialog");
        crate::agent::engine::configure_subprocess(&mut kcmd);
        kcmd.arg("--getexistingdirectory");
        if let Some(init) = initial_path {
            let trimmed = init.trim();
            if !trimmed.is_empty() {
                kcmd.arg(trimmed);
            }
        }
        if let Ok(output) = kcmd.output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !stdout.is_empty() {
                    return Ok(Some(stdout));
                }
            } else if output.status.code() == Some(1) {
                return Ok(None);
            }
        }

        // 3. Fallback: Python3 tkinter filedialog
        let mut py_cmd = std::process::Command::new("python3");
        crate::agent::engine::configure_subprocess(&mut py_cmd);
        let py_script = r#"
import tkinter as tk
from tkinter import filedialog
import sys

try:
    root = tk.Tk()
    root.withdraw()
    root.attributes('-topmost', True)
    root.lift()
    root.focus_force()
    initial = sys.argv[1] if len(sys.argv) > 1 and sys.argv[1].strip() else None
    folder = filedialog.askdirectory(initialdir=initial, title="Select Workspace Folder")
    if folder:
        print(folder)
except Exception:
    sys.exit(2)
"#;
        py_cmd.arg("-c").arg(py_script);
        if let Some(init) = initial_path {
            py_cmd.arg(init.trim());
        }
        if let Ok(output) = py_cmd.output()
            && output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !stdout.is_empty() {
                return Ok(Some(stdout));
            }
            return Ok(None);
        }

        bail!("Unable to open file manager dialog (zenity, kdialog, or tkinter). Ensure GUI is available or enter the folder path manually.")
    }

    #[cfg(target_os = "windows")]
    {
        let selected_path_init = if let Some(init) = initial_path {
            let trimmed = init.trim().replace('"', "`\"");
            if !trimmed.is_empty() {
                format!("$dialog.SelectedPath = \"{}\"", trimmed)
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let ps_script = format!(
            r#"
        Add-Type -AssemblyName System.Windows.Forms
        $form = New-Object System.Windows.Forms.Form
        $form.TopMost = $true
        $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
        $dialog = New-Object System.Windows.Forms.FolderBrowserDialog
        $dialog.Description = "Select Workspace Folder"
        $dialog.ShowNewFolderButton = $true
        {}
        if ($dialog.ShowDialog($form) -eq [System.Windows.Forms.DialogResult]::OK) {{
            Write-Output $dialog.SelectedPath
        }}
        $form.Dispose()
        "#,
            selected_path_init
        );
        let mut cmd = std::process::Command::new("powershell");
        crate::agent::engine::configure_subprocess(&mut cmd);
        cmd.arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-WindowStyle")
            .arg("Hidden")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(ps_script);
        let output = cmd.output()?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !stdout.is_empty() {
                return Ok(Some(stdout));
            }
        }
        Ok(None)
    }

    #[cfg(target_os = "macos")]
    {
        let script = if let Some(init) = initial_path {
            let trimmed = init.trim().replace('"', "\\\"");
            if !trimmed.is_empty() {
                format!(
                    r#"tell current application
activate
return POSIX path of (choose folder with prompt "Select Workspace Folder" default location (POSIX file "{}"))
end tell"#,
                    trimmed
                )
            } else {
                r#"tell current application
activate
return POSIX path of (choose folder with prompt "Select Workspace Folder")
end tell"#.to_string()
            }
        } else {
            r#"tell current application
activate
return POSIX path of (choose folder with prompt "Select Workspace Folder")
end tell"#.to_string()
        };
        let mut cmd = std::process::Command::new("osascript");
        crate::agent::engine::configure_subprocess(&mut cmd);
        cmd.arg("-e").arg(&script);
        let output = cmd.output()?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !stdout.is_empty() {
                return Ok(Some(stdout));
            }
        }
        Ok(None)
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        bail!("This operating system does not support native folder dialogs yet.")
    }
}

/// Open an existing folder or file directly in the OS file manager (e.g. Thunar, Nautilus, Explorer, Finder)
pub fn open_path_in_file_manager(path_str: &str) -> Result<()> {
    let p = std::path::Path::new(path_str);
    if !p.exists() {
        bail!("Folder or file not found on disk: {}", path_str);
    }

    #[cfg(target_os = "linux")]
    {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(path_str);
        crate::agent::engine::configure_subprocess(&mut cmd);
        cmd.spawn()?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("explorer.exe");
        cmd.arg(path_str);
        crate::agent::engine::configure_subprocess(&mut cmd);
        cmd.spawn()?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        cmd.arg(path_str);
        crate::agent::engine::configure_subprocess(&mut cmd);
        cmd.spawn()?;
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        bail!("This platform does not support opening file manager natively yet.")
    }
}