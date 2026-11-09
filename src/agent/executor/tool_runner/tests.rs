use super::infer_tool_name_from_call;
use serde_json::json;

#[test]
fn test_infer_from_call_id_prefix() {
    let name = infer_tool_name_from_call("workspace_list_dir_0_9ff280cf", &json!({}));
    assert_eq!(name.as_deref(), Some("workspace_list_dir"));
}

#[test]
fn test_infer_from_arguments_shape() {
    assert_eq!(
        infer_tool_name_from_call("call_x", &json!({"command": "cargo test"})).as_deref(),
        Some("workspace_run_command")
    );
    assert_eq!(
        infer_tool_name_from_call("call_x", &json!({"old_text": "a", "new_text": "b"})).as_deref(),
        Some("workspace_edit_file")
    );
    assert_eq!(
        infer_tool_name_from_call("call_x", &json!({"file_path": "a.rs", "content": "x"}))
            .as_deref(),
        Some("workspace_write_file")
    );
    assert_eq!(
        infer_tool_name_from_call("call_x", &json!({"pattern": "fn main"})).as_deref(),
        Some("workspace_grep")
    );
    assert_eq!(
        infer_tool_name_from_call("call_x", &json!({"path": "a.rs", "start_line": 1})).as_deref(),
        Some("workspace_read_file")
    );
    assert_eq!(
        infer_tool_name_from_call("call_x", &json!({"path": "src", "depth": 2})).as_deref(),
        Some("workspace_list_dir")
    );
}

#[test]
fn test_infer_returns_none_when_unrecoverable() {
    assert_eq!(infer_tool_name_from_call("call_x", &json!({})), None);
}
