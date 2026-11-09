//! Post-execution side effects: verification persistence, evidence caching,
//! streak tracking and message records.

use serde_json::Value;

use crate::agent::diagnostics::{
    extract_command_diagnostics, group_diagnostics_by_file, is_command_output_error,
};
use crate::repository::agent as repository;

use super::super::state::TurnState;

pub(super) fn extract_modified_paths_from_command(cmd: &str) -> Vec<String> {
    let mut paths = Vec::new();

    // 1. Python open('path', 'w'/'a') or open("path", "w"/"a")
    if cmd.contains("open(") {
        for part in cmd.split("open(") {
            let trimmed = part.trim_start();
            if (trimmed.starts_with('\'') || trimmed.starts_with('"'))
                && let Some(quote) = trimmed.chars().next()
            {
                let rest = &trimmed[1..];
                if let Some((p, post)) = rest.split_once(quote)
                    && (post.contains("'w'")
                        || post.contains("\"w\"")
                        || post.contains("'a'")
                        || post.contains("\"a\"")
                        || post.contains("'wb'")
                        || post.contains("\"wb\""))
                {
                    let clean = p.trim_start_matches("./").trim();
                    if !clean.is_empty() && !paths.contains(&clean.to_string()) {
                        paths.push(clean.to_string());
                    }
                }
            }
        }
    }

    // 2. touch file.txt
    if cmd.starts_with("touch ") {
        for arg in cmd.strip_prefix("touch ").unwrap_or("").split_whitespace() {
            let clean = arg.trim_start_matches("./").trim();
            if !clean.starts_with('-') && !clean.is_empty() && !paths.contains(&clean.to_string()) {
                paths.push(clean.to_string());
            }
        }
    }

    paths
}

/// Everything the post-execution effects need, gathered in one place.
///
/// These fields used to arrive as positional arguments, half of them bare
/// `bool`s, so a swapped call could not be spotted at the call site.
pub(super) struct PostExecContext<'a> {
    pub state: &'a mut TurnState,
    pub tool_name: &'a str,
    pub tool_output: &'a str,
    pub args_val: &'a Value,
    pub target_arg_file: Option<&'a str>,
    pub is_edit_op: bool,
    pub is_inspection: bool,
    pub is_accessing_unmodified_target: bool,
    pub is_breaker_trip: bool,
    pub session_id: &'a str,
    pub project_id: &'a str,
    pub allowed_folders: &'a [String],
}

/// Post-execution bookkeeping for the mutating/failing branch of a tool call.
///
/// Keeps the turn state honest after an edit or a command ran: verification
/// flags, error queues, phase transitions and the ephemeral directive that
/// nudges the model toward the next step.
pub(super) async fn apply_post_execution(ctx: PostExecContext<'_>) {
    let PostExecContext {
        state,
        tool_name,
        tool_output,
        args_val,
        target_arg_file,
        is_edit_op,
        is_breaker_trip,
        session_id,
        project_id,
        allowed_folders,
        ..
    } = ctx;

    if is_edit_op {
        let is_edit_failed = tool_output.starts_with("Tool execution error")
            || tool_output.starts_with("Phase Gate")
            || tool_output.contains("'old_text' not found")
            || tool_output.contains("failed to apply edit");

        if is_edit_failed {
            state.last_tool_had_error = true;
        } else {
            if let Some(path) = target_arg_file {
                state.on_successful_edit(path);
                let clean = path.trim_start_matches("./");
                if let Some(slash_idx) = clean.find('/') {
                    let subdir = &clean[..slash_idx];
                    state.verified_subdirs.remove(subdir);
                }
                state.force_final_text_only = false;
                state.circuit_breaker_triggered = false;
                let _ = repository::mark_evidence_stale_by_file(project_id, path).await;
            }

            let targets_json =
                serde_json::to_string(&state.locked_targets).unwrap_or_else(|_| "[]".to_string());
            let _ = repository::lock_goal_targets(&state.goal_id, &targets_json).await;
            let _ = repository::update_goal_phase(&state.goal_id, "verification").await;

            let relevant_verifiers = crate::agent::languages::detect_verifiers_for_modified_files(
                allowed_folders,
                &state.modified_files,
            );
            let primary_verify = if !relevant_verifiers.is_empty() {
                relevant_verifiers.join(" && ")
            } else {
                crate::agent::languages::detect_primary_verification_command(allowed_folders)
                    .unwrap_or_else(|| "test/build runner".to_string())
            };
            state.ephemeral_directive = Some(format!(
                "ACTION EXECUTED: Target file updated successfully. You are now in VERIFICATION phase. Run the verification command using `workspace_run_command` (e.g. `{}`) to verify your changes.",
                primary_verify
            ));
        }
    } else if tool_name == "workspace_run_command" {
        state.global_inspection_streak = 0;
        if let Some(cmd) = args_val.get("command").and_then(|c| c.as_str()) {
            let trimmed_cmd = cmd.trim();
            if state.last_executed_command.as_deref() == Some(trimmed_cmd) {
                state.consecutive_command_count += 1;
            } else {
                state.consecutive_command_count = 1;
                state.last_executed_command = Some(trimmed_cmd.to_string());
            }

            if !state.executed_commands.contains(&trimmed_cmd.to_string()) {
                state.executed_commands.push(trimmed_cmd.to_string());
            }

            let is_err = is_command_output_error(tool_output);

            if is_err {
                state.last_command_had_error = true;
                state.has_passed_verification = false;

                let (grouped_diags, general_errs) = group_diagnostics_by_file(tool_output);
                let is_env_err =
                    crate::agent::diagnostics::is_environment_error(tool_output, &grouped_diags);
                state.last_command_is_environment_error = is_env_err;

                if !is_env_err {
                    for g in &grouped_diags {
                        state.pending_error_files.insert(g.file_path.clone());
                    }
                }

                // Populate deterministic Diagnostic Queue in RAM for this session
                let parsed_items =
                    crate::agent::diagnostics::parse_diagnostic_items_from_output(tool_output);
                if !parsed_items.is_empty() {
                    crate::agent::diagnostics::set_session_diagnostic_queue(
                        session_id,
                        parsed_items,
                        Some(cmd.to_string()),
                    );
                }

                let raw_compiler_head: String =
                    tool_output.lines().take(45).collect::<Vec<_>>().join("\n");

                let mut diag_bulletin = String::new();

                if !grouped_diags.is_empty() {
                    diag_bulletin.push_str(
                        "\n\n🚨 TARGET FILES WITH COMPILER ERRORS (Batch-edit these files):\n",
                    );
                    for g in grouped_diags.iter().take(12) {
                        let lines_str = if !g.lines.is_empty() {
                            format!(
                                " (lines: {})",
                                g.lines
                                    .iter()
                                    .map(|l| l.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        } else {
                            String::new()
                        };
                        diag_bulletin.push_str(&format!(
                            "- `{}` -> {} error(s){}\n",
                            g.file_path, g.error_count, lines_str
                        ));
                        for s in &g.sample_messages {
                            diag_bulletin.push_str(&format!("    * {}\n", s));
                        }
                    }
                } else {
                    let extracted_diags = extract_command_diagnostics(tool_output);
                    if !extracted_diags.is_empty() {
                        diag_bulletin.push_str(&format!(
                            "\n\n🚨 EXTRACTED COMPILER / RUNTIME DIAGNOSTICS:\n{}\n",
                            extracted_diags
                                .iter()
                                .map(|d| format!("- {}", d))
                                .collect::<Vec<_>>()
                                .join("\n")
                        ));
                    }
                }

                if !general_errs.is_empty() {
                    diag_bulletin.push_str(&format!("General: {}\n", general_errs.join(" | ")));
                }

                state.ephemeral_directive = Some(format!(
                    "🚨 COMPILATION / TEST FAILURE DETECTED for command `{cmd}`!\n\n\
                 RAW COMPILER ERROR OUTPUT:\n```\n{raw_compiler_head}\n```\
                 {diag_bulletin}\n\n\
                 CRITICAL BATCH-ACTION REQUIRED:\n\
                 1. Read the target files listed above in full (or use the lines indicated).\n\
                 2. Apply ALL fixes for each target file using `workspace_edit_file` (or `workspace_write_file`).\n\
                 3. DO NOT re-run compiler or verification commands until ALL broken target files have been edited!\n\
                 4. Once all target files are updated, run `{cmd}` to verify."
                ));
            } else {
                // Check if command modified files (e.g. python script with open(..., 'w'), touch, etc.)
                let touched = extract_modified_paths_from_command(trimmed_cmd);
                for path in touched {
                    state.on_successful_edit(&path);
                    crate::agent::diagnostics::mark_diagnostic_file_resolved(session_id, &path);
                    let _ = repository::mark_evidence_stale_by_file(project_id, &path).await;
                }
            }

            if crate::agent::languages::is_verification_command(cmd) {
                let was_rejected =
                    is_breaker_trip || tool_output.starts_with("Tool execution rejected");
                let is_passed = !is_err && !was_rejected;
                state.verification_attempts += 1;

                if is_passed {
                    // Successful verification clears the session diagnostic queue
                    crate::agent::diagnostics::clear_session_diagnostic_queue(session_id);
                }
                // Persist the verification and, when it passes, the goal conclusion in a
                // single transaction. Recording a passing verification while silently
                // failing to move the goal would leave it stuck in "action" forever, and
                // a lost verification row would let the goal conclude with no evidence.
                let conclude_on_pass = is_passed && {
                    let cmd_lower = trimmed_cmd.to_lowercase();
                    let all_detectable_verifiers =
                        crate::agent::languages::detect_verifiers_for_modified_files(
                            allowed_folders,
                            &state.modified_files,
                        );
                    all_detectable_verifiers.into_iter().all(|v| {
                        cmd_lower.contains(&v.to_lowercase())
                            || state
                                .executed_commands
                                .iter()
                                .any(|ec| ec.to_lowercase().contains(&v.to_lowercase()))
                    })
                };

                let next_status: Option<&str> = if conclude_on_pass {
                    Some("completed")
                } else if is_passed {
                    None
                } else {
                    Some("in_progress")
                };

                if let Err(e) = repository::record_verification_with_status(
                    &state.goal_id,
                    state.verification_attempts as i64,
                    cmd,
                    tool_output,
                    is_passed,
                    if is_passed { None } else { Some(tool_output) },
                    next_status,
                )
                .await
                {
                    crate::term_err!(
                        "[Verification] Failed to persist verification for goal {}: {}",
                        state.goal_id,
                        e
                    );
                }

                if is_passed {
                    // Track which subdirectory was verified
                    let cmd_lower = trimmed_cmd.to_lowercase();
                    let all_detectable_verifiers =
                        crate::agent::languages::detect_verifiers_for_modified_files(
                            allowed_folders,
                            &state.modified_files,
                        );
                    let remaining_unverified: Vec<String> = all_detectable_verifiers
                        .into_iter()
                        .filter(|v| {
                            !cmd_lower.contains(&v.to_lowercase())
                                && !state
                                    .executed_commands
                                    .iter()
                                    .any(|ec| ec.to_lowercase().contains(&v.to_lowercase()))
                        })
                        .collect();

                    if let Some(cwd) = args_val.get("cwd").and_then(|v| v.as_str()) {
                        let clean_cwd = cwd.trim_start_matches("./");
                        state.verified_subdirs.insert(clean_cwd.to_string());
                    }

                    if !remaining_unverified.is_empty() {
                        state.has_passed_verification = false;
                        state.last_command_had_error = false;
                        state.ephemeral_directive = Some(format!(
                            "Component check succeeded for `{cmd}`. Notice: Other modified project components still require verification (e.g. `{}`). Please proceed to verify the remaining components before concluding.",
                            remaining_unverified.join(" && ")
                        ));
                    } else {
                        state.has_passed_verification = true;
                        state.last_command_had_error = false;
                        state.pending_error_files.clear();
                        state.current_phase = "concluded".to_string();
                        state.force_final_text_only = true;
                        // Phase transition already committed by record_verification_with_status.
                        state.ephemeral_directive = Some(
                        "VERIFICATION PASSED: The build/test command succeeded with 0 errors! All active tasks and verifications are complete. Conclude your turn now with a clear summary for the user without calling any more tools.".to_string()
                    );
                    }
                } else {
                    state.has_passed_verification = false;
                    state.current_phase = "action".to_string();
                    let _ = repository::update_goal_phase(&state.goal_id, "action").await;

                    let (grouped_diags, _) = group_diagnostics_by_file(tool_output);
                    for g in &grouped_diags {
                        state.pending_error_files.insert(g.file_path.clone());
                    }
                    let mut diag_bulletin = String::new();
                    if !grouped_diags.is_empty() {
                        diag_bulletin.push_str("\n\n🚨 VERIFICATION FAILURE TARGET FILES:\n");
                        for g in grouped_diags.iter().take(10) {
                            diag_bulletin.push_str(&format!(
                                "- `{}` ({} errors at lines {:?})\n",
                                g.file_path, g.error_count, g.lines
                            ));
                        }
                    }

                    state.ephemeral_directive = Some(format!(
                        "VERIFICATION FAILED: Verification command `{cmd}` reported errors.{diag_bulletin}\n\nACTION REQUIRED: Read the failing files, modify them using `workspace_edit_file` or `workspace_write_file`, and re-verify before concluding."
                    ));
                }
            }
        }
    }
}

/// Corrective directives, inspection-streak advice and the evidence cache.
///
/// Runs once the tool has executed: it turns what happened into guidance for
/// the next step, then records the file versions the goal can cite as proof.
pub(super) async fn apply_post_inspection(ctx: PostExecContext<'_>) {
    let PostExecContext {
        state,
        tool_name,
        tool_output,
        args_val,
        target_arg_file,
        is_edit_op,
        is_inspection,
        is_accessing_unmodified_target,
        project_id,
        allowed_folders,
        ..
    } = ctx;

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

    let is_file_inspection = tool_name == "workspace_read_file" || tool_name == "workspace_grep";
    if is_file_inspection && let Some(path) = target_arg_file {
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
        if let Some(file_path) = target_arg_file {
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
        && let Some(file_path) = target_arg_file
    {
        let preview_summary = format!("Modified/Created file {}", file_path);
        let file_ver = crate::agent::tools::fs::resolve_allowed_path(file_path, allowed_folders)
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
}
