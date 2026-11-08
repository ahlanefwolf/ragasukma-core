use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

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

/// Resets every session still marked `running` back to `idle`.
///
/// `TurnStatusGuard` covers every way a turn can end *inside* the process, but it cannot
/// cover the process itself going away: a crash, a force-quit, or a kill during a turn
/// leaves the DB row at `running` with no owner left to clear it. `bridge.rs` rebuilds the
/// TUI's `is_running` from that column on every `SessionsLoaded`, so without this sweep the
/// session displays a spinner forever, including across restarts.
///
/// Runs once at startup from [`crate::agent::maintenance::retention_task`]. Scoped to
/// `running` only: a `paused` or `failed` session is a deliberate state that must survive a
/// restart. Returns how many rows were corrected.
pub async fn reset_stale_running_sessions() -> Result<usize> {
    repository::reset_stale_running_sessions().await
}

#[doc(hidden)]
/// Drops the file backups belonging to one session.
///
/// This is deliberately *not* called at a turn boundary any more. Backups are keyed per
/// session, and the tool contract says `workspace_rollback` reverts "every file this session
/// touched" �?— so a later turn must still be able to undo what an earlier turn wrote. Releasing
/// at the turn boundary is what made the tool lose its data one step before it was used.
/// It runs when the session itself is deleted instead (`delete_session` calls
/// `snapshot::release_session` directly; this wrapper exists for the boundary tests).
pub fn release_session_snapshots(session_id: &str) -> bool {
    crate::agent::snapshot::release_session(session_id)
}

use crate::agent::models::AgentSessionEvent;
use crate::agent::repository;

// ---------------------------------------------------------------------------
// Turn generations (N-2)
// ---------------------------------------------------------------------------

// `TurnStatusGuard::drop` cannot await, so on the abort path (the TUI aborts the turn task on
// cancel) it *schedules* the status reset as a detached task. That detached task races the
// next turn: if the user cancels and immediately sends a new prompt, the new turn sets
// `running`, and only then does the old turn's cleanup run and set `idle` again �� emiting
// `StatusChange { idle }` while the session is very much working.
//
// A per-session generation counter removes the race. Every turn claims the next generation at
// `TurnStatusGuard::new`; a cleanup may only write the status if it still owns the latest
// generation. The check and the "invalidate" are the same atomic step, so a stale cleanup
// either runs before the new turn claims its generation (harmless: it is overwritten) or sees
// itself superseded and does nothing.
static TURN_GENERATIONS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

fn turn_generations() -> &'static Mutex<HashMap<String, u64>> {
    TURN_GENERATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Claims the next generation for `session_id`, invalidating every earlier turn's cleanup.
fn claim_turn_generation(session_id: &str) -> u64 {
    let mut map = turn_generations()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let next = map.get(session_id).copied().unwrap_or(0) + 1;
    map.insert(session_id.to_string(), next);
    next
}

/// True while `generation` is still the newest turn claimed for `session_id`.
///
/// A stale cleanup must never prune the entry: whenever `latest > generation` there is a
/// newer turn still in flight that *owns* that entry, and removing it would make that turn
/// fail to recognise itself as latest later on - its cleanup (idle status + notification)
/// would then be skipped and the session would be stuck on "running" again (BUG-03).
///
/// The entry instead lives as long as the session can still run turns, and is reclaimed by
/// [`forget_session_generation`] when the session is deleted.
fn is_latest_turn(session_id: &str, generation: u64) -> bool {
    let map = turn_generations()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.get(session_id).copied() == Some(generation)
}

/// Drops the generation counter for a session that no longer exists.
///
/// This is the only safe place to reclaim the entry: at this point no turn can be in flight,
/// so nothing can mistake the reset for a superseded generation.
pub(crate) fn forget_session_generation(session_id: &str) {
    let mut map = turn_generations()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.remove(session_id);
}

/// How a turn ended, as seen by the caller.
///
/// This used to be a bare `Result<String>`, which left cancellation with no way to be
/// reported: a turn that was stopped early had to return either an empty "successful"
/// reply or a fabricated error. Both lie to the caller. Making the outcome explicit lets
/// the bridge distinguish "the model answered" from "the user stopped it".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// The model produced a final answer for the user.
    Completed(String),
    /// The turn stopped before producing an answer because the session was cancelled or
    /// its status was changed externally. `reason` is for logs, not for the user.
    Cancelled { reason: String },
}

impl TurnOutcome {
    /// The reply text, or an empty string when the turn was cancelled.
    #[must_use]
    pub fn reply(&self) -> &str {
        match self {
            Self::Completed(text) => text,
            Self::Cancelled { .. } => "",
        }
    }

    /// Whether the turn reached a final answer.
    #[must_use]
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed(_))
    }
}

/// Guarantees the session leaves the turn with status `idle`.
///
/// `execute_session_turn_inner` marks the session `running` and then has six `?` early
/// returns (history load, goal resolution, prompt build, assistant message insert, tool
/// runner, final message insert). None of those used to reset the status, so a single DB
/// or tool failure left the row `running` permanently. That is not cosmetic: the TUI
/// rebuilds `is_running` straight from the `sessions.status` column, so the session showed
/// a spinning "RUNNING" badge until the process restarted, and still after it.
///
/// The guard also releases the turn's file backups. On the success and LLM-error paths the
/// inner function already does that, and `clear()` is idempotent, so the extra call costs
/// nothing while covering the six early returns that used to leak the store.
struct TurnStatusGuard<'a> {
    session_id: &'a str,
    /// Which turn for this session this guard owns. Cleanup only lands while it is still the
    /// newest one; see [`claim_turn_generation`].
    generation: u64,
    armed: bool,
}

impl<'a> TurnStatusGuard<'a> {
    fn new(session_id: &'a str) -> Self {
        Self {
            generation: claim_turn_generation(session_id),
            session_id,
            armed: true,
        }
    }

    /// Performs the cleanup exactly once. Must be awaited on every return path of the
    /// wrapper, because `Drop` can only schedule it, not complete it.
    async fn finish(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        Self::cleanup(self.session_id, self.generation).await;
    }

    async fn cleanup(session_id: &str, generation: u64) {
        // Backups are deliberately kept here: they belong to the session, not to this turn.
        // See `release_session_snapshots`, which runs when the session is deleted.

        // A newer turn already owns this session. Writing `idle` now would flip the status of
        // a turn that is still running, so the stale cleanup is dropped entirely — including
        // the broadcast below, which the UI treats as authoritative.
        if !is_latest_turn(session_id, generation) {
            tracing::debug!(
                "[Agent Turn] Skipping stale cleanup for session {session_id} (turn {generation})"
            );
            return;
        }

        // Errors here are logged, never propagated: the caller is already returning the
        // turn's real outcome and must not have it replaced by a status-write failure.
        if let Err(e) = repository::update_session_status(session_id, "idle").await {
            tracing::warn!(
                "[Agent Turn] Could not reset session {} to idle: {}",
                session_id,
                e
            );
        }

        broadcast_session_event(
            session_id,
            AgentSessionEvent::StatusChange {
                status: "idle".to_string(),
            },
        );
    }
}

impl Drop for TurnStatusGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Last resort. A panic or an aborted task (the TUI aborts the turn task on
        // cancel) never reaches `finish`, and `Drop` cannot await, so the cleanup is
        // scheduled instead. Without this the abort path would reintroduce the very bug
        // the guard exists to prevent.
        //
        // The spawned task carries this guard's generation, so if the user has already
        // started another turn by the time it runs, it quietly stands down instead of
        // clobbering the new turn's `running` status.
        let session_id = self.session_id.to_string();
        let generation = self.generation;
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    TurnStatusGuard::cleanup(&session_id, generation).await;
                });
            }
            Err(_) => {
                tracing::warn!(
                    "[Agent Turn] No runtime available to reset session {} to idle",
                    self.session_id
                );
            }
        }
    }
}

/// Runs one agent turn and guarantees the session is left `idle` afterwards.
///
/// Thin wrapper: all the real work lives in `execute_session_turn_inner`, which is free to
/// use `?` for its six failure points. The cleanup is unconditional here, so no future
/// early return added to the inner function can silently strand the status again.
pub async fn execute_session_turn(
    session_id: &str,
    model_override: Option<&str>,
) -> Result<TurnOutcome> {
    let mut guard = TurnStatusGuard::new(session_id);
    let result = execute_session_turn_inner(session_id, model_override).await;
    guard.finish().await;
    result
}

async fn execute_session_turn_inner(
    session_id: &str,
    model_override: Option<&str>,
) -> Result<TurnOutcome> {
    // 1. Load Session & Project
    let session = repository::get_session(session_id)
        .await?
        .context("Session not found")?;

    let project = repository::get_project(&session.project_id)
        .await?
        .context("Project not found")?;

    let allowed_folders = parse_project_folders(project.root_path.as_deref());
    for folder in &allowed_folders {
        // BUG-11: this used to be `let _ = ...`, so a failing `git init` or .gitignore write
        // was invisible. It is not fatal — the turn can proceed without a repo — but it has
        // to be visible, otherwise the project silently stays untracked and every later
        // git-based recovery step quietly does nothing.
        if let Err(e) = crate::agent::tools::ensure_git_and_gitignore(std::path::Path::new(folder)) {
            tracing::warn!(
                "[Agent Turn] Could not prepare git workspace at '{}': {}",
                folder,
                e
            );
        }
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
                "[Agent] Session {} status changed to '{}', stopping the turn.",
                session_id,
                status
            );
            // Returning here, not `break`. A `break` fell through to the iteration-limit
            // fallback below, which persisted a "partial changes applied / please continue"
            // assistant message and reclassified the goal as Failed or Verification. That
            // reported an exhausted budget for a turn the user had deliberately stopped.
            return Ok(TurnOutcome::Cancelled {
                reason: format!("session status became '{status}'"),
            });
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

                return Err(anyhow::anyhow!(err_str));
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

        return Ok(TurnOutcome::Completed(content_to_save.to_string()));
    }

    // Fallback: If iteration limit reached while still running tools
    let summary = gates::handle_iteration_limit_fallback(session_id, &state).await?;
    Ok(TurnOutcome::Completed(summary))
}

#[cfg(test)]
mod turn_boundary_tests {
    use super::{claim_turn_generation, is_latest_turn, release_session_snapshots, TurnStatusGuard};
    use crate::agent::repository;

    /// BUG-09 rewrite. The old test asserted that a finished *turn* emptied the backup
    /// store \u2014 precisely the behaviour that made workspace_rollback useless, because its
    /// contract is per session and therefore spans turns. Backups must survive a turn
    /// boundary and be dropped when the session itself is deleted. The second half is the
    /// BUG-02 regression: one session releasing its store must not disturb another's.
    #[test]
    fn backups_survive_a_turn_boundary_and_are_dropped_with_the_session() {
        let dir = std::env::temp_dir().join(format!("turn-snap-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mine = dir.join("mine.txt");
        let theirs = dir.join("theirs.txt");
        std::fs::write(&mine, b"before").unwrap();
        std::fs::write(&theirs, b"before").unwrap();

        let busy = crate::agent::snapshot::for_session("snap-boundary-busy");
        let other = crate::agent::snapshot::for_session("snap-boundary-other");
        busy.backup_file(&mine).unwrap();
        other.backup_file(&theirs).unwrap();

        // A turn boundary must not discard what the session still needs to roll back ...
        assert_eq!(
            busy.tracked_files().unwrap().len(),
            1,
            "a turn boundary must keep this session's backups"
        );
        // ... and must never reach into another session's store (BUG-02).
        assert_eq!(
            other.tracked_files().unwrap().len(),
            1,
            "a turn boundary in one session must not clear another session's backups"
        );

        assert!(
            release_session_snapshots("snap-boundary-busy"),
            "deleting a session must release its store"
        );
        // Assert through a fresh lookup, not through `busy`: a handle already held elsewhere
        // shares the backing map, so it keeps its data alive until it is dropped. That is
        // deliberate — an in-flight turn must not lose its backups mid-flight — but it means
        // the handle cannot observe the release. Re-acquiring creates an empty store.
        assert!(
            crate::agent::snapshot::for_session("snap-boundary-busy")
                .tracked_files()
                .unwrap()
                .is_empty(),
            "a released session must not keep backups in the registry"
        );
        assert_eq!(
            other.tracked_files().unwrap().len(),
            1,
            "deleting one session must not drop another session's backups"
        );

        assert!(release_session_snapshots("snap-boundary-other"));
        // Assert on these two sessions only. A global "zero sessions" check would be a false
        // signal here: other tests run in the same process and legitimately hold stores.
        assert!(
            crate::agent::snapshot::for_session("snap-boundary-other")
                .tracked_files()
                .unwrap()
                .is_empty(),
            "a released session must not keep backups in the registry"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression test for the "session stuck on running forever" bug.
    ///
    /// The turn marks the session `running` and then hits a DB or tool failure that
    /// propagates with `?`. Before the guard existed, that early return skipped every
    /// status reset, so the row stayed `running`. The TUI rebuilds `is_running` from that
    /// column, so the session showed a spinning RUNNING badge across restarts.
    ///
    /// The guard is tested directly because driving a real failure through
    /// `execute_session_turn` would need a live model. Testing `finish()` covers the
    /// mechanism the wrapper relies on for all six early returns at once.
    #[tokio::test]
    async fn a_failed_turn_leaves_the_session_idle() {
        let project = repository::create_project("guard-test", "regression", None, None)
            .await
            .expect("project fixture must be creatable");
        let session = repository::create_session(&project.id, "guard", None, None, None)
            .await
            .expect("session fixture must be creatable");

        repository::update_session_status(&session.id, "running")
            .await
            .expect("the turn must be able to mark the session running");

        // Simulate the turn having recorded a backup before it failed.
        let dir = std::env::temp_dir().join(format!("turn-guard-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("half-written.txt");
        std::fs::write(&file, b"before").unwrap();
        crate::agent::snapshot::for_session(&session.id)
            .backup_file(&file)
            .unwrap();

        let mut guard = TurnStatusGuard::new(&session.id);
        guard.finish().await;

        let status = repository::get_session_status(&session.id)
            .await
            .expect("status must be readable");
        assert_eq!(
            status.as_deref(),
            Some("idle"),
            "a turn that failed must not leave the session marked running"
        );
        // The failed turn must not have discarded the session's rollback data: the user can
        // still ask for it to be reverted after a failure, and that spans turns.
        assert_eq!(
            crate::agent::snapshot::for_session(&session.id)
                .tracked_files()
                .expect("tracked_files must not fail on a healthy lock")
                .len(),
            1,
            "a failed turn must not throw away the session's backups"
        );
        assert!(release_session_snapshots(&session.id));

        let _ = repository::delete_session(&session.id).await;
        let _ = repository::delete_project(&project.id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `finish` must be idempotent: the wrapper always calls it, and `Drop` may also have
    /// run. A second cleanup must not flip the status or panic.
    #[tokio::test]
    async fn finishing_twice_is_harmless() {
        let project = repository::create_project("guard-idem", "regression", None, None)
            .await
            .expect("project fixture must be creatable");
        let session = repository::create_session(&project.id, "guard", None, None, None)
            .await
            .expect("session fixture must be creatable");
        repository::update_session_status(&session.id, "running")
            .await
            .expect("status must be writable");

        let mut guard = TurnStatusGuard::new(&session.id);
        guard.finish().await;
        guard.finish().await;

        let status = repository::get_session_status(&session.id)
            .await
            .expect("status must be readable");
        assert_eq!(status.as_deref(), Some("idle"));

        let _ = repository::delete_session(&session.id).await;
        let _ = repository::delete_project(&project.id).await;
    }

    /// N-2 regression: a stale cleanup must not disturb the generation owned by a newer turn.
    ///
    /// `TurnStatusGuard::drop` can only *schedule* its cleanup, so a superseded turn may reach
    /// the check after a newer turn has already claimed its own generation. That stale check
    /// must answer "not latest" and leave the newer turn's claim intact; if it pruned the
    /// entry, the newer turn would later fail to recognise itself as latest and its cleanup
    /// (idle status + notification) would be skipped entirely - reviving BUG-03.
    #[test]
    fn a_stale_cleanup_must_not_disown_the_newer_turn() {
        let session = format!("n2-stale-{}", std::process::id());

        let first = claim_turn_generation(&session);
        assert!(
            is_latest_turn(&session, first),
            "the only turn to have claimed the session must be the latest"
        );

        let second = claim_turn_generation(&session);
        assert!(second > first, "each claim must advance the generation");
        assert!(
            is_latest_turn(&session, second),
            "a newly claimed turn must be recognised as the latest"
        );

        // The first turn's orphaned cleanup runs late and must be ignored.
        assert!(
            !is_latest_turn(&session, first),
            "a superseded turn must not report itself as the latest"
        );
        // ... and critically, the second turn must still own the session afterwards.
        assert!(
            is_latest_turn(&session, second),
            "a stale cleanup must not delete the newer turn's generation"
        );

        // A session that never claimed anything is never the latest.
        assert!(!is_latest_turn(&format!("n2-unknown-{}", std::process::id()), 1));
    }
}