use super::*;

/// Session the tool calls below act as. Backups are per session, so every call
/// needs one; using a constant keeps the tests from leaking files into each other.
const TEST_SESSION_ID: &str = "tools-test-session";
use std::fs;
use std::path::Path;

#[test]
fn test_generate_smart_gitignore_node() {
    let temp_dir = std::env::temp_dir().join(format!(
        "ragasukma_test_node_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
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
    let temp_dir = std::env::temp_dir().join(format!(
        "ragasukma_test_rust_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&temp_dir);
    let _ = fs::write(temp_dir.join("Cargo.toml"), "[package]\nname = \"foo\"");

    let gitignore = generate_smart_gitignore(&temp_dir);
    assert!(gitignore.contains("/target/"));
    assert!(gitignore.contains(".DS_Store"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_generate_smart_gitignore_python() {
    let temp_dir = std::env::temp_dir().join(format!(
        "ragasukma_test_py_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&temp_dir);
    let _ = fs::write(temp_dir.join("requirements.txt"), "flask==2.0.0");

    let gitignore = generate_smart_gitignore(&temp_dir);
    assert!(gitignore.contains("__pycache__/"));
    assert!(gitignore.contains(".venv/"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_ensure_git_and_gitignore_initializes() {
    let temp_dir = std::env::temp_dir().join(format!(
        "ragasukma_test_init_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
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
    let temp_dir = std::env::temp_dir().join(format!(
        "ragasukma_test_grep_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
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
    let temp_dir = std::env::temp_dir().join(format!(
        "ragasukma_cmd_norm_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&temp_dir);
    let roots = vec![temp_dir.to_string_lossy().to_string()];
    let folder_name = temp_dir.file_name().unwrap().to_str().unwrap();

    let cmd = format!(
        "(cd {}/backend && cargo test) && (cd {}/frontend && npm test)",
        folder_name, folder_name
    );
    let normalized = normalize_workspace_command(&cmd, &temp_dir, &roots);
    assert_eq!(
        normalized,
        "(cd backend && cargo test) && (cd frontend && npm test)"
    );

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
fn test_resolve_allowed_path_traversal_rejection() {
    let temp_dir = std::env::temp_dir().join(format!(
        "ragasukma_sec_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&temp_dir);
    let roots = vec![temp_dir.to_string_lossy().to_string()];

    // Should reject ../ escaping
    assert!(resolve_allowed_path("../outside.txt", &roots).is_err());
    assert!(resolve_allowed_path("../../outside.txt", &roots).is_err());
    assert!(resolve_allowed_path("subdir/../../outside.txt", &roots).is_err());

    // Should allow valid relative path inside roots
    let valid_path = temp_dir.join("inside.txt");
    let _ = fs::write(&valid_path, "test");
    assert!(resolve_allowed_path("inside.txt", &roots).is_ok());

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_atomic_write_file_success() {
    let temp_dir = std::env::temp_dir().join(format!(
        "ragasukma_atomic_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&temp_dir);
    let target_file = temp_dir.join("atomic_target.txt");

    assert!(atomic_write_file(&target_file, b"first content").is_ok());
    assert_eq!(fs::read_to_string(&target_file).unwrap(), "first content");

    assert!(atomic_write_file(&target_file, b"overwritten atomic content").is_ok());
    assert_eq!(fs::read_to_string(&target_file).unwrap(), "overwritten atomic content");

    let _ = fs::remove_dir_all(&temp_dir);
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
    let err = apply_resilient_edit(
        content,
        "function completelyDifferentNonExistentFeature() {\n  return 999;\n}",
        "new",
        "test.ts",
    )
    .unwrap_err();
    let err_msg = err.to_string();
    assert!(err_msg.contains("'old_text' not found"));
    assert!(
        err_msg.contains("Use `workspace_read_file`")
            || err_msg.contains("Closest actual content in 'test.ts'")
    );
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
    let temp_dir = std::env::temp_dir().join(format!(
        "ragasukma_read_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
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
        TEST_SESSION_ID,
    )
    .await
    .unwrap();
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
        TEST_SESSION_ID,
    )
    .await
    .unwrap();
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
        TEST_SESSION_ID,
    )
    .await
    .unwrap();
    assert_eq!(out_full.lines().count(), 300);

    let _ = std_fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_workspace_list_dir_shallow_with_counts() {
    let temp_dir = std::env::temp_dir().join(format!(
        "ragasukma_listdir_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
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
        TEST_SESSION_ID,
    )
    .await
    .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&res_raw).unwrap();
    let entries = parsed.get("entries").and_then(|e| e.as_array()).unwrap();
    assert_eq!(parsed["total_dirs"], 1);
    assert_eq!(parsed["total_files"], 1);

    let has_child_count = entries.iter().any(|e| {
        e.as_str()
            .is_some_and(|s| s.contains("subfolder/ (2 items)"))
    });
    assert!(has_child_count);

    let _ = std_fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_workspace_write_file_compact_metadata() {
    let temp_dir = std::env::temp_dir().join(format!(
        "ragasukma_write_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std_fs::create_dir_all(&temp_dir);

    let roots = vec![temp_dir.to_string_lossy().to_string()];
    let out_json_str = execute_tool_call(
        "workspace_write_file",
        &serde_json::json!({
            "file_path": "new_created_file.txt",
            "content": "Line A\nLine B\nLine C"
        }),
        &roots,
        TEST_SESSION_ID,
    )
    .await
    .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&out_json_str).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["bytes_written"], 20);
    assert!(parsed.get("old_content").is_none());
    assert!(parsed.get("new_content").is_none());

    let _ = std_fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_workspace_grep_compact_tabular_format() {
    let temp_dir = std::env::temp_dir().join(format!(
        "ragasukma_grep_tab_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std_fs::create_dir_all(&temp_dir);

    let test_file = temp_dir.join("logic.rs");
    let _ = std_fs::write(
        &test_file,
        "pub fn calculate_price() {\n    let discount = 10;\n}\n",
    );

    let roots = vec![temp_dir.to_string_lossy().to_string()];
    let grep_out = execute_tool_call(
        "workspace_grep",
        &serde_json::json!({ "query": "calculate_price" }),
        &roots,
        TEST_SESSION_ID,
    )
    .await
    .unwrap();

    assert!(grep_out.contains("Found 1 match(es) for query 'calculate_price'"));
    assert!(grep_out.contains("logic.rs:1: pub fn calculate_price() {"));

    let _ = std_fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_workspace_grep_with_folder_arg_and_edit_file_schema() {
    let temp_dir = std::env::temp_dir().join(format!(
        "ragasukma_grep_folder_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
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
    assert!(
        has_edit,
        "workspace_edit_file must be exposed in dynamic agent tools schema"
    );

    // Every write/edit backs up the file first, so the revert tool must actually be
    // callable. A backup nobody can roll back is dead weight, which is exactly what
    // this tool was before it was added to the schema.
    let has_rollback = dynamic_schema.iter().any(|s| {
        s.pointer("/function/name").and_then(|n| n.as_str()) == Some("workspace_rollback")
    });
    assert!(
        has_rollback,
        "workspace_rollback must be exposed so backups can be undone"
    );

    // 2. Validate workspace_grep with 'folder' parameter resolves subfolder cleanly
    let grep_out = execute_tool_call(
        "workspace_grep",
        &serde_json::json!({
            "folder": "backend",
            "query": "start_doku_payment"
        }),
        &roots,
        TEST_SESSION_ID,
    )
    .await
    .unwrap();

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
        TEST_SESSION_ID,
    )
    .await
    .unwrap();

    assert!(edit_out.contains("\"status\":\"ok\""));
    let updated_code = std_fs::read_to_string(&test_file).unwrap();
    assert!(updated_code.contains("start_jokul_doku_payment"));

    let _ = std_fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_mcp_dynamic_tools_registration_and_configuration() {
    let roots: Vec<String> = vec![];
    let project = crate::repository::agent::create_project("test_proj", "test_desc", None, None).await.unwrap();
    let session = crate::repository::agent::create_session(&project.id, "test_session", None, None, None).await.unwrap();
    let test_session = session.id;

    // 1. Register dummy test MCP server
    let reg_out = execute_tool_call(
        "mcp_register_server",
        &serde_json::json!({
            "server_id": "test-dummy-server",
            "type": "stdio",
            "command_or_url": "dummy-cli",
            "args": ["serve"]
        }),
        &roots,
        &test_session,
    )
    .await
    .unwrap();

    assert!(reg_out.contains("test-dummy-server"));
    assert!(reg_out.contains("successfully registered"));

    // 2. Configure session credentials & active flag
    let conf_out = execute_tool_call(
        "mcp_configure_session",
        &serde_json::json!({
            "server_id": "test-dummy-server",
            "env": { "TEST_KEY": "dummy_secret_value" },
            "is_active": true
        }),
        &roots,
        &test_session,
    )
    .await
    .unwrap();

    assert!(conf_out.contains("test-dummy-server"));
    assert!(conf_out.contains("session configuration updated"));

    // 3. List available MCP servers and verify test-dummy-server is active in current session
    let list_out = execute_tool_call(
        "mcp_list_available",
        &serde_json::json!({}),
        &roots,
        &test_session,
    )
    .await
    .unwrap();

    assert!(list_out.contains("test-dummy-server"));
    assert!(list_out.contains("Active in current session"));
}
