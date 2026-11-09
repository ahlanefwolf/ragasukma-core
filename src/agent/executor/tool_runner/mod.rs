//! Executes the tool calls requested by one assistant turn.
//!
//! Split (Refactor #3, Stage C) from a single 1041-line file into concern
//! modules: audit trail, argument heuristics, hard gates, dispatch and
//! post-execution effects.

mod args;
mod audit;
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

use args::infer_tool_name_from_call;
use audit::{ToolAudit, audit_tool_call};

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
            let path = if path.is_empty() {
                "target".to_string()
            } else {
                path
            };
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
        effects::apply_post_execution(effects::PostExecContext {
            state,
            tool_name: &tool_name,
            tool_output: &tool_output,
            args_val: &args_val,
            target_arg_file: target_arg_file.as_deref(),
            is_edit_op,
            is_inspection,
            is_accessing_unmodified_target,
            is_breaker_trip,
            session_id,
            project_id,
            allowed_folders,
        })
        .await;

        effects::apply_post_inspection(effects::PostExecContext {
            state,
            tool_name: &tool_name,
            tool_output: &tool_output,
            args_val: &args_val,
            target_arg_file: target_arg_file.as_deref(),
            is_edit_op,
            is_inspection,
            is_accessing_unmodified_target,
            is_breaker_trip,
            session_id,
            project_id,
            allowed_folders,
        })
        .await;

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
