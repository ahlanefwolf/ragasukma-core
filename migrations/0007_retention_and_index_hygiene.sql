-- 0007_retention_and_index_hygiene.sql
-- Query-plan hygiene + retention support.
--
-- 1. Covering indexes so "newest first" reads serve their ORDER BY from the index
--    instead of materializing a TEMP B-TREE.
-- 2. Supporting indexes for the retention sweeps implemented in
--    `repository::delete_expired_messages` / `repository::delete_expired_audit_logs`.
--
-- NOTE: retention cutoffs are NOT encoded here. They are passed in by the caller,
-- so the policy stays configurable at runtime rather than hardcoded in SQL.

PRAGMA foreign_keys = ON;

-- ============================================================================
-- 1. Newest-first read paths
-- ============================================================================

-- goals: WHERE session_id = ? ORDER BY created_at DESC
CREATE INDEX IF NOT EXISTS idx_goals_session_created
    ON goals(session_id, created_at DESC);

-- goals: WHERE project_id = ? ORDER BY created_at DESC
CREATE INDEX IF NOT EXISTS idx_goals_project_created
    ON goals(project_id, created_at DESC);

-- goals: WHERE parent_goal_id = ? (existing index retained; add ordering companion)
CREATE INDEX IF NOT EXISTS idx_goals_parent_created
    ON goals(parent_goal_id, created_at DESC);

-- agent_audit_logs: WHERE goal_id = ? ORDER BY created_at DESC, id DESC
CREATE INDEX IF NOT EXISTS idx_audit_goal_created
    ON agent_audit_logs(goal_id, created_at DESC, id DESC);

-- agent_audit_logs: WHERE session_id = ? ORDER BY created_at DESC, id DESC
CREATE INDEX IF NOT EXISTS idx_audit_session_created
    ON agent_audit_logs(session_id, created_at DESC, id DESC);

-- evidence_cache: WHERE project_id = ? ORDER BY updated_at DESC
CREATE INDEX IF NOT EXISTS idx_evidence_proj_updated
    ON evidence_cache(project_id, updated_at DESC);

-- evidence_cache: JOIN goal_evidence ... ORDER BY ge.relevance_score DESC, e.updated_at DESC
-- Goal-filtered evidence lookups repeatedly sort on updated_at; back it per project.
CREATE INDEX IF NOT EXISTS idx_evidence_updated
    ON evidence_cache(updated_at DESC);

-- ============================================================================
-- 2. Retention sweep support
-- ============================================================================

-- messages: DELETE ... WHERE created_at < ?
CREATE INDEX IF NOT EXISTS idx_messages_created
    ON messages(created_at);

-- agent_audit_logs: DELETE ... WHERE created_at < ?
-- (idx_audit_created_at already exists, retained intentionally)

-- ============================================================================
-- 3. Drop the dead index
-- ============================================================================
-- `idx_evidence_proj_file(project_id, file_path)` was never selected by the
-- planner: every project-scoped query also orders by `updated_at`, and the
-- dedup UNIQUE index already covers `project_id` as its leftmost column.
-- It only costs writes on every evidence upsert.
DROP INDEX IF EXISTS idx_evidence_proj_file;
