use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: String,
    pub root_path: Option<String>,
    pub custom_instructions: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ProjectMcpServer {
    pub project_id: String,
    pub server_id: String,
    pub is_enabled: i64,
}
