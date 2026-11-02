use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static V1_COUNTER: AtomicUsize = AtomicUsize::new(1);

pub fn generate_v1_uuid(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let count = V1_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}-{:x}-{:x}", prefix, now, count)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct V1ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct V1ToolCall {
    #[serde(default = "default_tool_call_id")]
    pub id: String,
    #[serde(default = "default_function_type", rename = "type")]
    pub call_type: String,
    pub function: V1ToolCallFunction,
    #[serde(default, alias = "thought_signature", alias = "thoughtSignature")]
    pub thought_signature: Option<String>,
    #[serde(default)]
    pub extra_content: Option<serde_json::Value>,
    #[serde(default)]
    pub provider_specific_fields: Option<serde_json::Value>,
}

fn default_tool_call_id() -> String {
    format!("call_{}", generate_v1_uuid("tc"))
}

fn default_function_type() -> String {
    "function".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct V1Message {
    pub role: String,
    #[serde(default)]
    pub content: Option<serde_json::Value>,
    #[serde(default)]
    pub tool_calls: Option<Vec<V1ToolCall>>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, alias = "thought_signature", alias = "thoughtSignature")]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct V1ChatRequest {
    pub model: String,
    pub messages: Vec<V1Message>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub enum V1StreamEvent {
    ContentDelta(String),
    ThoughtDelta(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    },
    KeepAlive,
    Finish {
        finish_reason: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct V1Response {
    pub content: String,
    pub tool_calls: Option<Vec<V1ToolCall>>,
    pub finish_reason: String,
}

/// Helper function to extract text content from OpenAI message content
pub fn extract_text_content(val: &Option<serde_json::Value>) -> String {
    match val {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => {
            let mut res = String::new();
            for item in arr {
                if let Some(txt) = item.get("text").and_then(|t| t.as_str()) {
                    res.push_str(txt);
                }
            }
            res
        }
        _ => String::new(),
    }
}

/// Sanitize function name to conform to common tool calling standards
pub fn sanitize_function_name(name: &str) -> String {
    if name.is_empty() {
        return "_unknown".to_string();
    }
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == ':' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();

    if let Some(first) = s.chars().next()
        && !first.is_ascii_alphabetic() && first != '_' {
            s.insert(0, '_');
        }

    if s.len() > 64 {
        s.truncate(64);
    }
    s
}
