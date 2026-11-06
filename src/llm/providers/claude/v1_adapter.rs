use super::constants::*;
use crate::llm::types::ChatMessage;
use crate::llm::v1_engine::{V1ChatRequest, V1StreamEvent, extract_text_content, generate_v1_uuid};
use serde_json::json;

pub fn build_claude_payload_from_messages(
    model: &str,
    messages: &[ChatMessage],
    temperature: f32,
    max_tokens: u32,
) -> serde_json::Value {
    let clean_model = model.trim().trim_start_matches("cc/");
    let mut claude_messages = Vec::new();
    let mut system_prompt = CLAUDE_DEFAULT_SYSTEM_PROMPT.to_string();

    for msg in messages {
        if msg.role == "system" {
            if !msg.content.trim().is_empty() {
                system_prompt = msg.content.clone();
            }
            continue;
        }

        let role = if msg.role == "assistant" {
            "assistant"
        } else {
            "user"
        };

        claude_messages.push(json!({
            "role": role,
            "content": msg.content
        }));
    }

    let max_tok = if max_tokens == 0 { 8192 } else { max_tokens };

    json!({
        "model": clean_model,
        "messages": claude_messages,
        "system": system_prompt,
        "max_tokens": max_tok,
        "temperature": temperature,
        "stream": true,
    })
}

/// Detect whether the requested Claude model supports and requests extended thinking.
/// Matches version >= 3.7 (e.g. claude-3-7, claude-3.7, claude-4, claude-opus-4, etc.)
/// or explicit thinking suffixes like (low), (medium), (high), -thinking, etc.
pub fn is_claude_thinking_model(model: &str) -> bool {
    let lower = model.to_lowercase();
    let trimmed = lower.trim();

    // Explicit thinking keyword or suffix tags
    if trimmed.contains("thinking")
        || trimmed.contains("-think")
        || trimmed.ends_with("(low)")
        || trimmed.ends_with("(medium)")
        || trimmed.ends_with("(high)")
    {
        return true;
    }

    // Version-based check: 3.7+ or 4+ models support thinking
    // Claude 3 (3.0), 3.5 (Sonnet/Haiku) do NOT support extended thinking.
    trimmed.contains("claude-3-7")
        || trimmed.contains("claude-3.7")
        || trimmed.contains("claude-3-8")
        || trimmed.contains("claude-3.8")
        || trimmed.contains("claude-3-9")
        || trimmed.contains("claude-3.9")
        || (trimmed.contains("claude-")
            && !trimmed.contains("claude-3")
            && !trimmed.contains("claude-2")
            && !trimmed.contains("claude-1"))
}

/// Resolve thinking budget tokens based on model name tags or defaults
pub fn resolve_claude_thinking_budget(model: &str) -> u32 {
    let lower = model.to_lowercase();
    if lower.contains("(low)") || lower.contains("-low") {
        1024
    } else if lower.contains("(high)") || lower.contains("-high") {
        4096
    } else {
        2048 // Standard balanced budget
    }
}

pub fn build_claude_payload_from_v1(req: &V1ChatRequest) -> serde_json::Value {
    let clean_model = req.model.trim().trim_start_matches("cc/");
    let mut claude_messages: Vec<serde_json::Value> = Vec::new();
    let mut system_prompt = CLAUDE_DEFAULT_SYSTEM_PROMPT.to_string();
    let mut has_system = false;

    // We merge consecutive messages with the same role as required by Anthropic
    for msg in &req.messages {
        let content_str = extract_text_content(&msg.content);

        if msg.role == "system" || msg.role == "developer" {
            if !content_str.is_empty() && !has_system {
                system_prompt = content_str.clone();
                has_system = true;
            }
            continue;
        }

        let role = if msg.role == "assistant" {
            "assistant"
        } else {
            "user"
        };

        if msg.role == "tool" {
            let tool_use_id = msg
                .tool_call_id
                .clone()
                .unwrap_or_else(|| format!("call_{}", generate_v1_uuid("cc")));

            let tool_result_block = json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content_str
            });

            // Append to previous user message or create a new user message
            let can_append = if let Some(last) = claude_messages.last_mut() {
                if last.get("role").and_then(|r| r.as_str()) == Some("user") {
                    if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                        arr.push(tool_result_block.clone());
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };

            if !can_append {
                claude_messages.push(json!({
                    "role": "user",
                    "content": [tool_result_block]
                }));
            }
            continue;
        }

        let mut content_blocks = Vec::new();

        if !content_str.is_empty() {
            content_blocks.push(json!({
                "type": "text",
                "text": content_str
            }));
        }

        if msg.role == "assistant"
            && let Some(tool_calls) = &msg.tool_calls
        {
            for tc in tool_calls {
                let name = tc.function.name.trim();
                if name.is_empty() {
                    continue;
                }
                let args_json: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| json!({}));

                content_blocks.push(json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": name,
                    "input": args_json
                }));
            }
        }

        if content_blocks.is_empty() {
            continue;
        }

        let can_append = if let Some(last) = claude_messages.last_mut() {
            if last.get("role").and_then(|r| r.as_str()) == Some(role) {
                if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                    arr.extend(content_blocks.clone());
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        if !can_append {
            claude_messages.push(json!({
                "role": role,
                "content": content_blocks
            }));
        }
    }

    let mut payload = json!({
        "model": clean_model,
        "messages": claude_messages,
        "system": system_prompt,
        "stream": true,
        "max_tokens": req.max_tokens.unwrap_or(8192)
    });

    if let Some(tools) = &req.tools {
        let mut claude_tools = Vec::new();
        for tool in tools {
            if let Some(func) = tool.get("function") {
                let name = func
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                if !name.is_empty() {
                    let desc = func
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let params = func
                        .get("parameters")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
                    claude_tools.push(json!({
                        "name": name,
                        "description": desc,
                        "input_schema": params
                    }));
                }
            } else if let Some(name) = tool.get("name").and_then(|v| v.as_str())
                && !name.trim().is_empty()
            {
                claude_tools.push(tool.clone());
            }
        }
        if !claude_tools.is_empty() {
            payload["tools"] = json!(claude_tools);
        }
    }

    let is_thinking_claude = is_claude_thinking_model(clean_model);
    if is_thinking_claude {
        let budget = resolve_claude_thinking_budget(clean_model);
        let max_tok = req.max_tokens.unwrap_or(8192).max(budget + 2048);
        payload["max_tokens"] = json!(max_tok);
        payload["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": budget
        });
        // Anthropic requires temperature to be omitted or strictly 1.0 when extended thinking is enabled
        payload.as_object_mut().map(|obj| obj.remove("temperature"));
    } else if let Some(temp) = req.temperature {
        payload["temperature"] = json!(temp);
    }

    payload
}

#[derive(Default)]
pub struct ClaudeSseParserState {
    pub current_call_id: Option<String>,
    pub current_call_name: Option<String>,
    pub current_tool_index: usize,
    pub block_index_to_tool_index: std::collections::HashMap<usize, usize>,
}

impl ClaudeSseParserState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn parse_line(&mut self, line: &str) -> Vec<V1StreamEvent> {
        let trimmed = line.trim();
        if !trimmed.starts_with("data:") {
            return Vec::new();
        }
        let data_str = trimmed.trim_start_matches("data:").trim();
        if data_str == "[DONE]" || data_str.is_empty() {
            return Vec::new();
        }

        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data_str) else {
            return Vec::new();
        };

        let event_type = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let mut events = Vec::new();

        match event_type {
            "content_block_start" => {
                let block_idx = parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                if let Some(block) = parsed.get("content_block") {
                    let b_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if b_type == "tool_use" {
                        let id = block
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let name = block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let tool_idx = self.current_tool_index;
                        self.current_tool_index += 1;
                        self.block_index_to_tool_index.insert(block_idx, tool_idx);

                        self.current_call_id = id.clone();
                        self.current_call_name = name.clone();

                        events.push(V1StreamEvent::ToolCallDelta {
                            index: tool_idx,
                            id,
                            name,
                            arguments: String::new(),
                        });
                    }
                }
            }
            "content_block_delta" => {
                let block_idx = parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                if let Some(delta) = parsed.get("delta") {
                    let d_type = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if d_type == "text_delta" {
                        if let Some(text) = delta.get("text").and_then(|v| v.as_str())
                            && !text.is_empty()
                        {
                            events.push(V1StreamEvent::ContentDelta(text.to_string()));
                        }
                    } else if d_type == "thinking_delta" {
                        if let Some(th) = delta.get("thinking").and_then(|v| v.as_str())
                            && !th.is_empty()
                        {
                            events.push(V1StreamEvent::ThoughtDelta(th.to_string()));
                        }
                    } else if d_type == "input_json_delta"
                        && let Some(pj) = delta.get("partial_json").and_then(|v| v.as_str())
                    {
                        let tool_idx = self
                            .block_index_to_tool_index
                            .get(&block_idx)
                            .copied()
                            .unwrap_or(0);
                        events.push(V1StreamEvent::ToolCallDelta {
                            index: tool_idx,
                            id: self.current_call_id.clone(),
                            name: self.current_call_name.clone(),
                            arguments: pj.to_string(),
                        });
                    }
                }
            }
            "message_delta" => {
                if let Some(stop_reason) = parsed
                    .pointer("/delta/stop_reason")
                    .and_then(|v| v.as_str())
                {
                    let finish = match stop_reason {
                        "tool_use" => "tool_calls",
                        "max_tokens" => "length",
                        _ => "stop",
                    };
                    events.push(V1StreamEvent::Finish {
                        finish_reason: finish.to_string(),
                    });
                }
            }
            _ => {}
        }

        events
    }
}
