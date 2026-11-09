//! Hard gates and circuit breakers evaluated before a tool call runs.
//!
//! Each gate is a pure predicate over [`TurnState`] and the parsed arguments,
//! so the decision to deny a call can be tested without touching the database,
//! the network, or the agent loop. `tool_runner::mod` only decides *what* to do
//! with a denial (audit row + message back to the model).

use serde_json::Value;

use super::super::state::TurnState;

/// Why a tool call was refused before it ran.
///
/// Exhaustive so the caller must handle every case; the `reason` string is what
/// gets written to the audit trail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denial {
    /// The turn's circuit breaker already tripped, or the model was told to
    /// answer in text only.
    CircuitBreakerTriggered,
    /// Verification already passed and this call adds no new work.
    VerificationAlreadyPassed,
    /// The same command is being re-run without intervening file changes.
    RepeatedCommand { command: String },
    /// Too many consecutive inspections without a code modification.
    InspectionBudgetExhausted { streak: usize },
}

impl Denial {
    /// Stable audit slug, e.g. `"verification_already_passed"`.
    pub fn reason(&self) -> &'static str {
        match self {
            Denial::CircuitBreakerTriggered => "circuit_breaker_triggered",
            Denial::VerificationAlreadyPassed => "verification_already_passed",
            Denial::RepeatedCommand { .. } => "repeated_command_without_progress",
            Denial::InspectionBudgetExhausted { .. } => "inspection_budget_exhausted",
        }
    }

    /// Message handed back to the model as the tool's output, so it knows how to
    /// recover instead of retrying blindly.
    pub fn message(&self) -> String {
        match self {
            Denial::CircuitBreakerTriggered => "Tool execution rejected: Tool calling is closed for this turn. Conclude your turn immediately and provide your summary to the user now.".to_string(),
            Denial::VerificationAlreadyPassed => "Verification has already succeeded with 0 errors. All active errors resolved. Tool calling is now closed. Conclude your turn immediately and provide your summary to the user now.".to_string(),
            Denial::RepeatedCommand { command } => format!(
                "Command repetition limit reached for `{command}`. This command was already executed without subsequent file modifications. Tool calling is now closed. Conclude your turn now and report the results directly to the user."
            ),
            Denial::InspectionBudgetExhausted { streak } => format!(
                "Inspection budget reached ({streak} consecutive reads without code modifications). Tool calling is closed to prevent unproductive wandering. Conclude your turn now and provide your findings, answers, or plan directly to the user."
            ),
        }
    }
}

/// Number of consecutive inspections of a single target that triggers the
/// repeated-read notice. Kept as a named constant because the value is
/// referenced by the message sent to the model.
pub const REPEATED_READ_NOTICE_THRESHOLD: usize = 5;

/// Hard inspection ceiling, independent of the turn budget.
pub const INSPECTION_CEILING: usize = 20;

/// `command` argument of a call, trimmed and lowercased, or `""`.
fn lower_command(args: &Value) -> String {
    args.get("command")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase()
}

/// True for calls that only observe the workspace, including read-only shell
/// commands such as `cat`, `ls`, `head`, `tail`, `grep` and `find`.
pub fn is_inspection(tool_name: &str, args: &Value) -> bool {
    match tool_name {
        "workspace_read_file" | "workspace_grep" | "workspace_list_dir" => true,
        "workspace_run_command" => {
            let cmd = lower_command(args);
            cmd.starts_with("cat ")
                || cmd.starts_with("ls")
                || cmd.starts_with("head ")
                || cmd.starts_with("tail ")
                || cmd.starts_with("grep ")
                || cmd.starts_with("find ")
        }
        _ => false,
    }
}

/// True when `tool_name` reads, edits or writes a target that has not been
/// modified yet in this turn.
pub fn is_work_on_new_target(tool_name: &str, is_accessing_unmodified_target: bool) -> bool {
    matches!(
        tool_name,
        "workspace_read_file" | "workspace_edit_file" | "workspace_write_file"
    ) && is_accessing_unmodified_target
}

/// True when the target file lives in a subdirectory this session has not
/// verified yet. A target with no subdirectory component never qualifies.
pub fn is_accessing_different_component(target: Option<&str>, state: &TurnState) -> bool {
    let Some(path) = target else {
        return false;
    };
    let clean = path.trim_start_matches("./");
    let components: Vec<&str> = clean.split('/').collect();
    if components.len() < 2 {
        return false;
    }
    let subdir = components[0];
    !state.verified_subdirs.contains(subdir) && !state.verified_subdirs.is_empty()
}

/// True when a terminal command after a passing verification is bookkeeping
/// (`git add`/`git commit`) rather than new work.
pub fn is_post_verification_terminal_command(
    tool_name: &str,
    args: &Value,
    state: &TurnState,
) -> bool {
    if !(state.has_passed_verification
        && !state.last_command_had_error
        && state.pending_error_files.is_empty()
        && tool_name == "workspace_run_command")
    {
        return false;
    }
    let cmd = lower_command(args);
    cmd.starts_with("git ")
        || cmd.contains("git commit")
        || cmd.contains("git add")
        || state
            .executed_commands
            .iter()
            .any(|ec| ec.contains(&cmd) || cmd.contains(ec))
}

/// True when the same command is re-run without progress in between.
pub fn is_repeated_command(tool_name: &str, args: &Value, state: &TurnState) -> bool {
    if tool_name != "workspace_run_command" {
        return false;
    }
    let cmd = args
        .get("command")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim();
    !cmd.is_empty()
        && state.last_executed_command.as_deref() == Some(cmd)
        && state.consecutive_command_count >= 1
}

/// True when a read targets the exact same slice that was already read this
/// turn at least [`REPEATED_READ_NOTICE_THRESHOLD`] times.
pub fn is_repeated_read(tool_name: &str, args: &Value, state: &TurnState) -> Option<String> {
    if tool_name != "workspace_read_file" {
        return None;
    }
    let start_line = args.get("start_line").and_then(|v| v.as_i64());
    let end_line = args.get("end_line").and_then(|v| v.as_i64());
    let path = args
        .get("file_path")
        .or_else(|| args.get("path"))
        .and_then(|p| p.as_str())
        .unwrap_or("");
    let is_exact_duplicate =
        state
            .recent_read_slices
            .contains(&(path.to_string(), start_line, end_line));
    let same_file_count = state
        .consecutive_read_same_file
        .get(path)
        .copied()
        .unwrap_or(0);
    if is_exact_duplicate && same_file_count >= REPEATED_READ_NOTICE_THRESHOLD {
        Some(path.to_string())
    } else {
        None
    }
}

/// Evaluates every hard gate in priority order, returning the first denial.
///
/// `is_target_in_error_list` exempts targets that still have compiler errors, so
/// a session cannot be locked out of the files it is supposed to fix.
/// True when this call re-runs a shell verification command.
///
/// Only shell-execution tools are inspected; file reads, edits, and searches
/// are never verification commands and must stay available after a pass.
fn is_verification_command(tool_name: &str, args: &Value) -> bool {
    let is_shell = tool_name.contains("run_command") || tool_name.contains("shell");
    if !is_shell {
        return false;
    }
    args.get("command")
        .and_then(|c| c.as_str())
        .is_some_and(crate::agent::languages::is_verification_command)
}

pub fn evaluate(
    tool_name: &str,
    args: &Value,
    state: &TurnState,
    is_target_in_error_list: bool,
) -> Option<Denial> {
    if state.circuit_breaker_triggered {
        return Some(Denial::CircuitBreakerTriggered);
    }

    // A passed verification used to hard-close every tool for the rest of the
    // turn. That is how a premature "VERIFICATION PASSED" became unrecoverable:
    // the model could not even inspect files to discover that another component
    // in the workspace was still broken. Verification success is now advisory —
    // it only denies re-running the exact verification command that already
    // passed (see `VerificationAlreadyPassed` below), while reads, edits, and
    // unrelated commands stay available.
    if state.verification_complete && is_verification_command(tool_name, args) {
        return Some(Denial::VerificationAlreadyPassed);
    }

    let is_new_target = is_work_on_new_target(
        tool_name,
        !state.modified_files.iter().any(|mf| {
            let target = args
                .get("file_path")
                .or_else(|| args.get("path"))
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let clean = target.trim_start_matches("./");
            let mf_clean = mf.trim_start_matches("./");
            mf_clean == clean || mf_clean.ends_with(clean) || clean.ends_with(mf_clean)
        }),
    );

    let target = args
        .get("file_path")
        .or_else(|| args.get("path"))
        .and_then(|p| p.as_str());
    let different_component = is_accessing_different_component(target, state);

    let verification_closed = state.has_passed_verification
        && !state.last_command_had_error
        && state.pending_error_files.is_empty()
        && !is_new_target
        && !different_component
        && (tool_name != "workspace_run_command"
            || is_post_verification_terminal_command(tool_name, args, state));
    if verification_closed {
        return Some(Denial::VerificationAlreadyPassed);
    }

    if is_repeated_command(tool_name, args, state) {
        let command = args
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        return Some(Denial::RepeatedCommand { command });
    }

    if is_inspection(tool_name, args)
        && !is_target_in_error_list
        && state.global_inspection_streak >= INSPECTION_CEILING
    {
        return Some(Denial::InspectionBudgetExhausted {
            streak: state.global_inspection_streak,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state() -> TurnState {
        TurnState::new("goal".into(), "action".into(), vec![], 0)
    }

    #[test]
    fn circuit_breaker_wins_over_everything() {
        let mut s = state();
        s.circuit_breaker_triggered = true;
        assert_eq!(
            evaluate("workspace_read_file", &json!({}), &s, false),
            Some(Denial::CircuitBreakerTriggered)
        );
    }

    #[test]
    fn force_final_text_only_keeps_inspection_tools_open() {
        // Soft-nudge: a passed verification must not lock reads/searches, or a
        // premature "VERIFICATION PASSED" can never be corrected by inspecting
        // the rest of the workspace.
        let mut s = state();
        s.verification_complete = true;
        assert_eq!(
            evaluate("workspace_grep", &json!({"pattern": "x"}), &s, false),
            None
        );
    }

    #[test]
    fn force_final_text_only_still_denies_rerunning_verification() {
        let mut s = state();
        s.verification_complete = true;
        assert_eq!(
            evaluate(
                "workspace_run_command",
                &json!({"command": "cargo test"}),
                &s,
                false
            ),
            Some(Denial::VerificationAlreadyPassed)
        );
    }

    #[test]
    fn inspection_recognises_readonly_shell_commands() {
        assert!(is_inspection(
            "workspace_run_command",
            &json!({"command": "cat src/lib.rs"})
        ));
        assert!(is_inspection(
            "workspace_run_command",
            &json!({"command": "  grep -rn TODO"})
        ));
        assert!(is_inspection("workspace_list_dir", &json!({})));
        assert!(!is_inspection(
            "workspace_run_command",
            &json!({"command": "cargo test"})
        ));
        assert!(!is_inspection("workspace_edit_file", &json!({})));
    }

    #[test]
    fn new_target_covers_read_edit_and_write_only() {
        assert!(is_work_on_new_target("workspace_edit_file", true));
        assert!(is_work_on_new_target("workspace_read_file", true));
        assert!(!is_work_on_new_target("workspace_edit_file", false));
        assert!(!is_work_on_new_target("workspace_grep", true));
    }

    #[test]
    fn different_component_needs_a_subdirectory_and_a_verified_set() {
        let s = state();
        assert!(!is_accessing_different_component(Some("src/lib.rs"), &s));
        assert!(!is_accessing_different_component(Some("lib.rs"), &s));

        let mut s = state();
        s.verified_subdirs.insert("verified_dir".into());
        assert!(is_accessing_different_component(Some("other/x.rs"), &s));
        assert!(!is_accessing_different_component(
            Some("verified_dir/x.rs"),
            &s
        ));
        assert!(!is_accessing_different_component(None, &s));
    }

    #[test]
    fn verification_closes_tools_but_not_unmodified_targets() {
        let mut s = state();
        s.has_passed_verification = true;
        let args = json!({"file_path": "src/a.rs"});

        // A read of a file we never touched is new work, so the gate must stay
        // open: `is_work_on_new_target` suppresses verification closure.
        assert_eq!(evaluate("workspace_read_file", &args, &s, false), None);

        // Once the target is a file we already edited, the read adds nothing
        // new, so the gate closes the turn.
        s.modified_files.push("src/a.rs".into());
        assert_eq!(
            evaluate("workspace_read_file", &args, &s, false),
            Some(Denial::VerificationAlreadyPassed)
        );
    }

    #[test]
    fn verification_stays_open_while_errors_are_pending() {
        let mut s = state();
        s.has_passed_verification = true;
        s.pending_error_files.insert("src/a.rs".into());
        assert_eq!(
            evaluate(
                "workspace_read_file",
                &json!({"file_path": "src/b.rs"}),
                &s,
                false
            ),
            None
        );
    }

    #[test]
    fn git_bookkeeping_is_allowed_after_verification() {
        let mut s = state();
        s.has_passed_verification = true;
        assert!(is_post_verification_terminal_command(
            "workspace_run_command",
            &json!({"command": "git commit -m x"}),
            &s
        ));
        assert!(!is_post_verification_terminal_command(
            "workspace_run_command",
            &json!({"command": "cargo test"}),
            &s
        ));
    }

    #[test]
    fn repeated_command_requires_matching_previous_command() {
        let mut s = state();
        assert!(!is_repeated_command(
            "workspace_run_command",
            &json!({"command": "cargo test"}),
            &s
        ));

        s.last_executed_command = Some("cargo test".into());
        s.consecutive_command_count = 1;
        assert!(is_repeated_command(
            "workspace_run_command",
            &json!({"command": "cargo test"}),
            &s
        ));
        assert!(!is_repeated_command(
            "workspace_run_command",
            &json!({"command": "cargo build"}),
            &s
        ));
        // A blank command is never "repeated" — it would otherwise trip the gate
        // before the blank-name guard reports the real problem.
        assert!(!is_repeated_command(
            "workspace_run_command",
            &json!({"command": "  "}),
            &s
        ));
    }

    #[test]
    fn inspection_ceiling_only_trips_for_inspections_off_the_error_list() {
        let mut s = state();
        s.global_inspection_streak = INSPECTION_CEILING;
        let read = json!({"file_path": "src/a.rs"});

        assert_eq!(
            evaluate("workspace_read_file", &read, &s, false),
            Some(Denial::InspectionBudgetExhausted {
                streak: INSPECTION_CEILING
            })
        );
        // Targets with unresolved compiler errors stay readable.
        assert_eq!(evaluate("workspace_read_file", &read, &s, true), None);
        // Edits are never gated by the inspection ceiling.
        assert_eq!(evaluate("workspace_edit_file", &read, &s, false), None);
    }

    #[test]
    fn denial_reasons_are_stable_audit_slugs() {
        assert_eq!(
            Denial::CircuitBreakerTriggered.reason(),
            "circuit_breaker_triggered"
        );
        assert_eq!(
            Denial::VerificationAlreadyPassed.reason(),
            "verification_already_passed"
        );
        assert_eq!(
            Denial::RepeatedCommand {
                command: "x".into()
            }
            .reason(),
            "repeated_command_without_progress"
        );
        assert_eq!(
            Denial::InspectionBudgetExhausted { streak: 20 }.reason(),
            "inspection_budget_exhausted"
        );
    }

    #[test]
    fn denial_messages_name_the_offending_command_and_streak() {
        assert!(
            Denial::RepeatedCommand {
                command: "cargo test".into()
            }
            .message()
            .contains("cargo test")
        );
        assert!(
            Denial::InspectionBudgetExhausted { streak: 21 }
                .message()
                .contains("21")
        );
    }
}
