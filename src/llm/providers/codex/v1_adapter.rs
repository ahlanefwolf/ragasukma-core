use super::constants::*;
use crate::llm::types::ChatMessage;
use crate::llm::v1_engine::{V1ChatRequest, V1StreamEvent, extract_text_content, generate_v1_uuid};
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

fn clamp_call_id(id: &str) -> String {
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

#[derive(Default)]
pub struct CodexSseParserState {
    pub current_call_id: Option<String>,
    pub current_call_name: Option<String>,
    pub current_tool_index: usize,
    pub call_id_to_index: std::collections::HashMap<String, usize>,
    pub resp_tool_args_emitted: std::collections::HashSet<usize>,
}

impl CodexSseParserState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn parse_line(&mut self, line: &str) -> Vec<V1StreamEvent> {
        let trimmed = line.trim();
        // Ignore SSE keep-alive comments like ": ping" or empty lines
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

        // 1. Function Call Output Item Added (identical to 9router response.output_item.added)
        if event_type == "response.output_item.added" {
            if let Some(item) = parsed.get("item") {
                let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if item_type == "function_call" || item_type == "custom_tool_call" {
                    let call_id = item
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("call_{}", self.current_tool_index));
                    let name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_default();

                    let item_key = item.get("id").and_then(|v| v.as_str()).unwrap_or(&call_id);
                    let idx = if let Some(&existing_idx) = self.call_id_to_index.get(item_key) {
                        existing_idx
                    } else {
                        let new_idx = self.current_tool_index;
                        self.current_tool_index += 1;
                        self.call_id_to_index.insert(item_key.to_string(), new_idx);
                        self.call_id_to_index.insert(call_id.clone(), new_idx);
                        new_idx
                    };

                    self.current_call_id = Some(call_id.clone());
                    self.current_call_name = Some(name.clone());

                    events.push(V1StreamEvent::ToolCallDelta {
                        index: idx,
                        id: Some(call_id),
                        name: Some(name),
                        arguments: String::new(),
                    });
                }
            }
            return events;
        }

        // 2. Function Call Arguments Delta (9router: response.function_call_arguments.delta & custom_tool_call_input.delta)
        if event_type == "response.function_call_arguments.delta"
            || event_type == "response.custom_tool_call_input.delta"
        {
            let delta = parsed.get("delta").and_then(|v| v.as_str()).unwrap_or("");
            if !delta.is_empty() {
                let item_id = parsed.get("item_id").and_then(|v| v.as_str());
                let idx = item_id
                    .and_then(|id| self.call_id_to_index.get(id).copied())
                    .unwrap_or_else(|| self.current_tool_index.saturating_sub(1));

                self.resp_tool_args_emitted.insert(idx);

                events.push(V1StreamEvent::ToolCallDelta {
                    index: idx,
                    id: self.current_call_id.clone(),
                    name: self.current_call_name.clone(),
                    arguments: delta.to_string(),
                });
            }
            return events;
        }

        // 3. Fallback when tool item finishes (9router: response.output_item.done)
        // If arguments were not emitted as deltas, emit the full arguments here
        if event_type == "response.output_item.done" {
            if let Some(item) = parsed.get("item") {
                let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if item_type == "function_call" || item_type == "custom_tool_call" {
                    let item_id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let idx = self
                        .call_id_to_index
                        .get(item_id)
                        .copied()
                        .unwrap_or_else(|| self.current_tool_index.saturating_sub(1));

                    if !self.resp_tool_args_emitted.contains(&idx)
                        && let Some(args_str) = item.get("arguments").and_then(|v| v.as_str())
                        && !args_str.is_empty()
                    {
                        self.resp_tool_args_emitted.insert(idx);
                        events.push(V1StreamEvent::ToolCallDelta {
                            index: idx,
                            id: self.current_call_id.clone(),
                            name: self.current_call_name.clone(),
                            arguments: args_str.to_string(),
                        });
                    }
                }
            }
            return events;
        }

        // 4. Reasoning / Thought Deltas (9router: response.reasoning_summary_text.delta)
        if event_type == "response.reasoning_summary_text.delta" {
            if let Some(delta) = parsed.get("delta").and_then(|v| v.as_str())
                && !delta.is_empty()
            {
                events.push(V1StreamEvent::ThoughtDelta(delta.to_string()));
            }
            return events;
        }

        // 5. Explicit error / failed events (9router: "error" || "response.failed")
        if event_type == "error" || event_type == "response.failed" {
            let err_msg = parsed
                .get("error")
                .and_then(|e| e.get("message").and_then(|m| m.as_str()))
                .or_else(|| {
                    parsed
                        .pointer("/response/error/message")
                        .and_then(|m| m.as_str())
                })
                .unwrap_or("Codex Responses stream failed");
            crate::term_err!("[Codex] Stream error event received: {}", err_msg);
            events.push(V1StreamEvent::ContentDelta(format!(
                "\n[Error: {}]\n",
                err_msg
            )));
            events.push(V1StreamEvent::Finish {
                finish_reason: "error".to_string(),
            });
            return events;
        }

        // 6. Explicit 9router text delta: "response.output_text.delta" or "response.text.delta"
        if (event_type == "response.output_text.delta" || event_type == "response.text.delta")
            && let Some(delta) = parsed.get("delta").and_then(|v| v.as_str())
            && !delta.is_empty()
        {
            events.push(V1StreamEvent::ContentDelta(delta.to_string()));
            return events;
        }

        // 7. Completion / incomplete markers → emit terminal Finish (9router parity:
        //    response.completed → "stop"; response.incomplete → map incomplete_details.reason:
        //    max_output_tokens → "length", content_filter → "content_filter", else "incomplete")
        if event_type == "response.completed" || event_type == "response.done" {
            events.push(V1StreamEvent::Finish {
                finish_reason: "stop".to_string(),
            });
            return events;
        }
        if event_type == "response.incomplete" {
            let reason = parsed
                .pointer("/response/incomplete_details/reason")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let finish = match reason {
                "max_output_tokens" => "length",
                "content_filter" => "content_filter",
                _ => "incomplete",
            };
            events.push(V1StreamEvent::Finish {
                finish_reason: finish.to_string(),
            });
            return events;
        }
        if event_type == "response.output_text.done" {
            return events;
        }

        // 8. Generic delta fallback
        if let Some(delta) = parsed.get("delta").and_then(|v| v.as_str()) {
            if !delta.is_empty() {
                events.push(V1StreamEvent::ContentDelta(delta.to_string()));
            }
            return events;
        }

        if event_type.ends_with(".delta") {
            if let Some(text) = parsed.get("text").and_then(|v| v.as_str())
                && !text.is_empty()
            {
                events.push(V1StreamEvent::ContentDelta(text.to_string()));
            }
            return events;
        }

        // 9. Alternative OpenAI streaming format: choices[0].delta.content
        if let Some(content) = parsed
            .pointer("/choices/0/delta/content")
            .and_then(|v| v.as_str())
        {
            if !content.is_empty() {
                events.push(V1StreamEvent::ContentDelta(content.to_string()));
            }
            return events;
        }

        events
    }
}

pub fn parse_codex_sse_line(line: &str) -> Option<String> {
    let mut state = CodexSseParserState::new();
    let events = state.parse_line(line);
    for ev in events {
        if let V1StreamEvent::ContentDelta(c) = ev {
            return Some(c);
        }
    }
    None
}
