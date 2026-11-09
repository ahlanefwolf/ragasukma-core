use sqlx::SqlitePool;

pub mod chat_history;
pub mod todo;

// NOTE: the legacy SQLite-backed skill CRUD module (formerly `repository::skill`) was
// removed. Skills are now discovered dynamically from `.skill` / `.skills` folders on
// disk via `crate::agent::skills::scan_project_skills`. The old `skills` table created
// in migration `0003_todos_and_skills.sql` is intentionally left in place (dropping or
// rewriting a shipped migration would break the `_sqlx_migrations` checksum chain for
// existing installs); it is simply no longer read or written. Clean it up with a new
// forward migration if/when desired.

pub use chat_history::{
    ChatHistoryPaginatedResponse, ChatHistoryRecord, ChatTurnItem, delete_history_by_date,
    get_chat_event_sender, get_history_by_date_paginated, get_history_dates,
    get_local_date_and_time, log_conversation_turn, save_chat_turn,
};
pub use todo::{
    TodoItem, add_todo_to_db, complete_todo_in_db, delete_todo_from_db, get_active_todos_from_db,
    get_all_todos_from_db,
};

/// Runs all SQLx migrations embedded in ragasukma-core against the given SQLite pool.
pub async fn migrate_database(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}
