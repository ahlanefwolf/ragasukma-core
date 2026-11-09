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

/// Post-execution bookkeeping for the mutating/failing branch of a tool call.
///
/// Keeps the turn state honest after an edit or a command ran: verification
/// flags, error queues, phase transitions and the ephemeral directive that
/// nudges the model toward the next step.
#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_post_execution(
    state: &mut TurnState,
    tool_name: &str,
    tool_output: &str,
    args_val: &Value,
    target_arg_file: Option<&str>,
    is_edit_op: bool,
    session_id: &str,
    project_id: &str,
    allowed_folders: &[String],
    is_breaker_trip: bool,
) {
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
