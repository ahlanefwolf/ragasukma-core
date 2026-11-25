use crate::llm::v1_engine::V1StreamEvent;
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

