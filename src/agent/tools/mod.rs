pub mod dialogs;
pub mod editor;
pub mod fs;
pub mod search;
pub mod tags;

use anyhow::{Context, Result, bail};
use serde_json::json;
use std::fs as std_fs;
use std::path::Path;

pub use dialogs::{open_path_in_file_manager, pick_folder_dialog};
pub use editor::apply_resilient_edit;
pub use fs::{
    create_workspace_command, create_workspace_command_with_shell, ensure_git_and_gitignore,
    ensure_git_and_gitignore_opt, generate_file_outline, generate_smart_gitignore,
    resolve_allowed_path, to_workspace_folder_path,
};
pub use search::{WorkspaceFileInfo, list_workspace_files, run_workspace_grep};
pub use tags::expand_file_tags;

/// Smartly truncates output keeping both the head and tail, omitting the middle.
/// Perfectly Unicode/char-safe without mid-character slice panics.
pub fn truncate_smart(s: &str, max_head: usize, max_tail: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_head + max_tail {
        return s.to_string();
    }
    let head: String = s.chars().take(max_head).collect();
    let tail_skip = char_count.saturating_sub(max_tail);
    let tail: String = s.chars().skip(tail_skip).collect();
    let omitted = char_count - max_head - max_tail;
    format!(
        "{}\n\n... [{} characters omitted to preserve token budget] ...\n\n{}",
        head, omitted, tail
    )
}

/// Checks if a tool is strictly read-only and free of side effects.
pub fn is_read_only_tool(name: &str) -> bool {
    matches!(
        name,
        "workspace_read_file" | "workspace_list_dir" | "workspace_grep"
    )
}

/// Returns dynamic tool definitions combining built-in workspace tools with project MCP tools.
/// Specialized tools (workspace inspection/editing and active MCP tools) are prioritized FIRST,
/// while `workspace_run_command` (terminal fallback) is placed LAST.
pub fn get_dynamic_agent_tools_schema(
    mcp_tools: &[crate::agent::mcp::ActiveMcpTool],
) -> Vec<serde_json::Value> {
    let mut schema = Vec::new();

    // 1. Core workspace structural tools
    schema.push(json!({
        "type": "function",
        "function": {
            "name": "workspace_list_dir",
            "description": "List files and subdirectories in a directory with shallow depth (default depth 1). Shows child counts for subdirectories so you know what is inside without full recursion. Specify 'depth' (1-3) if needed.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative or absolute path to the directory within the workspace"
                    },
                    "depth": {
                        "type": "integer",
                        "description": "Directory traversal depth (1 to 3, default: 1 for shallow listing)"
                    }
                },
                "required": ["path"]
            }
        }
    }));

    schema.push(json!({
        "type": "function",
        "function": {
            "name": "workspace_read_file",
            "description": "Read text content of a file in the workspace. For large files (>250 lines), automatically returns a structural outline with symbol names and line numbers to save tokens. Use `start_line` and `end_line` to read specific slices, or set `mode: \"full\"` to force reading the entire file.",
            "parameters": {
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the file to read"
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "Start line number (1-indexed, optional)"
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "End line number (1-indexed, optional)"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["outline", "full"],
                        "description": "Reading mode: 'outline' returns a structural outline with line numbers; 'full' forces reading the full file even if large; default automatically returns outline for files >250 lines."
                    }
                },
                "required": ["file_path"]
            }
        }
    }));

    schema.push(json!({
        "type": "function",
        "function": {
            "name": "workspace_write_file",
            "description": "Write or overwrite content of a file in the workspace. Automatically formats code based on file extension (Prettier for JS/TS/CSS/JSON, rustfmt for Rust, gofmt for Go, black for Python) and tracks modification metadata.",
            "parameters": {
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the file to write (relative to workspace or absolute)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Complete file content to write"
                    }
                },
                "required": ["file_path", "content"]
            }
        }
    }));

    schema.push(json!({
        "type": "function",
        "function": {
            "name": "workspace_edit_file",
            "description": "Precisely replace specific lines or code blocks within an existing file. Provide exact 'old_text' to match and 'new_text' to replace it with. Uses resilient whitespace-tolerant matching with automatic rollback on error.",
            "parameters": {
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the file to edit (relative to workspace or absolute)"
                    },
                    "old_text": {
                        "type": "string",
                        "description": "Exact text or code snippet to be replaced (include 1-2 surrounding lines if needed for uniqueness)"
                    },
                    "new_text": {
                        "type": "string",
                        "description": "New replacement code snippet"
                    }
                },
                "required": ["file_path", "old_text", "new_text"]
            }
        }
    }));

    schema.push(json!({
        "type": "function",
        "function": {
            "name": "workspace_rollback",
            "description": "Revert files to the state they were in before this session modified them. Every write and edit is backed up automatically. Pass 'file_path' to revert a single file, or omit it to revert every file this session touched. Use this when the user rejects a change, when an edit went in the wrong direction, or when a build broke and you want to start over from a known-good state.",
            "parameters": {
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Optional path to a single file to revert. Omit to revert every file modified during this session."
                    }
                },
                "required": []
            }
        }
    }));

    schema.push(json!({
        "type": "function",
        "function": {
            "name": "workspace_grep",
            "description": "Fast regex and literal text search across files in the workspace (ignores node_modules, target, .git, etc.). Returns up to 50 results formatted with file paths and line numbers.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Regex pattern or search text"
                    },
                    "folder": {
                        "type": "string",
                        "description": "Subdirectory to search (optional, defaults to first root folder)"
                    }
                },
                "required": ["query"]
            }
        }
    }));

    // Meta-tool for 2-Tier On-Demand Extension Activation
    schema.push(json!({
        "type": "function",
        "function": {
            "name": "activate_extension",
            "description": "Activate an installed MCP extension package on-demand (e.g. 'figcraft'). Once activated, all dedicated tools provided by this extension become immediately available in your tool declaration schema.",
            "parameters": {
                "type": "object",
                "properties": {
                    "extension_name": {
                        "type": "string",
                        "description": "The exact name of the extension / MCP server to activate (e.g. 'figcraft')"
                    }
                },
                "required": ["extension_name"]
            }
        }
    }));

    // Meta-tool for On-Demand Skill Activation from .skill / .skills catalog
    schema.push(json!({
        "type": "function",
        "function": {
            "name": "use_skill",
            "description": "Load and activate specialized skill guidelines on-demand from the workspace's .skill or .skills catalog. Returns the complete instructions and persona guidelines for the skill.",
            "parameters": {
                "type": "object",
                "properties": {
                    "skill_name": {
                        "type": "string",
                        "description": "The slug or name of the skill to use (e.g. 'code-check')"
                    }
                },
                "required": ["skill_name"]
            }
        }
    }));

    // Dynamic MCP Management Tools (Session & Catalog Lifecycle)
    schema.push(json!({
        "type": "function",
        "function": {
            "name": "mcp_register_server",
            "description": "Register or update an MCP server blueprint globally into the system catalog (mcp_servers). Specify server id (e.g. 'figcraft', 'cartesia'), transport type ('stdio' or 'http'), command or URL, and optional args.",
            "parameters": {
                "type": "object",
                "properties": {
                    "server_id": {
                        "type": "string",
                        "description": "Unique identifier for the MCP server (e.g. 'figcraft', 'cartesia', 'playwright')"
                    },
                    "type": {
                        "type": "string",
                        "enum": ["stdio", "http"],
                        "description": "Transport protocol: 'stdio' for local CLI commands or 'http' for remote SSE/HTTP endpoints"
                    },
                    "command_or_url": {
                        "type": "string",
                        "description": "The executable CLI command (e.g. 'figcraft-cli') or HTTP endpoint URL (e.g. 'https://mcp.cartesia.ai/mcp')"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "CLI arguments for stdio transport (e.g. ['mcp'])"
                    }
                },
                "required": ["server_id", "type", "command_or_url"]
            }
        }
    }));

    schema.push(json!({
        "type": "function",
        "function": {
            "name": "mcp_configure_session",
            "description": "Configure an MCP server for the current chat session. Injects session-specific credentials (such as API keys or auth tokens) and toggles active status.",
            "parameters": {
                "type": "object",
                "properties": {
                    "server_id": {
                        "type": "string",
                        "description": "The identifier of the registered MCP server to configure"
                    },
                    "env": {
                        "type": "object",
                        "description": "Key-value map of environment variables or secret API tokens for this session (e.g. {'CARTESIA_API_KEY': 'sk_...'})"
                    },
                    "is_active": {
                        "type": "boolean",
                        "description": "Whether this server should be active (true) or deactivated (false) in the current session"
                    }
                },
                "required": ["server_id"]
            }
        }
    }));

    schema.push(json!({
        "type": "function",
        "function": {
            "name": "mcp_list_available",
            "description": "List all registered MCP servers in the global catalog and check their status in the current session.",
            "parameters": {
                "type": "object",
                "properties": {}
            }
        }
    }));

    // 2.5 Active Project MCP Tools (Registered directly as native LLM functions)
    for tool in mcp_tools {
        let desc = tool
            .definition
            .description
            .clone()
            .unwrap_or_else(|| format!("Active MCP tool '{}' from server '{}'", tool.original_name, tool.server_name));
        let params = if tool.definition.input_schema.is_object() {
            tool.definition.input_schema.clone()
        } else {
            json!({
                "type": "object",
                "properties": {}
            })
        };

        schema.push(json!({
            "type": "function",
            "function": {
                "name": tool.namespaced_name,
                "description": desc,
                "parameters": params
            }
        }));
    }

    let shell_info = crate::agent::terminal::active_shell_info();

    // 3. Generic Shell Fallback (Always placed LAST to discourage manual shell scripts for tasks with dedicated tools)
    schema.push(json!({
        "type": "function",
        "function": {
            "name": "workspace_run_command",
            "description": format!(
                "Execute terminal command on host OS '{}' using {}. NOTE: Always prefer dedicated workspace_* and active mcp__* tools first. Only use this command for project-wide compilation/builds (e.g. 'cargo build', 'npm run build'), running test suites ('cargo test', 'npm test'), git commits, or tasks where no dedicated tool exists. Do NOT use this with 'curl' or custom scripts when dedicated fetch/inspect tools are available.",
                std::env::consts::OS,
                shell_info.description
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": format!("Full shell command to execute. {}", shell_info.multiple_cmd_hint)
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory relative to workspace or folder alias (defaults to primary root)"
                    },
                    "shell": {
                        "type": "string",
                        "enum": ["auto", "bash", "powershell", "pwsh", "cmd", "zsh", "sh"],
                        "description": "Optional shell interpreter override. Supported: 'auto' (default), 'bash' (Git Bash on Windows or /bin/bash on Unix), 'powershell', 'pwsh' (PowerShell Core), 'cmd' (Windows cmd.exe), 'zsh', 'sh'."
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "description": "Optional maximum execution timeout in seconds (default: 120, max: 600)."
                    }
                },
                "required": ["command"]
            }
        }
    }));

    schema
}

/// Execute a tool call safely
/// Normalizes terminal commands by stripping redundant root folder prefixes in `cd` calls.
/// e.g. when `work_dir` is `/home/user/scenario`, `cd scenario/backend` is automatically normalized to `cd backend`.
pub fn normalize_workspace_command(
    command: &str,
    work_dir: &Path,
    allowed_roots: &[String],
) -> String {
    let mut normalized = command.to_string();

    let mut root_folder_names = Vec::new();
    if let Some(name) = work_dir.file_name().and_then(|n| n.to_str()) {
        root_folder_names.push(name.to_string());
    }
    for root in allowed_roots {
        let p = Path::new(root);
        if let Some(name) = p.file_name().and_then(|n| n.to_str())
            && !root_folder_names.contains(&name.to_string())
        {
            root_folder_names.push(name.to_string());
        }
    }

    for folder_name in root_folder_names {
        if !work_dir.join(&folder_name).exists() {
            let pat1 = format!("cd {}/", folder_name);
            let pat2 = format!("cd ./{}/", folder_name);
            normalized = normalized.replace(&pat2, "cd ");
            normalized = normalized.replace(&pat1, "cd ");

            let pat3 = format!("cd {}", folder_name);
            let pat4 = format!("cd ./{}", folder_name);
            for delim in &[" ", ";", "&&", ")", "\n"] {
                normalized =
                    normalized.replace(&format!("{}{}", pat4, delim), &format!("cd .{}", delim));
                normalized =
                    normalized.replace(&format!("{}{}", pat3, delim), &format!("cd .{}", delim));
            }
            if normalized.trim() == pat3 || normalized.trim() == pat4 {
                normalized = "cd .".to_string();
            }
        }
    }

    normalized
}

/// Validates that a shell command does not contain obvious destructive or system-bricking operations.
pub fn validate_safe_command(command: &str) -> Result<()> {
    let lower = command.to_lowercase();
    let trimmed = lower.trim();

    // Check for fork bombs
    if trimmed.contains(":(){ :|:& };:") || trimmed.contains(":(){:|:&};:") {
        bail!("Operation blocked: Detected fork bomb pattern.");
    }

    // Check for root destruction patterns
    let dangerous_rm = [
        "rm -rf /",
        "rm -fr /",
        "rm -rf /*",
        "rm -fr /*",
        "rm -r -f /",
        "rm -f -r /",
    ];
    for pat in &dangerous_rm {
        if trimmed.contains(pat) {
            bail!(
                "Operation blocked: Destructive command pattern '{}' is not permitted.",
                pat
            );
        }
    }

    // Check for raw block device writes / format
    let block_dev_patterns = [
        "> /dev/sd",
        "> /dev/nvme",
        "> /dev/hd",
        ">/dev/sd",
        ">/dev/nvme",
        ">/dev/hd",
        "mkfs.",
        "mkfs ",
    ];
    for pat in &block_dev_patterns {
        if trimmed.contains(pat) {
            bail!(
                "Operation blocked: Potential raw disk write or format pattern '{}' is not permitted.",
                pat
            );
        }
    }

    // Check for shutdown / reboot commands
    let shutdown_patterns = [
        "shutdown -h",
        "shutdown -r",
        "shutdown now",
        "poweroff",
        "reboot",
        "init 0",
        "init 6",
    ];
    for pat in &shutdown_patterns {
        for token in trimmed.split([';', '&', '|', '\n']) {
            let token_trim = token.trim();
            if token_trim == *pat || token_trim.starts_with(&format!("{} ", pat)) {
                bail!(
                    "Operation blocked: System power control command '{}' is not permitted.",
                    pat
                );
            }
        }
    }

    // Block direct edits/tampering with node_modules or .git via shell commands
    let node_modules_write_patterns = ["node_modules/", "node_modules ", ".git/"];
    let is_write_like = trimmed.contains("> ")
        || trimmed.contains(">>")
        || trimmed.contains("rm ")
        || trimmed.contains("mv ")
        || trimmed.contains("cp ")
        || trimmed.contains("python3 -c")
        || trimmed.contains("python -c");
    if is_write_like {
        for pat in &node_modules_write_patterns {
            if trimmed.contains(pat) {
                bail!(
                    "Operation blocked: Direct modification or tampering of internal '{}' via terminal commands is not permitted. Apply fixes directly to project application source code instead.",
                    pat
                );
            }
        }
    }

    Ok(())
}

pub async fn execute_tool_call(
    name: &str,
    args: &serde_json::Value,
    allowed_roots: &[String],
    session_id: &str,
) -> Result<String> {
    match name {
        "workspace_list_dir" => {
            let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let resolved = resolve_allowed_path(path_str, allowed_roots)?;

            if !resolved.exists() {
                bail!("Directory '{}' not found", path_str);
            }
            if !resolved.is_dir() {
                bail!("'{}' is not a directory", path_str);
            }

            let requested_depth = args
                .get("depth")
                .and_then(|v| v.as_u64())
                .map(|d| (d as usize).clamp(1, 3))
                .unwrap_or(1);

            let mut builder = ignore::WalkBuilder::new(&resolved);
            builder
                .max_depth(Some(requested_depth))
                .hidden(false)
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                .parents(true);

            let mut overrides = ignore::overrides::OverrideBuilder::new(&resolved);
            let _ = overrides.add("!.git");
            let _ = overrides.add("!.git/**");
            let _ = overrides.add("!.DS_Store");
            let _ = overrides.add("!Thumbs.db");
            if let Ok(ov) = overrides.build() {
                builder.overrides(ov);
            }

            let mut entries = Vec::new();
            let mut total_files = 0;
            let mut total_dirs = 0;

            for entry in builder.build().flatten() {
                if entry.depth() == 0 {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if name == ".git" || name == ".DS_Store" || name == "Thumbs.db" {
                    continue;
                }
                let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                let entry_path = entry.path();

                let rel_display = if requested_depth > 1 {
                    entry_path
                        .strip_prefix(&resolved)
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_else(|_| name.clone())
                } else {
                    name.clone()
                };

                if is_dir {
                    total_dirs += 1;
                    // Count immediate children inside this directory (shallow check)
                    let child_count = std_fs::read_dir(entry_path)
                        .map(|rd| {
                            rd.flatten()
                                .filter(|e| {
                                    let n = e.file_name().to_string_lossy().to_string();
                                    n != ".git" && n != ".DS_Store" && n != "Thumbs.db"
                                })
                                .count()
                        })
                        .unwrap_or(0);

                    entries.push(format!(
                        "{}/ ({} item{})",
                        rel_display,
                        child_count,
                        if child_count == 1 { "" } else { "s" }
                    ));
                } else {
                    total_files += 1;
                    entries.push(rel_display);
                }
            }
            entries.sort();
            let display_path = to_workspace_folder_path(&resolved, allowed_roots);
            Ok(json!({
                "path": display_path,
                "total_files": total_files,
                "total_dirs": total_dirs,
                "entries": entries
            })
            .to_string())
        }

        "workspace_read_file" => {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .context("Parameter 'file_path' is required")?;
            let resolved = resolve_allowed_path(file_path, allowed_roots)?;

            if !resolved.exists() {
                bail!("File '{}' not found", file_path);
            }
            if resolved.is_dir() {
                bail!("'{}' is a directory, not a file", file_path);
            }

            let content = std_fs::read_to_string(&resolved)
                .context(format!("Failed to read file '{}'", file_path))?;

            let parse_line = |key: &str| -> Option<usize> {
                args.get(key).and_then(|v| {
                    v.as_u64()
                        .map(|n| n as usize)
                        .or_else(|| v.as_i64().filter(|&n| n > 0).map(|n| n as usize))
                        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<usize>().ok()))
                })
            };
            let start = parse_line("start_line");
            let end = parse_line("end_line");
            let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("");
            let is_explicit_outline = mode == "outline"
                || args
                    .get("outline")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
            let is_explicit_full =
                mode == "full" || args.get("full").and_then(|v| v.as_bool()).unwrap_or(false);

            if start.is_some() || end.is_some() {
                let lines: Vec<&str> = content.lines().collect();
                let start_idx = start.unwrap_or(1).saturating_sub(1);
                let end_idx = end.unwrap_or(lines.len()).min(lines.len());
                let sliced = if start_idx < end_idx {
                    lines[start_idx..end_idx]
                        .iter()
                        .enumerate()
                        .map(|(idx, line)| format!("{:>6}\t{}", start_idx + idx + 1, line))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    String::new()
                };
                Ok(sliced)
            } else if is_explicit_outline || (!is_explicit_full && content.lines().count() > 250) {
                let display_path = to_workspace_folder_path(&resolved, allowed_roots);
                Ok(generate_file_outline(&content, &display_path))
            } else {
                Ok(content)
            }
        }

        "workspace_write_file" => {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .context("Parameter 'file_path' is required")?;
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .context("Parameter 'content' is required")?;

            let resolved = resolve_allowed_path(file_path, allowed_roots)?;
            // BUG-06: this used to be `let _ = ...backup_file(...)`, which discarded the
            // error. A failed backup means the write below cannot be undone, but the tool
            // description promises a rollback is always available, so the write is refused
            // rather than performed un-restorably.
            crate::agent::snapshot::for_session(session_id)
                .backup_file(&resolved)
                .with_context(|| {
                    format!(
                        "Refusing to write '{}': a rollback point could not be recorded",
                        file_path
                    )
                })?;
            if let Some(parent) = resolved.parent() {
                std_fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directories for '{}'", file_path))?;
            }
            std_fs::write(&resolved, content)
                .with_context(|| format!("Failed to write to '{}'", file_path))?;

            Ok(json!({
                "status": "success",
                "bytes_written": content.len(),
                "file_path": file_path,
            })
            .to_string())
        }
        "workspace_rollback" => {
            // Backups are keyed by session, so a rollback can only ever see files this
            // session wrote. That is what the description promised all along.
            let snapshot = crate::agent::snapshot::for_session(session_id);
            let file_path = args.get("file_path").and_then(|v| v.as_str());
            if let Some(fp) = file_path {
                let resolved = resolve_allowed_path(fp, allowed_roots)?;
                let restored = snapshot.rollback_file(&resolved)?;
                // `false` means nothing was recorded for this path in this session, i.e. the
                // file was never modified here. Reporting `"success"` with `restored: false`
                // made the model claim a revert that never happened.
                let status = if restored { "success" } else { "not_tracked" };
                Ok(json!({
                    "status": status,
                    "restored": restored,
                    "file_path": fp,
                    "message": if restored {
                        "File restored to its pre-session state."
                    } else {
                        "No backup exists for this file in this session, so nothing was changed. \
                         Do not claim it was reverted."
                    }
                })
                .to_string())
            } else {
                let report = snapshot.rollback_all()?;
                // Read the verdict before `restored` is consumed by `into_iter` below.
                let complete = report.is_complete();
                let paths: Vec<String> = report
                    .restored
                    .into_iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();
                // A partial rollback must not be advertised as success. The model decides
                // what to tell the user from this payload, and `"success"` here previously
                // meant "listed every tracked file", not "restored every tracked file".
                if !complete {
                    let failures: Vec<serde_json::Value> = report
                        .failed
                        .iter()
                        .map(|(p, err)| {
                            json!({
                                "file_path": p.to_string_lossy().to_string(),
                                "error": err,
                            })
                        })
                        .collect();
                    return Ok(json!({
                        "status": "partial",
                        "restored_count": paths.len(),
                        "restored_files": paths,
                        "failed_count": failures.len(),
                        "failed_files": failures,
                        "message": "Some files could not be restored and are still modified. \
                                    Report this to the user; do not claim the rollback succeeded.",
                    })
                    .to_string());
                }
                Ok(json!({
                    "status": "success",
                    "restored_count": paths.len(),
                    "restored_files": paths
                })
                .to_string())
            }
        }
        "workspace_edit_file" => {
            let file_path = args
                .get("file_path")
                .or_else(|| args.get("path"))
                .and_then(|v| v.as_str())
                .context("Parameter 'file_path' is required")?;
            let old_text = args
                .get("old_text")
                .and_then(|v| v.as_str())
                .context("Parameter 'old_text' is required")?;
            let new_text = args
                .get("new_text")
                .and_then(|v| v.as_str())
                .context("Parameter 'new_text' is required")?;

            let resolved = resolve_allowed_path(file_path, allowed_roots)?;

            if !resolved.exists() {
                bail!("File '{}' not found to edit", file_path);
            }
            if resolved.is_dir() {
                bail!("'{}' is a directory, not a file", file_path);
            }

            // BUG-06: the backup used to run *before* the two checks above and its error was
            // discarded. Backing up first recorded a rollback point for a call that then
            // failed — an entry the session would later "restore", rewriting a file the turn
            // never touched. Checking first means a backup only exists for edits that happen.
            crate::agent::snapshot::for_session(session_id)
                .backup_file(&resolved)
                .with_context(|| {
                    format!(
                        "Refusing to edit '{}': a rollback point could not be recorded",
                        file_path
                    )
                })?;

            let content = std_fs::read_to_string(&resolved)
                .context(format!("Failed to read file '{}' for editing", file_path))?;

            let updated_content = apply_resilient_edit(&content, old_text, new_text, file_path)?;

            std_fs::write(&resolved, &updated_content)?;

            let display_path = to_workspace_folder_path(&resolved, allowed_roots);
            let result_json = serde_json::json!({
                "status": "ok",
                "path": display_path,
                "file_path": file_path,
                "bytes_written": updated_content.len(),
                "old_text_len": old_text.len(),
                "new_text_len": new_text.len(),
            });

            Ok(result_json.to_string())
        }

        "workspace_grep" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .context("Parameter 'query' is required")?
                .to_string();
            let sub_path = args
                .get("folder")
                .or_else(|| args.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let resolved = resolve_allowed_path(sub_path, allowed_roots)?;

            let resolved_clone = resolved.clone();
            let roots_clone = allowed_roots.to_vec();
            let query_clone = query.clone();

            let grep_result = tokio::task::spawn_blocking(move || {
                run_workspace_grep(&resolved_clone, &query_clone, &roots_clone)
            })
            .await
            .context("Workspace grep worker failed")??;

            // Format into token-efficient tabular grep format for LLM context
            let matches = grep_result.get("results").and_then(|r| r.as_array());
            let truncated = grep_result
                .get("truncated")
                .and_then(|t| t.as_bool())
                .unwrap_or(false);
            if let Some(list) = matches {
                if list.is_empty() {
                    Ok(format!("No matches found for query '{}'", query))
                } else {
                    let mut out = format!(
                        "Found {} match(es) for query '{}'{}:\n",
                        list.len(),
                        query,
                        if truncated {
                            " (truncated at limit)"
                        } else {
                            ""
                        }
                    );
                    for item in list {
                        let file = item.get("file").and_then(|f| f.as_str()).unwrap_or("");
                        let line = item.get("line").and_then(|l| l.as_u64()).unwrap_or(0);
                        let content = item.get("content").and_then(|c| c.as_str()).unwrap_or("");
                        out.push_str(&format!("- {}:{}: {}\n", file, line, content));
                    }
                    Ok(out)
                }
            } else {
                Ok(grep_result.to_string())
            }
        }

        "workspace_run_command" => {
            let command_str = args
                .get("command")
                .and_then(|v| v.as_str())
                .context("Parameter 'command' is required")?;
            let sub_path = args.get("cwd").and_then(|v| v.as_str()).unwrap_or(".");
            let work_dir = resolve_allowed_path(sub_path, allowed_roots)?;

            if !work_dir.exists() || !work_dir.is_dir() {
                bail!("Working directory '{}' not found", sub_path);
            }

            let effective_command =
                normalize_workspace_command(command_str, &work_dir, allowed_roots);
            validate_safe_command(&effective_command)?;

            let requested_shell = args
                .get("shell")
                .and_then(|v| v.as_str())
                .and_then(crate::agent::terminal::ShellType::parse);

            let std_proc = create_workspace_command_with_shell(&effective_command, requested_shell);
            let mut proc = tokio::process::Command::from(std_proc);
            proc.current_dir(&work_dir);
            proc.kill_on_drop(true);

            // Execute command with configurable asynchronous timeout (default 120s, max 600s)
            let timeout_secs = args
                .get("timeout_seconds")
                .or_else(|| args.get("timeout"))
                .and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_i64().filter(|&n| n > 0).map(|n| n as u64))
                        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
                })
                .map(|s| s.clamp(5, 600))
                .unwrap_or(120);

            let timeout_duration = std::time::Duration::from_secs(timeout_secs);
            let output = match tokio::time::timeout(timeout_duration, proc.output()).await {
                Ok(Ok(out)) => out,
                Ok(Err(e)) => {
                    bail!("Failed to execute shell command: '{}': {}", command_str, e);
                }
                Err(_) => {
                    let mut res_map = serde_json::Map::new();
                    res_map.insert("command".to_string(), serde_json::json!(command_str));
                    res_map.insert(
                        "cwd".to_string(),
                        serde_json::json!(work_dir.display().to_string()),
                    );
                    res_map.insert("exit_code".to_string(), serde_json::json!(-1));
                    res_map.insert("success".to_string(), serde_json::json!(false));
                    res_map.insert("status".to_string(), serde_json::json!("TIMEOUT"));
                    let timeout_msg = format!(
                        "❌ COMMAND TIMED OUT after {} seconds: `{command_str}`. The process was automatically killed. Avoid long-running dev servers, interactive prompts, or hanging tests. If the build or test suite needs more time, specify `timeout_seconds` (up to 600s).",
                        timeout_secs
                    );
                    res_map.insert("stdout".to_string(), serde_json::json!(""));
                    res_map.insert("stderr".to_string(), serde_json::json!(&timeout_msg));
                    res_map.insert("error_banner".to_string(), serde_json::json!(timeout_msg));
                    return Ok(serde_json::Value::Object(res_map).to_string());
                }
            };

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let exit_code = output.status.code().unwrap_or(-1);
            let success = output.status.success() && exit_code == 0;

            // Preserve tail where compiler diagnostics and error traces live (4k head + 16k tail)
            let trimmed_stdout = truncate_smart(&stdout, 4_000, 16_000);
            let trimmed_stderr = truncate_smart(&stderr, 2_000, 8_000);

            let mut res_map = serde_json::Map::new();
            res_map.insert("command".to_string(), serde_json::json!(command_str));
            res_map.insert(
                "cwd".to_string(),
                serde_json::json!(work_dir.display().to_string()),
            );
            res_map.insert("exit_code".to_string(), serde_json::json!(exit_code));
            res_map.insert("success".to_string(), serde_json::json!(success));

            if !success {
                res_map.insert("status".to_string(), serde_json::json!("FAILED"));

                let mut combined_output = String::new();
                if !stdout.is_empty() {
                    combined_output.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    combined_output.push('\n');
                    combined_output.push_str(&stderr);
                }

                let diags =
                    crate::agent::diagnostics::extract_command_diagnostics(&combined_output);
                let banner = if !diags.is_empty() {
                    format!(
                        "❌ COMMAND FAILED with exit code {exit_code}.\nIdentified Errors:\n{}\n\nReview the stdout/stderr below, edit the offending files, and re-run verification.",
                        diags
                            .iter()
                            .map(|d| format!("  - {}", d))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                } else {
                    format!(
                        "❌ COMMAND FAILED with exit code {exit_code}. Review stdout and stderr below to locate errors, fix the offending files, and re-run verification."
                    )
                };

                res_map.insert("error_banner".to_string(), serde_json::json!(banner));
            } else {
                res_map.insert("status".to_string(), serde_json::json!("SUCCESS"));
            }

            res_map.insert("stdout".to_string(), serde_json::json!(trimmed_stdout));
            res_map.insert("stderr".to_string(), serde_json::json!(trimmed_stderr));

            let res_json = serde_json::Value::Object(res_map);

            Ok(res_json.to_string())
        }

        "activate_extension" => {
            let extension_name = args
                .get("extension_name")
                .and_then(|v| v.as_str())
                .context("Parameter 'extension_name' is required")?;
            crate::agent::executor::state::activate_session_extension(session_id, extension_name);

            let all_servers = crate::repository::agent::list_mcp_servers()
                .await
                .unwrap_or_default();
            let matched_server = all_servers
                .into_iter()
                .find(|s| s.name.eq_ignore_ascii_case(extension_name) || s.id.eq_ignore_ascii_case(extension_name));

            let mut details = format!(
                "Extension '{}' has been successfully activated for this session.",
                extension_name
            );

            if let Some(server) = matched_server {
                let tools = crate::agent::mcp::resolve_active_mcp_tools(&[server]).await;
                if !tools.is_empty() {
                    details.push_str("\n\nAvailable tools from this extension:\n");
                    for t in tools {
                        let desc = t.definition.description.as_deref().unwrap_or("").trim();
                        let desc = if desc.is_empty() {
                            "No description provided"
                        } else {
                            desc
                        };
                        let cmd_str = t
                            .markdown_spec
                            .as_deref()
                            .and_then(crate::agent::mcp::extract_command_template);

                        if let Some(usage) = cmd_str {
                            details.push_str(&format!(
                                "- `{}`: {}\n  Usage: `{}`\n",
                                t.original_name, desc, usage
                            ));
                        } else {
                            details.push_str(&format!("- `{}`: {}\n", t.original_name, desc));
                        }
                    }
                    details.push_str("\nYou can now execute these commands or tools directly.");
                } else {
                    details.push_str(
                        " However, no tools were found or discovered for this extension.",
                    );
                }
            } else {
                details.push_str(" Its dedicated tools are now available for execution.");
            }

            Ok(details)
        }

        "use_skill" => {
            let skill_name = args
                .get("skill_name")
                .or_else(|| args.get("name"))
                .or_else(|| args.get("slug"))
                .and_then(|v| v.as_str())
                .context("Parameter 'skill_name' is required")?;

            crate::agent::executor::state::activate_session_skill(session_id, skill_name);

            let session = crate::repository::agent::get_session(session_id)
                .await?
                .context("Session not found")?;
            let project = crate::repository::agent::get_project(&session.project_id)
                .await?
                .context("Project not found")?;
            let allowed_folders =
                crate::agent::executor::parse_project_folders(project.root_path.as_deref());

            let all_skills = crate::agent::skills::scan_project_skills(
                project.root_path.as_deref(),
                &allowed_folders,
            );

            let needle = skill_name.to_lowercase();
            let matched_skill = all_skills.into_iter().find(|s| {
                s.slug.to_lowercase() == needle
                    || s.name.to_lowercase() == needle
                    || s.id.to_lowercase() == needle
                    || needle.contains(&s.slug.to_lowercase())
            });

            if let Some(skill) = matched_skill {
                crate::agent::executor::state::activate_session_skill(session_id, &skill.slug);
                crate::agent::executor::state::activate_session_skill(session_id, &skill.name);
                Ok(format!(
                    "Skill '{}' ({}) loaded and activated successfully.\n\n### Instructions for Skill: {}\n{}",
                    skill.name, skill.slug, skill.name, skill.system_prompt
                ))
            } else {
                Ok(format!(
                    "Skill '{}' was not found in the workspace or global .skill/.skills directories. Please check available skills in the system prompt.",
                    skill_name
                ))
            }
        }

        "mcp_register_server" => {
            let server_id = args
                .get("server_id")
                .or_else(|| args.get("id"))
                .and_then(|v| v.as_str())
                .context("Parameter 'server_id' is required")?
                .trim();
            if server_id.is_empty() {
                bail!("Parameter 'server_id' cannot be empty");
            }

            let transport_type = args
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("stdio")
                .to_lowercase();

            let command_or_url = args
                .get("command_or_url")
                .or_else(|| args.get("command"))
                .or_else(|| args.get("url"))
                .and_then(|v| v.as_str())
                .context("Parameter 'command_or_url' is required")?
                .trim();
            if command_or_url.is_empty() {
                bail!("Parameter 'command_or_url' cannot be empty");
            }

            let config_json = if transport_type == "http" {
                serde_json::json!({
                    "url": command_or_url
                })
            } else {
                let args_list: Vec<String> = args
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                serde_json::json!({
                    "command": command_or_url,
                    "args": args_list
                })
            };

            crate::repository::agent::create_or_update_mcp_server(
                server_id,
                server_id,
                &config_json.to_string(),
                true,
            )
            .await?;

            Ok(format!(
                "MCP server '{}' ({}) successfully registered.\nConfiguration: {}\nYou can activate it using `activate_extension(extension_name: \"{}\")`.",
                server_id, transport_type, config_json, server_id
            ))
        }

        "mcp_configure_session" => {
            let server_id = args
                .get("server_id")
                .or_else(|| args.get("id"))
                .and_then(|v| v.as_str())
                .context("Parameter 'server_id' is required")?
                .trim();
            if server_id.is_empty() {
                bail!("Parameter 'server_id' cannot be empty");
            }

            let is_active = args
                .get("is_active")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            let env_str = if let Some(env_val) = args.get("env") {
                if env_val.is_object() {
                    env_val.to_string()
                } else {
                    "{}".to_string()
                }
            } else {
                "{}".to_string()
            };

            crate::repository::agent::upsert_session_mcp_server(
                session_id,
                server_id,
                &env_str,
                is_active,
            )
            .await?;

            if is_active {
                crate::agent::executor::state::activate_session_extension(session_id, server_id);
            } else {
                crate::agent::executor::state::deactivate_session_extension(session_id, server_id);
            }

            Ok(format!(
                "MCP server '{}' session configuration updated: is_active={}, env configured.",
                server_id, is_active
            ))
        }

        "mcp_list_available" => {
            let servers = crate::repository::agent::list_mcp_servers().await?;
            if servers.is_empty() {
                return Ok("No MCP servers are currently registered in the system catalog.".to_string());
            }

            let session_configs = crate::repository::agent::list_session_mcp_servers(session_id)
                .await
                .unwrap_or_default();
            let mut output = String::from("### 🌐 System MCP Servers:\n");
            for s in servers {
                let session_info = session_configs.iter().find(|sc| sc.server_id == s.id);
                let status_str = match session_info {
                    Some(sc) if sc.is_active == 1 => "Active in current session",
                    Some(_) => "Disabled in current session",
                    None => "Dormant (available to activate)",
                };
                output.push_str(&format!("- `{}` (type: {}): {}\n", s.id, s.r#type, status_str));
            }
            output.push_str("\nTo activate a dormant server in this session, call `activate_extension(extension_name: \"<server_id>\")`.");
            Ok(output)
        }

        _ => bail!("Unknown tool '{}'", name),
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
