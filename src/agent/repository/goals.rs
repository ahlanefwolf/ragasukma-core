use super::{generate_id, now_secs};
use crate::agent::models::{EvidenceCache, Goal, GoalVerification};
use anyhow::Result;

// ============================================================================
// GOAL STORE REPOSITORY
// ============================================================================

#[allow(clippy::too_many_arguments)]
pub async fn create_goal(
    id: &str,
    project_id: &str,
    session_id: Option<&str>,
    parent_goal_id: Option<&str>,
    title: &str,
    demand: &str,
    success_criteria_json: &str,
    constraints_json: &str,
) -> Result<Goal> {
    let pool = super::pool().await;
    let now = now_secs();

    // Check if goal already exists. If it exists and is in_progress, prevent unintended state overwrite.
    let existing = sqlx::query_as::<_, Goal>(
        "SELECT id, project_id, session_id, parent_goal_id, title, demand, success_criteria, constraints, phase, status, target_files, discovery_count, conclusion, created_at, updated_at, version, context_status, context_error \
         FROM goals WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    if let Some(mut existing_goal) = existing {
        if existing_goal.status == "in_progress" {
            // Already actively running; do not clobber running state, return existing
            return Ok(existing_goal);
        }

        // If completed or failed, allow explicit restart with incremented version
        let new_version = existing_goal.version + 1;
        let res = sqlx::query(
            "UPDATE goals SET \
                project_id = ?1, \
                session_id = ?2, \
                parent_goal_id = ?3, \
                title = ?4, \
                demand = ?5, \
                success_criteria = ?6, \
                constraints = ?7, \
                phase = 'discovery', \
                status = 'in_progress', \
                target_files = '[]', \
                discovery_count = 0, \
                conclusion = NULL, \
                context_status = 'healthy', \
                context_error = NULL, \
                version = ?8, \
                updated_at = ?9 \
             WHERE id = ?10 AND version = ?11",
        )
        .bind(project_id)
        .bind(session_id)
        .bind(parent_goal_id)
        .bind(title)
        .bind(demand)
        .bind(success_criteria_json)
        .bind(constraints_json)
        .bind(new_version)
        .bind(now)
        .bind(id)
        .bind(existing_goal.version)
        .execute(pool)
        .await?;

        if res.rows_affected() == 0 {
            anyhow::bail!(
                "Concurrent goal modification detected while updating goal '{}'",
                id
            );
        }

        existing_goal.project_id = project_id.to_string();
        existing_goal.session_id = session_id.map(|s| s.to_string());
        existing_goal.parent_goal_id = parent_goal_id.map(|s| s.to_string());
        existing_goal.title = title.to_string();
        existing_goal.demand = demand.to_string();
        existing_goal.success_criteria = success_criteria_json.to_string();
        existing_goal.constraints = constraints_json.to_string();
        existing_goal.phase = "discovery".to_string();
        existing_goal.status = "in_progress".to_string();
        existing_goal.target_files = "[]".to_string();
        existing_goal.discovery_count = 0;
        existing_goal.conclusion = None;
        existing_goal.version = new_version;
        existing_goal.context_status = "healthy".to_string();
        existing_goal.context_error = None;
        existing_goal.updated_at = now;
        return Ok(existing_goal);
    }

    sqlx::query(
        "INSERT INTO goals (id, project_id, session_id, parent_goal_id, title, demand, success_criteria, constraints, phase, status, target_files, discovery_count, created_at, updated_at, version, context_status, context_error) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'discovery', 'in_progress', '[]', 0, ?9, ?10, 1, 'healthy', NULL)",
    )
    .bind(id)
    .bind(project_id)
    .bind(session_id)
    .bind(parent_goal_id)
    .bind(title)
    .bind(demand)
    .bind(success_criteria_json)
    .bind(constraints_json)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(Goal {
        id: id.to_string(),
        project_id: project_id.to_string(),
        session_id: session_id.map(|s| s.to_string()),
        parent_goal_id: parent_goal_id.map(|s| s.to_string()),
        title: title.to_string(),
        demand: demand.to_string(),
        success_criteria: success_criteria_json.to_string(),
        constraints: constraints_json.to_string(),
        phase: "discovery".to_string(),
        status: "in_progress".to_string(),
        target_files: "[]".to_string(),
        discovery_count: 0,
        conclusion: None,
        created_at: now,
        updated_at: now,
        version: 1,
        context_status: "healthy".to_string(),
        context_error: None,
    })
}

pub async fn get_goal(goal_id: &str) -> Result<Option<Goal>> {
    let pool = super::pool().await;
    let goal = sqlx::query_as::<_, Goal>(
        "SELECT id, project_id, session_id, parent_goal_id, title, demand, success_criteria, constraints, phase, status, target_files, discovery_count, conclusion, created_at, updated_at, version, context_status, context_error \
         FROM goals WHERE id = ?1",
    )
    .bind(goal_id)
    .fetch_optional(pool)
    .await?;
    Ok(goal)
}

pub async fn list_session_goals(session_id: &str) -> Result<Vec<Goal>> {
    let pool = super::pool().await;
    let goals = sqlx::query_as::<_, Goal>(
        "SELECT id, project_id, session_id, parent_goal_id, title, demand, success_criteria, constraints, phase, status, target_files, discovery_count, conclusion, created_at, updated_at, version, context_status, context_error \
         FROM goals WHERE session_id = ?1 ORDER BY created_at DESC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(goals)
}

pub async fn list_project_goals(project_id: &str) -> Result<Vec<Goal>> {
    let pool = super::pool().await;
    let goals = sqlx::query_as::<_, Goal>(
        "SELECT id, project_id, session_id, parent_goal_id, title, demand, success_criteria, constraints, phase, status, target_files, discovery_count, conclusion, created_at, updated_at, version, context_status, context_error \
         FROM goals WHERE project_id = ?1 ORDER BY created_at DESC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    Ok(goals)
}

pub async fn update_goal_phase(goal_id: &str, phase: &str) -> Result<()> {
    let pool = super::pool().await;
    let now = now_secs();
    let res = sqlx::query(
        "UPDATE goals SET phase = ?1, version = version + 1, updated_at = ?2 WHERE id = ?3",
    )
    .bind(phase)
    .bind(now)
    .bind(goal_id)
    .execute(pool)
    .await?;

    if res.rows_affected() == 0 {
        anyhow::bail!("Goal '{}' not found for phase update", goal_id);
    }
    Ok(())
}

pub async fn transition_goal_phase(
    goal_id: &str,
    expected_phase: &str,
    new_phase: &str,
) -> Result<()> {
    let pool = super::pool().await;
    let now = now_secs();
    let res = sqlx::query(
        "UPDATE goals SET phase = ?1, version = version + 1, updated_at = ?2 \
         WHERE id = ?3 AND phase = ?4",
    )
    .bind(new_phase)
    .bind(now)
    .bind(goal_id)
    .bind(expected_phase)
    .execute(pool)
    .await?;

    if res.rows_affected() == 0 {
        anyhow::bail!(
            "Atomic transition rejected: Goal '{}' is not in expected phase '{}' or concurrent modification occurred",
            goal_id,
            expected_phase
        );
    }
    Ok(())
}

pub async fn lock_goal_targets(goal_id: &str, target_files_json: &str) -> Result<()> {
    let pool = super::pool().await;
    let now = now_secs();
    let res = sqlx::query(
        "UPDATE goals SET target_files = ?1, phase = 'action', version = version + 1, updated_at = ?2 \
         WHERE id = ?3 AND phase = 'discovery'",
    )
    .bind(target_files_json)
    .bind(now)
    .bind(goal_id)
    .execute(pool)
    .await?;

    if res.rows_affected() == 0 {
        // If already in action or verification, ensure idempotent lock update without phase violation
        let fallback = sqlx::query(
            "UPDATE goals SET target_files = ?1, version = version + 1, updated_at = ?2 WHERE id = ?3",
        )
        .bind(target_files_json)
        .bind(now)
        .bind(goal_id)
        .execute(pool)
        .await?;

        if fallback.rows_affected() == 0 {
            anyhow::bail!("Goal '{}' not found to lock targets", goal_id);
        }
    }
    Ok(())
}

pub async fn update_goal_status(goal_id: &str, status: &str) -> Result<()> {
    let pool = super::pool().await;
    let now = now_secs();
    let res = sqlx::query(
        "UPDATE goals SET status = ?1, version = version + 1, updated_at = ?2 WHERE id = ?3",
    )
    .bind(status)
    .bind(now)
    .bind(goal_id)
    .execute(pool)
    .await?;

    if res.rows_affected() == 0 {
        anyhow::bail!("Goal '{}' not found for status update", goal_id);
    }
    Ok(())
}

pub async fn update_goal_context_health(
    goal_id: &str,
    context_status: &str,
    context_error: Option<&str>,
) -> Result<()> {
    let pool = super::pool().await;
    let now = now_secs();
    sqlx::query(
        "UPDATE goals SET context_status = ?1, context_error = ?2, version = version + 1, updated_at = ?3 WHERE id = ?4",
    )
    .bind(context_status)
    .bind(context_error)
    .bind(now)
    .bind(goal_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn conclude_goal(goal_id: &str, status: &str, conclusion: &str) -> Result<()> {
    conclude_goal_with_verification(goal_id, status, conclusion, None).await
}

/// Writes conclusion text without touching status, phase, or version.
///
/// Exists so callers and tests can represent a goal that carries conclusion text while still
/// being open — the state that made unfinished work look finished. Normal turn endings should go
/// through `conclude_goal` or `conclude_goal_if_verified` instead.
pub async fn update_goal_conclusion(goal_id: &str, conclusion: &str) -> Result<()> {
    let pool = super::pool().await;
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
    let pool = super::pool().await;
    let now = now_secs();

    let mut tx = pool.begin().await?;

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
    let pool = super::pool().await;
    let now = now_secs();

    let mut tx = pool.begin().await?;

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

#[allow(clippy::too_many_arguments)]
pub async fn save_evidence(
    project_id: &str,
    file_path: &str,
    file_version: &str,
    start_line: Option<i64>,
    end_line: Option<i64>,
    symbol_name: Option<&str>,
    summary: &str,
    raw_snippet: Option<&str>,
) -> Result<EvidenceCache> {
    save_evidence_and_link(
        project_id,
        file_path,
        file_version,
        start_line,
        end_line,
        symbol_name,
        summary,
        raw_snippet,
        None,
    )
    .await
}

/// Persists a workspace slice and, when `goal_link` is given, links it to that goal
/// inside the same transaction.
///
/// Writing the evidence and linking it are one logical step. Doing them separately
/// leaves orphaned rows in `evidence_cache` whenever the link fails — cache rows that no
/// goal can ever read back.
#[allow(clippy::too_many_arguments)]
pub async fn save_evidence_and_link(
    project_id: &str,
    file_path: &str,
    file_version: &str,
    start_line: Option<i64>,
    end_line: Option<i64>,
    symbol_name: Option<&str>,
    summary: &str,
    raw_snippet: Option<&str>,
    goal_link: Option<(&str, f64, bool)>,
) -> Result<EvidenceCache> {
    let pool = super::pool().await;
    let now = now_secs();
    let id = generate_id("ev");

    let mut tx = pool.begin().await?;

    // Upsert on the (project, file, line range) key: re-reading the same slice must
    // refresh the existing row, not append a duplicate. RETURNING gives us the id
    // that actually won, which may be a pre-existing row rather than `id`.
    let (persisted_id, persisted_created_at): (String, i64) = sqlx::query_as(
        "INSERT INTO evidence_cache (id, project_id, file_path, file_version, start_line, end_line, symbol_name, summary, raw_snippet, is_stale, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10, ?11) \
         ON CONFLICT(project_id, file_path, COALESCE(start_line, -1), COALESCE(end_line, -1)) DO UPDATE SET \
           file_version = excluded.file_version, \
           symbol_name  = excluded.symbol_name, \
           summary      = excluded.summary, \
           raw_snippet  = excluded.raw_snippet, \
           is_stale     = 0, \
           updated_at   = excluded.updated_at \
         RETURNING id, created_at",
    )
    .bind(&id)
    .bind(project_id)
    .bind(file_path)
    .bind(file_version)
    .bind(start_line)
    .bind(end_line)
    .bind(symbol_name)
    .bind(summary)
    .bind(raw_snippet)
    .bind(now)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;

    if let Some((goal_id, relevance_score, is_active)) = goal_link {
        let active_int = if is_active { 1 } else { 0 };
        // `MAX(...)` keeps the highest rank a file has ever earned for this goal. Without it,
        // re-reading an already-edited file would overwrite its "modified" score with the weaker
        // "read" score, and the prompt cap would then drop the file the agent actually changed.
        sqlx::query(
            "INSERT INTO goal_evidence (goal_id, evidence_id, relevance_score, is_active) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(goal_id, evidence_id) DO UPDATE SET \
                relevance_score = MAX(goal_evidence.relevance_score, excluded.relevance_score), \
                is_active = excluded.is_active",
        )
        .bind(goal_id)
        .bind(&persisted_id)
        .bind(relevance_score)
        .bind(active_int)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(EvidenceCache {
        id: persisted_id,
        project_id: project_id.to_string(),
        file_path: file_path.to_string(),
        file_version: file_version.to_string(),
        start_line,
        end_line,
        symbol_name: symbol_name.map(|s| s.to_string()),
        summary: summary.to_string(),
        raw_snippet: raw_snippet.map(|s| s.to_string()),
        is_stale: 0,
        created_at: persisted_created_at,
        updated_at: now,
    })
}

pub async fn list_evidences_by_project(project_id: &str) -> Result<Vec<EvidenceCache>> {
    let pool = super::pool().await;
    let items = sqlx::query_as::<_, EvidenceCache>(
        "SELECT id, project_id, file_path, file_version, start_line, end_line, symbol_name, summary, raw_snippet, is_stale, created_at, updated_at \
         FROM evidence_cache WHERE project_id = ?1 ORDER BY updated_at DESC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    Ok(items)
}

pub async fn mark_evidence_stale_by_file(project_id: &str, file_path: &str) -> Result<()> {
    let pool = super::pool().await;
    let now = now_secs();
    sqlx::query("UPDATE evidence_cache SET is_stale = 1, updated_at = ?1 WHERE project_id = ?2 AND file_path = ?3")
        .bind(now)
        .bind(project_id)
        .bind(file_path)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn link_evidence_to_goal(
    goal_id: &str,
    evidence_id: &str,
    relevance_score: f64,
    is_active: bool,
) -> Result<()> {
    let pool = super::pool().await;
    let active_int = if is_active { 1 } else { 0 };

    // The score keeps the better of the two values. Linking says "consider this file for this goal",
    // which never makes an already-relevant file less relevant; overwriting let an inherited 1.0 be
    // replaced by an almost-zero inheritance score, quietly demoting the file the agent had just
    // edited. `save_evidence_and_link` already behaved this way, so both writers now agree.
    sqlx::query(
        "INSERT INTO goal_evidence (goal_id, evidence_id, relevance_score, is_active) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(goal_id, evidence_id) DO UPDATE SET \
            relevance_score = MAX(goal_evidence.relevance_score, excluded.relevance_score), \
            is_active = excluded.is_active",
    )
    .bind(goal_id)
    .bind(evidence_id)
    .bind(relevance_score)
    .bind(active_int)
    .execute(pool)
    .await?;

    Ok(())
}

/// Row shape for `get_active_evidences_for_goal`: an evidence slice plus its goal-local rank.
///
/// Declared flat (rather than `(EvidenceCache, f64)`) because `sqlx::FromRow` is not implemented
/// for nested tuples.
#[derive(Debug, Clone, sqlx::FromRow)]
struct RankedEvidence {
    id: String,
    project_id: String,
    file_path: String,
    file_version: String,
    start_line: Option<i64>,
    end_line: Option<i64>,
    symbol_name: Option<String>,
    summary: String,
    raw_snippet: Option<String>,
    is_stale: i64,
    created_at: i64,
    updated_at: i64,
    relevance_score: f64,
    /// Rank after recency is folded in. Computed per call, never persisted.
    #[sqlx(default)]
    ranked_score: f64,
}

impl RankedEvidence {
    fn into_evidence(self) -> EvidenceCache {
        EvidenceCache {
            id: self.id,
            project_id: self.project_id,
            file_path: self.file_path,
            file_version: self.file_version,
            start_line: self.start_line,
            end_line: self.end_line,
            symbol_name: self.symbol_name,
            summary: self.summary,
            raw_snippet: self.raw_snippet,
            is_stale: self.is_stale,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

pub async fn get_active_evidences_for_goal(goal_id: &str) -> Result<Vec<EvidenceCache>> {
    let pool = super::pool().await;
    // Ordering is applied in Rust below: the relevance score lives on `goal_evidence` while
    // `updated_at` lives on `evidence_cache`, so no single index can serve this ORDER BY and the
    // planner would materialise a TEMP B-TREE. The result set is already narrowed to one goal and
    // is small, so an in-memory sort is cheaper than any extra index.
    //
    // Recency is folded in here rather than stored, so a long session keeps demoting stale context
    // without any background job rewriting rows. `updated_at` is already fetched for ordering.
    let mut rows = sqlx::query_as::<_, RankedEvidence>(
        "SELECT e.id, e.project_id, e.file_path, e.file_version, e.start_line, e.end_line, e.symbol_name, e.summary, e.raw_snippet, e.is_stale, e.created_at, e.updated_at, ge.relevance_score \
         FROM evidence_cache e \
         JOIN goal_evidence ge ON e.id = ge.evidence_id \
         WHERE ge.goal_id = ?1 AND ge.is_active = 1 AND e.is_stale = 0",
    )
    .bind(goal_id)
    .fetch_all(pool)
    .await?;

    let now = now_secs();
    for row in &mut rows {
        row.ranked_score = row.relevance_score
            * crate::agent::evidence_ranking::recency_factor(now - row.updated_at);
    }

    rows.sort_by(|a, b| {
        b.ranked_score
            .total_cmp(&a.ranked_score)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.id.cmp(&b.id))
    });

    Ok(rows
        .into_iter()
        .map(RankedEvidence::into_evidence)
        .collect())
}

// ============================================================================
// GOAL VERIFICATIONS REPOSITORY
// ============================================================================

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
    let pool = super::pool().await;
    let now = now_secs();
    let id = generate_id("verif");
    let passed_int = if is_passed { 1 } else { 0 };

    let mut tx = pool.begin().await?;

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
        is_passed: passed_int,
        feedback: feedback.map(|s| s.to_string()),
        created_at: now,
    })
}

pub async fn list_goal_verifications(goal_id: &str) -> Result<Vec<GoalVerification>> {
    let pool = super::pool().await;
    let rows = sqlx::query_as::<_, GoalVerification>(
        "SELECT id, goal_id, iteration, test_command, test_output, is_passed, feedback, created_at \
         FROM goal_verifications WHERE goal_id = ?1 ORDER BY iteration ASC",
    )
    .bind(goal_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[cfg(test)]
mod transaction_tests {
    use super::*;

    async fn seed_goal() -> (String, String) {
        let project = super::super::create_project("TxTest", "", None, None)
            .await
            .unwrap();
        let session = super::super::create_session(&project.id, "S", None, None, None)
            .await
            .unwrap();
        let goal_id = generate_id("goal_tx");
        create_goal(
            &goal_id,
            &project.id,
            Some(&session.id),
            None,
            "Tx goal",
            "demand",
            "[]",
            "[]",
        )
        .await
        .unwrap();
        (project.id, goal_id)
    }

    #[tokio::test]
    async fn verification_and_status_commit_together() {
        let _guard = crate::agent::repository::testsupport::db_test_lock().await;
        let (_pid, goal_id) = seed_goal().await;

        record_verification_with_status(
            &goal_id,
            1,
            "cargo test",
            "ok",
            true,
            None,
            Some("action"),
        )
        .await
        .unwrap();

        let verifs = list_goal_verifications(&goal_id).await.unwrap();
        assert_eq!(verifs.len(), 1);
        assert_eq!(verifs[0].is_passed, 1);

        let goal = get_goal(&goal_id).await.unwrap().unwrap();
        assert_eq!(goal.status, "action", "status must follow the verification");
    }

    #[tokio::test]
    async fn verification_rolls_back_when_goal_is_missing() {
        let _guard = crate::agent::repository::testsupport::db_test_lock().await;
        // The insert would succeed on its own; the failed status update must undo it.
        let ghost = generate_id("ghost_goal");
        let res = record_verification_with_status(
            &ghost,
            1,
            "cargo test",
            "ok",
            true,
            None,
            Some("action"),
        )
        .await;
        assert!(res.is_err(), "expected failure for missing goal");

        let leftovers = list_goal_verifications(&ghost).await.unwrap();
        assert!(
            leftovers.is_empty(),
            "no verification row may survive a rolled-back transaction"
        );
    }

    #[tokio::test]
    async fn conclude_requires_a_passing_verification() {
        let _guard = crate::agent::repository::testsupport::db_test_lock().await;
        let (_pid, goal_id) = seed_goal().await;

        let res = conclude_goal_if_verified(&goal_id, "completed", "done").await;
        assert!(res.is_err(), "concluding without verification must fail");

        let goal = get_goal(&goal_id).await.unwrap().unwrap();
        assert_ne!(goal.status, "completed");
    }

    #[tokio::test]
    async fn conclude_succeeds_after_passing_verification() {
        let _guard = crate::agent::repository::testsupport::db_test_lock().await;
        let (_pid, goal_id) = seed_goal().await;
        record_verification_with_status(&goal_id, 1, "cargo test", "ok", true, None, None)
            .await
            .unwrap();

        conclude_goal_if_verified(&goal_id, "completed", "done")
            .await
            .unwrap();

        let goal = get_goal(&goal_id).await.unwrap().unwrap();
        assert_eq!(goal.status, "completed");
        assert_eq!(goal.phase, "concluded");
    }

    #[tokio::test]
    async fn evidence_and_link_are_written_together() {
        let _guard = crate::agent::repository::testsupport::db_test_lock().await;
        let (project_id, goal_id) = seed_goal().await;

        let ev = save_evidence_and_link(
            &project_id,
            "src/a.rs",
            "v1",
            Some(1),
            Some(9),
            None,
            "summary",
            None,
            Some((&goal_id, 1.0, true)),
        )
        .await
        .unwrap();

        let linked = get_active_evidences_for_goal(&goal_id).await.unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].id, ev.id);
    }

    /// Re-reading an edited file must not downgrade its rank, otherwise the prompt cap could drop
    /// the very file the agent changed.
    #[tokio::test]
    async fn rereading_an_edited_file_keeps_its_higher_rank() {
        let _guard = crate::agent::repository::testsupport::db_test_lock().await;
        let (project_id, goal_id) = seed_goal().await;
        let modified = crate::agent::evidence_ranking::rank_evidence(
            crate::agent::evidence_ranking::EvidenceOrigin::Modified,
            0,
        );
        let read = crate::agent::evidence_ranking::rank_evidence(
            crate::agent::evidence_ranking::EvidenceOrigin::Read,
            0,
        );

        save_evidence_and_link(
            &project_id,
            "src/a.rs",
            "v1",
            None,
            None,
            None,
            "edited",
            None,
            Some((&goal_id, modified, true)),
        )
        .await
        .unwrap();
        // Agent reads the same file again later in the session.
        save_evidence_and_link(
            &project_id,
            "src/a.rs",
            "v2",
            None,
            None,
            None,
            "read again",
            None,
            Some((&goal_id, read, true)),
        )
        .await
        .unwrap();

        let score: f64 = sqlx::query_scalar(
            "SELECT ge.relevance_score FROM goal_evidence ge              JOIN evidence_cache e ON e.id = ge.evidence_id              WHERE ge.goal_id = ?1 AND e.file_path = 'src/a.rs'",
        )
        .bind(&goal_id)
        .fetch_one(super::super::pool().await)
        .await
        .unwrap();

        assert_eq!(
            score, modified,
            "rank must never regress when a file is re-read"
        );
    }

    /// Recency must actually reach the production read path, not just the pure ranking helper.
    ///
    /// The fixture gives the *older* file a **higher stored score** so the `updated_at` tie-break
    /// cannot produce the expected order by itself: only an applied recency factor can push the
    /// recent-but-lower-ranked file to the top.
    #[tokio::test]
    async fn stale_evidence_is_ordered_below_recently_touched_evidence() {
        let _guard = crate::agent::repository::testsupport::db_test_lock().await;
        let (project_id, goal_id) = seed_goal().await;
        let pool = super::super::pool().await;

        // Old file: stored score 1.0, but aged past the recency floor.
        save_evidence_and_link(
            &project_id,
            "old.rs",
            "v1",
            None,
            None,
            None,
            "s",
            None,
            Some((&goal_id, 1.0, true)),
        )
        .await
        .unwrap();
        // New file: stored score 0.8, freshly observed.
        save_evidence_and_link(
            &project_id,
            "new.rs",
            "v1",
            None,
            None,
            None,
            "s",
            None,
            Some((&goal_id, 0.8, true)),
        )
        .await
        .unwrap();

        // Age the old entry well past the decay window so its factor sits at the floor (0.5).
        sqlx::query("UPDATE evidence_cache SET updated_at = updated_at - 864000 WHERE project_id = ?1 AND file_path = 'old.rs'")
            .bind(&project_id)
            .execute(pool)
            .await
            .unwrap();

        let ordered = get_active_evidences_for_goal(&goal_id).await.unwrap();
        let paths: Vec<&str> = ordered.iter().map(|e| e.file_path.as_str()).collect();
        // Without recency the order would be old.rs (1.0) then new.rs (0.8).
        assert_eq!(
            paths,
            vec!["new.rs", "old.rs"],
            "a recently touched file must outrank an old file with a higher static score"
        );
    }
}
