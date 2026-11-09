use super::super::{generate_id, now_secs};
use crate::agent::models::EvidenceCache;
use anyhow::Result;

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
    let pool = crate::repository::agent::pool().await;
    save_evidence_and_link_with_pool(
        pool,
        project_id,
        file_path,
        file_version,
        start_line,
        end_line,
        symbol_name,
        summary,
        raw_snippet,
        goal_link,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn save_evidence_and_link_with_pool(
    pool: &sqlx::SqlitePool,
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
    let now = now_secs();
    let id = generate_id("ev");

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

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
        is_stale: false,
        created_at: persisted_created_at,
        updated_at: now,
    })
}

pub async fn list_evidences_by_project(project_id: &str) -> Result<Vec<EvidenceCache>> {
    let pool = crate::repository::agent::pool().await;
    list_evidences_by_project_with_pool(pool, project_id).await
}

pub async fn list_evidences_by_project_with_pool(
    pool: &sqlx::SqlitePool,
    project_id: &str,
) -> Result<Vec<EvidenceCache>> {
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
    let pool = crate::repository::agent::pool().await;
    mark_evidence_stale_by_file_with_pool(pool, project_id, file_path).await
}

pub async fn mark_evidence_stale_by_file_with_pool(
    pool: &sqlx::SqlitePool,
    project_id: &str,
    file_path: &str,
) -> Result<()> {
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
    let pool = crate::repository::agent::pool().await;
    link_evidence_to_goal_with_pool(pool, goal_id, evidence_id, relevance_score, is_active).await
}

pub async fn link_evidence_to_goal_with_pool(
    pool: &sqlx::SqlitePool,
    goal_id: &str,
    evidence_id: &str,
    relevance_score: f64,
    is_active: bool,
) -> Result<()> {
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
    is_stale: bool,
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
    let pool = crate::repository::agent::pool().await;
    get_active_evidences_for_goal_with_pool(pool, goal_id).await
}

pub async fn get_active_evidences_for_goal_with_pool(
    pool: &sqlx::SqlitePool,
    goal_id: &str,
) -> Result<Vec<EvidenceCache>> {
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
