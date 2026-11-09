use super::super::{generate_id, now_secs};
use crate::agent::models::GoalVerification;
use anyhow::Result;

pub async fn record_verification(
    goal_id: &str,
    iteration: i64,
    test_command: &str,
    test_output: &str,
    is_passed: bool,
    feedback: Option<&str>,
) -> Result<GoalVerification> {
    record_verification_with_status(
        goal_id,
        iteration,
        test_command,
        test_output,
        is_passed,
        feedback,
        None,
    )
    .await
}

/// Records a verification iteration and, optionally, moves the goal to a new status —
/// both inside one transaction.
///
/// Verification history and goal state must never disagree: a stored "passed" row that
/// did not move the goal, or a moved goal with no verification record, is silent
/// corruption. Either both writes land or neither does.
pub async fn record_verification_with_status(
    goal_id: &str,
    iteration: i64,
    test_command: &str,
    test_output: &str,
    is_passed: bool,
    feedback: Option<&str>,
    next_status: Option<&str>,
) -> Result<GoalVerification> {
    let pool = crate::repository::agent::pool().await;
    record_verification_with_status_with_pool(
        pool,
        goal_id,
        iteration,
        test_command,
        test_output,
        is_passed,
        feedback,
        next_status,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn record_verification_with_status_with_pool(
    pool: &sqlx::SqlitePool,
    goal_id: &str,
    iteration: i64,
    test_command: &str,
    test_output: &str,
    is_passed: bool,
    feedback: Option<&str>,
    next_status: Option<&str>,
) -> Result<GoalVerification> {
    let now = now_secs();
    let id = generate_id("verif");
    let passed_int = if is_passed { 1 } else { 0 };

    // BEGIN IMMEDIATE: see conclude_goal_if_verified — a deferred BEGIN that later writes
    // can hit SQLITE_BUSY_SNAPSHOT under WAL without retrying, bypassing busy_timeout.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    sqlx::query(
        "INSERT INTO goal_verifications (id, goal_id, iteration, test_command, test_output, is_passed, feedback, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(&id)
    .bind(goal_id)
    .bind(iteration)
    .bind(test_command)
    .bind(test_output)
    .bind(passed_int)
    .bind(feedback)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    if let Some(status) = next_status {
        let res = sqlx::query(
            "UPDATE goals SET status = ?1, version = version + 1, updated_at = ?2 WHERE id = ?3",
        )
        .bind(status)
        .bind(now)
        .bind(goal_id)
        .execute(&mut *tx)
        .await?;

        if res.rows_affected() == 0 {
            // Roll back so no orphan verification row survives a missing goal.
            tx.rollback().await?;
            anyhow::bail!(
                "Goal '{}' not found for verification status update",
                goal_id
            );
        }
    }

    tx.commit().await?;

    Ok(GoalVerification {
        id,
        goal_id: goal_id.to_string(),
        iteration,
        test_command: test_command.to_string(),
        test_output: test_output.to_string(),
        is_passed,
        feedback: feedback.map(|s| s.to_string()),
        created_at: now,
    })
}

pub async fn list_goal_verifications(goal_id: &str) -> Result<Vec<GoalVerification>> {
    let pool = crate::repository::agent::pool().await;
    list_goal_verifications_with_pool(pool, goal_id).await
}

pub async fn list_goal_verifications_with_pool(
    pool: &sqlx::SqlitePool,
    goal_id: &str,
) -> Result<Vec<GoalVerification>> {
    let rows = sqlx::query_as::<_, GoalVerification>(
        "SELECT id, goal_id, iteration, test_command, test_output, is_passed, feedback, created_at \
         FROM goal_verifications WHERE goal_id = ?1 ORDER BY iteration ASC",
    )
    .bind(goal_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
