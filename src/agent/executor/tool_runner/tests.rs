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

/// An in-memory pool with migrations applied and a project + goal already
/// seeded, returning their ids.
///
/// Evidence writes use `save_evidence_and_link`, which stores the slice *and*
/// its goal link in a single transaction. A test that reaches an evidence path
/// without a real `projects` and `goals` row will fail on a foreign key, and
/// because the caller only logs that error the symptom is a bare "not found"
/// rather than a clear FK message. Going through this helper keeps every
/// evidence test from re-discovering that the hard way.
async fn seeded_pool() -> (sqlx::SqlitePool, &'static str, &'static str) {
    use sqlx::sqlite::SqlitePoolOptions;

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    crate::repository::migrate_database(&pool).await.unwrap();

    let project_id = "test_proj_effects";
    sqlx::query(
        "INSERT INTO projects (id, name, description, created_at, updated_at) VALUES (?1, ?2, ?3, 1000, 1000)",
    )
    .bind(project_id)
    .bind("Effects Test")
    .bind("covers the post-inspection evidence write")
    .execute(&pool)
    .await
    .unwrap();

    let goal_id = "goal_effects";
    sqlx::query(
        "INSERT INTO goals (id, project_id, title, demand, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 1000, 1000)",
    )
    .bind(goal_id)
    .bind(project_id)
    .bind("Cache a read")
    .bind("Exercise the post-inspection evidence write")
    .execute(&pool)
    .await
    .unwrap();

    (pool, project_id, goal_id)
}

#[tokio::test]
async fn post_inspection_caches_read_into_evidence_table() {
    use super::effects::{PostExecContext, apply_post_inspection};
    use crate::agent::executor::state::TurnState;

    let (pool, project_id, goal_id) = seeded_pool().await;

    // A real file inside an allowed folder gives a genuine file_version.
    let dir = std::env::temp_dir().join(format!("effects_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("sample.rs");
    std::fs::write(&file, "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
    let file_path = file.to_string_lossy().to_string();
    let allowed_folders = vec![dir.to_string_lossy().to_string()];

    let args_val = json!({ "path": file_path, "start_line": 1, "end_line": 3 });
    let mut state = TurnState::new(goal_id.to_string(), "action".to_string(), vec![], 0);

    apply_post_inspection(PostExecContext {
        state: &mut state,
        tool_name: "workspace_read_file",
        tool_output: "fn main() {\n    println!(\"hi\");\n}\n",
        args_val: &args_val,
        target_arg_file: Some(&file_path),
        is_edit_op: false,
        is_inspection: true,
        is_accessing_unmodified_target: true,
        is_breaker_trip: false,
        session_id: "sess_effects",
        project_id,
        allowed_folders: &allowed_folders,
        pool: &pool,
    })
    .await;

    let (cached_path, is_stale): (String, i64) =
        sqlx::query_as("SELECT file_path, is_stale FROM evidence_cache WHERE project_id = ?1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .expect("the read should have been cached");

    assert_eq!(cached_path, file_path);
    assert_eq!(is_stale, 0, "a freshly read file is not stale");

    std::fs::remove_dir_all(&dir).ok();
}

/// The mirror of the test above: with the goal row absent, the evidence write is
/// rejected by the goal-link foreign key and nothing is cached. Pinning this
/// keeps the requirement visible — if the write ever starts succeeding without a
/// goal, the "one transaction" guarantee has quietly been broken.
#[tokio::test]
async fn post_inspection_without_goal_row_caches_nothing() {
    use super::effects::{PostExecContext, apply_post_inspection};
    use crate::agent::executor::state::TurnState;

    let (pool, project_id, _goal_id) = seeded_pool().await;
    sqlx::query("DELETE FROM goals WHERE project_id = ?1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();

    let args_val = json!({ "path": "src/sample.rs", "start_line": 1, "end_line": 3 });
    let mut state = TurnState::new("goal_absent".to_string(), "action".to_string(), vec![], 0);

    apply_post_inspection(PostExecContext {
        state: &mut state,
        tool_name: "workspace_read_file",
        tool_output: "fn main() {}\n",
        args_val: &args_val,
        target_arg_file: Some("src/sample.rs"),
        is_edit_op: false,
        is_inspection: true,
        is_accessing_unmodified_target: true,
        is_breaker_trip: false,
        session_id: "sess_effects",
        project_id,
        allowed_folders: &[],
        pool: &pool,
    })
    .await;

    let cached: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM evidence_cache WHERE project_id = ?1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(
        cached, 0,
        "evidence must not be cached when the goal link cannot be written"
    );
}
