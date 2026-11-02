use serde_json::json;
use std::collections::{HashMap, HashSet};

use crate::agent::compressor::{
    compact_assistant_tool_calls_smart, compact_tool_output_smart, estimate_message_tokens,
    summarize_evicted_messages,
};
use crate::agent::models::Message;
use crate::agent::tools::expand_file_tags;
use crate::llm::v1_engine::{V1Message, V1ToolCall};

/// How many of the newest messages are loaded from the database for one turn.
///
/// The sliding window in `prepare_turn_messages` keeps at most `MAX_HISTORY_MESSAGES`
/// messages, so loading a small multiple of that gives the pruner room to work without
/// reading the whole session every turn.
pub const TURN_HISTORY_TAIL_LIMIT: i64 = 64;

/// Hard cap on how many history messages may be sent to the model in one turn.
pub const MAX_HISTORY_MESSAGES: usize = 24;

pub fn prepare_turn_messages(
    session_id: &str,
    system_prompt: &str,
    db_messages: &[Message],
    allowed_folders: &[String],
    ephemeral_directive: Option<String>,
    history_token_budget: usize,
) -> Vec<V1Message> {
    let mut v1_messages = Vec::new();

    // Push System prompt first
    v1_messages.push(V1Message {
        role: "system".to_string(),
        content: Some(json!(system_prompt)),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        thought_signature: None,
    });

    // Dynamic Token Budget & Sliding Window Pruning
    let total_history_tokens: usize = db_messages.iter().map(estimate_message_tokens).sum();

    let mut evicted_older_messages: Vec<&Message> = Vec::new();
    let messages_to_send: Vec<&Message> = if total_history_tokens > history_token_budget || db_messages.len() > MAX_HISTORY_MESSAGES {
        let mut pruned = Vec::new();
        if let Some(first_user_msg) = db_messages.iter().find(|m| m.role == "user") {
            pruned.push(first_user_msg);
        }

        // Find tail start index by working backwards from the newest messages until token budget is met
        let mut accumulated_tail_tokens = 0;
        let mut tail_start_idx = db_messages.len().saturating_sub(1);
        while tail_start_idx > 0 {
            let msg_tok = estimate_message_tokens(&db_messages[tail_start_idx]);
            if accumulated_tail_tokens + msg_tok > history_token_budget && (db_messages.len() - tail_start_idx) >= 10 {
                break;
            }
            accumulated_tail_tokens += msg_tok;
            tail_start_idx = tail_start_idx.saturating_sub(1);
        }

        // Never start tail right on a "tool" message (which would orphan it without its parent assistant)
        while tail_start_idx > 0 && db_messages.get(tail_start_idx).map(|m| m.role.as_str()) == Some("tool") {
            tail_start_idx = tail_start_idx.saturating_sub(1);
        }

        // Collect all evicted messages between first_user_msg and tail_start_idx for rolling compaction
        let first_user_id = db_messages.iter().find(|m| m.role == "user").map(|m| &m.id);
        for m in &db_messages[..tail_start_idx] {
            if Some(&m.id) != first_user_id {
                evicted_older_messages.push(m);
            }
        }

        for m in &db_messages[tail_start_idx..] {
            if !pruned.iter().any(|existing| existing.id == m.id) {
                pruned.push(m);
            }
        }
        if let Some(last_user_msg) = db_messages.iter().rev().find(|m| m.role == "user") {
            if !pruned.iter().any(|existing| existing.id == last_user_msg.id) {
                pruned.push(last_user_msg);
            }
        }
        // Strict sequence sort so user prompt is NEVER before older assistant/tool messages
        pruned.sort_by_key(|m| m.sequence_num);
        pruned
    } else {
        db_messages.iter().collect()
    };

    // If older messages were evicted by the sliding window, inject a rolling compact summary
    if !evicted_older_messages.is_empty() {
        let eviction_summary = summarize_evicted_messages(&evicted_older_messages);
        if !eviction_summary.is_empty() {
            v1_messages.push(V1Message {
                role: "user".to_string(),
                content: Some(json!(eviction_summary)),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                thought_signature: None,
            });
        }
    }

    // Bidirectional Tool Call Validation & Mapping:
    // 1. Collect all valid tool_call IDs and their associated function names emitted by assistant messages,
    // as well as files modified by subsequent edits for intermediate read compaction
    let mut known_tool_call_ids = HashSet::new();
    let mut tool_id_to_name: HashMap<String, String> = HashMap::new();
    let mut files_modified_in_window: HashSet<String> = HashSet::new();
    for msg in &messages_to_send {
        if msg.role == "assistant" {
            if let Some(ref tc_raw) = msg.tool_calls {
                if let Ok(calls) = serde_json::from_str::<Vec<V1ToolCall>>(tc_raw) {
                    for c in calls {
                        known_tool_call_ids.insert(c.id.clone());
                        if c.function.name == "workspace_edit_file" || c.function.name == "workspace_write_file" {
                            if let Ok(args_obj) = serde_json::from_str::<serde_json::Value>(&c.function.arguments) {
                                if let Some(path) = args_obj.get("file_path").or_else(|| args_obj.get("path")).and_then(|p| p.as_str()) {
                                    let clean = path.trim_start_matches("./").to_string();
                                    files_modified_in_window.insert(clean);
                                }
                            }
                        }
                        tool_id_to_name.insert(c.id, c.function.name);
                    }
                }
            }
        }
    }

    // 2. Identify all tool messages that actually have valid parent assistant calls in this window
    let mut retained_tool_ids = HashSet::new();
    for msg in &messages_to_send {
        if msg.role == "tool" {
            if let Some(ref tcid) = msg.tool_call_id {
                if known_tool_call_ids.contains(tcid) {
                    retained_tool_ids.insert(tcid.clone());
                }
            }
        }
    }

    let total_tool_msgs = messages_to_send.iter().filter(|m| m.role == "tool").count();
    let total_assistant_msgs = messages_to_send.iter().filter(|m| m.role == "assistant").count();
    let mut tool_counter = 0;
    let mut assistant_counter = 0;

    for msg in messages_to_send {
        // Drop orphaned tool messages whose parent assistant tool_call was pruned out
        if msg.role == "tool" {
            if let Some(ref tcid) = msg.tool_call_id {
                if !retained_tool_ids.contains(tcid) {
                    continue;
                }
            }
        }

        let mut tc_parsed: Option<Vec<V1ToolCall>> = msg
            .tool_calls
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok());

        // Bidirectional safety: If assistant has tool_calls whose tool responses were pruned out,
        // prune the unfulfilled tool_calls so API providers (OpenAI, Gemini, Codex) do not reject with HTTP 400.
        // Also compact older historical file writes/edits to prevent prompt bloat.
        if msg.role == "assistant" {
            assistant_counter += 1;
            let is_recent_tail = assistant_counter > total_assistant_msgs.saturating_sub(4);
            if let Some(ref mut calls) = tc_parsed {
                calls.retain(|c| retained_tool_ids.contains(&c.id));
                if calls.is_empty() {
                    tc_parsed = None;
                } else {
                    compact_assistant_tool_calls_smart(calls, is_recent_tail);
                }
            }
        }

        // If a tool message is directly followed by a user message in history
        // (e.g. previous turn was interrupted or aborted before assistant replied),
        // insert an intermediate assistant acknowledgement so that API providers (Gemini, Claude, OpenAI)
        // do not see an unacknowledged tool response directly followed by a new user turn.
        if msg.role == "user" {
            if let Some(last_v1) = v1_messages.last() {
                if last_v1.role == "tool" {
                    v1_messages.push(V1Message {
                        role: "assistant".to_string(),
                        content: Some(json!("I have executed the requested tool operations and inspected the workspace.")),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                        thought_signature: None,
                    });
                }
            }
        }

        let content_val = if msg.role == "user" {
            msg.content.as_ref().map(|c| {
                let expanded = expand_file_tags(c, allowed_folders);
                json!(expanded)
            })
        } else if msg.role == "tool" {
            tool_counter += 1;
            let is_recent_tail = tool_counter > total_tool_msgs.saturating_sub(6);
            let tool_name = msg.tool_call_id.as_ref().and_then(|id| tool_id_to_name.get(id)).map(|s| s.as_str());
            msg.content.as_ref().map(|c| {
                // If this is an older file read on a file that has subsequently been edited in this session,
                // stub it out aggressively to save thousands of tokens
                if tool_name == Some("workspace_read_file") && !is_recent_tail && c.len() > 600 {
                    let first_line = c.lines().next().unwrap_or("").trim();
                    let compacted = format!(
                        "{}\n[... {} characters of prior file read omitted because this file was subsequently updated ...]",
                        first_line,
                        c.len()
                    );
                    json!(compacted)
                } else {
                    let compacted = compact_tool_output_smart(c, tool_name, is_recent_tail);
                    json!(compacted)
                }
            })
        } else if msg.role == "assistant" && tc_parsed.is_none() {
            // Ensure assistant message without tool_calls always has non-empty text content
            match msg.content.as_ref() {
                Some(c) if !c.trim().is_empty() => Some(json!(c)),
                _ => Some(json!("(Completed tool operations)")),
            }
        } else {
            msg.content.as_ref().map(|c| json!(c))
        };

        v1_messages.push(V1Message {
            role: msg.role.clone(),
            content: content_val,
            tool_calls: tc_parsed,
            tool_call_id: msg.tool_call_id.clone(),
            name: None,
            thought_signature: None,
        });
    }

    // Ephemeral in-memory prodding directive without polluting DB messages
    let mut final_directive = ephemeral_directive;

    // Check if there is an active compiler/diagnostic queue in RAM for this session
    if let Some(diag_queue) = crate::agent::diagnostics::get_session_diagnostic_queue(session_id) {
        let unresolved: Vec<_> = diag_queue.items.iter().filter(|i| !i.resolved).collect();
        if !unresolved.is_empty() {
            let mut diag_msg = format!(
                "⚠️ ACTIVE DIAGNOSTIC REPAIR QUEUE ({} broken file(s) remaining out of {} total):\n",
                unresolved.len(),
                diag_queue.items.len()
            );
            diag_msg.push_str("CRITICAL: Each file below contains multiple error lines. You MUST fix ALL error lines in each file in a SINGLE PASS (read once, edit once per file) before running re-compile:\n\n");
            for (idx, item) in unresolved.iter().take(8).enumerate() {
                let lines_display = if item.lines.is_empty() {
                    "unspecified lines".to_string()
                } else {
                    format!("lines [{}]", item.lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", "))
                };
                diag_msg.push_str(&format!("{}. `{}` ({})\n", idx + 1, item.file_path, lines_display));
                for msg in item.error_messages.iter().take(4) {
                    diag_msg.push_str(&format!("   - {}\n", msg));
                }
            }
            diag_msg.push_str("\nINSTRUCTION: Formulate all line fixes for each target file and apply them using python3 via `workspace_run_command` (or `workspace_write_file`). Only re-run verification once all files are fixed.");

            final_directive = Some(match final_directive {
                Some(prev) => format!("{}\n\n{}", prev, diag_msg),
                None => diag_msg,
            });
        } else if let Some(cmd) = diag_queue.pending_compile_cmd {
            let verify_msg = format!(
                "✅ ALL DIAGNOSTIC ITEMS IN QUEUE ARE MARKED REPAIRED!\nNow execute verification command: `{}` via `workspace_run_command` to verify the build.",
                cmd
            );
            crate::agent::diagnostics::clear_session_diagnostic_queue(session_id);
            final_directive = Some(match final_directive {
                Some(prev) => format!("{}\n\n{}", prev, verify_msg),
                None => verify_msg,
            });
        }
    }

    if let Some(directive) = final_directive {
        let is_last_tool = v1_messages.last().map(|m| m.role == "tool").unwrap_or(false);
        if is_last_tool {
            if let Some(last_msg) = v1_messages.last_mut() {
                let existing_text = last_msg
                    .content
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let combined = if existing_text.is_empty() {
                    format!("[System Directive]: {}", directive)
                } else {
                    format!("{}\n\n[System Directive]: {}", existing_text, directive)
                };
                last_msg.content = Some(serde_json::Value::String(combined));
            }
        } else {
            v1_messages.push(V1Message {
                role: "user".to_string(),
                content: Some(serde_json::Value::String(directive)),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                thought_signature: None,
            });
        }
    }

    v1_messages
}
