use anyhow::{Context, Result};

pub mod gates;
pub mod helpers;
pub mod messages;
pub mod prompt;
pub mod snapshot;
pub mod state;
pub mod stream;
pub mod tool_runner;

// Re-export submodules for full backwards compatibility
pub use super::compressor::*;
pub use super::diagnostics::*;
pub use super::events::*;
pub use helpers::{
    detect_project_verification_commands, is_verification_command, parse_project_folders,
};
pub use snapshot::WorkspaceSnapshot;
pub use state::TurnState;

/// Releases the per-turn file backups once the turn is over.
///
/// Every write/edit stores the previous file bytes so the session can be reverted. That
/// store is global and lives as long as the process, so without this it grows with every
/// file the agent ever touches. Called from every exit path of `execute_session_turn`,
/// after the turn can no longer need to restore anything.
fn release_turn_snapshots() {
    crate::agent::executor::snapshot::WorkspaceSnapshot::global().clear();
}

use crate::agent::models::AgentSessionEvent;
use crate::agent::repository;

pub async fn execute_session_turn(
    session_id: &str,
    model_override: Option<&str>,
) -> Result<String> {
    // 1. Load Session & Project
    let session = repository::get_session(session_id)
        .await?
        .context("Session not found")?;

    let project = repository::get_project(&session.project_id)
        .await?
        .context("Project not found")?;

    let allowed_folders = parse_project_folders(project.root_path.as_deref());
    for folder in &allowed_folders {
        let _ = crate::agent::tools::ensure_git_and_gitignore(std::path::Path::new(folder));
    }
    let prompt_skills =
        crate::agent::skills::scan_project_skills(project.root_path.as_deref(), &allowed_folders);
    let active_mcp_servers = repository::get_project_active_mcp_servers(&project.id)
        .await
        .unwrap_or_default();
    let mcp_tools = crate::agent::mcp::resolve_active_mcp_tools(&active_mcp_servers).await;
    let dynamic_tools_schema = crate::agent::tools::get_dynamic_agent_tools_schema(&mcp_tools);

    // 2. Mark session as running
    let _ = repository::update_session_status(session_id, "running").await;
    broadcast_session_event(
        session_id,
        AgentSessionEvent::StatusChange {
            status: "running".to_string(),
        },
    );

    // 3. Load message history and resolve active goal.
    //
    // Only the recent tail is needed: `prepare_turn_messages` prunes the context to a
    // bounded window anyway, so reading the entire session on every turn is wasted work
    // that grows quadratically with session length.
    let mut db_messages = repository::list_messages_paginated(
        session_id,
        Some(messages::TURN_HISTORY_TAIL_LIMIT),
        None,
        None,
    )
    .await?;
    let (goal_id, current_phase, locked_targets) =
        state::resolve_or_create_goal(&session, &project, &db_messages, &allowed_folders).await?;

    // 4. Resolve the model first: the system prompt adapts to what the provider
    //    can actually do (e.g. native hidden reasoning vs. inline <thought> tags).
    let (model, is_codex) = state::resolve_model(&session, model_override).await;
    let _family = state::detect_provider_family(&model);
    let capabilities = state::resolve_model_capabilities_async(&model).await;

    let system_prompt = prompt::build_system_prompt(prompt::AgentPromptContext {
        project: &project,
        allowed_folders: &allowed_folders,
        prompt_skills: &prompt_skills,
        active_mcp_servers: &active_mcp_servers,
        mcp_tools: &mcp_tools,
        session_id,
        goal_id: &goal_id,
        capabilities,
    })
    .await?;

    // 5. Initialize turn state
    let prior_verifs = repository::list_goal_verifications(&goal_id)
        .await
        .unwrap_or_default();
    let mut state = TurnState::new(
        goal_id.clone(),
        current_phase,
        locked_targets,
        prior_verifs.len(),
    );

    let detected_verifier =
        crate::agent::languages::detect_primary_verification_command(&allowed_folders);

    // 6. Autonomous Tool Execution Loop
    while state.iteration < state.max_safety_iterations {
        state.iteration += 1;

        // Check if session was stopped or aborted externally
        if let Ok(Some(status)) = repository::get_session_status(session_id).await
            && status != "running"
        {
            tracing::info!(
                "[Agent] Session {} status changed to '{}', breaking iteration loop.",
                session_id,
                status
            );
            break;
        }

        broadcast_session_event(
            session_id,
            AgentSessionEvent::StepInfo {
                iteration: state.iteration,
                action: "thinking".to_string(),
                details: Some(format!(
                    "Model {} is thinking & selecting actions...",
                    model
                )),
            },
        );

        // Prepare context messages for model
        let current_directive = state.ephemeral_directive.take();
        let v1_messages = messages::prepare_turn_messages(
            session_id,
            &system_prompt,
            &db_messages,
            &allowed_folders,
            current_directive,
            capabilities.history_token_budget,
        );

        // Request context accounting: calculate and log total token breakdown
        let sys_tokens = estimate_tokens(&system_prompt);
        let msg_tokens = v1_messages
            .iter()
            .map(|m| {
                m.content
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .map(estimate_tokens)
                    .unwrap_or(0)
            })
            .sum::<usize>();
        let tool_tokens = estimate_tokens(
            &serde_json::to_string(&dynamic_tools_schema).unwrap_or_default(),
        );
        let total_est_tokens = sys_tokens + msg_tokens + tool_tokens;

        tracing::info!(
            model = %model,
            iteration = state.iteration,
            sys_tokens,
            msg_tokens,
            tool_tokens,
            total_est_tokens,
            history_budget = capabilities.history_token_budget,
            native_reasoning = capabilities.native_reasoning,
            "Agent iteration request context accounting"
        );

        // 7. Stream from provider.
        // Retries for transient failures (429/5xx/timeouts/connection drops) live in exactly one
        // place: `stream::execute_model_stream`. Wrapping it in a second retry loop here used to
        // multiply attempts (3 x 3 = 9 requests per failure) and amplify rate limiting.
        let turn_res = stream::execute_model_stream(
            session_id,
            &model,
            is_codex,
            session.temperature,
            v1_messages.clone(),
            dynamic_tools_schema.clone(),
            state.force_final_text_only,
            state.iteration,
        )
        .await;

        let turn_res = match turn_res {
            Ok(res) => res,
            Err(e) => {
                let err_str = e.to_string();
                let err_msg_content = format!(
                    "⚠️ **Error communicating with AI model**: {}\n\nYou can switch the model using the dropdown above or send another prompt to retry.",
                    err_str
                );

                if let Ok(err_msg) = repository::create_message(
                    session_id,
                    "assistant",
                    Some(&err_msg_content),
                    None,
                    None,
                    None,
                    "error",
                    None,
                    None,
                )
                .await
                {
                    broadcast_session_event(session_id, AgentSessionEvent::MessageCreated(err_msg));
                }

                let _ = repository::update_session_status(session_id, "idle").await;
                broadcast_session_event(
                    session_id,
                    AgentSessionEvent::StatusChange {
                        status: "idle".to_string(),
                    },
                );
                broadcast_session_event(
                    session_id,
                    AgentSessionEvent::Error {
                        message: err_str.clone(),
                    },
                );

                release_turn_snapshots();
                return Ok(err_msg_content);
            }
        };

        let thought_opt = if turn_res.thought.trim().is_empty() {
            None
        } else {
            Some(turn_res.thought.as_str())
        };

        // If tool calls were returned, execute them
        if let Some(mut tool_calls) = turn_res.tool_calls {
            if !tool_calls.is_empty() {
                // Enforce strictly 1 tool call per turn to guarantee stability and prevent empty argument errors
                if tool_calls.len() > 1 {
                    tracing::warn!(
                        "[Agent Turn] Model returned {} parallel tool calls; enforcing single tool invocation",
                        tool_calls.len()
                    );
                    // Prefer the first tool call with non-empty arguments, or fallback to the first call
                    let chosen = tool_calls
                        .into_iter()
                        .find(|tc| !tc.function.arguments.trim().is_empty())
                        .unwrap_or_else(|| crate::llm::v1_engine::V1ToolCall {
                            id: format!("call_{}", crate::llm::v1_engine::generate_v1_uuid("tc")),
                            call_type: "function".to_string(),
                            function: crate::llm::v1_engine::V1ToolCallFunction {
                                name: String::new(),
                                arguments: String::new(),
                            },
                            thought_signature: None,
                            extra_content: None,
                            provider_specific_fields: None,
                        });
                    tool_calls = if chosen.function.name.trim().is_empty() {
                        Vec::new()
                    } else {
                        vec![chosen]
                    };
                }

                // Drop any call whose name is blank. Persisting such a call only poisons
                // the conversation history and makes the model repeat the same mistake.
                let blank_calls = tool_calls
                    .iter()
                    .filter(|tc| tc.function.name.trim().is_empty())
                    .count();
                if blank_calls > 0 {
                    tracing::warn!(
                        "[Agent Turn] Dropped {} tool call(s) with an empty name on iteration {}",
                        blank_calls,
                        state.iteration
                    );
                    tool_calls.retain(|tc| !tc.function.name.trim().is_empty());
                    state.consecutive_tool_errors += blank_calls;
                    state.ephemeral_directive = Some(
                        "Your last response contained a tool call with no tool name, so it was discarded. Call a tool by its exact name with valid JSON arguments, or answer the user directly.".to_string()
                    );
                }
            }

            if !tool_calls.is_empty() {
                let tc_json = serde_json::to_string(&tool_calls).unwrap_or_default();
                let assistant_text = if turn_res.content.is_empty() {
                    None
                } else {
                    Some(turn_res.content.as_str())
                };

                let assistant_msg = repository::create_message(
                    session_id,
                    "assistant",
                    assistant_text,
                    thought_opt,
                    Some(&tc_json),
                    None,
                    "success",
                    Some(turn_res.estimated_input_tokens as i64),
                    Some(turn_res.estimated_output_tokens as i64),
                )
                .await?;

                broadcast_session_event(
                    session_id,
                    AgentSessionEvent::MessageCreated(assistant_msg.clone()),
                );
                db_messages.push(assistant_msg);

                tool_runner::execute_turn_tool_calls(
                    session_id,
                    &project.id,
                    &allowed_folders,
                    &mcp_tools,
                    &tool_calls,
                    &mut state,
                    &mut db_messages,
                )
                .await?;

                // Circuit breaker: if the tool surface keeps failing in exactly the same
                // way, stop burning the iteration budget and force a final answer.
                if state.repeated_tool_error_count >= 5 {
                    tracing::warn!(
                        "[Agent Circuit Breaker] Repeated tool failure x{} on iteration {}: {:?}",
                        state.repeated_tool_error_count,
                        state.iteration,
                        state.last_tool_error_signature
                    );
                    state.trip_circuit_breaker(
                        "CIRCUIT BREAKER: the same tool failure repeated 5 times. Tools are now closed for this turn. Explain to the user what facts you already established, what blocked you, and what they should check.",
                    );
                } else if state.consecutive_tool_errors >= 8 {
                    tracing::warn!(
                        "[Agent Circuit Breaker] {} consecutive tool errors on iteration {}",
                        state.consecutive_tool_errors,
                        state.iteration
                    );
                    state.trip_circuit_breaker(
                        "CIRCUIT BREAKER: consecutive tool errors exceeded the safe limit. Tools are now closed for this turn. Summarize findings and answer the user now.",
                    );
                }

                // If the breaker tripped, do not loop back into tool calling; fall
                // through so the model must produce a final text answer.
                if state.circuit_breaker_triggered {
                    continue;
                }

                // Approaching safety limit notice
                // Warn once the turn has consumed more than half its budget.
                if state.iteration >= state.inspect_soft_limit {
                    state.ephemeral_directive = Some(format!(
                        "BUDGET: you have used {} of {} tool executions for this request. Wrap up: finish the remaining edits, run verification, and give the user a concrete summary of what was established.",
                        state.iteration, state.max_safety_iterations
                    ));
                }

                continue;
            }
        }

        // Evaluate completion gates
        match gates::evaluate_turn_gates(
            &mut state,
            &turn_res.content,
            &allowed_folders,
            detected_verifier.as_deref(),
        ) {
            gates::GateResult::Continue => continue,
            gates::GateResult::Pass => {}
        }

        // Final turn conclusion
        let composed_content =
            gates::compose_final_turn_content(&state, &turn_res.content, &turn_res.thought);
        let content_to_save = composed_content.as_str();

        // Final turn conclusion.
        //
        // Three endings, three outcomes. Collapsing them into one "completed" write is what used
        // to record a clarifying question as a goal's conclusion, which then leaked back into the
        // next turn's prompt as if it were finished work.
        let orch = crate::agent::orchestrator::GoalOrchestrator::new(&goal_id, &project.id);
        if state.last_command_had_error || state.last_tool_had_error {
            let _ = orch
                .conclude_with_status(
                    crate::agent::orchestrator::GoalState::Failed,
                    content_to_save,
                )
                .await;
        } else if state.has_taken_any_action || !state.modified_files.is_empty() {
            // Real work happened. `conclude_after_work` prefers a verified completion and only
            // labels the goal unverified when no passing verification exists.
            if let Err(e) = orch.conclude_after_work(content_to_save).await {
                tracing::warn!(
                    "[Agent Conclusion Warning] Goal {} could not be concluded: {}",
                    goal_id,
                    e
                );
            }
        } else {
            // The turn only produced conversation: an answer, a clarifying question, or nothing at
            // all. That ends the turn, not the goal. The goal stays open so a follow-up continues
            // it instead of silently starting over, and no conclusion text is invented for it.
            let _ = repository::update_session_status(session_id, "idle").await;
        }

        let final_msg = repository::create_message(
            session_id,
            "assistant",
            Some(content_to_save),
            thought_opt,
            None,
            None,
            "success",
            Some(turn_res.estimated_input_tokens as i64),
            Some(turn_res.estimated_output_tokens as i64),
        )
        .await?;

        broadcast_session_event(session_id, AgentSessionEvent::MessageCreated(final_msg));
        broadcast_session_event(
            session_id,
            AgentSessionEvent::Done {
                reply: content_to_save.to_string(),
            },
        );

        let _ = repository::update_session_status(session_id, "idle").await;
        broadcast_session_event(
            session_id,
            AgentSessionEvent::StatusChange {
                status: "idle".to_string(),
            },
        );

        release_turn_snapshots();
        return Ok(content_to_save.to_string());
    }

    // Fallback: If iteration limit reached while still running tools
    release_turn_snapshots();
    gates::handle_iteration_limit_fallback(session_id, &state).await
}

#[cfg(test)]
mod turn_boundary_tests {
    use super::release_turn_snapshots;
    use crate::agent::executor::snapshot::WorkspaceSnapshot;

    /// Guards the behaviour, not the helper: a turn boundary must leave the global
    /// snapshot store empty. Testing `WorkspaceSnapshot::clear()` directly proves the
    /// method works but says nothing about whether anything ever calls it, which is how
    /// this store ended up growing for the lifetime of the process unnoticed.
    #[test]
    fn a_finished_turn_leaves_no_backups_behind() {
        let dir = std::env::temp_dir().join(format!("turn-snap-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("touched.txt");
        std::fs::write(&file, b"before").unwrap();

        WorkspaceSnapshot::global().backup_file(&file).unwrap();
        assert!(
            !WorkspaceSnapshot::global().tracked_files().is_empty(),
            "precondition: the turn must have recorded a backup"
        );

        release_turn_snapshots();

        assert!(
            WorkspaceSnapshot::global().tracked_files().is_empty(),
            "the global store must be released when the turn ends"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
