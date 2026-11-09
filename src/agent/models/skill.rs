use serde::{Deserialize, Serialize};

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
