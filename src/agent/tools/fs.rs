use anyhow::{bail, Result};
#[cfg(not(target_os = "windows"))]
use std::fs;
use std::path::{Path, PathBuf};

/// Verify that a target path is allowed within one of the project's root folders.
/// Seamlessly supports multi-root workspaces, folder aliases, and relative paths across all platforms.
pub fn resolve_allowed_path(raw_path: &str, allowed_roots: &[String]) -> Result<PathBuf> {
    if allowed_roots.is_empty() {
        bail!("Workspace has no registered root folders.");
    }

    let trimmed = raw_path.trim();
    let cleaned = trimmed
        .strip_prefix("@")
        .or_else(|| trimmed.strip_prefix("./@"))
        .unwrap_or(trimmed);

    let input_path = Path::new(cleaned);

    // 1. If absolute path: verify it starts with an allowed root
    if input_path.is_absolute() {
        let canonical = if input_path.exists() {
            input_path.canonicalize().unwrap_or_else(|_| input_path.to_path_buf())
        } else if let Some(parent) = input_path.parent() {
            if parent.exists() {
                parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf()).join(input_path.file_name().unwrap_or_default())
            } else {
                input_path.to_path_buf()
            }
        } else {
            input_path.to_path_buf()
        };

        let is_inside = allowed_roots.iter().any(|r| {
            let r_path = Path::new(r);
            let r_canon = r_path.canonicalize().unwrap_or_else(|_| r_path.to_path_buf());
            canonical.starts_with(&r_canon)
        });

        if !is_inside {
            bail!(
                "Access denied: path '{}' is outside allowed workspace folders ({:?})",
                raw_path,
                allowed_roots
            );
        }
        return Ok(canonical);
    }

    // 2. Relative path: Check if path matches an existing file/dir in any allowed root directly
    for root in allowed_roots {
        let candidate = Path::new(root).join(input_path);
        if candidate.exists() {
            return Ok(candidate.canonicalize().unwrap_or(candidate));
        }
    }

    // 3. Multi-root folder alias matching:
    // e.g. input_path is "frontend" or "frontend/src/App.tsx", matching root ".../frontend"
    for root in allowed_roots {
        let root_p = Path::new(root);
        if let Some(folder_name) = root_p.file_name().and_then(|n| n.to_str()) {
            if input_path == Path::new(folder_name) {
                return Ok(root_p.canonicalize().unwrap_or_else(|_| root_p.to_path_buf()));
            }
            if let Ok(rel) = input_path.strip_prefix(folder_name) {
                let candidate = root_p.join(rel);
                if candidate.exists() {
                    return Ok(candidate.canonicalize().unwrap_or(candidate));
                }
            }
        }
    }

    // 4. For new files or directories that do not exist yet (e.g. write_file)
    let candidate = if let Some(matched_alias_path) = allowed_roots.iter().find_map(|root| {
        let root_p = Path::new(root);
        if let Some(folder_name) = root_p.file_name().and_then(|n| n.to_str()) {
            if let Ok(rel) = input_path.strip_prefix(folder_name) {
                return Some(root_p.join(rel));
            }
        }
        None
    }) {
        matched_alias_path
    } else {
        // 5. Default fallback to primary root folder
        Path::new(&allowed_roots[0]).join(input_path)
    };

    // Verify candidate path does not escape allowed workspace roots via ".." or symlinks
    let target_canonical = if candidate.exists() {
        candidate.canonicalize().unwrap_or(candidate)
    } else if let Some(parent) = candidate.parent() {
        if parent.exists() {
            parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf()).join(candidate.file_name().unwrap_or_default())
        } else {
            candidate
        }
    } else {
        candidate
    };

    let is_inside = allowed_roots.iter().any(|r| {
        let r_path = Path::new(r);
        let r_canon = r_path.canonicalize().unwrap_or_else(|_| r_path.to_path_buf());
        target_canonical.starts_with(&r_canon)
    });

    if !is_inside {
        bail!(
            "Access denied: path '{}' escapes allowed workspace roots ({:?})",
            raw_path,
            allowed_roots
        );
    }

    Ok(target_canonical)
}

/// Convert any full path into a clean, workspace-relative path prefixed with its registered root folder name.
/// e.g. `/home/user/project/portofolio/src/App.tsx` -> `portofolio/src/App.tsx`
pub fn to_workspace_folder_path(path: &Path, allowed_roots: &[String]) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    for root in allowed_roots {
        let root_p = Path::new(root);
        let root_canon = root_p.canonicalize().unwrap_or_else(|_| root_p.to_path_buf());
        if let Ok(rel) = canonical.strip_prefix(&root_canon) {
            let folder_name = root_p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if rel.as_os_str().is_empty() {
                return folder_name.to_string();
            }
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if folder_name.is_empty() {
                return rel_str;
            }
            return format!("{}/{}", folder_name, rel_str);
        }
    }

    path.to_string_lossy().replace('\\', "/")
}

/// Inspects project indicator files and generates a tailored .gitignore template.
pub fn generate_smart_gitignore(root_path: &Path) -> String {
    let has_node = root_path.join("package.json").exists()
        || root_path.join("pnpm-lock.yaml").exists()
        || root_path.join("yarn.lock").exists()
        || root_path.join("bun.lockb").exists();

    let has_rust = root_path.join("Cargo.toml").exists();

    let has_python = root_path.join("pyproject.toml").exists()
        || root_path.join("requirements.txt").exists()
        || root_path.join("Pipfile").exists()
        || root_path.join("setup.py").exists();

    let has_go = root_path.join("go.mod").exists();
    let has_php = root_path.join("composer.json").exists();
    let has_java = root_path.join("pom.xml").exists()
        || root_path.join("build.gradle").exists()
        || root_path.join("build.gradle.kts").exists();
    let has_c_cpp = root_path.join("CMakeLists.txt").exists() || root_path.join("Makefile").exists();

    let mut out = String::new();
    out.push_str("# Universal Workspace .gitignore (Auto-generated by Ragasukma Agent)\n\n");

    out.push_str("# Operating System & Editor Artifacts\n");
    out.push_str(".DS_Store\n");
    out.push_str("Thumbs.db\n");
    out.push_str("*.swp\n");
    out.push_str("*.swo\n");
    out.push_str("*~\n");
    out.push_str(".idea/\n");
    out.push_str(".vscode/\n");
    out.push_str("*.sublime-project\n");
    out.push_str("*.sublime-workspace\n\n");

    out.push_str("# Environment & Secret Variables\n");
    out.push_str(".env\n");
    out.push_str(".env.*\n");
    out.push_str("!.env.example\n\n");

    if has_node {
        out.push_str("# Node.js / Web / Frontend\n");
        out.push_str("node_modules/\n");
        out.push_str("dist/\n");
        out.push_str("build/\n");
        out.push_str(".next/\n");
        out.push_str(".nuxt/\n");
        out.push_str(".turbo/\n");
        out.push_str(".cache/\n");
        out.push_str("coverage/\n");
        out.push_str("*.log\n");
        out.push_str("npm-debug.log*\n");
        out.push_str("yarn-debug.log*\n");
        out.push_str("yarn-error.log*\n");
        out.push_str("pnpm-debug.log*\n\n");
    }

    if has_rust {
        out.push_str("# Rust / Cargo\n");
        out.push_str("/target/\n");
        out.push_str("**/*.rs.bk\n");
        out.push_str("*.pdb\n\n");
    }

    if has_python {
        out.push_str("# Python\n");
        out.push_str("__pycache__/\n");
        out.push_str("*.py[cod]\n");
        out.push_str("*$py.class\n");
        out.push_str("*.so\n");
        out.push_str(".venv/\n");
        out.push_str("venv/\n");
        out.push_str("env/\n");
        out.push_str("ENV/\n");
        out.push_str("build/\n");
        out.push_str("dist/\n");
        out.push_str("*.egg-info/\n");
        out.push_str(".pytest_cache/\n");
        out.push_str(".mypy_cache/\n");
        out.push_str(".ruff_cache/\n\n");
    }

    if has_go {
        out.push_str("# Go\n");
        out.push_str("/bin/\n");
        out.push_str("/dist/\n");
        out.push_str("vendor/\n\n");
    }

    if has_php {
        out.push_str("# PHP / Composer\n");
        out.push_str("/vendor/\n\n");
    }

    if has_java {
        out.push_str("# Java / Gradle / Maven\n");
        out.push_str("target/\n");
        out.push_str("build/\n");
        out.push_str(".gradle/\n\n");
    }

    if has_c_cpp {
        out.push_str("# C / C++ / Build\n");
        out.push_str("build/\n");
        out.push_str("bin/\n");
        out.push_str("*.o\n");
        out.push_str("*.a\n");
        out.push_str("*.so\n");
        out.push_str("*.dylib\n\n");
    }

    if !has_node && !has_rust && !has_python && !has_go && !has_php && !has_java && !has_c_cpp {
        out.push_str("# Generic Common Ignore Patterns\n");
        out.push_str("node_modules/\n");
        out.push_str("dist/\n");
        out.push_str("build/\n");
        out.push_str("target/\n");
        out.push_str(".cache/\n");
        out.push_str("__pycache__/\n");
        out.push_str(".venv/\n");
        out.push_str("venv/\n");
        out.push_str("*.log\n\n");
    }

    out
}

/// Ensures that a workspace root has a git repository initialized and a tailored .gitignore file.
/// If .git does not exist, runs `git init`.
/// If .gitignore does not exist, inspects project files and generates a smart .gitignore.
pub fn ensure_git_and_gitignore(root_path: &Path) -> Result<()> {
    ensure_git_and_gitignore_opt(root_path, true)
}

/// Ensures .gitignore exists, and conditionally runs `git init` based on `auto_git_init` configuration.
pub fn ensure_git_and_gitignore_opt(root_path: &Path, auto_git_init: bool) -> Result<()> {
    if !root_path.exists() || !root_path.is_dir() {
        return Ok(());
    }

    // 1. Check & init git if not present and auto_git_init is enabled
    let git_dir = root_path.join(".git");
    if !git_dir.exists() && auto_git_init {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("init").current_dir(root_path);
        crate::agent::engine::configure_subprocess(&mut cmd);
        match cmd.output() {
            Ok(out) => {
                if out.status.success() {
                    println!("[Agent Workspace] Initialized git repo at: {:?}", root_path);
                } else {
                    eprintln!(
                        "[Agent Workspace] Failed to run 'git init' at {:?}: {}",
                        root_path,
                        String::from_utf8_lossy(&out.stderr)
                    );
                }
            }
            Err(e) => {
                eprintln!("[Agent Workspace] Could not execute 'git init' at {:?}: {}", root_path, e);
            }
        }
    } else if !git_dir.exists() && !auto_git_init {
        println!("[Agent Workspace] Git auto-init skipped (opt-in disabled) for {:?}", root_path);
    }

    // 2. Check & create smart .gitignore if missing
    let gitignore_path = root_path.join(".gitignore");
    if !gitignore_path.exists() {
        let gitignore_content = generate_smart_gitignore(root_path);
        if let Err(e) = std::fs::write(&gitignore_path, gitignore_content) {
            eprintln!("[Agent Workspace] Failed to write smart .gitignore at {:?}: {}", gitignore_path, e);
        } else {
            println!("[Agent Workspace] Created smart .gitignore at: {:?}", gitignore_path);
        }
    }

    Ok(())
}

/// Builds a cross-platform shell Command with an enriched PATH environment so tools like
/// npm, npx, pnpm, yarn, node, cargo, python, etc. are reliably found across Linux, macOS, and Windows.
pub fn create_workspace_command(command_str: &str) -> std::process::Command {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("powershell");
        crate::agent::engine::configure_subprocess(&mut cmd);
        cmd.arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-WindowStyle")
            .arg("Hidden")
            .arg("-Command")
            .arg(command_str);

        // Enrich Windows PATH if needed
        let current_path = std::env::var("PATH").unwrap_or_default();
        let mut path_entries: Vec<PathBuf> = std::env::split_paths(&current_path).collect();

        if let Ok(appdata) = std::env::var("APPDATA") {
            let npm_roaming = PathBuf::from(&appdata).join("npm");
            if npm_roaming.exists() && !path_entries.contains(&npm_roaming) {
                path_entries.insert(0, npm_roaming);
            }
        }
        if let Ok(userprofile) = std::env::var("USERPROFILE") {
            let cargo_bin = PathBuf::from(&userprofile).join(".cargo").join("bin");
            if cargo_bin.exists() && !path_entries.contains(&cargo_bin) {
                path_entries.insert(0, cargo_bin);
            }
        }
        let node_prog = PathBuf::from(r"C:\Program Files\nodejs");
        if node_prog.exists() && !path_entries.contains(&node_prog) {
            path_entries.insert(0, node_prog);
        }

        if let Ok(new_path) = std::env::join_paths(path_entries) {
            cmd.env("PATH", new_path);
        }
        cmd
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Enrich Unix PATH with common Node, Python, and Rust manager directories
        let current_path = std::env::var("PATH").unwrap_or_default();
        let mut path_entries: Vec<PathBuf> = std::env::split_paths(&current_path).collect();

        if let Ok(home) = std::env::var("HOME") {
            let home_path = Path::new(&home);
            let candidates = [
                home_path.join(".local/bin"),
                home_path.join(".cargo/bin"),
                home_path.join(".bun/bin"),
                home_path.join(".volta/bin"),
                home_path.join(".fnm/current/bin"),
                home_path.join(".asdf/shims"),
                home_path.join(".asdf/bin"),
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/opt/homebrew/bin"),
            ];

            for candidate in candidates {
                if candidate.exists() && !path_entries.contains(&candidate) {
                    path_entries.insert(0, candidate);
                }
            }

            // NVM active directory or latest installed node version
            if let Ok(nvm_bin) = std::env::var("NVM_BIN") {
                let nvm_path = PathBuf::from(nvm_bin);
                if nvm_path.exists() && !path_entries.contains(&nvm_path) {
                    path_entries.insert(0, nvm_path);
                }
            }

            let nvm_node_dir = home_path.join(".nvm/versions/node");
            if nvm_node_dir.is_dir() {
                if let Ok(entries) = fs::read_dir(&nvm_node_dir) {
                    let mut versions: Vec<PathBuf> = entries
                        .filter_map(|e| e.ok())
                        .map(|e| e.path().join("bin"))
                        .filter(|p| p.is_dir())
                        .collect();
                    versions.sort();
                    if let Some(latest) = versions.pop() {
                        if !path_entries.contains(&latest) {
                            path_entries.insert(0, latest);
                        }
                    }
                }
            }
        }

        let new_path = std::env::join_paths(path_entries).unwrap_or_else(|_| current_path.into());

        let shell_var = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let (shell_bin, is_login) = if Path::new(&shell_var).exists() {
            let login = shell_var.ends_with("bash") || shell_var.ends_with("zsh");
            (shell_var, login)
        } else if Path::new("/bin/bash").exists() {
            ("/bin/bash".to_string(), true)
        } else {
            ("sh".to_string(), false)
        };

        let mut cmd = std::process::Command::new(&shell_bin);
        if is_login {
            cmd.arg("-l").arg("-c").arg(command_str);
        } else {
            cmd.arg("-c").arg(command_str);
        }
        cmd.env("PATH", new_path);
        cmd
    }
}

/// Generate a language-agnostic structural outline for large files.
/// Identifies major sections, top-level definitions, symbols, classes, functions, and headings
/// along with their 1-based line numbers.
pub fn generate_file_outline(content: &str, file_path: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let total_bytes = content.len();

    let mut outline_entries: Vec<(usize, String, bool)> = Vec::new();

    // Universal definition and heading markers across languages, formats, and configs
    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        let indent = line.chars().take_while(|c| c.is_whitespace()).count();

        // 1. Markdown / Documentation Headings (# Heading, ## Subheading)
        if trimmed.starts_with('#') && trimmed.chars().nth(1).is_some_and(|c| c == '#' || c == ' ') {
            outline_entries.push((line_num, trimmed.chars().take(120).collect(), true));
            continue;
        }

        // 2. Config / Section headers ([section], [[table]])
        if trimmed.starts_with('[') && trimmed.ends_with(']') && indent <= 2 {
            outline_entries.push((line_num, trimmed.to_string(), true));
            continue;
        }

        // 3. Top-level or shallow definition anchors (indentation <= 4 spaces or 1 tab)
        if indent <= 4 {
            let is_comment = trimmed.starts_with("//")
                || trimmed.starts_with("/*")
                || trimmed.starts_with('*')
                || trimmed.starts_with('#')
                || trimmed.starts_with("--")
                || trimmed.starts_with(';');

            if is_comment {
                // Keep major section banners/comments (e.g. "// --- Section ---")
                if trimmed.contains("---") || trimmed.contains("===") || trimmed.contains("###") {
                    outline_entries.push((line_num, trimmed.chars().take(100).collect(), false));
                }
                continue;
            }

            let first_word = trimmed.split_whitespace().next().unwrap_or("");
            let clean_first = first_word.trim_end_matches(':');

            // Major structural declarations across languages (functions, types, classes, modules)
            let is_major_decl = matches!(
                clean_first,
                "fn" | "pub" | "function" | "func" | "def" | "class" | "struct" | "enum" |
                "interface" | "type" | "trait" | "impl" | "export" | "mod" | "module" |
                "package" | "async" | "namespace" | "sub" | "proc" | "method" | "actor" |
                "record" | "schema" | "table" | "CREATE" | "SELECT" | "ALTER" | "DROP" |
                "router" | "endpoint" | "contract" | "service" | "proto"
            );

            // Top-level declarations only (indent == 0) for constants, variables, imports, config keys
            let is_toplevel_decl = indent == 0 && matches!(
                clean_first,
                "import" | "from" | "const" | "let" | "var" | "val" | "static" | "data" | "entity" | "message"
            );

            let is_block_header = indent == 0 && (trimmed.ends_with('{') || trimmed.ends_with(':') || trimmed.contains("=> {"));
            let is_top_key = indent == 0 && (trimmed.contains(':') && !trimmed.contains("::")) && !trimmed.starts_with('{');

            if is_major_decl || is_toplevel_decl || is_block_header || is_top_key {
                let display = if trimmed.chars().count() > 140 {
                    format!("{} ...", trimmed.chars().take(140).collect::<String>())
                } else {
                    trimmed.to_string()
                };
                outline_entries.push((line_num, display, is_major_decl));
            }
        }
    }

    // If file is large but unstructured / minified, provide periodic sample lines
    if outline_entries.is_empty() {
        let step = (total_lines / 20).max(1);
        for idx in (0..total_lines).step_by(step).take(20) {
            let line_num = idx + 1;
            let trimmed = lines[idx].trim();
            if !trimmed.is_empty() {
                let display = trimmed.chars().take(100).collect::<String>();
                outline_entries.push((line_num, display, false));
            }
        }
    }

    // Limit outline size to maximum 80 landmark symbols, prioritizing major declarations
    if outline_entries.len() > 80 {
        let major_count = outline_entries.iter().filter(|(_, _, is_maj)| *is_maj).count();
        if major_count > 0 && major_count <= 80 {
            let mut result = Vec::new();
            let step = (outline_entries.len() as f64 / 40.0).ceil() as usize;
            for (idx, entry) in outline_entries.iter().enumerate() {
                if entry.2 || idx % step == 0 {
                    result.push(entry.clone());
                }
            }
            result.dedup_by_key(|e| e.0);
            outline_entries = result;
        } else {
            let step = (outline_entries.len() as f64 / 70.0).ceil() as usize;
            let mut sampled = Vec::new();
            for (i, entry) in outline_entries.iter().enumerate() {
                if i % step == 0 || i == outline_entries.len() - 1 || entry.2 {
                    sampled.push(entry.clone());
                }
            }
            sampled.dedup_by_key(|e| e.0);
            outline_entries = sampled;
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "# File outline for '{}' (total {} lines, {} bytes):\n\
         # File is large: to inspect specific code sections, call `workspace_read_file` with `start_line` and `end_line`.\n\
         # To read the full raw file anyway, call `workspace_read_file` with `mode: \"full\"`.\n\n",
        file_path, total_lines, total_bytes
    ));

    for (line_num, line_str, _) in &outline_entries {
        out.push_str(&format!("L{:4}: {}\n", line_num, line_str));
    }

    out.push_str(&format!(
        "\n# Showing {} structural landmarks across {} lines. Use `start_line` and `end_line` to inspect targeted ranges.",
        outline_entries.len(),
        total_lines
    ));

    out
}
