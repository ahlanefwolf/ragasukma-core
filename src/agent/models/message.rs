use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub sequence_num: i64,
    pub role: String,
    pub content: Option<String>,
    pub thought: Option<String>,
    pub tool_calls: Option<String>,
    pub tool_call_id: Option<String>,
    pub status: String,
    pub token_usage_input: Option<i64>,
    pub token_usage_output: Option<i64>,
    pub created_at: i64,
}
