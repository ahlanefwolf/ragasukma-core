use anyhow::Result;
use sqlx::SqlitePool;

use crate::agent::models::AgentAuditLog;

use super::{generate_id, now_secs};

/// Reads the tool-execution audit trail for a goal, newest first.
/// The table is written on every tool call; without this reader the log was pure
/// dead weight and users had no way to see what the agent actually touched.
pub async fn list_goal_audit_logs(goal_id: &str, limit: i64) -> Result<Vec<AgentAuditLog>> {
    let pool = super::pool().await;
    list_goal_audit_logs_with_pool(pool, goal_id, limit).await
}

/// Explicit-pool core of [`list_goal_audit_logs`] (testable with a temp pool).
pub async fn list_goal_audit_logs_with_pool(
    pool: &SqlitePool,
    goal_id: &str,
    limit: i64,
) -> Result<Vec<AgentAuditLog>> {
    let rows = sqlx::query_as::<_, AgentAuditLog>(
        "SELECT id, session_id, goal_id, tool_name, target_path, command, status, reason, duration_ms, created_at \
         FROM agent_audit_logs WHERE goal_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2",
    )
    .bind(goal_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Reads the audit trail for a whole session, newest first.
pub async fn list_session_audit_logs(session_id: &str, limit: i64) -> Result<Vec<AgentAuditLog>> {
    let pool = super::pool().await;
    list_session_audit_logs_with_pool(pool, session_id, limit).await
}

/// Explicit-pool core of [`list_session_audit_logs`] (testable with a temp pool).
pub async fn list_session_audit_logs_with_pool(
    pool: &SqlitePool,
    session_id: &str,
    limit: i64,
) -> Result<Vec<AgentAuditLog>> {
    let rows = sqlx::query_as::<_, AgentAuditLog>(
        "SELECT id, session_id, goal_id, tool_name, target_path, command, status, reason, duration_ms, created_at \
         FROM agent_audit_logs WHERE session_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2",
    )
    .bind(session_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Deletes audit rows older than `cutoff_epoch_secs` and returns how many were removed.
///
/// The cutoff is supplied by the caller so the retention window stays a runtime policy
/// instead of being hardcoded into the query.
pub async fn delete_expired_audit_logs(cutoff_epoch_secs: i64) -> Result<u64> {
    let pool = super::pool().await;
    delete_expired_audit_logs_with_pool(pool, cutoff_epoch_secs).await
}

/// Explicit-pool core of [`delete_expired_audit_logs`] (testable with a temp pool).
pub async fn delete_expired_audit_logs_with_pool(
    pool: &SqlitePool,
    cutoff_epoch_secs: i64,
) -> Result<u64> {
    let res = sqlx::query("DELETE FROM agent_audit_logs WHERE created_at < ?1")
        .bind(cutoff_epoch_secs)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

#[allow(clippy::too_many_arguments)]
pub async fn record_agent_audit(
    session_id: Option<&str>,
    goal_id: Option<&str>,
    tool_name: &str,
    target_path: Option<&str>,
    command: Option<&str>,
    status: &str,
    reason: Option<&str>,
    duration_ms: Option<i64>,
) -> Result<()> {
    let pool = super::pool().await;
    record_agent_audit_with_pool(
        pool,
        session_id,
        goal_id,
        tool_name,
        target_path,
        command,
        status,
        reason,
        duration_ms,
    )
    .await
}

/// Explicit-pool core of [`record_agent_audit`] (testable with a temp pool).
#[allow(clippy::too_many_arguments)]
pub async fn record_agent_audit_with_pool(
    pool: &SqlitePool,
    session_id: Option<&str>,
    goal_id: Option<&str>,
    tool_name: &str,
    target_path: Option<&str>,
    command: Option<&str>,
    status: &str,
    reason: Option<&str>,
    duration_ms: Option<i64>,
) -> Result<()> {
    let now = now_secs();
    let id = generate_id("audit");

    sqlx::query(
        "INSERT INTO agent_audit_logs (id, session_id, goal_id, tool_name, target_path, command, status, reason, duration_ms, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(&id)
    .bind(session_id)
    .bind(goal_id)
    .bind(tool_name)
    .bind(target_path)
    .bind(command)
    .bind(status)
    .bind(reason)
    .bind(duration_ms)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(())
}
