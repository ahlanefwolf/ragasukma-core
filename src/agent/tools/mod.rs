pub mod dialogs;
pub mod editor;
pub mod fs;
pub mod search;
pub mod tags;

use anyhow::{bail, Context, Result};
use serde_json::json;
use std::fs as std_fs;
use std::path::Path;

pub use dialogs::{open_path_in_file_manager, pick_folder_dialog};
pub use editor::apply_resilient_edit;
pub use fs::{
    create_workspace_command, ensure_git_and_gitignore, ensure_git_and_gitignore_opt, generate_file_outline,
    generate_smart_gitignore, resolve_allowed_path, to_workspace_folder_path,
};
pub use search::{list_workspace_files, run_workspace_grep, WorkspaceFileInfo};
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
pub fn get_dynamic_agent_tools_schema(mcp_tools: &[crate::agent::mcp::ActiveMcpTool]) -> Vec<serde_json::Value> {
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

    // 2. Active Dedicated MCP External Tools (Prioritized over manual shell commands)
    for tool in mcp_tools {
        let desc = format!(
            "[Dedicated Tool from {}] {}",
            tool.server_name,
            tool.definition.description.as_deref().unwrap_or("Active MCP tool")
        );
        schema.push(json!({
            "type": "function",
            "function": {
                "name": tool.namespaced_name,
                "description": desc,
                "parameters": tool.definition.input_schema
            }
        }));
    }

    // 3. Generic Shell Fallback (Always placed LAST to discourage manual shell scripts for tasks with dedicated tools)
    schema.push(json!({
        "type": "function",
        "function": {
            "name": "workspace_run_command",
            "description": format!(
                "Execute terminal command on host OS '{}' using {}. NOTE: Always prefer dedicated workspace_* and active mcp__* tools first. Only use this command for project-wide compilation/builds (e.g. 'cargo build', 'npm run build'), running test suites ('cargo test', 'npm test'), git commits, or tasks where no dedicated tool exists. Do NOT use this with 'curl' or custom scripts when dedicated fetch/inspect tools are available.",
                std::env::consts::OS,
                if cfg!(target_os = "windows") { "PowerShell" } else { "user login shell (bash/zsh)" }
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Full shell command to execute. Multiple sequential commands can be combined (use ';' on Windows PowerShell, '&&' on Unix)."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory relative to workspace or folder alias (defaults to primary root)"
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
pub fn normalize_workspace_command(command: &str, work_dir: &Path, allowed_roots: &[String]) -> String {
    let mut normalized = command.to_string();

    let mut root_folder_names = Vec::new();
    if let Some(name) = work_dir.file_name().and_then(|n| n.to_str()) {
        root_folder_names.push(name.to_string());
    }
    for root in allowed_roots {
        let p = Path::new(root);
        if let Some(name) = p.file_name().and_then(|n| n.to_str())
            && !root_folder_names.contains(&name.to_string()) {
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
                normalized = normalized.replace(&format!("{}{}", pat4, delim), &format!("cd .{}", delim));
                normalized = normalized.replace(&format!("{}{}", pat3, delim), &format!("cd .{}", delim));
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
            bail!("Operation blocked: Destructive command pattern '{}' is not permitted.", pat);
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
            bail!("Operation blocked: Potential raw disk write or format pattern '{}' is not permitted.", pat);
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
                bail!("Operation blocked: System power control command '{}' is not permitted.", pat);
            }
        }
    }

    // Block direct edits/tampering with node_modules or .git via shell commands
    let node_modules_write_patterns = [
        "node_modules/",
        "node_modules ",
        ".git/",
    ];
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
                bail!("Operation blocked: Direct modification or tampering of internal '{}' via terminal commands is not permitted. Apply fixes directly to project application source code instead.", pat);
            }
        }
    }

    Ok(())
}

pub async fn execute_tool_call(
    name: &str,
    args: &serde_json::Value,
    allowed_roots: &[String],
) -> Result<String> {
    match name {
        "workspace_list_dir" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
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

                    entries.push(format!("{}/ ({} item{})", rel_display, child_count, if child_count == 1 { "" } else { "s" }));
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
            }).to_string())
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
            let is_explicit_outline = mode == "outline" || args.get("outline").and_then(|v| v.as_bool()).unwrap_or(false);
            let is_explicit_full = mode == "full" || args.get("full").and_then(|v| v.as_bool()).unwrap_or(false);

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
            let _ = crate::agent::snapshot::WorkspaceSnapshot::global().backup_file(&resolved);
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
            }).to_string())
        }
        "workspace_rollback" => {
            let file_path = args.get("file_path").and_then(|v| v.as_str());
            if let Some(fp) = file_path {
                let resolved = resolve_allowed_path(fp, allowed_roots)?;
                let restored = crate::agent::snapshot::WorkspaceSnapshot::global().rollback_file(&resolved)?;
                Ok(json!({
                    "status": "success",
                    "restored": restored,
                    "file_path": fp
                }).to_string())
            } else {
                let restored = crate::agent::snapshot::WorkspaceSnapshot::global().rollback_all()?;
                let paths: Vec<String> = restored.into_iter().map(|p| p.to_string_lossy().to_string()).collect();
                Ok(json!({
                    "status": "success",
                    "restored_count": paths.len(),
                    "restored_files": paths
                }).to_string())
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
            let _ = crate::agent::snapshot::WorkspaceSnapshot::global().backup_file(&resolved);

            if !resolved.exists() {
                bail!("File '{}' not found to edit", file_path);
            }
            if resolved.is_dir() {
                bail!("'{}' is a directory, not a file", file_path);
            }

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
            let truncated = grep_result.get("truncated").and_then(|t| t.as_bool()).unwrap_or(false);
            if let Some(list) = matches {
                if list.is_empty() {
                    Ok(format!("No matches found for query '{}'", query))
                } else {
                    let mut out = format!(
                        "Found {} match(es) for query '{}'{}:\n",
                        list.len(),
                        query,
                        if truncated { " (truncated at limit)" } else { "" }
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
            let sub_path = args
                .get("cwd")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let work_dir = resolve_allowed_path(sub_path, allowed_roots)?;

            if !work_dir.exists() || !work_dir.is_dir() {
                bail!("Working directory '{}' not found", sub_path);
            }

            let effective_command = normalize_workspace_command(command_str, &work_dir, allowed_roots);
            validate_safe_command(&effective_command)?;

            let std_proc = create_workspace_command(&effective_command);
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
                    res_map.insert("cwd".to_string(), serde_json::json!(work_dir.display().to_string()));
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
            res_map.insert("cwd".to_string(), serde_json::json!(work_dir.display().to_string()));
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

                let diags = crate::agent::diagnostics::extract_command_diagnostics(&combined_output);
                let banner = if !diags.is_empty() {
                    format!(
                        "❌ COMMAND FAILED with exit code {exit_code}.\nIdentified Errors:\n{}\n\nReview the stdout/stderr below, edit the offending files, and re-run verification.",
                        diags.iter().map(|d| format!("  - {}", d)).collect::<Vec<_>>().join("\n")
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

        _ => bail!("Unknown tool '{}'", name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn test_generate_smart_gitignore_node() {
        let temp_dir = std::env::temp_dir().join(format!("ragasukma_test_node_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = fs::create_dir_all(&temp_dir);
        let _ = fs::write(temp_dir.join("package.json"), "{}");

        let gitignore = generate_smart_gitignore(&temp_dir);
        assert!(gitignore.contains("node_modules/"));
        assert!(gitignore.contains("dist/"));
        assert!(gitignore.contains(".DS_Store"));
        assert!(gitignore.contains(".env"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_generate_smart_gitignore_rust() {
        let temp_dir = std::env::temp_dir().join(format!("ragasukma_test_rust_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = fs::create_dir_all(&temp_dir);
        let _ = fs::write(temp_dir.join("Cargo.toml"), "[package]\nname = \"foo\"");

        let gitignore = generate_smart_gitignore(&temp_dir);
        assert!(gitignore.contains("/target/"));
        assert!(gitignore.contains(".DS_Store"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_generate_smart_gitignore_python() {
        let temp_dir = std::env::temp_dir().join(format!("ragasukma_test_py_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = fs::create_dir_all(&temp_dir);
        let _ = fs::write(temp_dir.join("requirements.txt"), "flask==2.0.0");

        let gitignore = generate_smart_gitignore(&temp_dir);
        assert!(gitignore.contains("__pycache__/"));
        assert!(gitignore.contains(".venv/"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_ensure_git_and_gitignore_initializes() {
        let temp_dir = std::env::temp_dir().join(format!("ragasukma_test_init_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = fs::create_dir_all(&temp_dir);
        let _ = fs::write(temp_dir.join("package.json"), "{}");

        let res = ensure_git_and_gitignore(&temp_dir);
        assert!(res.is_ok());
        assert!(temp_dir.join(".git").exists());
        assert!(temp_dir.join(".gitignore").exists());

        let content = fs::read_to_string(temp_dir.join(".gitignore")).unwrap();
        assert!(content.contains("node_modules/"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_run_workspace_grep_line_truncation() {
        let temp_dir = std::env::temp_dir().join(format!("ragasukma_test_grep_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = fs::create_dir_all(&temp_dir);
        let long_line = format!("const TARGET_STRING = '{}';", "A".repeat(1200));
        let test_file = temp_dir.join("sample.js");
        let _ = fs::write(&test_file, &long_line);

        let roots = vec![temp_dir.to_string_lossy().to_string()];
        let val = run_workspace_grep(&temp_dir, "TARGET_STRING", &roots).unwrap();
        let matches = val.get("results").and_then(|r| r.as_array()).unwrap();
        assert_eq!(matches.len(), 1);
        let snippet = matches[0].get("content").and_then(|c| c.as_str()).unwrap();
        assert!(snippet.contains("[truncated]"));
        assert!(snippet.len() < 700);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_to_workspace_folder_path() {
        let roots = vec!["/tmp/my-project".to_string()];
        let path = Path::new("/tmp/my-project/src/components/Button.tsx");
        assert_eq!(
            to_workspace_folder_path(path, &roots),
            "my-project/src/components/Button.tsx"
        );
    }

    #[test]
    fn test_normalize_workspace_command() {
        let temp_dir = std::env::temp_dir().join(format!("ragasukma_cmd_norm_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = fs::create_dir_all(&temp_dir);
        let roots = vec![temp_dir.to_string_lossy().to_string()];
        let folder_name = temp_dir.file_name().unwrap().to_str().unwrap();

        let cmd = format!("(cd {}/backend && cargo test) && (cd {}/frontend && npm test)", folder_name, folder_name);
        let normalized = normalize_workspace_command(&cmd, &temp_dir, &roots);
        assert_eq!(normalized, "(cd backend && cargo test) && (cd frontend && npm test)");

        let single_cmd = format!("cd {} && cargo test", folder_name);
        let normalized_single = normalize_workspace_command(&single_cmd, &temp_dir, &roots);
        assert_eq!(normalized_single, "cd . && cargo test");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_expand_file_tags_no_tags() {
        let roots = vec!["/tmp".to_string()];
        let input = "hello world without tags";
        assert_eq!(expand_file_tags(input, &roots), input);
    }

    #[test]
    fn test_create_workspace_command_execution() {
        let mut cmd = create_workspace_command("echo '__agent_shell_ok__'");
        let output = cmd.output().expect("Failed to execute test command");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("__agent_shell_ok__"));
    }

    #[test]
    fn test_apply_resilient_edit_whitespace_and_indentation() {
        let content = "function foo() {\n    const a = 1;\n    const b = 2;\n    return a + b;\n}";
        // AI sent 2 spaces instead of 4 spaces
        let old_text = "  const a = 1;\n  const b = 2;";
        let new_text = "  const a = 10;\n  const b = 20;";
        let updated = apply_resilient_edit(content, old_text, new_text, "test.ts").unwrap();
        assert!(updated.contains("const a = 10;"));
        assert!(updated.contains("return a + b;"));
    }

    #[test]
    fn test_apply_resilient_edit_quote_and_semicolon_tolerance() {
        let content = "export const config = {\n  mode: 'production',\n  port: 8080\n};";
        // AI used double quotes and trailing comma
        let old_text = "mode: \"production\",";
        let new_text = "mode: 'staging',";
        let updated = apply_resilient_edit(content, old_text, new_text, "config.ts").unwrap();
        assert!(updated.contains("mode: 'staging'"));
    }

    #[test]
    fn test_apply_resilient_edit_anchor_matching() {
        let content = "class Service {\n  init() {\n    // comment here\n    setup();\n  }\n}";
        // AI skipped the comment inside
        let old_text = "init() {\n  setup();\n}";
        let new_text = "init() {\n  initV2();\n}";
        let updated = apply_resilient_edit(content, old_text, new_text, "service.ts").unwrap();
        assert!(updated.contains("initV2();"));
    }

    #[test]
    fn test_apply_resilient_edit_diagnostic_hint() {
        let content = "line 1\nline 2\nconst specialFeature = true;\nline 4\nline 5";
        let err = apply_resilient_edit(content, "function completelyDifferentNonExistentFeature() {\n  return 999;\n}", "new", "test.ts").unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("'old_text' not found"));
        assert!(err_msg.contains("Use `workspace_read_file`") || err_msg.contains("Closest actual content in 'test.ts'"));
    }

    #[test]
    fn test_generate_file_outline_polyglot() {
        let mut sample_code = String::new();
        sample_code.push_str("# Project Documentation Header\n");
        sample_code.push_str("pub struct AppConfig {\n");
        sample_code.push_str("    pub port: u16,\n");
        sample_code.push_str("}\n\n");
        sample_code.push_str("pub fn start_server() {\n");
        sample_code.push_str("    println!(\"Server running\");\n");
        sample_code.push_str("}\n\n");
        sample_code.push_str("class UserService:\n");
        sample_code.push_str("    def get_user(self, user_id):\n");
        sample_code.push_str("        pass\n\n");
        sample_code.push_str("[database_section]\n");
        sample_code.push_str("host = \"localhost\"\n");

        for i in 0..100 {
            sample_code.push_str(&format!("// filler line {}\n", i));
        }

        let outline = generate_file_outline(&sample_code, "test/app.code");
        assert!(outline.contains("# File outline for 'test/app.code'"));
        assert!(outline.contains("Project Documentation Header"));
        assert!(outline.contains("pub struct AppConfig"));
        assert!(outline.contains("pub fn start_server"));
        assert!(outline.contains("class UserService:"));
        assert!(outline.contains("[database_section]"));
        assert!(outline.contains("L   1:"));
    }

    #[tokio::test]
    async fn test_workspace_read_file_slice_and_outline() {
        let temp_dir = std::env::temp_dir().join(format!("ragasukma_read_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = std_fs::create_dir_all(&temp_dir);

        let mut lines = Vec::new();
        for i in 1..=300 {
            if i == 10 {
                lines.push("pub fn initialize_core() {".to_string());
            } else if i == 50 {
                lines.push("pub fn terminate_core() {".to_string());
            } else {
                lines.push(format!("    let val_{} = {};", i, i));
            }
        }
        let content = lines.join("\n");
        let file_path = temp_dir.join("large_file.src");
        let _ = std_fs::write(&file_path, &content);

        let roots = vec![temp_dir.to_string_lossy().to_string()];

        // 1. Unspecified lines on large file (>250 lines) returns outline by default
        let out_default = execute_tool_call(
            "workspace_read_file",
            &serde_json::json!({ "file_path": "large_file.src" }),
            &roots,
        ).await.unwrap();
        assert!(out_default.contains("# File outline for"));
        assert!(out_default.contains("initialize_core"));
        assert!(out_default.contains("terminate_core"));

        // 2. Targeted slice using start_line and end_line (lines 9, 10, 11, 12 = 4 lines)
        let out_slice = execute_tool_call(
            "workspace_read_file",
            &serde_json::json!({
                "file_path": "large_file.src",
                "start_line": 9,
                "end_line": 12
            }),
            &roots,
        ).await.unwrap();
        assert!(out_slice.contains("pub fn initialize_core"));
        assert_eq!(out_slice.lines().count(), 4);

        // 3. Explicit mode: "full" on large file returns raw content
        let out_full = execute_tool_call(
            "workspace_read_file",
            &serde_json::json!({
                "file_path": "large_file.src",
                "mode": "full"
            }),
            &roots,
        ).await.unwrap();
        assert_eq!(out_full.lines().count(), 300);

        let _ = std_fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_workspace_list_dir_shallow_with_counts() {
        let temp_dir = std::env::temp_dir().join(format!("ragasukma_listdir_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let sub_dir = temp_dir.join("subfolder");
        let _ = std_fs::create_dir_all(&sub_dir);

        let _ = std_fs::write(temp_dir.join("root_file.txt"), "hello");
        let _ = std_fs::write(sub_dir.join("child_1.txt"), "c1");
        let _ = std_fs::write(sub_dir.join("child_2.txt"), "c2");

        let roots = vec![temp_dir.to_string_lossy().to_string()];

        let res_raw = execute_tool_call(
            "workspace_list_dir",
            &serde_json::json!({ "path": "." }),
            &roots,
        ).await.unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&res_raw).unwrap();
        let entries = parsed.get("entries").and_then(|e| e.as_array()).unwrap();
        assert_eq!(parsed["total_dirs"], 1);
        assert_eq!(parsed["total_files"], 1);

        let has_child_count = entries.iter().any(|e| {
            e.as_str().map_or(false, |s| s.contains("subfolder/ (2 items)"))
        });
        assert!(has_child_count);

        let _ = std_fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_workspace_write_file_compact_metadata() {
        let temp_dir = std::env::temp_dir().join(format!("ragasukma_write_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = std_fs::create_dir_all(&temp_dir);

        let roots = vec![temp_dir.to_string_lossy().to_string()];
        let out_json_str = execute_tool_call(
            "workspace_write_file",
            &serde_json::json!({
                "file_path": "new_created_file.txt",
                "content": "Line A\nLine B\nLine C"
            }),
            &roots,
        ).await.unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&out_json_str).unwrap();
        assert_eq!(parsed["status"], "success");
        assert_eq!(parsed["bytes_written"], 20);
        assert!(parsed.get("old_content").is_none());
        assert!(parsed.get("new_content").is_none());

        let _ = std_fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_workspace_grep_compact_tabular_format() {
        let temp_dir = std::env::temp_dir().join(format!("ragasukma_grep_tab_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = std_fs::create_dir_all(&temp_dir);

        let test_file = temp_dir.join("logic.rs");
        let _ = std_fs::write(&test_file, "pub fn calculate_price() {\n    let discount = 10;\n}\n");

        let roots = vec![temp_dir.to_string_lossy().to_string()];
        let grep_out = execute_tool_call(
            "workspace_grep",
            &serde_json::json!({ "query": "calculate_price" }),
            &roots,
        ).await.unwrap();

        assert!(grep_out.contains("Found 1 match(es) for query 'calculate_price'"));
        assert!(grep_out.contains("logic.rs:1: pub fn calculate_price() {"));

        let _ = std_fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_workspace_grep_with_folder_arg_and_edit_file_schema() {
        let temp_dir = std::env::temp_dir().join(format!("ragasukma_grep_folder_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let sub_folder = temp_dir.join("backend");
        let _ = std_fs::create_dir_all(&sub_folder);

        let test_file = sub_folder.join("server.rs");
        let _ = std_fs::write(&test_file, "pub fn start_doku_payment() {}\n");

        let roots = vec![temp_dir.to_string_lossy().to_string()];

        // 1. Validate that schema exposes workspace_edit_file
        let dynamic_schema = get_dynamic_agent_tools_schema(&[]);
        let has_edit = dynamic_schema.iter().any(|s| {
            s.pointer("/function/name").and_then(|n| n.as_str()) == Some("workspace_edit_file")
        });
        assert!(has_edit, "workspace_edit_file must be exposed in dynamic agent tools schema");

        // Every write/edit backs up the file first, so the revert tool must actually be
        // callable. A backup nobody can roll back is dead weight, which is exactly what
        // this tool was before it was added to the schema.
        let has_rollback = dynamic_schema.iter().any(|s| {
            s.pointer("/function/name").and_then(|n| n.as_str()) == Some("workspace_rollback")
        });
        assert!(has_rollback, "workspace_rollback must be exposed so backups can be undone");

        // 2. Validate workspace_grep with 'folder' parameter resolves subfolder cleanly
        let grep_out = execute_tool_call(
            "workspace_grep",
            &serde_json::json!({
                "folder": "backend",
                "query": "start_doku_payment"
            }),
            &roots,
        ).await.unwrap();

        assert!(grep_out.contains("start_doku_payment"));
        assert!(grep_out.contains("server.rs"));

        // 3. Validate workspace_edit_file execution with fallback 'path' and 'file_path'
        let edit_out = execute_tool_call(
            "workspace_edit_file",
            &serde_json::json!({
                "path": "backend/server.rs",
                "old_text": "start_doku_payment",
                "new_text": "start_jokul_doku_payment"
            }),
            &roots,
        ).await.unwrap();

        assert!(edit_out.contains("\"status\":\"ok\""));
        let updated_code = std_fs::read_to_string(&test_file).unwrap();
        assert!(updated_code.contains("start_jokul_doku_payment"));

        let _ = std_fs::remove_dir_all(&temp_dir);
    }
}