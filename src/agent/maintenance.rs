//! Database maintenance: bounded retention for high-churn agent telemetry.
//!
//! `messages` and `agent_audit_logs` grow with every model turn and every tool call.
//! Left alone they grow without bound. A sweep runs once at startup and then
//! periodically, so long-lived sessions are also pruned.
//!
//! Retention windows and the sweep interval are read from the environment so the
//! policy stays a runtime knob rather than a value baked into SQL:
//!
//! - `RAGASUKMA_AGENT_MESSAGE_RETENTION_DAYS`    (default 30, `0` disables)
//! - `RAGASUKMA_AGENT_AUDIT_RETENTION_DAYS`      (default 30, `0` disables)
//! - `RAGASUKMA_AGENT_SESSION_RETENTION_DAYS`    (default 90, `0` disables)
//! - `RAGASUKMA_AGENT_RETENTION_INTERVAL_HOURS`  (default 6, `0` = startup only)

use crate::agent::repository;

pub const ENV_MESSAGE_RETENTION_DAYS: &str = "RAGASUKMA_AGENT_MESSAGE_RETENTION_DAYS";
pub const ENV_AUDIT_RETENTION_DAYS: &str = "RAGASUKMA_AGENT_AUDIT_RETENTION_DAYS";
pub const ENV_RETENTION_INTERVAL_HOURS: &str = "RAGASUKMA_AGENT_RETENTION_INTERVAL_HOURS";
pub const ENV_SESSION_RETENTION_DAYS: &str = "RAGASUKMA_AGENT_SESSION_RETENTION_DAYS";

pub const DEFAULT_MESSAGE_RETENTION_DAYS: u64 = 30;
pub const DEFAULT_AUDIT_RETENTION_DAYS: u64 = 30;
pub const DEFAULT_RETENTION_INTERVAL_HOURS: u64 = 6;
pub const DEFAULT_SESSION_RETENTION_DAYS: u64 = 90;

pub const SECONDS_PER_DAY: u64 = 86_400;
pub const SECONDS_PER_HOUR: u64 = 3_600;

/// Parses a retention window in days from a raw value, falling back to `default`.
/// Returns `None` when retention is explicitly disabled (`0`).
pub fn parse_retention_days(raw: Option<&str>, default: u64) -> Option<u64> {
    let days = raw
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(default);

    if days == 0 { None } else { Some(days) }
}

/// Reads a retention window in days from the environment, falling back to `default`.
pub fn retention_days(env_key: &str, default: u64) -> Option<u64> {
    let raw = std::env::var(env_key).ok();
    parse_retention_days(raw.as_deref(), default)
}

/// Converts a day count into an absolute epoch-second cutoff relative to now.
pub fn cutoff_epoch_secs(days: u64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_sub(days.saturating_mul(SECONDS_PER_DAY)) as i64
}

/// Prunes expired `messages` and `agent_audit_logs` rows.
///
/// This is best-effort: failures are logged and never abort startup, because losing
/// maintenance should not stop the application from running.
pub async fn run_retention_sweep() {
    if let Some(days) = retention_days(ENV_MESSAGE_RETENTION_DAYS, DEFAULT_MESSAGE_RETENTION_DAYS) {
        let cutoff = cutoff_epoch_secs(days);
        match repository::delete_expired_messages(cutoff).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                "[Retention] Pruned {} message row(s) older than {}d",
                n,
                days
            ),
            Err(e) => tracing::error!("[Retention] Message sweep failed: {e}"),
        }
    }

    if let Some(days) = retention_days(ENV_AUDIT_RETENTION_DAYS, DEFAULT_AUDIT_RETENTION_DAYS) {
        let cutoff = cutoff_epoch_secs(days);
        match repository::delete_expired_audit_logs(cutoff).await {
            Ok(0) => {}
            Ok(n) => tracing::info!("[Retention] Pruned {} audit row(s) older than {}d", n, days),
            Err(e) => tracing::error!("[Retention] Audit sweep failed: {e}"),
        }
    }

    // Stale sessions carry their goals and evidence with them, so this also reclaims
    // everything that hung off abandoned conversations.
    if let Some(days) = retention_days(ENV_SESSION_RETENTION_DAYS, DEFAULT_SESSION_RETENTION_DAYS) {
        let cutoff = cutoff_epoch_secs(days);
        match repository::delete_expired_sessions(cutoff).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                "[Retention] Pruned {} session(s) idle for over {}d",
                n,
                days
            ),
            Err(e) => tracing::error!("[Retention] Session sweep failed: {e}"),
        }
        match repository::delete_orphan_projects().await {
            Ok(0) => {}
            Ok(n) => tracing::info!("[Retention] Pruned {} project(s) with no sessions", n),
            Err(e) => tracing::error!("[Retention] Orphan project sweep failed: {e}"),
        }
    }
}

/// Runs the background retention loop indefinitely.
pub async fn run_retention_loop() {
    retention_task().await;
}

/// Background retention task loop.
///
/// Runs once immediately at startup, then repeats every configured interval.
/// Exits cleanly if interval hours is set to `0`.
pub async fn retention_task() {
    run_retention_sweep().await;

    let interval_hours = std::env::var(ENV_RETENTION_INTERVAL_HOURS)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_RETENTION_INTERVAL_HOURS);

    if interval_hours == 0 {
        return;
    }

    let interval = std::time::Duration::from_secs(interval_hours.saturating_mul(SECONDS_PER_HOUR));
    loop {
        tokio::time::sleep(interval).await;
        run_retention_sweep().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_days_zero_disables() {
        assert_eq!(parse_retention_days(Some("0"), 30), None);
    }

    #[test]
    fn retention_days_falls_back_to_default() {
        assert_eq!(parse_retention_days(None, 30), Some(30));
        assert_eq!(parse_retention_days(Some("not-a-number"), 30), Some(30));
    }

    #[test]
    fn retention_days_honors_override() {
        assert_eq!(parse_retention_days(Some("7"), 30), Some(7));
        assert_eq!(parse_retention_days(Some(" 14 "), 30), Some(14));
    }
}
