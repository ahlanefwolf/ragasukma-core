use sqlx::SqlitePool;

pub mod chat_history;
pub mod skill;
pub mod todo;

pub use chat_history::{
    ChatHistoryPaginatedResponse, ChatHistoryRecord, ChatTurnItem, delete_history_by_date,
    get_chat_event_sender, get_history_by_date_paginated, get_history_dates,
    get_local_date_and_time, log_conversation_turn, save_chat_turn,
};
pub use skill::{
    SkillItem, delete_skill_from_db, get_active_skills_from_db, get_all_skills_from_db,
    save_skill_to_db,
};
pub use todo::{
    TodoItem, add_todo_to_db, complete_todo_in_db, delete_todo_from_db, get_active_todos_from_db,
    get_all_todos_from_db,
};

/// Runs all SQLx migrations embedded in ragasukma-core against the given SQLite pool.
pub async fn migrate_database(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}
