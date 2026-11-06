use anyhow::Result;
use serde_json::json;
use std::collections::{HashMap, HashSet};

use crate::agent::events::broadcast_session_event;
use crate::agent::models::AgentSessionEvent;
use crate::llm::providers::antigravity::AntigravityProvider;
use crate::llm::providers::claude::ClaudeProvider;
use crate::llm::providers::codex::CodexProvider;
use crate::llm::v1_engine::{
    V1ChatRequest, V1Message, V1StreamEvent, V1ToolCall, V1ToolCallFunction,
};

/// Upper bound on how long a single streaming attempt may run before it is abandoned.
///
/// Without this bound a provider that accepts the connection and then stalls would keep
/// the `spawn_blocking` thread alive forever, so the agent could never conclude a turn.
const STREAM_ATTEMPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Total attempts (initial + retries) for one streaming call.
const MAX_STREAM_ATTEMPTS: u32 = 3;

/// First backoff step; later steps grow exponentially from here.
const BASE_RETRY_DELAY_MS: u64 = 800;

/// Ceiling for a single backoff sleep, applied after `Retry-After` is considered.
const MAX_RETRY_DELAY_MS: u64 = 30_000;

/// Computes how long to wait before retry number `attempt` (1-based).
///
/// Honours a server-provided `Retry-After` when present, otherwise applies exponential
/// backoff with full jitter so parallel sessions do not retry in lockstep.
fn retry_delay(attempt: u32, err_str: &str) -> std::time::Duration {
    if let Some(server_ms) = parse_retry_after_ms(err_str) {
        return std::time::Duration::from_millis(server_ms.min(MAX_RETRY_DELAY_MS));
    }

    let exp = BASE_RETRY_DELAY_MS.saturating_mul(1u64 << attempt.saturating_sub(1).min(10));
    let capped = exp.min(MAX_RETRY_DELAY_MS);

    // Full jitter in [capped/2, capped] keeps the spread while still backing off.
    let jitter_seed = jitter_seed();
    let half = capped / 2;
    let delay = half + (jitter_seed % half.max(1));
    std::time::Duration::from_millis(delay)
}

/// Extracts a `Retry-After` hint from an error string, in milliseconds.
/// Accepts both delta-seconds (`Retry-After: 5`) and structured JSON fields.
fn parse_retry_after_ms(err_str: &str) -> Option<u64> {
    let lower = err_str.to_ascii_lowercase();
    let idx = lower.find("retry-after")?;
    let rest = &err_str[idx..];

    let digits: String = rest
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();

    if digits.is_empty() {
        return None;
    }

    let secs: u64 = digits.parse().ok()?;
    Some(secs.saturating_mul(1000))
}

/// Cheap, dependency-free pseudo-random value used only to de-synchronise retries.
fn jitter_seed() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);

    // Mix the clock with a counter so two threads starting in the same nanosecond diverge.
    nanos
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(seq.wrapping_mul(0xBF58_476D_1CE4_E5B9))
        ^ (nanos >> 7)
}

/// Accumulates streaming tool-call deltas into `collected` without duplicating
/// arguments or losing the tool name.
///
/// Providers (OpenAI-compatible, Gemini, Claude) disagree on how they split a
/// tool call across deltas: some send `name` only on the first chunk, others
/// resend it, and some restart `index` at 0 after a retry. Blindly appending
/// `arguments` produced corrupted payloads like `{"path":"a"}{"path":"a"}`
/// and tool calls whose `name` was empty, which then failed with
/// `Unknown tool ''`.
fn accumulate_tool_call_delta(
    collected: &mut Vec<V1ToolCall>,
    seen: &mut HashMap<usize, ToolCallDeltaProgress>,
    index: usize,
    id: Option<String>,
    name: Option<String>,
    arguments: &str,
) {
    while collected.len() <= index {
        let slot = collected.len();
        collected.push(V1ToolCall {
            id: format!("call_{}", slot),
            call_type: "function".to_string(),
            function: V1ToolCallFunction {
                name: String::new(),
                arguments: String::new(),
            },
            thought_signature: None,
            extra_content: None,
            provider_specific_fields: None,
        });
    }

    let fresh_name = name.as_deref().is_some_and(|n| !n.trim().is_empty());
    let fresh_id = id.as_deref().is_some_and(|i| !i.trim().is_empty());
    let progress = seen.entry(index).or_default();

    // A delta that carries a *different* id or name for an index we already
    // filled means the provider restarted that slot (typical after a retry or
    // after a KeepAlive gap). Reset the accumulator so we do not concatenate
    // two unrelated argument payloads.
    let restarted = (fresh_id
        && progress
            .id
            .as_deref()
            .is_some_and(|prev| Some(prev) != id.as_deref()))
        || (fresh_name
            && progress
                .name
                .as_deref()
                .is_some_and(|prev| Some(prev) != name.as_deref()));
    if restarted {
        collected[index].function.arguments.clear();
        progress.saw_arguments = false;
    }

    if let Some(tc_id) = id
        && !tc_id.trim().is_empty()
    {
        collected[index].id = tc_id.clone();
        progress.id = Some(tc_id);
    }
    if let Some(tc_name) = name
        && !tc_name.trim().is_empty()
    {
        collected[index].function.name = tc_name.clone();
        progress.name = Some(tc_name);
    }

    if arguments.is_empty() {
        return;
    }

    let slot = &mut collected[index].function.arguments;
    if !progress.saw_arguments {
        // First argument chunk for this slot: never append onto stale data.
        slot.clear();
        progress.saw_arguments = true;
        slot.push_str(arguments);
    } else if slot.ends_with(arguments) {
        // Idempotent re-send of the same fragment; ignore the duplicate.
    } else {
        slot.push_str(arguments);
    }
}

#[derive(Default)]
struct ToolCallDeltaProgress {
    id: Option<String>,
    name: Option<String>,
    saw_arguments: bool,
}

pub struct ModelTurnResult {
    pub content: String,
    pub thought: String,
    pub tool_calls: Option<Vec<V1ToolCall>>,
    pub estimated_input_tokens: usize,
    pub estimated_output_tokens: usize,
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_model_stream(
    session_id: &str,
    model: &str,
    is_codex: bool,
    temperature: Option<f64>,
    messages: Vec<V1Message>,
    tools_schema: Vec<serde_json::Value>,
    force_final_text_only: bool,
    iteration: usize,
) -> Result<ModelTurnResult> {
    let (tools_payload, tool_choice_payload) = if force_final_text_only {
        (None, Some(json!("none")))
    } else {
        (Some(tools_schema), None)
    };

    let chat_req = V1ChatRequest {
        model: model.to_string(),
        messages,
        temperature: temperature.map(|t| (t as f32).min(0.2)).or(Some(0.1)),
        max_tokens: Some(8192),
        stream: Some(true),
        tools: tools_payload,
        tool_choice: tool_choice_payload,
    };

    let input_chars: usize = chat_req
        .messages
        .iter()
        .map(|m| m.content.as_ref().map(|c| c.to_string().len()).unwrap_or(0))
        .sum();
    let estimated_input_tokens = (input_chars / 4).max(1);
    let turn_start_time = std::time::Instant::now();

    let req_clone = chat_req.clone();
    let sess_id_clone = session_id.to_string();
    let model_clone = model.to_string();
    let current_iteration = iteration;
    let cancellation_flag = crate::agent::events::get_session_cancellation_flag(session_id);

    let call_res = tokio::task::spawn_blocking(move || {
        let mut last_err = None;
        let mut deltas_broadcasted = false;

        for attempt in 1..=MAX_STREAM_ATTEMPTS {
            if cancellation_flag.load(std::sync::atomic::Ordering::Relaxed) {
                anyhow::bail!("Session cancelled by user");
            }

            // Ghost Streaming mitigation: If retrying after tokens were already streamed to UI,
            // send a StreamReset event so UI clears partial accumulated tokens and avoids duplicate output.
            if attempt > 1 && deltas_broadcasted {
                broadcast_session_event(&sess_id_clone, AgentSessionEvent::StreamReset);
                deltas_broadcasted = false;
            }

            let mut accumulated_content = String::new();
            let mut accumulated_thought = String::new();
            let mut collected_tool_calls: Vec<V1ToolCall> = Vec::new();

            let family = crate::agent::executor::state::detect_provider_family(&model_clone);

            let attempt_result =
                if family == crate::agent::executor::state::ModelProviderFamily::Claude {
                    let mut announced_tool_indices: HashSet<usize> = HashSet::new();
                    let mut delta_progress: HashMap<usize, ToolCallDeltaProgress> = HashMap::new();
                    let mut provider = ClaudeProvider::new();
                    let res = provider.chat_stream_v1(&req_clone, |event| {
                        if cancellation_flag.load(std::sync::atomic::Ordering::Relaxed) {
                            anyhow::bail!("Session cancelled by user");
                        }
                        match event {
                            V1StreamEvent::ContentDelta(delta) => {
                                accumulated_content.push_str(&delta);
                                deltas_broadcasted = true;
                                broadcast_session_event(
                                    &sess_id_clone,
                                    AgentSessionEvent::Delta { delta },
                                );
                            }
                            V1StreamEvent::ThoughtDelta(thought) => {
                                accumulated_thought.push_str(&thought);
                                deltas_broadcasted = true;
                                broadcast_session_event(
                                    &sess_id_clone,
                                    AgentSessionEvent::ThoughtDelta { delta: thought },
                                );
                            }
                            V1StreamEvent::ToolCallDelta {
                                index,
                                id,
                                name,
                                arguments,
                            } => {
                                let announced_name = name
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|n| !n.is_empty())
                                    .map(str::to_string);
                                accumulate_tool_call_delta(
                                    &mut collected_tool_calls,
                                    &mut delta_progress,
                                    index,
                                    id,
                                    name,
                                    &arguments,
                                );
                                if let Some(tc_name) = announced_name
                                    && announced_tool_indices.insert(index)
                                {
                                    broadcast_session_event(
                                        &sess_id_clone,
                                        AgentSessionEvent::StepInfo {
                                            iteration: current_iteration,
                                            action: format!("tool:{}", tc_name),
                                            details: Some(format!("Preparing `{}`...", tc_name)),
                                        },
                                    );
                                }
                            }
                            _ => {}
                        }
                        Ok(())
                    });
                    res.map(|v1_res| {
                        let tc = if v1_res.tool_calls.is_some() {
                            v1_res.tool_calls
                        } else if !collected_tool_calls.is_empty() {
                            Some(collected_tool_calls)
                        } else {
                            None
                        };
                        (accumulated_content, accumulated_thought, tc)
                    })
                } else if family == crate::agent::executor::state::ModelProviderFamily::Codex
                    || is_codex
                {
                    let mut announced_tool_indices: HashSet<usize> = HashSet::new();
                    let mut delta_progress: HashMap<usize, ToolCallDeltaProgress> = HashMap::new();
                    let mut provider = CodexProvider::new();
                    let res = provider.chat_stream_v1(&req_clone, |event| {
                        if cancellation_flag.load(std::sync::atomic::Ordering::Relaxed) {
                            anyhow::bail!("Session cancelled by user");
                        }
                        match event {
                            V1StreamEvent::ContentDelta(delta) => {
                                accumulated_content.push_str(&delta);
                                deltas_broadcasted = true;
                                broadcast_session_event(
                                    &sess_id_clone,
                                    AgentSessionEvent::Delta { delta },
                                );
                            }
                            V1StreamEvent::ThoughtDelta(thought) => {
                                accumulated_thought.push_str(&thought);
                                deltas_broadcasted = true;
                                broadcast_session_event(
                                    &sess_id_clone,
                                    AgentSessionEvent::ThoughtDelta { delta: thought },
                                );
                            }
                            V1StreamEvent::ToolCallDelta {
                                index,
                                id,
                                name,
                                arguments,
                            } => {
                                let announced_name = name
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|n| !n.is_empty())
                                    .map(str::to_string);
                                accumulate_tool_call_delta(
                                    &mut collected_tool_calls,
                                    &mut delta_progress,
                                    index,
                                    id,
                                    name,
                                    &arguments,
                                );
                                if let Some(tc_name) = announced_name
                                    && announced_tool_indices.insert(index)
                                {
                                    broadcast_session_event(
                                        &sess_id_clone,
                                        AgentSessionEvent::StepInfo {
                                            iteration: current_iteration,
                                            action: format!("tool:{}", tc_name),
                                            details: Some(format!("Preparing `{}`...", tc_name)),
                                        },
                                    );
                                }
                            }
                            _ => {}
                        }
                        Ok(())
                    });
                    res.map(|v1_res| {
                        let tc = if v1_res.tool_calls.is_some() {
                            v1_res.tool_calls
                        } else if !collected_tool_calls.is_empty() {
                            Some(collected_tool_calls)
                        } else {
                            None
                        };
                        (accumulated_content, accumulated_thought, tc)
                    })
                } else if family == crate::agent::executor::state::ModelProviderFamily::Custom {
                    let mut announced_tool_indices: HashSet<usize> = HashSet::new();
                    let mut delta_progress: HashMap<usize, ToolCallDeltaProgress> = HashMap::new();
                    let mut provider = crate::llm::providers::custom::CustomProvider::new();
                    let res = provider.chat_stream_v1(&req_clone, |event| {
                        if cancellation_flag.load(std::sync::atomic::Ordering::Relaxed) {
                            anyhow::bail!("Session cancelled by user");
                        }
                        match event {
                            V1StreamEvent::ContentDelta(delta) => {
                                accumulated_content.push_str(&delta);
                                deltas_broadcasted = true;
                                broadcast_session_event(
                                    &sess_id_clone,
                                    AgentSessionEvent::Delta { delta },
                                );
                            }
                            V1StreamEvent::ThoughtDelta(thought) => {
                                accumulated_thought.push_str(&thought);
                                deltas_broadcasted = true;
                                broadcast_session_event(
                                    &sess_id_clone,
                                    AgentSessionEvent::ThoughtDelta { delta: thought },
                                );
                            }
                            V1StreamEvent::ToolCallDelta {
                                index,
                                id,
                                name,
                                arguments,
                            } => {
                                let announced_name = name
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|n| !n.is_empty())
                                    .map(str::to_string);
                                accumulate_tool_call_delta(
                                    &mut collected_tool_calls,
                                    &mut delta_progress,
                                    index,
                                    id,
                                    name,
                                    &arguments,
                                );
                                if let Some(tc_name) = announced_name
                                    && announced_tool_indices.insert(index)
                                {
                                    broadcast_session_event(
                                        &sess_id_clone,
                                        AgentSessionEvent::StepInfo {
                                            iteration: current_iteration,
                                            action: format!("tool:{}", tc_name),
                                            details: Some(format!("Preparing `{}`...", tc_name)),
                                        },
                                    );
                                }
                            }
                            _ => {}
                        }
                        Ok(())
                    });
                    res.map(|v1_res| {
                        let tc = if v1_res.tool_calls.is_some() {
                            v1_res.tool_calls
                        } else if !collected_tool_calls.is_empty() {
                            Some(collected_tool_calls)
                        } else {
                            None
                        };
                        (accumulated_content, accumulated_thought, tc)
                    })
                } else {
                    let mut announced_tool_indices: HashSet<usize> = HashSet::new();
                    let mut delta_progress: HashMap<usize, ToolCallDeltaProgress> = HashMap::new();
                    let mut provider = AntigravityProvider::new();
                    let res = provider.chat_stream_v1(&req_clone, |event| {
                        if cancellation_flag.load(std::sync::atomic::Ordering::Relaxed) {
                            anyhow::bail!("Session cancelled by user");
                        }
                        match event {
                            V1StreamEvent::ContentDelta(delta) => {
                                accumulated_content.push_str(&delta);
                                deltas_broadcasted = true;
                                broadcast_session_event(
                                    &sess_id_clone,
                                    AgentSessionEvent::Delta { delta },
                                );
                            }
                            V1StreamEvent::ThoughtDelta(thought) => {
                                accumulated_thought.push_str(&thought);
                                deltas_broadcasted = true;
                                broadcast_session_event(
                                    &sess_id_clone,
                                    AgentSessionEvent::ThoughtDelta { delta: thought },
                                );
                            }
                            V1StreamEvent::ToolCallDelta {
                                index,
                                id,
                                name,
                                arguments,
                            } => {
                                let announced_name = name
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|n| !n.is_empty())
                                    .map(str::to_string);
                                accumulate_tool_call_delta(
                                    &mut collected_tool_calls,
                                    &mut delta_progress,
                                    index,
                                    id,
                                    name,
                                    &arguments,
                                );
                                if let Some(tc_name) = announced_name
                                    && announced_tool_indices.insert(index)
                                {
                                    broadcast_session_event(
                                        &sess_id_clone,
                                        AgentSessionEvent::StepInfo {
                                            iteration: current_iteration,
                                            action: format!("tool:{}", tc_name),
                                            details: Some(format!("Preparing `{}`...", tc_name)),
                                        },
                                    );
                                }
                            }
                            _ => {}
                        }
                        Ok(())
                    });
                    res.map(|v1_res| {
                        let tc = if v1_res.tool_calls.is_some() {
                            v1_res.tool_calls
                        } else if !collected_tool_calls.is_empty() {
                            Some(collected_tool_calls)
                        } else {
                            None
                        };
                        (accumulated_content, accumulated_thought, tc)
                    })
                };

            match attempt_result {
                Ok(val) => return Ok(val),
                Err(e) => {
                    let err_str = e.to_string();
                    if cancellation_flag.load(std::sync::atomic::Ordering::Relaxed)
                        || err_str.contains("Session cancelled")
                    {
                        return Err(e);
                    }

                    let is_retryable = err_str.contains("429")
                        || err_str.contains("503")
                        || err_str.contains("502")
                        || err_str.contains("timeout")
                        || err_str.contains("connection closed")
                        || err_str.contains("broken pipe");

                    last_err = Some(e);
                    if attempt < MAX_STREAM_ATTEMPTS && is_retryable {
                        let delay = retry_delay(attempt, err_str.as_str());
                        broadcast_session_event(
                            &sess_id_clone,
                            AgentSessionEvent::StepInfo {
                                iteration: current_iteration,
                                action: "retry".to_string(),
                                details: Some(format!(
                                    "Upstream hiccup, retrying in {}ms (attempt {}/{})",
                                    delay.as_millis(),
                                    attempt + 1,
                                    MAX_STREAM_ATTEMPTS
                                )),
                            },
                        );
                        std::thread::sleep(delay);
                        continue;
                    } else {
                        break;
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Unknown upstream connection error")))
    });

    // Bound the whole retry loop. A provider that accepts the connection and then stalls
    // would otherwise pin this blocking thread indefinitely and hang the turn forever.
    let join_res = match tokio::time::timeout(STREAM_ATTEMPT_TIMEOUT, call_res).await {
        Ok(joined) => joined,
        Err(_) => {
            broadcast_session_event(
                session_id,
                AgentSessionEvent::StepInfo {
                    iteration,
                    action: "timeout".to_string(),
                    details: Some(format!(
                        "Upstream produced no response within {}s; abandoning this attempt.",
                        STREAM_ATTEMPT_TIMEOUT.as_secs()
                    )),
                },
            );
            anyhow::bail!(
                "Upstream stream timed out after {}s with no completion",
                STREAM_ATTEMPT_TIMEOUT.as_secs()
            );
        }
    };

    let call_res = join_res.map_err(|e| anyhow::anyhow!("Stream worker task failed: {e}"))??;

    let (mut final_content, mut final_thought, tool_calls_opt) = call_res;

    // Universal Synthesized Thought Extraction:
    // If the model produced thought within `<thought>...</thought>` tags in its text content,
    // extract it into `final_thought` and keep clean narrative text in `final_content`.
    if final_content.contains("<thought>") {
        let mut clean_content = String::new();
        let mut extracted_thoughts = Vec::new();
        let mut remaining = final_content.as_str();

        while let Some(open_idx) = remaining.find("<thought>") {
            clean_content.push_str(&remaining[..open_idx]);
            let after_open = &remaining[open_idx + "<thought>".len()..];
            if let Some(close_idx) = after_open.find("</thought>") {
                let inner_thought = after_open[..close_idx].trim();
                if !inner_thought.is_empty() {
                    extracted_thoughts.push(inner_thought.to_string());
                }
                remaining = &after_open[close_idx + "</thought>".len()..];
            } else {
                // Unclosed thought tag (stream cut off or model omitted close tag)
                let inner_thought = after_open.trim();
                if !inner_thought.is_empty() {
                    extracted_thoughts.push(inner_thought.to_string());
                }
                remaining = "";
                break;
            }
        }
        clean_content.push_str(remaining);

        if !extracted_thoughts.is_empty() {
            let joined_extracted = extracted_thoughts.join("\n\n");
            if final_thought.trim().is_empty() {
                final_thought = joined_extracted;
            } else {
                final_thought = format!("{}\n\n{}", final_thought.trim(), joined_extracted);
            }
        }
        final_content = clean_content.trim().to_string();
    }

    let duration_ms = turn_start_time.elapsed().as_millis() as u64;
    let tool_chars: usize = tool_calls_opt
        .as_ref()
        .map(|calls| {
            calls
                .iter()
                .map(|tc| tc.function.name.len() + tc.function.arguments.len())
                .sum()
        })
        .unwrap_or(0);
    let output_chars = final_content.len() + final_thought.len() + tool_chars;
    let estimated_output_tokens = (output_chars / 4).max(1);

    // Record turn usage to llm_usage table
    let provider_name = match crate::agent::executor::state::detect_provider_family(model) {
        crate::agent::executor::state::ModelProviderFamily::Claude => "claude",
        crate::agent::executor::state::ModelProviderFamily::Codex => "codex",
        crate::agent::executor::state::ModelProviderFamily::Antigravity => "antigravity",
        crate::agent::executor::state::ModelProviderFamily::Custom => "custom",
    };
    let _ = crate::llm::usage::record_usage_async(
        provider_name,
        model,
        estimated_input_tokens,
        0,
        estimated_output_tokens,
        duration_ms,
        "success",
    )
    .await;

    Ok(ModelTurnResult {
        content: final_content,
        thought: final_thought,
        tool_calls: tool_calls_opt,
        estimated_input_tokens,
        estimated_output_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::accumulate_tool_call_delta;
    use super::{MAX_RETRY_DELAY_MS, parse_retry_after_ms, retry_delay};
    use crate::llm::v1_engine::V1ToolCall;
    use std::collections::HashMap;

    #[test]
    fn test_extract_synthesized_thought_tags() {
        let input_content = "<thought>\nInspect file scenario/backend/src/main.rs due to Todo struct initializer error.\n</thought>\nI will inspect main.rs first.";
        let mut clean_content = String::new();
        let mut extracted_thoughts = Vec::new();
        let mut remaining = input_content;

        while let Some(open_idx) = remaining.find("<thought>") {
            clean_content.push_str(&remaining[..open_idx]);
            let after_open = &remaining[open_idx + "<thought>".len()..];
            if let Some(close_idx) = after_open.find("</thought>") {
                let inner = after_open[..close_idx].trim();
                if !inner.is_empty() {
                    extracted_thoughts.push(inner.to_string());
                }
                remaining = &after_open[close_idx + "</thought>".len()..];
            } else {
                let inner = after_open.trim();
                if !inner.is_empty() {
                    extracted_thoughts.push(inner.to_string());
                }
                remaining = "";
                break;
            }
        }
        clean_content.push_str(remaining);

        assert_eq!(extracted_thoughts.len(), 1);
        assert!(extracted_thoughts[0].contains("Inspect file scenario/backend/src/main.rs"));
        assert_eq!(clean_content.trim(), "I will inspect main.rs first.");
    }

    #[test]
    fn test_accumulate_ignores_blank_name_after_real_name() {
        // Exact wire pattern from cbai/deepseek-v4.1-flash via 9Router:
        // delta 1 carries the name, every later delta repeats `"name": ""`.
        // The blank must never overwrite the real name.
        let mut collected = empty_call();
        let mut seen = HashMap::new();

        accumulate_tool_call_delta(
            &mut collected,
            &mut seen,
            0,
            Some("call_00_BlwK".into()),
            Some("workspace_list_dir".into()),
            "",
        );
        accumulate_tool_call_delta(
            &mut collected,
            &mut seen,
            0,
            Some("call_00_BlwK".into()),
            Some(String::new()),
            "{\"path",
        );
        accumulate_tool_call_delta(
            &mut collected,
            &mut seen,
            0,
            Some("call_00_BlwK".into()),
            Some(String::new()),
            "\":\"src/agent\"}",
        );

        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].function.name, "workspace_list_dir");
        assert_eq!(collected[0].function.arguments, "{\"path\":\"src/agent\"}");
    }

    #[test]
    fn test_accumulate_whitespace_name_is_ignored() {
        let mut collected = empty_call();
        let mut seen = HashMap::new();

        accumulate_tool_call_delta(
            &mut collected,
            &mut seen,
            0,
            Some("c".into()),
            Some("workspace_grep".into()),
            "",
        );
        accumulate_tool_call_delta(&mut collected, &mut seen, 0, None, Some("   ".into()), "{}");

        assert_eq!(collected[0].function.name, "workspace_grep");
    }

    fn empty_call() -> Vec<V1ToolCall> {
        Vec::new()
    }

    #[test]
    fn test_accumulate_keeps_name_when_first_delta_has_no_name() {
        // Regression for `Unknown tool ''`: the name must survive when a later
        // delta carries it, and arguments must not be lost.
        let mut collected = empty_call();
        let mut seen = HashMap::new();

        accumulate_tool_call_delta(
            &mut collected,
            &mut seen,
            0,
            Some("call_1".into()),
            None,
            "",
        );
        accumulate_tool_call_delta(
            &mut collected,
            &mut seen,
            0,
            None,
            Some("workspace_list_dir".into()),
            "{\"path\":\"ragasukma/src\"}",
        );

        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].function.name, "workspace_list_dir");
        assert_eq!(
            collected[0].function.arguments,
            "{\"path\":\"ragasukma/src\"}"
        );
    }

    #[test]
    fn test_accumulate_does_not_duplicate_repeated_fragment() {
        // Regression for payloads like {"path":"a"}{"path":"a"}.
        let mut collected = empty_call();
        let mut seen = HashMap::new();
        let fragment = "{\"path\":\"a\"}";

        accumulate_tool_call_delta(
            &mut collected,
            &mut seen,
            0,
            Some("c".into()),
            Some("workspace_grep".into()),
            fragment,
        );
        accumulate_tool_call_delta(
            &mut collected,
            &mut seen,
            0,
            None,
            Some("workspace_grep".into()),
            fragment,
        );

        assert_eq!(collected[0].function.arguments, fragment);
    }

    #[test]
    fn test_accumulate_joins_partial_fragments() {
        let mut collected = empty_call();
        let mut seen = HashMap::new();

        accumulate_tool_call_delta(
            &mut collected,
            &mut seen,
            0,
            Some("c".into()),
            Some("workspace_read_file".into()),
            "{\"path\":",
        );
        accumulate_tool_call_delta(&mut collected, &mut seen, 0, None, None, "\"src/main.rs\"}");

        assert_eq!(collected[0].function.name, "workspace_read_file");
        assert_eq!(
            collected[0].function.arguments,
            "{\"path\":\"src/main.rs\"}"
        );
    }

    #[test]
    fn test_accumulate_resets_on_restarted_slot() {
        // A retry can reuse index 0 with a new id/name; stale arguments must not leak.
        let mut collected = empty_call();
        let mut seen = HashMap::new();

        accumulate_tool_call_delta(
            &mut collected,
            &mut seen,
            0,
            Some("call_A".into()),
            Some("workspace_list_dir".into()),
            "{\"path\":\"old\"}",
        );
        accumulate_tool_call_delta(
            &mut collected,
            &mut seen,
            0,
            Some("call_B".into()),
            Some("workspace_list_dir".into()),
            "{\"path\":\"new\"}",
        );

        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].id, "call_B");
        assert_eq!(collected[0].function.arguments, "{\"path\":\"new\"}");
    }

    #[test]
    fn test_accumulate_handles_parallel_indices() {
        let mut collected = empty_call();
        let mut seen = HashMap::new();

        accumulate_tool_call_delta(
            &mut collected,
            &mut seen,
            1,
            Some("b".into()),
            Some("workspace_grep".into()),
            "{}",
        );
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[1].function.name, "workspace_grep");
        assert!(collected[0].function.name.is_empty());
    }

    #[test]
    fn retry_delay_honours_retry_after_header() {
        let d = retry_delay(1, "429 Too Many Requests, Retry-After: 4");
        assert_eq!(d, std::time::Duration::from_millis(4_000));
    }

    #[test]
    fn retry_delay_is_capped_by_ceiling() {
        let d = retry_delay(1, "503 Retry-After: 99999");
        assert_eq!(d, std::time::Duration::from_millis(MAX_RETRY_DELAY_MS));
    }

    #[test]
    fn retry_delay_backs_off_and_never_exceeds_cap() {
        for attempt in 1..=6 {
            let d = retry_delay(attempt, "503 service unavailable");
            assert!(d.as_millis() > 0, "attempt {attempt} must wait");
            assert!(
                d.as_millis() <= MAX_RETRY_DELAY_MS as u128,
                "attempt {attempt} exceeded cap"
            );
        }
    }

    #[test]
    fn retry_delay_jitters_between_calls() {
        // Full jitter: repeated calls at the same attempt should not be byte-identical.
        let samples: Vec<u128> = (0..16)
            .map(|_| retry_delay(3, "502 bad gateway").as_millis())
            .collect();
        assert!(
            samples.windows(2).any(|w| w[0] != w[1]),
            "no jitter observed"
        );
    }

    #[test]
    fn parse_retry_after_ignores_absent_header() {
        assert_eq!(parse_retry_after_ms("connection closed"), None);
    }
}
