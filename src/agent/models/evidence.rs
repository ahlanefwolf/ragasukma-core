use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use super::message::Message;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct EvidenceCache {
    pub id: String,
    pub project_id: String,
    pub file_path: String,
    pub file_version: String,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub symbol_name: Option<String>,
    pub summary: String,
    pub raw_snippet: Option<String>,
    pub is_stale: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct GoalEvidence {
    pub goal_id: String,
    pub evidence_id: String,
    pub relevance_score: f64,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct GoalVerification {
    pub id: String,
    pub goal_id: String,
    pub iteration: i64,
    pub test_command: String,
    pub test_output: String,
    pub is_passed: bool,
    pub feedback: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentSessionEvent {
    #[serde(rename = "status")]
    StatusChange { status: String },
    #[serde(rename = "message")]
    MessageCreated(Message),
    #[serde(rename = "step")]
    StepInfo {
        iteration: usize,
        action: String,
        details: Option<String>,
    },
    #[serde(rename = "done")]
    Done { reply: String },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "delta")]
    Delta { delta: String },
    #[serde(rename = "thought_delta")]
    ThoughtDelta { delta: String },
    #[serde(rename = "stream_reset")]
    StreamReset,
}
