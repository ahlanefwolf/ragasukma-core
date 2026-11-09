use super::super::{generate_id, now_secs};
use anyhow::Result;

pub async fn conclude_goal(goal_id: &str, status: &str, conclusion: &str) -> Result<()> {
    conclude_goal_with_verification(goal_id, status, conclusion, None).await
}

/// Writes conclusion text without touching status, phase, or version.
///
/// Exists so callers and tests can represent a goal that carries conclusion text while still
/// being open — the state that made unfinished work look finished. Normal turn endings should go
/// through `conclude_goal` or `conclude_goal_if_verified` instead.
pub async fn update_goal_conclusion(goal_id: &str, conclusion: &str) -> Result<()> {
    let pool = crate::repository::agent::pool().await;
    update_goal_conclusion_with_pool(pool, goal_id, conclusion).await
}

pub async fn update_goal_conclusion_with_pool(
    pool: &sqlx::SqlitePool,
    goal_id: &str,
    conclusion: &str,
) -> Result<()> {
    let now = now_secs();
    let res = sqlx::query("UPDATE goals SET conclusion = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(conclusion)
        .bind(now)
        .bind(goal_id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        anyhow::bail!("Goal '{}' not found for conclusion update", goal_id);
    }
    Ok(())
}

/// Concludes a goal only when a passing verification exists — checked and applied
/// atomically.
///
/// The guard and the write happen in the same transaction. Checking first and updating
/// afterwards would let a concurrent rollback or a racing conclusion slip between the two.
pub async fn conclude_goal_if_verified(
    goal_id: &str,
    status: &str,
    conclusion: &str,
) -> Result<()> {
    let pool = crate::repository::agent::pool().await;
    conclude_goal_if_verified_with_pool(pool, goal_id, status, conclusion).await
}

pub async fn conclude_goal_if_verified_with_pool(
    pool: &sqlx::SqlitePool,
    goal_id: &str,
    status: &str,
    conclusion: &str,
) -> Result<()> {
    let now = now_secs();

    // BEGIN IMMEDIATE takes the write lock up front. A plain deferred BEGIN starts as a
    // reader and only upgrades to a writer at the UPDATE below; under WAL that upgrade
    // fails with SQLITE_BUSY_SNAPSHOT *without* honouring busy_timeout, because waiting
    // would invalidate the snapshot. Taking the write lock at BEGIN puts any contention on
    // the busy_timeout path instead, so concurrent writers wait rather than error out.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let goal_exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM goals WHERE id = ?1")
        .bind(goal_id)
        .fetch_optional(&mut *tx)
        .await?;
    if goal_exists.is_none() {
        tx.rollback().await?;
        anyhow::bail!("Goal '{}' not found to conclude", goal_id);
    }

    let has_passing: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM goal_verifications WHERE goal_id = ?1 AND is_passed = 1 LIMIT 1",
    )
    .bind(goal_id)
    .fetch_optional(&mut *tx)
    .await?;

    if has_passing.is_none() {
        let already_completed: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM goals WHERE id = ?1 AND status = 'completed' LIMIT 1",
        )
        .bind(goal_id)
        .fetch_optional(&mut *tx)
        .await?;

        if already_completed.is_none() {
            tx.rollback().await?;
            anyhow::bail!(
                "Cannot conclude goal {}: No passing verification record found.",
                goal_id
            );
        }
    }

    sqlx::query(
        "UPDATE goals SET status = ?1, phase = 'concluded', conclusion = ?2, version = version + 1, updated_at = ?3 \
         WHERE id = ?4 AND status != 'concluded'",
    )
    .bind(status)
    .bind(conclusion)
    .bind(now)
    .bind(goal_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Concludes a goal and optionally records the final verification in the same transaction.
///
/// A goal must never be marked concluded while its supporting verification row failed to
/// persist, and vice versa — the two are one indivisible fact.
pub async fn conclude_goal_with_verification(
    goal_id: &str,
    status: &str,
    conclusion: &str,
    verification: Option<(&str, &str, bool)>,
) -> Result<()> {
    let pool = crate::repository::agent::pool().await;
    conclude_goal_with_verification_with_pool(pool, goal_id, status, conclusion, verification).await
}

pub async fn conclude_goal_with_verification_with_pool(
    pool: &sqlx::SqlitePool,
    goal_id: &str,
    status: &str,
    conclusion: &str,
    verification: Option<(&str, &str, bool)>,
) -> Result<()> {
    let now = now_secs();

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let res = sqlx::query(
        "UPDATE goals SET status = ?1, phase = 'concluded', conclusion = ?2, version = version + 1, updated_at = ?3 \
         WHERE id = ?4 AND status != 'concluded'",
    )
    .bind(status)
    .bind(conclusion)
    .bind(now)
    .bind(goal_id)
    .execute(&mut *tx)
    .await?;

    if res.rows_affected() == 0 {
        // Distinguish "already concluded" (fine) from "missing goal" (error).
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM goals WHERE id = ?1")
            .bind(goal_id)
            .fetch_optional(&mut *tx)
            .await?;
        if exists.is_none() {
            tx.rollback().await?;
            anyhow::bail!("Goal '{}' not found to conclude", goal_id);
        }
    }

    if let Some((test_command, test_output, is_passed)) = verification {
        let id = generate_id("verif");
        let passed_int = if is_passed { 1 } else { 0 };
        let iteration: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(iteration), 0) + 1 FROM goal_verifications WHERE goal_id = ?1",
        )
        .bind(goal_id)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO goal_verifications (id, goal_id, iteration, test_command, test_output, is_passed, feedback, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
        )
        .bind(&id)
        .bind(goal_id)
        .bind(iteration)
        .bind(test_command)
        .bind(test_output)
        .bind(passed_int)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

// ============================================================================
// EVIDENCE CACHE REPOSITORY
// ============================================================================
