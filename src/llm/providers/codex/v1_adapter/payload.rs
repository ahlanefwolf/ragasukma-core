use super::super::constants::*;
use crate::llm::types::ChatMessage;
use crate::llm::v1_engine::{V1ChatRequest, extract_text_content, generate_v1_uuid};
use serde_json::json;

pub fn build_codex_payload_from_messages(
    model: &str,
    messages: &[ChatMessage],
    _temperature: f32,
    _max_tokens: u32,
) -> serde_json::Value {
    let clean_model = model.trim().trim_start_matches("cx/");
    let mut input_items = Vec::new();
    let mut instructions = CODEX_DEFAULT_INSTRUCTIONS.to_string();

    for msg in messages {
        if msg.role == "system" || msg.role == "developer" {
            if !msg.content.trim().is_empty() {
                instructions = msg.content.clone();
            }
            continue;
        }

        let content_type = if msg.role == "assistant" {
            "output_text"
        } else {
            "input_text"
        };

        let text = if msg.role == "tool" {
            format!("[TOOL OBSERVATION]:\n{}", msg.content)
        } else {
            msg.content.clone()
        };

        input_items.push(json!({
            "type": "message",
            "role": if msg.role == "assistant" { "assistant" } else { "user" },
            "content": [
                {
                    "type": content_type,
                    "text": text
                }
            ]
        }));
    }

    json!({
        "model": clean_model,
        "input": input_items,
        "instructions": instructions,
        "stream": true,
        "store": false,
    })
}

pub(super) fn clamp_call_id(id: &str) -> String {
    let clean = id.trim();
    if clean.is_empty() {
        return format!("call_{}", generate_v1_uuid("cx"));
    }
    if clean.len() > 64 {
        clean[..64].to_string()
    } else {
        clean.to_string()
    }
}

pub fn build_codex_payload_from_v1(req: &V1ChatRequest) -> serde_json::Value {
    let clean_model = req.model.trim().trim_start_matches("cx/");
    let mut input_items = Vec::new();
    let mut system_instructions = CODEX_DEFAULT_INSTRUCTIONS.to_string();
    let mut has_system = false;

    for msg in &req.messages {
        let content_str = extract_text_content(&msg.content);

        if msg.role == "system" || msg.role == "developer" {
            if !content_str.is_empty() && !has_system {
                system_instructions = content_str.clone();
                has_system = true;
            }
            continue;
        }

        if msg.role == "user" {
            if !content_str.is_empty() {
                input_items.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": content_str
                        }
                    ]
                }));
            }
        } else if msg.role == "assistant" {
            // Push reasoning item if present before assistant message (9router / OpenAI Responses spec)
            if let Some(ts) = &msg.thought_signature
                && !ts.trim().is_empty()
            {
                input_items.push(json!({
                    "type": "reasoning",
                    "summary": [
                        {
                            "type": "summary_text",
                            "text": ts
                        }
                    ]
                }));
            }

            // Push message text if not empty
            if !content_str.trim().is_empty() {
                input_items.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": content_str
                        }
                    ]
                }));
            }

            // Convert assistant tool_calls into function_call items according to Responses API spec
            if let Some(tool_calls) = &msg.tool_calls {
                for tc in tool_calls {
                    let name = tc.function.name.trim();
                    if name.is_empty() {
                        continue;
                    }
                    let call_id = clamp_call_id(&tc.id);
                    let args_str = if tc.function.arguments.trim().is_empty() {
                        "{}".to_string()
                    } else {
                        tc.function.arguments.clone()
                    };

                    input_items.push(json!({
                        "type": "function_call",
                        "call_id": call_id,
                        "name": name,
                        "arguments": args_str
                    }));
                }
            }
        } else if msg.role == "tool" {
            // Convert tool observation into function_call_output item
            let call_id = msg
                .tool_call_id
                .as_deref()
                .map(clamp_call_id)
                .unwrap_or_else(|| format!("call_{}", generate_v1_uuid("cx")));

            input_items.push(json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": content_str
            }));
        }
    }

    let mut payload = json!({
        "model": clean_model,
        "input": input_items,
        "instructions": system_instructions,
        "stream": true,
        "store": false
    });

    // Translate tools if provided
    if let Some(tools) = &req.tools {
        let mut translated_tools = Vec::new();
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
                    translated_tools.push(json!({
                        "type": "function",
                        "name": name,
                        "description": desc,
                        "parameters": params
                    }));
                }
            } else if let Some(name) = tool.get("name").and_then(|v| v.as_str())
                && !name.trim().is_empty()
            {
                translated_tools.push(tool.clone());
            }
        }
        if !translated_tools.is_empty() {
            payload["tools"] = json!(translated_tools);
        }
    }

    if let Some(max_tokens) = req.max_tokens {
        payload["max_output_tokens"] = json!(max_tokens);
    }

    payload
}
