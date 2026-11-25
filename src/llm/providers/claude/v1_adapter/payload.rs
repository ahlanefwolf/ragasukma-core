use super::super::constants::*;
use crate::llm::types::ChatMessage;
use crate::llm::v1_engine::{V1ChatRequest, extract_text_content, generate_v1_uuid};
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

    // Enable Anthropic prompt caching on the second-to-last turn in multi-turn conversation
    let msg_count = claude_messages.len();
    if msg_count >= 2 {
        let prev_idx = msg_count - 2;
        if let Some(prev_msg) = claude_messages
            .get_mut(prev_idx)
            .and_then(|m| m.as_object_mut())
        {
            if let Some(content_arr) = prev_msg.get_mut("content").and_then(|c| c.as_array_mut()) {
                if let Some(last_block) = content_arr.last_mut().and_then(|b| b.as_object_mut()) {
                    last_block.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
                }
            } else if let Some(content_str) = prev_msg
                .get("content")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string())
            {
                prev_msg.insert(
                    "content".to_string(),
                    json!([
                        {
                            "type": "text",
                            "text": content_str,
                            "cache_control": { "type": "ephemeral" }
                        }
                    ]),
                );
            }
        }
    }

    let mut payload = json!({
        "model": clean_model,
        "messages": claude_messages,
        "stream": true,
        "max_tokens": req.max_tokens.unwrap_or(8192)
    });

    if !system_prompt.trim().is_empty() {
        payload["system"] = json!([
            {
                "type": "text",
                "text": system_prompt,
                "cache_control": { "type": "ephemeral" }
            }
        ]);
    }

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
            if let Some(last_tool) = claude_tools.last_mut().and_then(|t| t.as_object_mut()) {
                last_tool.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
            }
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
