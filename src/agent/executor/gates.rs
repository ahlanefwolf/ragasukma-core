use anyhow::Result;

use super::state::TurnState;
use crate::agent::events::broadcast_session_event;
use crate::agent::models::AgentSessionEvent;
use crate::repository::agent as repository;

pub enum GateResult {
    Continue,
    Pass,
}

/// Decides whether the assistant has genuinely finished, using only structural
/// signals that hold in every language.
///
/// No keyword lists: the model already understands the request, and intent
/// detection by vocabulary breaks as soon as the user switches language, mixes
/// languages, or phrases things unusually. What we can rely on is what actually
/// happened during the turn.
fn turn_reached_a_conclusion(state: &TurnState, final_content: &str) -> bool {
    // Real work happened, so whatever the model says is a legitimate wrap-up.
    if state.has_taken_any_action || !state.modified_files.is_empty() {
        return true;
    }

    // Escalation budget exhausted: never trap the agent in a loop. Let it speak.
    if state.intent_without_action_nudges >= 2 {
        return true;
    }

    // Nothing was done and the reply is a bare acknowledgement or noise.
    let trimmed = final_content.trim();
    if trimmed.is_empty() {
        return false;
    }

    // A reply that contains no line breaks, no list markers, no code fence and
    // very few sentences is, structurally, a one-off remark rather than a final
    // report. This does not depend on the language of the text.
    let line_breaks = trimmed.matches('\n').count();
    let has_structure = line_breaks >= 2
        || trimmed.contains("- ")
        || trimmed.contains("* ")
        || trimmed.contains('`')
        || trimmed.contains("1.")
        || trimmed.contains("| ");

    if has_structure {
        return true;
    }

    // Sentence terminators are language-agnostic punctuation; count them.
    let terminators = trimmed
        .matches('.')
        .chain(trimmed.matches('!'))
        .chain(trimmed.matches('?'))
        .chain(trimmed.matches('\u{2026}'))
        .count();

    terminators >= 3
}

pub fn evaluate_turn_gates(
    state: &mut TurnState,
    final_content: &str,
    allowed_folders: &[String],
    detected_verifier: Option<&str>,
) -> GateResult {
    // 1. If the model returned no conversational content to the user yet
    if final_content.trim().is_empty() {
        state.consecutive_empty_turns += 1;
        if state.consecutive_empty_turns <= 1 && state.iteration < state.max_safety_iterations {
            state.ephemeral_directive = Some(
                "You have completed workspace operations. Provide a concise summary of the work done or directly answer the user's inquiry.".to_string(),
            );
            return GateResult::Continue;
        }
        return GateResult::Pass;
    } else {
        state.consecutive_empty_turns = 0;
    }

    // 1b. CONCLUSION GATE:
    // A model (DeepSeek in particular) often replies with "Oke, gue eksplor dulu"
    // and zero tool calls. Previously any non-empty text passed every gate, so the
    // turn ended immediately and the user had to prod it with "gimana coba".
    //
    // The decision is deliberately language-agnostic: we never inspect the wording
    // of the user's message. We only ask whether this turn actually concluded
    // anything measurable, and we give the model a small, bounded budget of nudges
    // before accepting its answer. This behaves identically for any language.
    if !turn_reached_a_conclusion(state, final_content)
        && !state.force_final_text_only
        && state.iteration < state.max_safety_iterations - 2
    {
        state.intent_without_action_nudges += 1;
        if state.intent_without_action_nudges <= 1 {
            state.ephemeral_directive = Some(
                "You have not inspected or changed anything yet, and the user's request is not resolved. Act now: invoke the appropriate tool (e.g. `workspace_list_dir` or `workspace_grep`) instead of describing what you are about to do. If you can already answer completely from what you know, answer conclusively now.".to_string()
            );
            return GateResult::Continue;
        }

        state.ephemeral_directive = Some(
            "Still no action taken. Either perform the tool calls now, or give your complete final answer based on what you already know. Do not restate your plan.".to_string()
        );
        return GateResult::Continue;
    }

    // 2. Action Gate:
    // Only require code edits if there was a local code/compiler failure or tool execution error.
    // If the failure was purely an external environment, network, port, or missing binary issue
    // and no workspace code files are broken, allow the agent to explain the environment problem.
    let is_pure_env_failure =
        state.last_command_is_environment_error && state.pending_error_files.is_empty();
    let must_apply_changes = (state.last_command_had_error || state.last_tool_had_error)
        && !is_pure_env_failure
        && !state.circuit_breaker_triggered
        && !state.force_final_text_only;

    let has_substantive_response = final_content.trim().len() >= 40;

    if must_apply_changes
        && state.modified_files.is_empty()
        && !has_substantive_response
        && state.iteration < state.max_safety_iterations - 2
    {
        state.ephemeral_directive = Some(
            "Unresolved code error: A command or tool reported errors and no files were modified. If a fix is needed, apply it using `workspace_edit_file` or `workspace_write_file`. If explaining the issue, provide your complete analysis to the user.".to_string()
        );
        return GateResult::Continue;
    }

    // 3. Hard Verification Enforcement Gate:
    let has_failing_checks = (!state.circuit_breaker_triggered && !state.force_final_text_only)
        && ((!state.has_passed_verification
            && state.verification_attempts > 0
            && !is_pure_env_failure)
            || (!is_pure_env_failure
                && (state.last_command_had_error || state.last_tool_had_error)));
    if has_failing_checks
        && !has_substantive_response
        && state.iteration < state.max_safety_iterations - 2
    {
        state.ephemeral_directive = Some(
            "Failing checks or unresolved errors remain. Inspect the error messages and fix the code using `workspace_edit_file` or `workspace_write_file`, or explain the findings to the user.".to_string()
        );
        return GateResult::Continue;
    }

    // 4. Verification Gate (Adaptive / Soft Gate):
    // Only prompt for verification if:
    // a) Files were modified,
    // b) Automated verification commands are actually available in the project,
    // c) The modified files are not exclusively non-executable assets (e.g. docs, configs, markup).
    let has_detected_verifiers = detected_verifier.is_some() || {
        let relevant = crate::agent::languages::detect_verifiers_for_modified_files(
            allowed_folders,
            &state.modified_files,
        );
        !relevant.is_empty()
    };

    let is_task_or_checklist_file = |path: &str| -> bool {
        let lower = path.to_lowercase();
        (lower.ends_with(".md") || lower.ends_with(".txt"))
            && (lower.contains("task")
                || lower.contains("todo")
                || lower.contains("checklist")
                || lower.contains("plan"))
    };

    let has_task_or_checklist_mod = state
        .modified_files
        .iter()
        .any(|f| is_task_or_checklist_file(f));

    let are_all_docs_or_configs = !state.modified_files.is_empty()
        && !has_task_or_checklist_mod
        && state.modified_files.iter().all(|f| {
            let lower = f.to_lowercase();
            lower.ends_with(".md")
                || lower.ends_with(".txt")
                || lower.ends_with(".json")
                || lower.ends_with(".yml")
                || lower.ends_with(".yaml")
                || lower.ends_with(".toml")
                || lower.ends_with(".css")
                || lower.ends_with(".scss")
                || lower.ends_with(".html")
                || lower.ends_with(".svg")
                || lower.ends_with(".env")
                || lower.contains(".env.")
                || lower.ends_with(".lock")
                || lower.ends_with(".gitignore")
        });

    if has_task_or_checklist_mod
        && !state.has_passed_verification
        && !state.verification_prompted
        && !state.circuit_breaker_triggered
        && !state.force_final_text_only
        && state.iteration < state.max_safety_iterations - 1
    {
        state.verification_prompted = true;
        let relevant_verifiers = crate::agent::languages::detect_verifiers_for_modified_files(
            allowed_folders,
            &state.modified_files,
        );
        let verifier_hint = if !relevant_verifiers.is_empty() {
            format!(
                "run verification (`{}`) and ",
                relevant_verifiers.join(" && ")
            )
        } else if let Some(pv) = detected_verifier {
            format!("run verification (`{}`) and ", pv)
        } else {
            String::new()
        };
        state.ephemeral_directive = Some(format!(
            "Deliverable verification: You updated a task/checklist file ({}). Before claiming complete status, {}physically verify that all requested deliverables and files exist and contain actual functional implementations (use `workspace_list_dir` or `workspace_read_file`). Never mark items complete without verifying the actual code on disk.",
            state.modified_files.join(", "),
            verifier_hint
        ));
        return GateResult::Continue;
    }

    if !state.modified_files.is_empty()
        && has_detected_verifiers
        && !are_all_docs_or_configs
        && !state.has_passed_verification
        && !state.verification_prompted
        && !state.circuit_breaker_triggered
        && !state.force_final_text_only
        && state.iteration < state.max_safety_iterations - 1
    {
        state.verification_prompted = true;
        let relevant_verifiers = crate::agent::languages::detect_verifiers_for_modified_files(
            allowed_folders,
            &state.modified_files,
        );
        let verifier_hint = if !relevant_verifiers.is_empty() {
            relevant_verifiers.join(" && ")
        } else if let Some(pv) = detected_verifier {
            pv.to_string()
        } else {
            "appropriate test or build commands".to_string()
        };
        state.ephemeral_directive = Some(format!(
            "VERIFICATION RECOMMENDATION: You modified {} file(s) ({}). Run `{}` using `workspace_run_command` to verify that there are no compilation or test errors. If verification is not applicable, you may proceed to conclude your summary.",
            state.modified_files.len(),
            state.modified_files.join(", "),
            verifier_hint
        ));
        return GateResult::Continue;
    }

    GateResult::Pass
}

pub fn compose_final_turn_content(
    state: &TurnState,
    final_content: &str,
    final_thought: &str,
) -> String {
    let trimmed_content = final_content.trim();

    let generated_summary = if !state.modified_files.is_empty() {
        let mut summary_lines = Vec::new();
        summary_lines.push("### 📋 Successfully Applied File Modifications:".to_string());
        for f in &state.modified_files {
            summary_lines.push(format!("- `{}`", f));
        }
        Some(summary_lines.join("\n"))
    } else {
        None
    };

    // If the model provided substantive final content, preserve it directly without prepending alarming banners
    if !trimmed_content.is_empty() {
        return match generated_summary {
            Some(summary) => format!("{}\n\n{}", trimmed_content, summary),
            None => trimmed_content.to_string(),
        };
    }

    // Only if final_content is empty, construct a fallback based on execution state
    let is_unresolved = (state.last_command_had_error && !state.last_command_is_environment_error)
        || state.last_tool_had_error
        || (state.verification_attempts > 0
            && !state.has_passed_verification
            && !state.last_command_is_environment_error);

    if is_unresolved {
        let error_details = if state.last_command_had_error {
            "Terminal verification/build command failed with code/compiler errors."
        } else if state.last_tool_had_error {
            "A file modification or tool execution failed."
        } else {
            "Required verification check has not passed cleanly."
        };
        format!(
            "⚠️ **Execution Ended with Unresolved Errors**\n\n{}\n\nPlease review the error outputs above or provide guidance.",
            error_details
        )
    } else if state.last_command_is_environment_error {
        "⚠️ **Environment / Command Execution Notice**\n\nCommand exited with an external environment or configuration error. Please check system dependencies, environment variables, or service status.".to_string()
    } else {
        match (trimmed_content.is_empty(), generated_summary.as_ref()) {
            (false, Some(summary)) => format!("{}\n\n{}", trimmed_content, summary),
            (false, None) => trimmed_content.to_string(),
            (true, Some(summary)) => summary.clone(),
            (true, None) if !final_thought.trim().is_empty() => final_thought.trim().to_string(),
            (true, None) => {
                "I have completed the operations and workspace inspection as requested.".to_string()
            }
        }
    }
}

pub async fn handle_iteration_limit_fallback(
    session_id: &str,
    state: &TurnState,
) -> Result<String> {
    // Check if there are recent assistant messages with substantive partial answers/findings
    let recent_findings = repository::list_messages(session_id)
        .await
        .ok()
        .and_then(|msgs| {
            msgs.into_iter()
                .rev()
                .find(|m| {
                    m.role == "assistant"
                        && m.content
                            .as_ref()
                            .map(|c| c.trim().len() >= 40)
                            .unwrap_or(false)
                })
                .and_then(|m| m.content)
        });

    let fallback_summary = if !state.modified_files.is_empty() {
        format!(
            "### ⚠️ Iteration Limit Reached — Partial Changes Applied\n\nThe following files were updated in this turn:\n{}\n\nSome sections or files may still need further modifications. Please review the current workspace changes or prompt me to continue with the remaining parts.",
            state
                .modified_files
                .iter()
                .map(|f| format!("- `{}`", f))
                .collect::<Vec<_>>()
                .join("\n")
        )
    } else if let Some(ref findings) = recent_findings {
        format!(
            "{}\n\n---\n*Note: Iteration limit reached for this turn. The findings above reflect workspace analysis completed so far.*",
            findings.trim()
        )
    } else {
        format!(
            "### ⚠️ Iteration Limit Reached\n\nCompleted workspace inspection and operations without modifying files.\n- Commands executed: {}\n\nPlease provide specific instructions if there are components or lines you want to change.",
            if state.executed_commands.is_empty() {
                "None".to_string()
            } else {
                state.executed_commands.join(", ")
            }
        )
    };

    let final_fallback_msg = repository::create_message(
        session_id,
        "assistant",
        Some(&fallback_summary),
        None,
        None,
        None,
        "success",
        None,
        None,
    )
    .await?;

    // An iteration-limited turn never concludes a goal. It either left changes in flight, stopped
    // mid-inspection, or produced findings, and none of that is proof the goal is done. The status
    // records where the work stands; the goal stays open so the user can ask for the rest.
    let final_fallback_status = if !state.modified_files.is_empty() {
        crate::agent::orchestrator::GoalState::Action
    } else if recent_findings.is_some() {
        crate::agent::orchestrator::GoalState::Verification
    } else {
        crate::agent::orchestrator::GoalState::Failed
    };
    let orch = crate::agent::orchestrator::GoalOrchestrator::new(&state.goal_id, "default");
    let _ = orch
        .conclude_with_status(final_fallback_status, &fallback_summary)
        .await;

    broadcast_session_event(
        session_id,
        AgentSessionEvent::MessageCreated(final_fallback_msg),
    );

    let _ = repository::update_session_status(session_id, "idle").await;
    broadcast_session_event(
        session_id,
        AgentSessionEvent::StatusChange {
            status: "idle".to_string(),
        },
    );
    broadcast_session_event(
        session_id,
        AgentSessionEvent::Done {
            reply: fallback_summary.clone(),
        },
    );

    Ok(fallback_summary)
}

#[cfg(test)]
mod tests {
    use super::{GateResult, turn_reached_a_conclusion};
    use crate::agent::executor::state::TurnState;

    fn idle_state() -> TurnState {
        TurnState::new(
            "goal_test".to_string(),
            "discovery".to_string(),
            Vec::new(),
            0,
        )
    }

    #[test]
    fn test_forces_action_when_nothing_inspected_yet() {
        // The reported failure: model replied "Oke, gue eksplor dulu..." with zero
        // tool calls and the turn ended.
        let mut state = idle_state();
        let result = super::evaluate_turn_gates(
            &mut state,
            "Oke, gue eksplor dulu struktur dan baca file-file intinya.",
            &[],
            None,
        );

        assert!(matches!(result, GateResult::Continue));
        assert!(state.ephemeral_directive.is_some());
        assert_eq!(state.intent_without_action_nudges, 1);
    }

    #[test]
    fn test_same_behaviour_across_languages() {
        // Identical handling for every language: no keyword lists involved.
        for msg in [
            "Siap, bentar gue intip dulu isinya.",
            "Sure, let me take a quick look first.",
            "\u{0e2a}\u{0e31}\u{0e01}\u{0e04}\u{0e23}\u{0e39}\u{0e1a}\u{0e17}\u{0e35}\u{0e19}\u{0e35}\u{0e49}\u{0e01}\u{0e48}\u{0e2d}\u{0e19}",
            "\u{3058}\u{3083}\u{3001}\u{898b}\u{3066}\u{307f}\u{308b}\u{306d}",
        ] {
            let mut state = idle_state();
            let result = super::evaluate_turn_gates(&mut state, msg, &[], None);
            assert!(
                matches!(result, GateResult::Continue),
                "should nudge: {msg}"
            );
        }
    }

    #[test]
    fn test_allows_conclusion_after_real_action() {
        let mut state = idle_state();
        state.has_taken_any_action = true;
        state.modified_files.push("src/main.rs".to_string());

        let answer = "Agent ini punya tiga kelemahan utama pada lapisan eksekusi dan parsing.";
        let result = super::evaluate_turn_gates(&mut state, answer, &[], None);
        assert!(matches!(result, GateResult::Pass));
    }

    #[test]
    fn test_structured_answer_without_tools_is_accepted() {
        // A question can be answered from existing knowledge; we must not force
        // tool usage just because no tool ran. A real report has structure.
        let mut state = idle_state();
        let answer = "Ada tiga alasan utama:\n\n- Ownership menghindari data race.\n- Borrow checker memvalidasi referensi saat kompilasi.\n- Tidak ada garbage collector.";
        let result = super::evaluate_turn_gates(&mut state, answer, &[], None);
        assert!(matches!(result, GateResult::Pass));
    }

    #[test]
    fn test_short_chatter_without_action_is_nudged() {
        let mut state = idle_state();
        let result = super::evaluate_turn_gates(
            &mut state,
            "Oke, gue eksplor dulu struktur dan baca file-file intinya.",
            &[],
            None,
        );
        assert!(matches!(result, GateResult::Continue));
    }

    #[test]
    fn test_budget_prevents_infinite_loop() {
        let mut state = idle_state();

        assert!(matches!(
            super::evaluate_turn_gates(&mut state, "ok", &[], None),
            GateResult::Continue
        ));
        assert!(matches!(
            super::evaluate_turn_gates(&mut state, "ok", &[], None),
            GateResult::Continue
        ));
        // Third attempt must be accepted so the agent can never be trapped.
        assert!(matches!(
            super::evaluate_turn_gates(&mut state, "ok", &[], None),
            GateResult::Pass
        ));
    }

    #[test]
    fn test_conclusion_ignores_wording_entirely() {
        let mut state = idle_state();
        assert!(!turn_reached_a_conclusion(&state, "halo bro"));
        assert!(!turn_reached_a_conclusion(&state, "hello there"));

        state.has_taken_any_action = true;
        assert!(turn_reached_a_conclusion(&state, "halo bro"));
    }
}
