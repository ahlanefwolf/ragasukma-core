//! Heuristics for recovering tool identity when the name is missing.

pub(super) fn infer_tool_name_from_call(call_id: &str, args: &serde_json::Value) -> Option<String> {
    // 1. Some providers embed the tool name in the call id, e.g.
    //    `workspace_list_dir_0_9ff280cf` or `call_workspace_grep_x`.
    const KNOWN_TOOLS: [&str; 6] = [
        "workspace_list_dir",
        "workspace_read_file",
        "workspace_write_file",
        "workspace_edit_file",
        "workspace_grep",
        "workspace_run_command",
    ];

    for tool in KNOWN_TOOLS {
        if call_id.contains(tool) {
            return Some(tool.to_string());
        }
    }

    // 2. Fall back to the argument shape, which is a strong signal.
    let has = |key: &str| args.get(key).is_some();
    if has("command") {
        return Some("workspace_run_command".to_string());
    }
    if has("old_text") {
        return Some("workspace_edit_file".to_string());
    }
    if has("content") && has("file_path") {
        return Some("workspace_write_file".to_string());
    }
    if has("pattern") {
        return Some("workspace_grep".to_string());
    }
    if has("start_line") || has("end_line") {
        return Some("workspace_read_file".to_string());
    }
    if has("depth") {
        return Some("workspace_list_dir".to_string());
    }

    None
}
