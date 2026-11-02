use super::*;
use crate::llm::providers::antigravity::constants::DEFAULT_THINKING_AG_SIGNATURE;
use crate::llm::v1_engine::{V1ChatRequest, V1ToolCall, V1ToolCallFunction};

#[test]
fn test_clean_parameters_schema_with_zed_types() {
    let raw_schema = serde_json::json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": "ZedToolParameters",
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "path": {
                "type": "string",
                "description": "File path"
            },
            "offset": {
                "type": ["integer", "null"],
                "description": "Offset in bytes"
            },
            "options": {
                "anyOf": [
                    { "type": "string" },
                    { "type": "null" }
                ]
            }
        },
        "required": ["path", "nonexistent"]
    });

    let cleaned = clean_parameters_schema(&raw_schema);

    // Assert root properties
    assert_eq!(cleaned["type"], "object");
    assert!(cleaned.get("$schema").is_none());
    assert!(cleaned.get("title").is_none());
    assert!(cleaned.get("additionalProperties").is_none());

    // Assert path
    assert_eq!(cleaned["properties"]["path"]["type"], "string");

    // Assert offset flattened from ["integer", "null"] to "integer" with nullable: true
    assert_eq!(cleaned["properties"]["offset"]["type"], "integer");
    assert_eq!(cleaned["properties"]["offset"]["nullable"], true);

    // Assert options flattened from anyOf to "string" with nullable: true
    assert_eq!(cleaned["properties"]["options"]["type"], "string");
    assert_eq!(cleaned["properties"]["options"]["nullable"], true);

    // Assert required pruned non-existent field
    let required = cleaned["required"].as_array().unwrap();
    assert_eq!(required.len(), 1);
    assert_eq!(required[0], "path");
}

#[test]
fn test_thought_signature_cache_and_resolve() {
    let call_id = "call_test_12345";
    let sig = "test_signature_abc_xyz==";
    store_thought_signature(call_id, sig);

    // Retrieve from cache
    assert_eq!(get_thought_signature(call_id), Some(sig.to_string()));
    assert_eq!(resolve_call_signature(call_id), Some(sig.to_string()));

    // Non-existent ID returns None
    assert_eq!(get_thought_signature("call_non_existent"), None);

    // Embedded signature in ID takes priority
    let embedded_id = "call_test_12345__thought__embedded_sig_999";
    assert_eq!(
        resolve_call_signature(embedded_id),
        Some("embedded_sig_999".to_string())
    );
}

#[test]
fn test_build_gemini_payload_attaches_thought_signature() {
    let req = V1ChatRequest {
        model: "ag/gemini-3.8-flash-high".to_string(),
        messages: vec![
            crate::llm::v1_engine::V1Message {
                role: "user".to_string(),
                content: Some(serde_json::json!("list directory contents")),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                thought_signature: None,
            },
            crate::llm::v1_engine::V1Message {
                role: "assistant".to_string(),
                content: None,
                tool_calls: Some(vec![V1ToolCall {
                    id: "call_list_dir_1".to_string(),
                    call_type: "function".to_string(),
                    function: V1ToolCallFunction {
                        name: "default_api:list_directory".to_string(),
                        arguments: "{\"dir_path\":\"/workspace\"}".to_string(),
                    },
                    thought_signature: None,
                    extra_content: None,
                    provider_specific_fields: None,
                }]),
                tool_call_id: None,
                name: None,
                thought_signature: None,
            },
            crate::llm::v1_engine::V1Message {
                role: "tool".to_string(),
                content: Some(serde_json::json!("[\"file1.txt\",\"file2.txt\"]")),
                tool_calls: None,
                tool_call_id: Some("call_list_dir_1".to_string()),
                name: None,
                thought_signature: None,
            },
        ],
        temperature: Some(0.7),
        max_tokens: Some(4096),
        stream: Some(true),
        tools: None,
        tool_choice: None,
    };

    let payload = build_gemini_payload(&req, "test-project");
    let contents = payload["request"]["contents"].as_array().unwrap();

    // Model turn must have functionCall; without cached or provided signature, thoughtSignature is omitted
    let model_turn = contents.iter().find(|c| c["role"] == "model").unwrap();
    let parts = model_turn["parts"].as_array().unwrap();
    let fc_part = parts
        .iter()
        .find(|p| p.get("functionCall").is_some())
        .unwrap();

    assert!(
        fc_part.get("thoughtSignature").is_none(),
        "thoughtSignature must be omitted when no signature is provided or cached to prevent Google API 400 TYPE_BYTES decoding errors"
    );
    assert_eq!(
        fc_part["functionCall"]["name"].as_str().unwrap(),
        "default_api:list_directory"
    );
}

#[test]
fn test_build_gemini_payload_uses_cached_signature() {
    let call_id = "call_cached_sig_test";
    let custom_sig = "EuwGCukGAXLI2custom_stored_signature";
    store_thought_signature(call_id, custom_sig);

    let req = V1ChatRequest {
        model: "gemini-3.8-flash-high".to_string(),
        messages: vec![
            crate::llm::v1_engine::V1Message {
                role: "user".to_string(),
                content: Some(serde_json::json!("hi")),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                thought_signature: None,
            },
            crate::llm::v1_engine::V1Message {
                role: "assistant".to_string(),
                content: None,
                tool_calls: Some(vec![V1ToolCall {
                    id: call_id.to_string(),
                    call_type: "function".to_string(),
                    function: V1ToolCallFunction {
                        name: "test_tool".to_string(),
                        arguments: "{}".to_string(),
                    },
                    thought_signature: None,
                    extra_content: None,
                    provider_specific_fields: None,
                }]),
                tool_call_id: None,
                name: None,
                thought_signature: None,
            },
        ],
        temperature: None,
        max_tokens: None,
        stream: None,
        tools: None,
        tool_choice: None,
    };

    let payload = build_gemini_payload(&req, "test-project");
    let contents = payload["request"]["contents"].as_array().unwrap();
    let model_turn = contents.iter().find(|c| c["role"] == "model").unwrap();
    let fc_part = &model_turn["parts"][0];

    assert_eq!(fc_part["thoughtSignature"].as_str().unwrap(), custom_sig);
}

#[test]
fn test_default_thinking_ag_signature_is_strictly_valid_base64() {
    assert_eq!(DEFAULT_THINKING_AG_SIGNATURE.len() % 4, 0);
    assert!(is_valid_base64(DEFAULT_THINKING_AG_SIGNATURE));
    assert_eq!(
        sanitize_thought_signature(DEFAULT_THINKING_AG_SIGNATURE),
        Some(DEFAULT_THINKING_AG_SIGNATURE.to_string())
    );
}

#[test]
fn test_base64_validation_and_sanitizer() {
    // Valid standard and padded base64
    assert!(is_valid_base64("EuwGCuk="));
    assert!(is_valid_base64("EuwGCukG"));
    assert_eq!(
        sanitize_thought_signature("EuwGCuk="),
        Some("EuwGCuk=".to_string())
    );

    // Auto-padded unpadded valid base64
    assert!(is_valid_base64("EuwGCg")); // len 6 -> + "==" -> len 8
    assert_eq!(
        sanitize_thought_signature("EuwGCg"),
        Some("EuwGCg==".to_string())
    );

    // Corrupted / impossible length (len % 4 == 1)
    assert!(!is_valid_base64("EuwG1"));
    assert_eq!(sanitize_thought_signature("EuwG1"), None);

    // Corrupted non-base64 characters
    assert!(!is_valid_base64("EuwG!@#$%^&*()"));
    assert_eq!(sanitize_thought_signature("EuwG!@#$%^&*()"), None);

    // Empty strings
    assert!(!is_valid_base64(""));
    assert_eq!(sanitize_thought_signature(""), None);
}

#[test]
fn test_embedded_thought_signature_in_tool_call_id() {
    let raw_sig = "EuwGCukGAXLI2valid_signature_token==";
    let base_id = "call_zed_read_file_42";
    let embedded_id = format!("{base_id}__thought__{raw_sig}");

    // resolve_call_signature should parse and sanitize the signature from embedded id
    assert_eq!(resolve_call_signature(&embedded_id), Some(raw_sig.to_string()));

    let req = V1ChatRequest {
        model: "gemini-3.8-flash-high".to_string(),
        messages: vec![
            crate::llm::v1_engine::V1Message {
                role: "user".to_string(),
                content: Some(serde_json::json!("Read file")),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                thought_signature: None,
            },
            crate::llm::v1_engine::V1Message {
                role: "assistant".to_string(),
                content: None,
                tool_calls: Some(vec![V1ToolCall {
                    id: embedded_id.clone(),
                    call_type: "function".to_string(),
                    function: V1ToolCallFunction {
                        name: "read_file".to_string(),
                        arguments: "{\"path\":\"src/main.rs\"}".to_string(),
                    },
                    thought_signature: None,
                    extra_content: None,
                    provider_specific_fields: None,
                }]),
                tool_call_id: None,
                name: None,
                thought_signature: None,
            },
            crate::llm::v1_engine::V1Message {
                role: "tool".to_string(),
                content: Some(serde_json::json!("file content here")),
                tool_calls: None,
                tool_call_id: Some(embedded_id.clone()),
                name: None,
                thought_signature: None,
            },
        ],
        temperature: None,
        max_tokens: None,
        stream: None,
        tools: None,
        tool_choice: None,
    };

    let payload = build_gemini_payload(&req, "test-project");
    let contents = payload["request"]["contents"].as_array().unwrap();

    // Verify model turn has clean id in functionCall and embedded signature in thoughtSignature
    let model_turn = contents.iter().find(|c| c["role"] == "model").unwrap();
    let fc = &model_turn["parts"][0]["functionCall"];
    assert_eq!(fc["id"].as_str().unwrap(), base_id);
    assert_eq!(fc["name"].as_str().unwrap(), "read_file");
    assert_eq!(
        model_turn["parts"][0]["thoughtSignature"].as_str().unwrap(),
        raw_sig
    );

    // Verify user turn with functionResponse has clean id matching base_id
    let user_tool_turn = contents.iter().find(|c| {
        c["role"] == "user" && c["parts"][0].get("functionResponse").is_some()
    }).unwrap();
    let fr = &user_tool_turn["parts"][0]["functionResponse"];
    assert_eq!(fr["id"].as_str().unwrap(), base_id);
    assert_eq!(fr["name"].as_str().unwrap(), "read_file");
}

#[test]
fn test_thought_signature_model_family_scoping() {
    let claude_call = "toolu_claude_123";
    let gemini_call = "call_gemini_456";
    let claude_sig = "EuwGCukClaudeValidSig01=";
    let gemini_sig = "EuwGCukGeminiValidSig01=";

    store_thought_signature_scoped(claude_call, claude_sig, Some("claude"));
    store_thought_signature_scoped(gemini_call, gemini_sig, Some("gemini"));

    // Scoped lookup prevents cross-pollination
    assert_eq!(get_thought_signature_scoped(claude_call, Some("claude")), Some(claude_sig.to_string()));
    assert_eq!(get_thought_signature_scoped(claude_call, Some("gemini")), None);

    assert_eq!(get_thought_signature_scoped(gemini_call, Some("gemini")), Some(gemini_sig.to_string()));
    assert_eq!(get_thought_signature_scoped(gemini_call, Some("claude")), None);

    // Unscoped lookup fallback
    assert_eq!(get_thought_signature(claude_call), Some(claude_sig.to_string()));
    assert_eq!(get_thought_signature(gemini_call), Some(gemini_sig.to_string()));
}

#[test]
fn test_claude_on_antigravity_attaches_claude_default_signature() {
    let req = V1ChatRequest {
        model: "ag/claude-sonnet-4-6".to_string(),
        messages: vec![
            crate::llm::v1_engine::V1Message {
                role: "user".to_string(),
                content: Some(serde_json::json!("run command")),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                thought_signature: None,
            },
            crate::llm::v1_engine::V1Message {
                role: "assistant".to_string(),
                content: None,
                tool_calls: Some(vec![V1ToolCall {
                    id: "toolu_vrtx_999".to_string(),
                    call_type: "function".to_string(),
                    function: V1ToolCallFunction {
                        name: "bash".to_string(),
                        arguments: "{\"command\":\"ls\"}".to_string(),
                    },
                    thought_signature: None,
                    extra_content: None,
                    provider_specific_fields: None,
                }]),
                tool_call_id: None,
                name: None,
                thought_signature: None,
            },
        ],
        temperature: None,
        max_tokens: None,
        stream: None,
        tools: None,
        tool_choice: None,
    };

    let payload = build_gemini_payload(&req, "test-project");
    let contents = payload["request"]["contents"].as_array().unwrap();
    let model_turn = contents.iter().find(|c| c["role"] == "model").unwrap();
    let fc_part = &model_turn["parts"][0];

    // Claude on Antigravity without cached signature must use DEFAULT_THINKING_CLAUDE_SIGNATURE
    assert_eq!(
        fc_part["thoughtSignature"].as_str().unwrap(),
        crate::llm::providers::antigravity::constants::DEFAULT_THINKING_CLAUDE_SIGNATURE
    );

    // Claude generationConfig must not contain Gemini thinkingConfig
    let gen_config = &payload["request"]["generationConfig"];
    assert!(gen_config.get("thinkingConfig").is_none());
}

#[test]
fn test_gemini_thinking_config_auto_enabled() {
    let req = V1ChatRequest {
        model: "ag/gemini-3.8-flash-high".to_string(),
        messages: vec![crate::llm::v1_engine::V1Message {
            role: "user".to_string(),
            content: Some(serde_json::json!("Explain Rust async")),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            thought_signature: None,
        }],
        temperature: None,
        max_tokens: None,
        stream: None,
        tools: None,
        tool_choice: None,
    };

    let payload = build_gemini_payload(&req, "test-project");
    let thinking_config = &payload["request"]["generationConfig"]["thinkingConfig"];
    assert!(thinking_config.is_object());
    assert_eq!(thinking_config["thinkingBudget"].as_i64().unwrap(), -1);
    assert_eq!(thinking_config["includeThoughts"].as_bool().unwrap(), true);
}

#[test]
fn test_gemini_thinking_budget_resolution() {
    use crate::llm::providers::antigravity::v1_adapter::resolve_gemini_thinking_budget;

    assert_eq!(resolve_gemini_thinking_budget("ag/gemini-3.8-flash-low"), (1024, true));
    assert_eq!(resolve_gemini_thinking_budget("gemini-3.8-flash-low"), (1024, true));
    assert_eq!(resolve_gemini_thinking_budget("gemini-3.8-flash(low)"), (1024, true));
    assert_eq!(resolve_gemini_thinking_budget("gemini-3.8-flash-minimal"), (512, true));
    assert_eq!(resolve_gemini_thinking_budget("gemini-3.8-flash-medium"), (4096, true));
    assert_eq!(resolve_gemini_thinking_budget("gemini-3.8-flash(medium)"), (4096, true));
    assert_eq!(resolve_gemini_thinking_budget("gemini-3.8-flash"), (4096, true));
    assert_eq!(resolve_gemini_thinking_budget("ag/gemini-3.8-flash-high"), (-1, true));
    assert_eq!(resolve_gemini_thinking_budget("gemini-3.8-flash-high"), (-1, true));
    assert_eq!(resolve_gemini_thinking_budget("gemini-3.8-flash(high)"), (-1, true));
    assert_eq!(resolve_gemini_thinking_budget("gemini-3.8-flash-none"), (0, false));
    assert_eq!(resolve_gemini_thinking_budget("gemini-3.8-flash(none)"), (0, false));
    assert_eq!(resolve_gemini_thinking_budget("gemini-3.8-flash-off"), (0, false));
}

#[test]
fn test_gemini_thinking_config_tiered_payloads() {
    let req_low = V1ChatRequest {
        model: "ag/gemini-3.8-flash-low".to_string(),
        messages: vec![crate::llm::v1_engine::V1Message {
            role: "user".to_string(),
            content: Some(serde_json::json!("Fix bug in code")),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            thought_signature: None,
        }],
        temperature: None,
        max_tokens: None,
        stream: None,
        tools: None,
        tool_choice: None,
    };
    let payload_low = build_gemini_payload(&req_low, "test-project");
    let tc_low = &payload_low["request"]["generationConfig"]["thinkingConfig"];
    assert_eq!(tc_low["thinkingBudget"].as_i64().unwrap(), 1024);
    assert_eq!(tc_low["includeThoughts"].as_bool().unwrap(), true);
    assert_eq!(payload_low["model"].as_str().unwrap(), "gemini-3.8-flash-low");

    let req_none = V1ChatRequest {
        model: "ag/gemini-3.8-flash-none".to_string(),
        messages: vec![crate::llm::v1_engine::V1Message {
            role: "user".to_string(),
            content: Some(serde_json::json!("Say hi")),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            thought_signature: None,
        }],
        temperature: None,
        max_tokens: None,
        stream: None,
        tools: None,
        tool_choice: None,
    };
    let payload_none = build_gemini_payload(&req_none, "test-project");
    let tc_none = &payload_none["request"]["generationConfig"]["thinkingConfig"];
    assert_eq!(tc_none["thinkingBudget"].as_i64().unwrap(), 0);
    assert_eq!(tc_none["includeThoughts"].as_bool().unwrap(), false);

    let req_paren = V1ChatRequest {
        model: "ag/gemini-3.8-flash-low(low)".to_string(),
        messages: vec![crate::llm::v1_engine::V1Message {
            role: "user".to_string(),
            content: Some(serde_json::json!("Fast edit")),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            thought_signature: None,
        }],
        temperature: None,
        max_tokens: None,
        stream: None,
        tools: None,
        tool_choice: None,
    };
    let payload_paren = build_gemini_payload(&req_paren, "test-project");
    let tc_paren = &payload_paren["request"]["generationConfig"]["thinkingConfig"];
    assert_eq!(tc_paren["thinkingBudget"].as_i64().unwrap(), 1024);
    assert_eq!(tc_paren["includeThoughts"].as_bool().unwrap(), true);
    assert_eq!(payload_paren["model"].as_str().unwrap(), "gemini-3.8-flash-low");
}
