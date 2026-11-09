//! Audit trail for tool invocations.

use crate::repository::agent as repository;


/// One audit record for a tool invocation.
pub(super) struct ToolAudit<'a> {
    pub(super) tool_name: &'a str,
    pub(super) target_path: Option<&'a str>,
    pub(super) command: Option<&'a str>,
    pub(super) status: &'a str,
    pub(super) reason: Option<&'a str>,
    pub(super) duration_ms: Option<i64>,
}

/// Writes an audit entry, surfacing the failure instead of discarding it.
///
/// Audit rows are the record of what the agent actually did on this machine. Silently
/// dropping them would mean a broken audit trail looks identical to an idle agent, so a
/// failed write is logged loudly. It deliberately does not abort the turn: refusing to
/// run tools because bookkeeping failed would be worse than a flagged gap.
pub(super) async fn audit_tool_call(session_id: &str, goal_id: &str, entry: ToolAudit<'_>) {
    let ToolAudit {
        tool_name,
        target_path,
        command,
        status,
        reason,
        duration_ms,
    } = entry;

    if let Err(e) = repository::record_agent_audit(
        Some(session_id),
        Some(goal_id),
        tool_name,
        target_path,
        command,
        status,
        reason,
        duration_ms,
    )
    .await
    {
        crate::term_err!(
            "[Audit] Failed to persist audit entry (tool={}, status={}): {}",
            tool_name,
            status,
            e
        );
    }
}

