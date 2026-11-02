use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct SkillItem {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub command: String,
    pub is_active: bool,
    pub created_at: i64,
}

pub async fn get_all_skills_from_db(pool: &SqlitePool) -> Result<Vec<SkillItem>, sqlx::Error> {
    sqlx::query_as::<_, SkillItem>(
        "SELECT id, name, description, command, is_active, created_at FROM skills ORDER BY id ASC",
    )
    .fetch_all(pool)
    .await
}

pub async fn get_active_skills_from_db(pool: &SqlitePool) -> Result<Vec<SkillItem>, sqlx::Error> {
    sqlx::query_as::<_, SkillItem>(
        "SELECT id, name, description, command, is_active, created_at FROM skills WHERE is_active = 1 ORDER BY id ASC",
    )
    .fetch_all(pool)
    .await
}

pub async fn save_skill_to_db(
    pool: &SqlitePool,
    name: &str,
    description: &str,
    command: &str,
    created_at: i64,
) -> Result<i64, sqlx::Error> {
    let res = sqlx::query(
        "INSERT INTO skills (name, description, command, is_active, created_at) VALUES (?1, ?2, ?3, 1, ?4)",
    )
    .bind(name)
    .bind(description)
    .bind(command)
    .bind(created_at)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

pub async fn delete_skill_from_db(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let res = sqlx::query("DELETE FROM skills WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}
