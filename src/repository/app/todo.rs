use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct TodoItem {
    pub id: i64,
    pub task: String,
    pub is_done: bool,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

pub async fn get_all_todos_from_db(pool: &SqlitePool) -> Result<Vec<TodoItem>, sqlx::Error> {
    sqlx::query_as::<_, TodoItem>(
        "SELECT id, task, is_done, created_at, completed_at FROM todos ORDER BY id ASC",
    )
    .fetch_all(pool)
    .await
}

pub async fn get_active_todos_from_db(pool: &SqlitePool) -> Result<Vec<TodoItem>, sqlx::Error> {
    sqlx::query_as::<_, TodoItem>(
        "SELECT id, task, is_done, created_at, completed_at FROM todos WHERE is_done = 0 ORDER BY id ASC",
    )
    .fetch_all(pool)
    .await
}

pub async fn add_todo_to_db(
    pool: &SqlitePool,
    task: &str,
    created_at: i64,
) -> Result<i64, sqlx::Error> {
    let res = sqlx::query("INSERT INTO todos (task, is_done, created_at) VALUES (?1, 0, ?2)")
        .bind(task)
        .bind(created_at)
        .execute(pool)
        .await?;
    Ok(res.last_insert_rowid())
}

pub async fn complete_todo_in_db(
    pool: &SqlitePool,
    id: i64,
    completed_at: i64,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query("UPDATE todos SET is_done = 1, completed_at = ?1 WHERE id = ?2")
        .bind(completed_at)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn delete_todo_from_db(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let res = sqlx::query("DELETE FROM todos WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}
