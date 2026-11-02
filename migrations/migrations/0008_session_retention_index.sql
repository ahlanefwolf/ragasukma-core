-- 0008_session_retention_index.sql
-- Index the session retention sweep.
--
-- `delete_expired_sessions` filters finished sessions by `updated_at`, which after
-- 0007 still required a full table scan. A partial index keeps the sweep O(matches)
-- and stays small because it only tracks sessions that are eligible for expiry.
--
-- NOTE: the retention cutoff is NOT encoded here. It is supplied by the caller
-- (`RAGASUKMA_AGENT_SESSION_RETENTION_DAYS`) so the policy stays runtime-configurable.

PRAGMA foreign_keys = ON;

CREATE INDEX IF NOT EXISTS idx_sessions_updated_at
    ON sessions(updated_at);
