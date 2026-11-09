//! Streaming execution of a v1 request (SSE parsing and event mapping).

use super::payload::build_gemini_payload;
use super::signature::{
    sanitize_thought_signature, signature_family, store_thought_signature_scoped,
};
use crate::llm::providers::antigravity::constants::*;
use crate::llm::providers::antigravity::types::generate_pseudo_uuid;
use crate::llm::v1_engine::{
    V1ChatRequest, V1Response, V1StreamEvent, V1ToolCall, V1ToolCallFunction,
};
use anyhow::{Context, Result};
use std::io::BufRead;

#[allow(clippy::too_many_arguments)]
pub fn execute_v1_stream<F>(
    client: &reqwest::blocking::Client,
    access_token: &str,
    project_id: &str,
    req: &V1ChatRequest,
    on_event: &mut F,
) -> Result<V1Response>
where
    F: FnMut(V1StreamEvent) -> Result<()>,
{
    let endpoint = format!(
        "{}/v1internal:streamGenerateContent?alt=sse",
        ANTIGRAVITY_BASE_URL
    );
    let payload = build_gemini_payload(req, project_id);

    let response = client
        .post(&endpoint)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", ANTIGRAVITY_USER_AGENT)
        .header("X-Client-Name", "antigravity")
        .header("X-Client-Version", "2.11.0")
        .json(&payload)
        .send()
        .context("Failed to send request to Antigravity stream endpoint")?;

    if !response.status().is_success() {
        return Err(crate::llm::types::LlmHttpError::from_blocking_response(
            "Antigravity",
            response,
        )
        .into());
    }

    let reader = std::io::BufReader::new(response);
    let mut full_content = String::new();
    let mut tool_calls = Vec::new();
    let mut function_index = 0;
    let mut finish_reason = "stop".to_string();
    let mut pending_thought_signature: Option<String> = None;
    let target_family = signature_family(&req.model);

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if let Some(data_str) = trimmed.strip_prefix("data: ") {
            if data_str.trim() == "[DONE]" {
                break;
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(data_str) {
                let candidates = val
                    .pointer("/response/candidates")
                    .or_else(|| val.pointer("/candidates"))
                    .and_then(|c| c.as_array());

                if let Some(cands) = candidates {
                    for cand in cands {
                        if let Some(f_reason) = cand.get("finishReason").and_then(|r| r.as_str())
                            && f_reason == "MAX_TOKENS"
                        {
                            finish_reason = "length".to_string();
                        }

                        if let Some(parts) =
                            cand.pointer("/content/parts").and_then(|p| p.as_array())
                        {
                            for part in parts {
                                let part_thought_sig = part
                                    .get("thoughtSignature")
                                    .or_else(|| part.get("thought_signature"))
                                    .and_then(|s| s.as_str())
                                    .filter(|s| !s.trim().is_empty())
                                    .map(|s| s.to_string());

                                if let Some(ref sig) = part_thought_sig {
                                    pending_thought_signature = Some(sig.clone());
                                }

                                let is_thought = part
                                    .get("thought")
                                    .and_then(|t| t.as_bool())
                                    .unwrap_or(false);

                                let has_fc = part.get("functionCall").is_some();
                                let has_text = part
                                    .get("text")
                                    .and_then(|t| t.as_str())
                                    .map(|t| !t.is_empty())
                                    .unwrap_or(false);

                                if part_thought_sig.is_some() && !has_fc && !has_text {
                                    continue;
                                }

                                if is_thought {
                                    if let Some(txt) = part.get("text").and_then(|t| t.as_str())
                                        && !txt.is_empty()
                                    {
                                        on_event(V1StreamEvent::ThoughtDelta(txt.to_string()))?;
                                    }
                                    // Keep client connection active during thinking phase
                                    on_event(V1StreamEvent::KeepAlive)?;
                                    continue;
                                }

                                if let Some(text) = part.get("text").and_then(|t| t.as_str())
                                    && !text.is_empty()
                                {
                                    full_content.push_str(text);
                                    on_event(V1StreamEvent::ContentDelta(text.to_string()))?;
                                }

                                if let Some(fc) = part.get("functionCall") {
                                    let fc_thought_sig = fc
                                        .get("thoughtSignature")
                                        .or_else(|| fc.get("thought_signature"))
                                        .and_then(|s| s.as_str())
                                        .filter(|s| !s.trim().is_empty())
                                        .map(|s| s.to_string());

                                    let sig = fc_thought_sig
                                        .or(part_thought_sig)
                                        .or_else(|| pending_thought_signature.clone());

                                    let name = fc
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let empty_args = serde_json::json!({});
                                    let args_val = fc.get("args").unwrap_or(&empty_args);
                                    let args_str = serde_json::to_string(args_val)
                                        .unwrap_or_else(|_| "{}".to_string());
                                    let base_id = fc
                                        .get("id")
                                        .and_then(|i| i.as_str())
                                        .map(|s| s.to_string())
                                        .unwrap_or_else(|| {
                                            format!(
                                                "call_{}_{}",
                                                generate_pseudo_uuid("fc"),
                                                function_index
                                            )
                                        });

                                    let call_id = if let Some(ref s) = sig {
                                        if let Some(valid_sig) = sanitize_thought_signature(s) {
                                            store_thought_signature_scoped(
                                                &base_id,
                                                &valid_sig,
                                                Some(target_family),
                                            );
                                            let embedded_id =
                                                format!("{base_id}__thought__{valid_sig}");
                                            store_thought_signature_scoped(
                                                &embedded_id,
                                                &valid_sig,
                                                Some(target_family),
                                            );
                                            embedded_id
                                        } else {
                                            base_id
                                        }
                                    } else {
                                        base_id
                                    };

                                    let tc = V1ToolCall {
                                        id: call_id.clone(),
                                        call_type: "function".to_string(),
                                        function: V1ToolCallFunction {
                                            name: name.clone(),
                                            arguments: args_str.clone(),
                                        },
                                        thought_signature: sig.clone(),
                                        extra_content: None,
                                        provider_specific_fields: None,
                                    };
                                    tool_calls.push(tc);

                                    on_event(V1StreamEvent::ToolCallDelta {
                                        index: function_index,
                                        id: Some(call_id),
                                        name: Some(name),
                                        arguments: args_str,
                                    })?;
                                    function_index += 1;
                                    finish_reason = "tool_calls".to_string();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if !tool_calls.is_empty() {
        finish_reason = "tool_calls".to_string();
    }

    on_event(V1StreamEvent::Finish {
        finish_reason: finish_reason.clone(),
    })?;

    Ok(V1Response {
        content: full_content,
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        finish_reason,
    })
}
