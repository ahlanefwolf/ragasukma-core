use super::*;
use crate::llm::v1_engine::{V1ChatRequest, V1Message, V1StreamEvent, V1ToolCall, V1ToolCallFunction};

#[test]
fn claude_thinking_model_detection_and_budget() {
    assert!(is_claude_thinking_model("cc/claude-sonnet-4-5-thinking"));
    assert!(is_claude_thinking_model("claude-3-7-sonnet"));
    assert!(!is_claude_thinking_model("cc/claude-3-5-haiku"));
    assert_eq!(resolve_claude_thinking_budget("cc/claude-opus-4-thinking"), 2048);
    assert_eq!(resolve_claude_thinking_budget("cc/claude-sonnet-4-5-thinking"), 2048);
}

#[test]
fn build_claude_payload_from_v1_includes_system_tools_and_messages() {
    let req = V1ChatRequest {
        model: "cc/claude-3-5-sonnet-20241022".to_string(),
        messages: vec![
            V1Message {
                role: "system".to_string(),
                content: Some(serde_json::Value::String("You are helpful.".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                thought_signature: None,
            },
            V1Message {
                role: "user".to_string(),
                content: Some(serde_json::Value::String("Check weather".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                thought_signature: None,
            },
            V1Message {
                role: "assistant".to_string(),
                content: None,
                name: None,
                tool_calls: Some(vec![V1ToolCall {
                    id: "call_1".to_string(),
                    call_type: "function".to_string(),
                    function: V1ToolCallFunction {
                        name: "get_weather".to_string(),
                        arguments: "{\"city\":\"Jakarta\"}".to_string(),
                    },
                    thought_signature: None,
                    extra_content: None,
                    provider_specific_fields: None,
                }]),
                tool_call_id: None,
                thought_signature: None,
            },
            V1Message {
                role: "tool".to_string(),
                content: Some(serde_json::Value::String("{\"temp\":30}".to_string())),
                name: Some("get_weather".to_string()),
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
                thought_signature: None,
            },
        ],
        temperature: Some(0.2),
        max_tokens: Some(1024),
        stream: Some(true),
        tools: Some(vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get weather",
                "parameters": { "type": "object", "properties": {} }
            }
        })]),
        tool_choice: None,
    };

    let payload = build_claude_payload_from_v1(&req);
    assert_eq!(payload["model"], "claude-3-5-sonnet-20241022");
    assert_eq!(payload["stream"], true);
    assert_eq!(payload["system"][0]["text"], "You are helpful.");
    assert_eq!(payload["tools"].as_array().unwrap().len(), 1);
    assert_eq!(payload["messages"].as_array().unwrap().len(), 3);
}

#[test]
fn claude_sse_parser_handles_text_thinking_tool_and_finish() {
    let mut state = ClaudeSseParserState::new();

    let text_ev = state.parse_line(
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
    );
    assert!(matches!(text_ev.as_slice(), [V1StreamEvent::ContentDelta(t)] if t == "Hello"));

    let think_ev = state.parse_line(
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Hmm"}}"#,
    );
    assert!(matches!(think_ev.as_slice(), [V1StreamEvent::ThoughtDelta(t)] if t == "Hmm"));

    let start_ev = state.parse_line(
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file"}}"#,
    );
    assert!(matches!(
        start_ev.as_slice(),
        [V1StreamEvent::ToolCallDelta { index: 0, id: Some(id), name: Some(name), .. }]
        if id == "toolu_1" && name == "read_file"
    ));

    let args_ev = state.parse_line(
        r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a.rs\"}"}}"#,
    );
    assert!(matches!(
        args_ev.as_slice(),
        [V1StreamEvent::ToolCallDelta { index: 0, arguments, .. }]
        if arguments == "{\"path\":\"a.rs\"}"
    ));

    let finish_ev = state.parse_line(
        r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#,
    );
    assert!(matches!(
        finish_ev.as_slice(),
        [V1StreamEvent::Finish { finish_reason }]
        if finish_reason == "tool_calls"
    ));
}
