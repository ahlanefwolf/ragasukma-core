use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AgentAuditLog {
    pub id: String,
    pub session_id: Option<String>,
    pub goal_id: Option<String>,
    pub tool_name: String,
    pub target_path: Option<String>,
    pub command: Option<String>,
    pub status: String,
    pub reason: Option<String>,
    pub duration_ms: Option<i64>,
    pub created_at: i64,
}
