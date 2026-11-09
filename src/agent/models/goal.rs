use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Goal {
    pub id: String,
    pub project_id: String,
    pub session_id: Option<String>,
    pub parent_goal_id: Option<String>,
    pub title: String,
    pub demand: String,
    pub success_criteria: String, // JSON Array
    pub constraints: String,      // JSON Array
    pub phase: String,            // 'discovery', 'action', 'verification', 'concluded'
    pub status: String, // 'in_progress', 'completed' (verified), 'completed_unverified', 'failed'
    pub target_files: String, // JSON Array of locked target files
    pub discovery_count: i64,
    pub conclusion: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    #[sqlx(default)]
    #[serde(default = "default_version")]
    pub version: i64,
    #[sqlx(default)]
    #[serde(default = "default_context_status")]
    pub context_status: String,
    #[sqlx(default)]
    #[serde(default)]
    pub context_error: Option<String>,
}

fn default_version() -> i64 {
    1
}

fn default_context_status() -> String {
    "healthy".to_string()
}
