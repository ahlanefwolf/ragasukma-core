-- 0009_drop_redundant_evidence_index.sql
-- Remove `idx_evidence_updated(updated_at DESC)`.
--
-- Verified with `EXPLAIN QUERY PLAN` on a fully migrated database: the planner never selects
-- this index. Every project-scoped evidence read filters on `project_id`, which this index
-- cannot serve as a search key, so queries take `idx_evidence_proj_updated(project_id, updated_at)`
-- instead. The index only cost an extra write on every evidence upsert.
--
-- `idx_evidence_proj_updated` is deliberately kept: it is the one the planner actually chooses
-- for `list_evidences_by_project`.

PRAGMA foreign_keys = ON;

DROP INDEX IF EXISTS idx_evidence_updated;
