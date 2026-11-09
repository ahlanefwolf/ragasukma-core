use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub status: String,
    pub model: Option<String>,
    pub temperature: Option<f64>,
    pub source_chat_id: Option<String>,
    pub metadata: String, // JSON
    pub created_at: i64,
    pub updated_at: i64,
    #[sqlx(default)]
    #[serde(default)]
    pub total_tokens_input: Option<i64>,
    #[sqlx(default)]
    #[serde(default)]
    pub total_tokens_output: Option<i64>,
    #[sqlx(default)]
    #[serde(default)]
    pub total_requests: Option<i64>,
}
