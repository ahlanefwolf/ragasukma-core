use super::*;
use crate::llm::v1_engine::{V1ChatRequest, V1Message, V1StreamEvent, V1ToolCall, V1ToolCallFunction};

#[test]
fn clamp_call_id_respects_64_char_limit() {
    assert_eq!(clamp_call_id("call_123"), "call_123");
    let long_id = "a".repeat(80);
    let clamped = clamp_call_id(&long_id);
    assert_eq!(clamped.len(), 64);
}

#[test]
fn build_codex_payload_from_v1_includes_instructions_input_and_tools() {
    let req = V1ChatRequest {
        model: "cx/gpt-5.3-codex".to_string(),
        messages: vec![
            V1Message {
                role: "system".to_string(),
                content: Some(serde_json::Value::String("Be concise.".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                thought_signature: None,
            },
            V1Message {
                role: "user".to_string(),
                content: Some(serde_json::Value::String("List files".to_string())),
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
                        name: "list_dir".to_string(),
                        arguments: "{\"path\":\".\"}".to_string(),
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
                content: Some(serde_json::Value::String("[\"src\"]".to_string())),
                name: Some("list_dir".to_string()),
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
                "name": "list_dir",
                "description": "List directory",
                "parameters": { "type": "object", "properties": {} }
            }
        })]),
        tool_choice: None,
    };

    let payload = build_codex_payload_from_v1(&req);
    assert_eq!(payload["model"], "gpt-5.3-codex");
    assert_eq!(payload["stream"], true);
    assert_eq!(payload["instructions"], "Be concise.");
    assert_eq!(payload["tools"].as_array().unwrap().len(), 1);
    assert!(payload["input"].as_array().unwrap().len() >= 3);
}

#[test]
fn codex_sse_parser_handles_text_reasoning_tool_and_completed() {
    let mut state = CodexSseParserState::new();

    let text_ev = state.parse_line(
        r#"data: {"type":"response.output_text.delta","delta":"Hi"}"#,
    );
    assert!(matches!(text_ev.as_slice(), [V1StreamEvent::ContentDelta(t)] if t == "Hi"));

    let think_ev = state.parse_line(
        r#"data: {"type":"response.reasoning_summary_text.delta","delta":"Thinking..."}"#,
    );
    assert!(matches!(think_ev.as_slice(), [V1StreamEvent::ThoughtDelta(t)] if t == "Thinking..."));

    let item_ev = state.parse_line(
        r#"data: {"type":"response.output_item.added","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read_file","arguments":""}}"#,
    );
    assert!(matches!(
        item_ev.as_slice(),
        [V1StreamEvent::ToolCallDelta { index: 0, id: Some(id), name: Some(name), .. }]
        if id == "call_1" && name == "read_file"
    ));

    let args_ev = state.parse_line(
        r#"data: {"type":"response.function_call_arguments.delta","call_id":"call_1","delta":"{\"path\":\"b.rs\"}"}"#,
    );
    assert!(matches!(
        args_ev.as_slice(),
        [V1StreamEvent::ToolCallDelta { index: 0, arguments, .. }]
        if arguments == "{\"path\":\"b.rs\"}"
    ));

    let done_ev = state.parse_line(
        r#"data: {"type":"response.completed","response":{"status":"completed"}}"#,
    );
    assert!(matches!(
        done_ev.as_slice(),
        [V1StreamEvent::Finish { finish_reason }]
        if finish_reason == "stop"
    ));
}
