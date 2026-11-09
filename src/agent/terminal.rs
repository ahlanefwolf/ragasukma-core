use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Supported shell types across Linux, Windows, and macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellType {
    /// Auto-detect based on environment configuration and host platform
    Auto,
    /// Windows PowerShell 5.1 (`powershell.exe`)
    PowerShell,
    /// Modern PowerShell 7+ (`pwsh`)
    Pwsh,
    /// Bourne-Again SHell (`/bin/bash` or Git Bash on Windows)
    Bash,
    /// Z Shell (`zsh`)
    Zsh,
    /// Windows Command Prompt (`cmd.exe`)
    Cmd,
    /// POSIX Standard Shell (`sh`)
    Sh,
}

impl ShellType {
    /// Parse shell identifier string from tool arguments or configuration.
    pub fn parse(s: &str) -> Option<Self> {
        let norm = s.trim().to_lowercase();
        match norm.as_str() {
            "auto" | "default" => Some(ShellType::Auto),
            "powershell" | "powershell.exe" | "ps" | "posh" => Some(ShellType::PowerShell),
            "pwsh" | "pwsh.exe" | "powershell-core" | "powershell_core" => Some(ShellType::Pwsh),
            "bash" | "bash.exe" | "gitbash" | "git-bash" | "git_bash" => Some(ShellType::Bash),
            "zsh" | "zsh.exe" => Some(ShellType::Zsh),
            "cmd" | "cmd.exe" | "batch" => Some(ShellType::Cmd),
            "sh" | "posix" => Some(ShellType::Sh),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ShellType::Auto => "auto",
            ShellType::PowerShell => "powershell",
            ShellType::Pwsh => "pwsh",
            ShellType::Bash => "bash",
            ShellType::Zsh => "zsh",
            ShellType::Cmd => "cmd",
            ShellType::Sh => "sh",
        }
    }

    /// Whether this shell type uses POSIX command chaining (`&&`)
    pub fn is_posix_chaining(&self) -> bool {
        match self {
            ShellType::Bash | ShellType::Zsh | ShellType::Sh => true,
            ShellType::PowerShell => false,
            // PowerShell 7 (pwsh) and Cmd both support &&
            ShellType::Pwsh | ShellType::Cmd => true,
            ShellType::Auto => {
                #[cfg(target_os = "windows")]
                {
                    false
                }
                #[cfg(not(target_os = "windows"))]
                {
                    true
                }
            }
        }
    }
}

/// Information describing the active shell for agent system prompts.
#[derive(Debug, Clone)]
pub struct ShellInfo {
    pub shell_type: ShellType,
    pub description: String,
    pub script_runner_desc: &'static str,
    pub multiple_cmd_hint: String,
}

/// Resolves the default shell type by inspecting environment variables and host defaults.
pub fn resolve_default_shell() -> ShellType {
    // 1. Honor explicit user override via environment variable
    if let Ok(env_shell) = std::env::var("RAGASUKMA_SHELL")
        && let Some(st) = ShellType::parse(&env_shell)
        && st != ShellType::Auto
    {
        return st;
    }

    // 2. Platform default
    #[cfg(target_os = "windows")]
    {
        // On Windows, if pwsh is available on PATH, prefer it; otherwise fallback to powershell
        if find_in_path("pwsh.exe").is_some() || find_in_path("pwsh").is_some() {
            ShellType::Pwsh
        } else {
            ShellType::PowerShell
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(shell_var) = std::env::var("SHELL") {
            if shell_var.ends_with("zsh") {
                return ShellType::Zsh;
            } else if shell_var.ends_with("bash") {
                return ShellType::Bash;
            }
        }
        if Path::new("/bin/bash").exists() {
            ShellType::Bash
        } else {
            ShellType::Sh
        }
    }
}

/// Inspects the environment to provide human-readable system prompt guidance for the agent.
pub fn active_shell_info() -> ShellInfo {
    let resolved = resolve_default_shell();

    #[cfg(target_os = "windows")]
    let (description, script_runner_desc, multiple_cmd_hint) = match resolved {
        ShellType::Bash => (
            "Windows Git Bash (POSIX login shell with enriched PATH). Use standard Unix/Linux shell syntax and forward slashes.".to_string(),
            "Scripts: Node.js (`node`) or Python (`python` / `python3`)",
            "When running sequential terminal commands, combine them into a single execution (chain with '&&') to save turn latency.".to_string(),
        ),
        ShellType::Pwsh => (
            "Windows PowerShell 7 Core (`pwsh`). Use modern PowerShell syntax. Supports '&&' for chaining commands.".to_string(),
            "Scripts: Node.js (`node`) or Python (`python` / `py -3`)",
            "When running sequential terminal commands, combine them into a single execution (chain with '&&' or ';') to save turn latency.".to_string(),
        ),
        _ => (
            "Windows PowerShell. Use native PowerShell syntax for scripts and paths. Note: you can specify `shell: 'bash'` in `workspace_run_command` if Git Bash is preferred.".to_string(),
            "Scripts: Node.js (`node`) or Python (`python` / `py -3`)",
            "When running sequential terminal commands, combine them into a single execution to save turn latency: use ';' (e.g. `cmd1 ; cmd2`) on Windows PowerShell instead of '&&' (or specify `shell: 'bash'` for POSIX '&&').".to_string(),
        ),
    };

    #[cfg(target_os = "macos")]
    let (description, script_runner_desc, multiple_cmd_hint) = (
        "macOS POSIX Shell (login shell with enriched PATH). Use standard Unix shell syntax.".to_string(),
        "Scripts: Node.js (`node`) or Python (`python3`)",
        "When running sequential terminal commands, combine them into a single execution (chain with '&&') to save turn latency.".to_string(),
    );

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let (description, script_runner_desc, multiple_cmd_hint) = (
        "Linux POSIX Shell (login shell with enriched PATH). Use standard Unix/Linux shell syntax.".to_string(),
        "Scripts: Node.js (`node`) or Python (`python3`)",
        "When running sequential terminal commands, combine them into a single execution (chain with '&&') to save turn latency.".to_string(),
    );

    ShellInfo {
        shell_type: resolved,
        description,
        script_runner_desc,
        multiple_cmd_hint,
    }
}

/// Helper to search for an executable binary in the system PATH.
pub fn find_in_path(executable: &str) -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(executable);
            if candidate.is_file() {
                return Some(candidate);
            }
            #[cfg(target_os = "windows")]
            {
                if !executable.ends_with(".exe") {
                    let exe_candidate = dir.join(format!("{}.exe", executable));
                    if exe_candidate.is_file() {
                        return Some(exe_candidate);
                    }
                }
            }
        }
    }
    None
}

/// Resolves Git Bash executable path on Windows.
#[cfg(target_os = "windows")]
pub fn find_git_bash() -> Option<PathBuf> {
    // 1. PATH search
    if let Some(p) = find_in_path("bash.exe") {
        return Some(p);
    }

    // 2. Standard installation paths
    let candidates = [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files\Git\usr\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\usr\bin\bash.exe",
    ];

    for candidate in &candidates {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }

    // 3. AppData / Per-user paths
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let p = PathBuf::from(local).join(r"Programs\Git\bin\bash.exe");
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(profile).join(r"AppData\Local\Programs\Git\bin\bash.exe");
        if p.is_file() {
            return Some(p);
        }
    }

    None
}

/// Builds an enriched PATH string containing common package manager and tool directories.
pub fn enriched_path() -> std::ffi::OsString {
    let current_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<PathBuf> = std::env::split_paths(&current_path).collect();

    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            paths.insert(0, PathBuf::from(appdata).join("npm"));
        }
        if let Ok(userprofile) = std::env::var("USERPROFILE") {
            paths.insert(0, PathBuf::from(&userprofile).join(".cargo").join("bin"));
            paths.insert(0, PathBuf::from(&userprofile).join(".bun").join("bin"));
        }
        paths.insert(0, PathBuf::from(r"C:\Program Files\nodejs"));

        // Add Git cmd / bin to PATH if present
        let git_cmd_candidates = [
            PathBuf::from(r"C:\Program Files\Git\cmd"),
            PathBuf::from(r"C:\Program Files\Git\bin"),
            PathBuf::from(r"C:\Program Files\Git\usr\bin"),
            PathBuf::from(r"C:\Program Files (x86)\Git\cmd"),
        ];
        for candidate in git_cmd_candidates {
            if candidate.is_dir() && !paths.contains(&candidate) {
                paths.insert(0, candidate);
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(home) = std::env::var("HOME") {
            let home_path = PathBuf::from(&home);
            paths.insert(0, home_path.join(".local").join("bin"));
            paths.insert(0, home_path.join(".cargo").join("bin"));
            paths.insert(0, home_path.join(".bun").join("bin"));
            paths.insert(0, home_path.join(".volta").join("bin"));
            paths.insert(0, home_path.join(".fnm").join("current").join("bin"));
            paths.insert(0, home_path.join(".asdf").join("shims"));
            paths.insert(0, home_path.join(".asdf").join("bin"));

            // Find nvm node versions if present
            let nvm_versions = home_path.join(".nvm").join("versions").join("node");
            if let Ok(entries) = std::fs::read_dir(nvm_versions) {
                let mut v_dirs: Vec<PathBuf> = entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path().join("bin"))
                    .filter(|p| p.is_dir())
                    .collect();
                v_dirs.sort();
                if let Some(latest) = v_dirs.pop() {
                    paths.insert(0, latest);
                }
            }
        }
        paths.insert(0, PathBuf::from("/usr/local/bin"));
        paths.insert(0, PathBuf::from("/opt/homebrew/bin"));
    }

    std::env::join_paths(paths).unwrap_or(current_path)
}

/// Constructs a cross-platform subprocess Command for the specified or default shell.
pub fn build_command(command_str: &str, requested_shell: Option<ShellType>) -> Command {
    let shell = match requested_shell {
        None | Some(ShellType::Auto) => resolve_default_shell(),
        Some(s) => s,
    };

    let path_val = enriched_path();

    match shell {
        ShellType::Bash => {
            #[cfg(target_os = "windows")]
            {
                let bash_bin = find_git_bash()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "bash.exe".to_string());

                let mut cmd = Command::new(bash_bin);
                crate::agent::engine::configure_subprocess(&mut cmd);
                cmd.arg("-l").arg("-c").arg(command_str);
                cmd.env("PATH", path_val);
                cmd
            }
            #[cfg(not(target_os = "windows"))]
            {
                let bin = if Path::new("/bin/bash").exists() {
                    "/bin/bash"
                } else {
                    "bash"
                };
                let mut cmd = Command::new(bin);
                crate::agent::engine::configure_subprocess(&mut cmd);
                cmd.arg("-l").arg("-c").arg(command_str);
                cmd.env("PATH", path_val);
                cmd
            }
        }

        ShellType::Zsh => {
            #[cfg(target_os = "windows")]
            {
                // Fallback to Git Bash or PowerShell on Windows
                build_command(command_str, Some(ShellType::Bash))
            }
            #[cfg(not(target_os = "windows"))]
            {
                let bin = if Path::new("/bin/zsh").exists() {
                    "/bin/zsh"
                } else {
                    "zsh"
                };
                let mut cmd = Command::new(bin);
                crate::agent::engine::configure_subprocess(&mut cmd);
                cmd.arg("-l").arg("-c").arg(command_str);
                cmd.env("PATH", path_val);
                cmd
            }
        }

        ShellType::PowerShell => {
            #[cfg(target_os = "windows")]
            {
                let mut cmd = Command::new("powershell");
                crate::agent::engine::configure_subprocess(&mut cmd);
                cmd.arg("-ExecutionPolicy")
                    .arg("Bypass")
                    .arg("-WindowStyle")
                    .arg("Hidden")
                    .arg("-Command")
                    .arg(command_str);
                cmd.env("PATH", path_val);
                cmd
            }
            #[cfg(not(target_os = "windows"))]
            {
                if find_in_path("pwsh").is_some() {
                    let mut cmd = Command::new("pwsh");
                    crate::agent::engine::configure_subprocess(&mut cmd);
                    cmd.arg("-NoProfile").arg("-Command").arg(command_str);
                    cmd.env("PATH", path_val);
                    cmd
                } else {
                    // Fallback to POSIX sh if PowerShell is unavailable on Unix
                    build_command(command_str, Some(ShellType::Sh))
                }
            }
        }

        ShellType::Pwsh => {
            let mut cmd = Command::new("pwsh");
            crate::agent::engine::configure_subprocess(&mut cmd);
            #[cfg(target_os = "windows")]
            {
                cmd.arg("-NoProfile")
                    .arg("-ExecutionPolicy")
                    .arg("Bypass")
                    .arg("-Command")
                    .arg(command_str);
            }
            #[cfg(not(target_os = "windows"))]
            {
                cmd.arg("-NoProfile").arg("-Command").arg(command_str);
            }
            cmd.env("PATH", path_val);
            cmd
        }

        ShellType::Cmd => {
            #[cfg(target_os = "windows")]
            {
                let mut cmd = Command::new("cmd.exe");
                crate::agent::engine::configure_subprocess(&mut cmd);
                cmd.arg("/d").arg("/c").arg(command_str);
                cmd.env("PATH", path_val);
                cmd
            }
            #[cfg(not(target_os = "windows"))]
            {
                build_command(command_str, Some(ShellType::Sh))
            }
        }

        ShellType::Sh | ShellType::Auto => {
            #[cfg(target_os = "windows")]
            {
                // Fallback to PowerShell on Windows if generic Sh requested
                build_command(command_str, Some(ShellType::PowerShell))
            }
            #[cfg(not(target_os = "windows"))]
            {
                let mut cmd = Command::new("sh");
                crate::agent::engine::configure_subprocess(&mut cmd);
                cmd.arg("-c").arg(command_str);
                cmd.env("PATH", path_val);
                cmd
            }
        }
    }
}

/// Creates a workspace command using an optional dynamic shell specification.
pub fn create_workspace_command_with_shell(
    command_str: &str,
    shell: Option<ShellType>,
) -> Command {
    build_command(command_str, shell)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_type_parse() {
        assert_eq!(ShellType::parse("powershell"), Some(ShellType::PowerShell));
        assert_eq!(ShellType::parse("powershell.exe"), Some(ShellType::PowerShell));
        assert_eq!(ShellType::parse("pwsh"), Some(ShellType::Pwsh));
        assert_eq!(ShellType::parse("bash"), Some(ShellType::Bash));
        assert_eq!(ShellType::parse("gitbash"), Some(ShellType::Bash));
        assert_eq!(ShellType::parse("git-bash"), Some(ShellType::Bash));
        assert_eq!(ShellType::parse("zsh"), Some(ShellType::Zsh));
        assert_eq!(ShellType::parse("cmd"), Some(ShellType::Cmd));
        assert_eq!(ShellType::parse("cmd.exe"), Some(ShellType::Cmd));
        assert_eq!(ShellType::parse("sh"), Some(ShellType::Sh));
        assert_eq!(ShellType::parse("auto"), Some(ShellType::Auto));
        assert_eq!(ShellType::parse("unknown_shell_foo"), None);
    }

    #[test]
    fn test_is_posix_chaining() {
        assert!(ShellType::Bash.is_posix_chaining());
        assert!(ShellType::Zsh.is_posix_chaining());
        assert!(ShellType::Sh.is_posix_chaining());
        assert!(!ShellType::PowerShell.is_posix_chaining());
    }

    #[test]
    fn test_build_command_runs_successfully() {
        let mut cmd = build_command("echo hello_terminal_test", None);
        let output = cmd.output().expect("Failed to execute built command");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello_terminal_test"));
    }
}
