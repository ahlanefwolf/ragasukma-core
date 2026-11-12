use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// Verify that a target path is allowed within one of the project's root folders.
/// Seamlessly supports multi-root workspaces, folder aliases, and relative paths across all platforms.
/// Enforces strict boundary checks to prevent path traversal and escaping via symlinks or `..`.
pub fn resolve_allowed_path(raw_path: &str, allowed_roots: &[String]) -> Result<PathBuf> {
    if allowed_roots.is_empty() {
        bail!("Workspace has no registered root folders.");
    }

    let trimmed = raw_path.trim();
    let cleaned = trimmed
        .strip_prefix("@/")
        .or_else(|| trimmed.strip_prefix('@'))
        .unwrap_or(trimmed);

    let input_path = Path::new(cleaned);

    // Canonicalize all allowed roots for strict boundary comparison
    let canon_roots: Vec<PathBuf> = allowed_roots
        .iter()
        .map(|r| {
            let p = Path::new(r);
            p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
        })
        .collect();

    let is_inside_canon_roots = |cand: &Path| -> bool {
        canon_roots.iter().any(|r| cand.starts_with(r))
    };

    // 1. Direct absolute path resolution with boundary check
    if input_path.is_absolute() {
        if let Ok(canonical) = input_path.canonicalize() {
            if is_inside_canon_roots(&canonical) {
                return Ok(canonical);
            }
            bail!(
                "Access denied: path '{}' is outside allowed workspace folders ({:?})",
                raw_path,
                allowed_roots
            );
        }

        let mut ancestor = input_path.parent();
        while let Some(a) = ancestor {
            if a.exists() {
                break;
            }
            ancestor = a.parent();
        }

        if let Some(existing_ancestor) = ancestor {
            let canon_ancestor = existing_ancestor
                .canonicalize()
                .unwrap_or_else(|_| existing_ancestor.to_path_buf());

            if !is_inside_canon_roots(&canon_ancestor) {
                bail!(
                    "Access denied: path '{}' is outside allowed workspace folders ({:?})",
                    raw_path,
                    allowed_roots
                );
            }

            let Ok(rel) = input_path.strip_prefix(existing_ancestor) else {
                bail!("Access denied: Path '{raw_path}' cannot be relativized to ancestor");
            };

            for comp in rel.components() {
                if comp == std::path::Component::ParentDir {
                    bail!("Access denied: Path '{raw_path}' contains parent traversal");
                }
            }

            let target = canon_ancestor.join(rel);
            if is_inside_canon_roots(&target) {
                return Ok(target);
            }
        }

        bail!(
            "Access denied: path '{}' is outside allowed workspace folders ({:?})",
            raw_path,
            allowed_roots
        );
    }

    // Reject raw relative paths with leading traversal that attempt to escape workspace
    let mut normalized_components = Vec::new();
    for comp in input_path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if normalized_components.is_empty() {
                    bail!(
                        "Access denied: path '{}' escapes allowed workspace roots ({:?})",
                        raw_path,
                        allowed_roots
                    );
                }
                normalized_components.pop();
            }
            std::path::Component::Normal(c) => normalized_components.push(c),
            _ => {}
        }
    }

    let normalized_rel: PathBuf = normalized_components.into_iter().collect();

    // 2. Multi-root folder alias matching
    for (idx, root) in allowed_roots.iter().enumerate() {
        let root_p = Path::new(root);
        if let Some(folder_name) = root_p.file_name().and_then(|n| n.to_str()) {
            if normalized_rel == Path::new(folder_name) || input_path == Path::new(folder_name) {
                return Ok(canon_roots[idx].clone());
            }

            let stripped = normalized_rel
                .strip_prefix(folder_name)
                .ok()
                .or_else(|| input_path.strip_prefix(folder_name).ok());

            if let Some(rel) = stripped {
                let candidate = root_p.join(rel);
                if candidate.exists()
                    && let Ok(canon) = candidate.canonicalize()
                    && (canon.starts_with(&canon_roots[idx]) || is_inside_canon_roots(&canon))
                {
                    return Ok(canon);
                }
            }
        }
    }

    // 3. Relative path: Check if path matches an existing file/dir in any allowed root directly
    for (idx, root) in allowed_roots.iter().enumerate() {
        let candidate = Path::new(root).join(&normalized_rel);
        if candidate.exists()
            && let Ok(canon) = candidate.canonicalize()
            && (canon.starts_with(&canon_roots[idx]) || is_inside_canon_roots(&canon))
        {
            return Ok(canon);
        }
    }

    // 4. For new files or directories that do not exist yet (e.g. write_file)
    let candidate = if let Some(matched_alias_path) = allowed_roots.iter().find_map(|root| {
        let root_p = Path::new(root);
        if let Some(folder_name) = root_p.file_name().and_then(|n| n.to_str()) {
            if let Ok(rel) = normalized_rel.strip_prefix(folder_name) {
                return Some(root_p.join(rel));
            }
            if let Ok(rel) = input_path.strip_prefix(folder_name) {
                return Some(root_p.join(rel));
            }
        }
        None
    }) {
        matched_alias_path
    } else {
        // 5. Default fallback to primary root folder
        Path::new(&allowed_roots[0]).join(&normalized_rel)
    };

    // Verify candidate path does not escape allowed workspace roots via ".." or symlinks
    let mut ancestor = candidate.parent();
    while let Some(a) = ancestor {
        if a.exists() {
            break;
        }
        ancestor = a.parent();
    }

    let target_canonical = if candidate.exists() {
        candidate.canonicalize().unwrap_or(candidate)
    } else if let Some(existing_ancestor) = ancestor {
        let canon_ancestor = existing_ancestor
            .canonicalize()
            .unwrap_or_else(|_| existing_ancestor.to_path_buf());
        if let Ok(rel) = candidate.strip_prefix(existing_ancestor) {
            for comp in rel.components() {
                if comp == std::path::Component::ParentDir {
                    bail!(
                        "Access denied: path '{}' escapes allowed workspace roots ({:?})",
                        raw_path,
                        allowed_roots
                    );
                }
            }
            canon_ancestor.join(rel)
        } else {
            candidate
        }
    } else {
        candidate
    };

    if is_inside_canon_roots(&target_canonical) {
        return Ok(target_canonical);
    }

    bail!(
        "Access denied: path '{}' escapes allowed workspace roots ({:?})",
        raw_path,
        allowed_roots
    );
}

/// Atomically write content to a file, replacing it safely to prevent corruption.
/// Writes to a temporary file in the same directory, flushes, syncs to disk, and renames.
pub fn atomic_write_file(path: &Path, content: impl AsRef<[u8]>) -> Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temp_name = format!(
        ".tmp_{}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
    );
    let temp_path = parent.join(temp_name);

    let mut temp_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)?;

    let write_res = (|| -> Result<()> {
        temp_file.write_all(content.as_ref())?;
        temp_file.flush()?;
        temp_file.sync_all()?;
        Ok(())
    })();

    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e);
    }
    drop(temp_file);

    if path.exists() && cfg!(windows) {
        if let Err(_rename_err) = std::fs::rename(&temp_path, path) {
            let _ = std::fs::remove_file(path);
            if let Err(replace_err) = std::fs::rename(&temp_path, path) {
                let _ = std::fs::remove_file(&temp_path);
                return Err(replace_err.into());
            }
        }
    } else if let Err(e) = std::fs::rename(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e.into());
    }

    Ok(())
}

/// Convert any full path into a clean, workspace-relative path prefixed with its registered root folder name.
/// e.g. `/home/user/project/portofolio/src/App.tsx` -> `portofolio/src/App.tsx`
pub fn to_workspace_folder_path(path: &Path, allowed_roots: &[String]) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    for root in allowed_roots {
        let root_p = Path::new(root);
        let root_canon = root_p
            .canonicalize()
            .unwrap_or_else(|_| root_p.to_path_buf());
        if let Ok(rel) = canonical.strip_prefix(&root_canon) {
            let folder_name = root_p.file_name().and_then(|n| n.to_str()).unwrap_or("");
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
    let has_c_cpp =
        root_path.join("CMakeLists.txt").exists() || root_path.join("Makefile").exists();

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
                    tracing::info!("[Agent Workspace] Initialized git repo at: {:?}", root_path);
                } else {
                    tracing::warn!(
                        "[Agent Workspace] Failed to run 'git init' at {:?}: {}",
                        root_path,
                        String::from_utf8_lossy(&out.stderr)
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    "[Agent Workspace] Could not execute 'git init' at {:?}: {}",
                    root_path,
                    e
                );
            }
        }
    } else if !git_dir.exists() && !auto_git_init {
        tracing::info!(
            "[Agent Workspace] Git auto-init skipped (opt-in disabled) for {:?}",
            root_path
        );
    }

    // 2. Check & create smart .gitignore if missing
    let gitignore_path = root_path.join(".gitignore");
    if !gitignore_path.exists() {
        let gitignore_content = generate_smart_gitignore(root_path);
        if let Err(e) = std::fs::write(&gitignore_path, gitignore_content) {
            tracing::warn!(
                "[Agent Workspace] Failed to write smart .gitignore at {:?}: {}",
                gitignore_path,
                e
            );
        } else {
            tracing::info!(
                "[Agent Workspace] Created smart .gitignore at: {:?}",
                gitignore_path
            );
        }
    }

    Ok(())
}

/// Builds a cross-platform shell Command with an enriched PATH environment so tools like
/// npm, npx, pnpm, yarn, node, cargo, python, etc. are reliably found across Linux, macOS, and Windows.
pub fn create_workspace_command(command_str: &str) -> std::process::Command {
    crate::agent::terminal::build_command(command_str, None)
}

/// Builds a shell Command with explicit shell interpreter override (bash, powershell, pwsh, cmd, zsh, sh).
pub fn create_workspace_command_with_shell(
    command_str: &str,
    shell: Option<crate::agent::terminal::ShellType>,
) -> std::process::Command {
    crate::agent::terminal::build_command(command_str, shell)
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
        if trimmed.starts_with('#') && trimmed.chars().nth(1).is_some_and(|c| c == '#' || c == ' ')
        {
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
                "fn" | "pub"
                    | "function"
                    | "func"
                    | "def"
                    | "class"
                    | "struct"
                    | "enum"
                    | "interface"
                    | "type"
                    | "trait"
                    | "impl"
                    | "export"
                    | "mod"
                    | "module"
                    | "package"
                    | "async"
                    | "namespace"
                    | "sub"
                    | "proc"
                    | "method"
                    | "actor"
                    | "record"
                    | "schema"
                    | "table"
                    | "CREATE"
                    | "SELECT"
                    | "ALTER"
                    | "DROP"
                    | "router"
                    | "endpoint"
                    | "contract"
                    | "service"
                    | "proto"
            );

            // Top-level declarations only (indent == 0) for constants, variables, imports, config keys
            let is_toplevel_decl = indent == 0
                && matches!(
                    clean_first,
                    "import"
                        | "from"
                        | "const"
                        | "let"
                        | "var"
                        | "val"
                        | "static"
                        | "data"
                        | "entity"
                        | "message"
                );

            let is_block_header = indent == 0
                && (trimmed.ends_with('{') || trimmed.ends_with(':') || trimmed.contains("=> {"));
            let is_top_key = indent == 0
                && (trimmed.contains(':') && !trimmed.contains("::"))
                && !trimmed.starts_with('{');

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
        let major_count = outline_entries
            .iter()
            .filter(|(_, _, is_maj)| *is_maj)
            .count();
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