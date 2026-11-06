use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSkill {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub system_prompt: String,
    pub tags: String,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default = "default_active")]
    pub is_active: i64,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

fn default_active() -> i64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub config: String, // Raw JSON configuration
    pub is_enabled: i64,
    pub created_at: i64,
}

impl McpServer {
    pub fn config_value(&self) -> serde_json::Value {
        serde_json::from_str(&self.config).unwrap_or_else(|_| serde_json::json!({}))
    }

    pub fn transport(&self) -> String {
        let v = self.config_value();
        v.get("transport")
            .and_then(|t| t.as_str())
            .unwrap_or_else(|| {
                if v.get("url").is_some() {
                    "sse"
                } else {
                    "stdio"
                }
            })
            .to_string()
    }

    pub fn command(&self) -> Option<String> {
        let v = self.config_value();
        v.get("command")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
    }

    pub fn url(&self) -> Option<String> {
        let v = self.config_value();
        v.get("url").and_then(|u| u.as_str()).map(|s| s.to_string())
    }

    pub fn args_json(&self) -> String {
        let v = self.config_value();
        v.get("args")
            .map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".to_string()))
            .unwrap_or_else(|| "[]".to_string())
    }

    pub fn env_json(&self) -> String {
        let v = self.config_value();
        v.get("env")
            .map(|e| serde_json::to_string(e).unwrap_or_else(|_| "{}".to_string()))
            .unwrap_or_else(|| "{}".to_string())
    }
}

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
    pub is_stale: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct GoalEvidence {
    pub goal_id: String,
    pub evidence_id: String,
    pub relevance_score: f64,
    pub is_active: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct GoalVerification {
    pub id: String,
    pub goal_id: String,
    pub iteration: i64,
    pub test_command: String,
    pub test_output: String,
    pub is_passed: i64,
    pub feedback: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
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
