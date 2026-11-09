//! Executes the tool calls requested by one assistant turn.
//!
//! Split (Refactor #3, Stage C) from a single 1041-line file into concern
//! modules: audit trail, argument heuristics, hard gates, dispatch and
//! post-execution effects.

mod audit;
mod args;
mod effects;
mod guards;

use anyhow::Result;
use serde_json::json;

use super::state::TurnState;
use crate::agent::compressor::{sanitize_tool_output_for_db, sanitize_tool_output_for_memory};
use crate::agent::events::broadcast_session_event;
use crate::agent::models::{AgentSessionEvent, Message};
use crate::agent::tools::execute_tool_call;
use crate::llm::v1_engine::V1ToolCall;
use crate::repository::agent as repository;

use audit::{ToolAudit, audit_tool_call};
use args::infer_tool_name_from_call;

pub async fn execute_turn_tool_calls(
    session_id: &str,
    project_id: &str,
    allowed_folders: &[String],
    mcp_tools: &[crate::agent::mcp::ActiveMcpTool],
    tool_calls: &[V1ToolCall],
    state: &mut TurnState,
    db_messages: &mut Vec<Message>,
) -> Result<()> {
    for tc in tool_calls {
        let args_val = serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
            .unwrap_or_else(|_| json!({}));

        // Guard: a tool call with a blank/unknown name can never execute. Instead of
        // feeding an opaque `Unknown tool ''` back to the model (which historically
        // caused long retry loops until the safety iteration cap), try to repair the
        // call from the arguments we did receive, and otherwise emit a directive that
        // tells the model exactly how to recover.
        let raw_name = tc.function.name.trim();
        let resolved_name = if raw_name.is_empty() {
            infer_tool_name_from_call(&tc.id, &args_val)
        } else {
            Some(raw_name.to_string())
        };

        let tool_name = match resolved_name {
            Some(name) => {
                if name != raw_name {
                    tracing::warn!(
                        "[Agent Tool Guard] Recovered empty tool name as '{}' for call {}",
                        name,
                        tc.id
                    );
                }
                name
            }
            None => {
                let msg = "Tool execution error: the tool call arrived with an empty name, so nothing was executed. Re-issue the call with an explicit tool name (`workspace_list_dir`, `workspace_read_file`, `workspace_grep`, `workspace_edit_file`, `workspace_write_file`, or `workspace_run_command`) and valid JSON arguments.";
                state.consecutive_tool_errors += 1;
                state.last_tool_had_error = true;
                if state.consecutive_tool_errors >= 2 {
                    state.ephemeral_directive = Some(
                        "CRITICAL: your previous tool calls were malformed (empty tool name). Do not repeat them. Call one of the available tools by its exact name with valid JSON arguments, or answer the user directly.".to_string()
                    );
                }

                audit_tool_call(
                    session_id,
                    &state.goal_id,
                    ToolAudit {
                        tool_name: "<empty>",
                        target_path: None,
                        command: None,
                        status: "failed",
                        reason: Some("Tool call rejected: empty tool name"),
                        duration_ms: Some(0),
                    },
                )
                .await;

                let tool_msg = repository::create_message(
                    session_id,
                    "tool",
                    Some(msg),
                    None,
                    Some(&serde_json::json!([]).to_string()),
                    Some(&tc.id),
                    "error",
                    None,
                    None,
                )
                .await?;
                db_messages.push(tool_msg);
                continue;
            }
        };

        broadcast_session_event(
            session_id,
            AgentSessionEvent::StepInfo {
                iteration: state.iteration,
                action: format!("tool:{}", tool_name),
                details: Some(format!("Executing `{}`...", tool_name)),
            },
        );

        let is_edit_op = tool_name == "workspace_write_file" || tool_name == "workspace_edit_file";

        let is_inspection = guards::is_inspection(&tool_name, &args_val);

        let target_arg_file = args_val
            .get("file_path")
            .or_else(|| args_val.get("path"))
            .and_then(|p| p.as_str())
            .map(|s| s.to_string());

        if is_edit_op && let Some(ref path) = target_arg_file {
            let clean = path.trim_start_matches("./").to_string();
            crate::agent::diagnostics::mark_diagnostic_file_resolved(session_id, &clean);
        }

        let is_target_in_error_list = target_arg_file
            .as_ref()
            .map(|p| {
                let clean = p.trim_start_matches("./");
                state
                    .pending_error_files
                    .iter()
                    .any(|ef| ef == clean || clean.ends_with(ef) || ef.ends_with(clean))
            })
            .unwrap_or(false);

        let is_accessing_unmodified_target = target_arg_file
            .as_ref()
            .map(|p| {
                let clean = p.trim_start_matches("./");
                !state.modified_files.iter().any(|mf| {
                    let mf_clean = mf.trim_start_matches("./");
                    mf_clean == clean || mf_clean.ends_with(clean) || clean.ends_with(mf_clean)
                })
            })
            .unwrap_or(false);

        let tool_start_instant = std::time::Instant::now();
        let cmd_param = args_val.get("command").and_then(|c| c.as_str());

        // HARD GATES & CIRCUIT BREAKERS
        let mut is_breaker_trip = false;
        let tool_output = if let Some(denial) =
            guards::evaluate(&tool_name, &args_val, state, is_target_in_error_list)
        {
            is_breaker_trip = true;
            audit_tool_call(
                session_id,
                &state.goal_id,
                ToolAudit {
                    tool_name: &tool_name,
                    target_path: target_arg_file.as_deref(),
                    command: cmd_param,
                    status: "denied",
                    reason: Some(denial.reason()),
                    duration_ms: Some(0),
                },
            )
            .await;
            denial.message()
        } else if let Some(path) = guards::is_repeated_read(&tool_name, &args_val, state) {
            let path = if path.is_empty() { "target".to_string() } else { path };
            format!(
                "Notice: You have already read this exact slice of `{path}` multiple times. Its content is available in your conversation history. Please proceed with modifying the file using `workspace_edit_file` or answering the user's question directly."
            )
        } else if tool_name.starts_with("mcp__")
            || mcp_tools
                .iter()
                .any(|t| t.original_name == tool_name || t.namespaced_name == tool_name)
        {
            // Dispatch to active MCP server
            match crate::agent::mcp::call_active_mcp_tool(mcp_tools, &tool_name, args_val.clone())
                .await
            {
                Ok(out) => {
                    let dur = tool_start_instant.elapsed().as_millis() as i64;
                    audit_tool_call(
                        session_id,
                        &state.goal_id,
                        ToolAudit {
                            tool_name: &tool_name,
                            target_path: None,
                            command: None,
                            status: "executed",
                            reason: None,
                            duration_ms: Some(dur),
                        },
                    )
                    .await;
                    out
                }
                Err(e) => {
                    let dur = tool_start_instant.elapsed().as_millis() as i64;
                    let err_str = e.to_string();
                    audit_tool_call(
                        session_id,
                        &state.goal_id,
                        ToolAudit {
                            tool_name: &tool_name,
                            target_path: None,
                            command: None,
                            status: "failed",
                            reason: Some(&err_str),
                            duration_ms: Some(dur),
                        },
                    )
                    .await;
                    format!("MCP execution error: {}", err_str)
                }
            }
        } else {
            match execute_tool_call(&tool_name, &args_val, allowed_folders, session_id).await {
                Ok(out) => {
                    let dur = tool_start_instant.elapsed().as_millis() as i64;
                    audit_tool_call(
                        session_id,
                        &state.goal_id,
                        ToolAudit {
                            tool_name: &tool_name,
                            target_path: target_arg_file.as_deref(),
                            command: cmd_param,
                            status: "executed",
                            reason: None,
                            duration_ms: Some(dur),
                        },
                    )
                    .await;

                    if tool_name == "workspace_read_file"
                        && let Some(ref p) = target_arg_file
                    {
                        let clean = p.trim_start_matches("./").to_string();
                        let start_line = args_val.get("start_line").and_then(|v| v.as_i64());
                        let end_line = args_val.get("end_line").and_then(|v| v.as_i64());
                        let is_outline = out.starts_with("# File outline for");
                        if start_line.is_none() && end_line.is_none() && !is_outline {
                            state.files_read_in_full.insert(clean);
                        }
                        state
                            .recent_read_slices
                            .insert((p.clone(), start_line, end_line));
                    }
                    out
                }
                Err(e) => {
                    let dur = tool_start_instant.elapsed().as_millis() as i64;
                    let err_str = e.to_string();
                    audit_tool_call(
                        session_id,
                        &state.goal_id,
                        ToolAudit {
                            tool_name: &tool_name,
                            target_path: target_arg_file.as_deref(),
                            command: cmd_param,
                            status: "failed",
                            reason: Some(&err_str),
                            duration_ms: Some(dur),
                        },
                    )
                    .await;
                    format!("Tool execution error: {}", err_str)
                }
            }
        };

        // Track consecutive/repeated tool failures so a broken tool surface cannot
        // silently burn the whole iteration budget. A different error message or a
        // successful call resets the streak.
        if tool_output.starts_with("Tool execution error")
            || tool_output.starts_with("MCP execution error")
        {
            state.consecutive_tool_errors += 1;
            let signature = tool_output
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            if state.last_tool_error_signature.as_deref() == Some(signature.as_str()) {
                state.repeated_tool_error_count += 1;
            } else {
                state.last_tool_error_signature = Some(signature);
                state.repeated_tool_error_count = 1;
            }

            if state.repeated_tool_error_count >= 2 {
                state.ephemeral_directive = Some(format!(
                    "REPEATED TOOL FAILURE: `{}` failed {} times in a row with the same error. Do not retry it unchanged. Change your arguments, pick a different tool, or answer the user directly.",
                    tool_name, state.repeated_tool_error_count
                ));
            }
        } else {
            state.consecutive_tool_errors = 0;
            state.last_tool_error_signature = None;
            state.repeated_tool_error_count = 0;
            state.has_taken_any_action = true;
        }

        if is_breaker_trip {
            state.trip_circuit_breaker(
                "CIRCUIT BREAKER TRIGGERED: Tool calling is permanently closed for this turn. Do NOT attempt to invoke any more tools. Provide your final response, summary, or answer to the user now."
            );
        }

        if is_inspection {
            state.global_inspection_streak += 1;
            if state.current_phase == "discovery"
                && let Some(ref path) = target_arg_file
            {
                let is_dir_inspection =
                    tool_name == "workspace_list_dir" || path.ends_with('/') || !path.contains('.');
                if !is_dir_inspection && !state.locked_targets.contains(path) {
                    state.locked_targets.push(path.clone());
                }
            }
        }

        // Auto-invalidation and tracking on file modification
        effects::apply_post_execution(
            state,
            &tool_name,
            &tool_output,
            &args_val,
            target_arg_file.as_deref(),
            is_edit_op,
            session_id,
            project_id,
            allowed_folders,
            is_breaker_trip,
        )
        .await;

        // If edit failed due to text mismatch or duplicate match, guide model directly back to target
        if tool_name == "workspace_edit_file" {
            if tool_output.contains("'old_text' not found") {
                state.ephemeral_directive = Some(
                    "Edit recovery: `workspace_edit_file` failed because 'old_text' did not match the file on disk. Inspect the exact slice using `workspace_read_file` if needed, and re-call `workspace_edit_file` with matching lines and 2-3 lines of surrounding context.".to_string()
                );
            } else if tool_output.contains("matches ") || tool_output.contains("matched ") {
                state.ephemeral_directive = Some(
                    "Edit recovery: 'old_text' matched multiple locations in the file. Include the enclosing block, function declaration, or 2-3 additional surrounding lines in 'old_text' to make it unique, and re-call `workspace_edit_file`.".to_string()
                );
            }
        }

        // Check inspection streaks (advise model to apply changes or conclude)
        // Escalating warning tied to the real turn budget instead of an arbitrary
        // constant, so the advice the model receives matches how much room it has.
        if state.global_inspection_streak >= state.inspection_streak_limit {
            state.ephemeral_directive = Some(format!(
                "Notice: you have run {} inspections without modifying code, and your turn budget is limited. Stop exploring: either apply the change with `workspace_edit_file` / `workspace_write_file`, or answer the user with what you have established so far.",
                state.global_inspection_streak
            ));
        }

        if state.has_passed_verification && is_inspection && !is_accessing_unmodified_target {
            state.ephemeral_directive = Some(
                "STOP: Verification has already passed for modified files! Do not perform redundant inspections. Conclude your response or proceed directly to edit remaining target files.".to_string()
            );
        }

        let is_file_inspection =
            tool_name == "workspace_read_file" || tool_name == "workspace_grep";
        if is_file_inspection && let Some(path) = target_arg_file.as_deref() {
            let start_line = args_val.get("start_line").and_then(|v| v.as_i64());
            let end_line = args_val.get("end_line").and_then(|v| v.as_i64());
            let is_exact_dup =
                state
                    .recent_read_slices
                    .contains(&(path.to_string(), start_line, end_line));
            if is_exact_dup || (start_line.is_none() && end_line.is_none()) {
                let count = state
                    .consecutive_read_same_file
                    .entry(path.to_string())
                    .or_insert(0);
                *count += 1;
                if *count >= 5 {
                    state.ephemeral_directive = Some(format!(
                        "Notice: You have inspected `{path}` {count} times this turn. If your task requires code modifications, call `workspace_edit_file` now. If answering a question, respond directly to the user."
                    ));
                }
            }
        }

        // Auto-cache discovered file contents into Evidence Cache with true SHA-256 version
        if tool_name == "workspace_read_file" && !tool_output.starts_with("Tool execution error") {
            if let Some(file_path) = target_arg_file.as_deref() {
                let start_line = args_val.get("start_line").and_then(|v| v.as_i64());
                let end_line = args_val.get("end_line").and_then(|v| v.as_i64());
                let preview_summary = format!(
                    "Read {} (lines {:?}-{:?})",
                    file_path,
                    start_line.unwrap_or(1),
                    end_line.unwrap_or(0)
                );
                let file_ver =
                    crate::agent::tools::fs::resolve_allowed_path(file_path, allowed_folders)
                        .ok()
                        .and_then(|p| crate::agent::context_builder::compute_file_version(&p).ok())
                        .unwrap_or_else(|| "live_mtime".to_string());
                let snippet: String = tool_output.chars().take(1500).collect();
                // A read is context, not proof of work, so it ranks below any edited file.
                // Freshly observed, so age is zero; recency decay is applied at read time.
                let score = crate::agent::evidence_ranking::rank_evidence(
                    crate::agent::evidence_ranking::EvidenceOrigin::Read,
                    0,
                );
                if let Err(e) = repository::save_evidence_and_link(
                    project_id,
                    file_path,
                    &file_ver,
                    start_line,
                    end_line,
                    None,
                    &preview_summary,
                    Some(&snippet),
                    Some((&state.goal_id, score, true)),
                )
                .await
                {
                    crate::term_err!("[Evidence] Failed to cache read of {}: {}", file_path, e);
                }
            }
        } else if is_edit_op
            && !tool_output.starts_with("Tool execution error")
            && let Some(file_path) = target_arg_file.as_deref()
        {
            let preview_summary = format!("Modified/Created file {}", file_path);
            let file_ver =
                crate::agent::tools::fs::resolve_allowed_path(file_path, allowed_folders)
                    .ok()
                    .and_then(|p| crate::agent::context_builder::compute_file_version(&p).ok())
                    .unwrap_or_else(|| "live_mtime".to_string());
            // An edited/created file is part of the solution, so it ranks at the top.
            let score = crate::agent::evidence_ranking::rank_evidence(
                crate::agent::evidence_ranking::EvidenceOrigin::Modified,
                0,
            );
            if let Err(e) = repository::save_evidence_and_link(
                project_id,
                file_path,
                &file_ver,
                None,
                None,
                None,
                &preview_summary,
                None,
                Some((&state.goal_id, score, true)),
            )
            .await
            {
                crate::term_err!("[Evidence] Failed to cache edit of {}: {}", file_path, e);
            }
        }

        // Sanitize large command/terminal output before DB persistence (avoid truncating source code reads)
        let is_cmd_run = tool_name == "workspace_run_command";
        let tool_output_for_db = if is_cmd_run {
            sanitize_tool_output_for_db(&tool_output)
        } else {
            tool_output.clone()
        };

        let tool_msg = repository::create_message(
            session_id,
            "tool",
            Some(&tool_output_for_db),
            None,
            None,
            Some(&tc.id),
            "success",
            None,
            None,
        )
        .await?;

        let mut in_memory_tool_msg = tool_msg;
        in_memory_tool_msg.content = Some(if is_cmd_run {
            sanitize_tool_output_for_memory(&tool_output)
        } else {
            tool_output.clone()
        });
        db_messages.push(in_memory_tool_msg);
    }

    Ok(())
}


#[cfg(test)]
mod tests;
