-- 0005_agent_hardening_and_safety.sql
-- Optimistic concurrency versioning, context health status, and centralized agent tool audit logging.

PRAGMA foreign_keys = ON;

-- 1. Add version and context_status to goals for Optimistic Concurrency Control (OCC) and cache health
ALTER TABLE goals ADD COLUMN version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE goals ADD COLUMN context_status TEXT NOT NULL DEFAULT 'healthy'; -- 'healthy', 'degraded', 'stale'
ALTER TABLE goals ADD COLUMN context_error TEXT;

-- 2. Audit logs for tool execution security, capabilities, and enforcement tracking
CREATE TABLE IF NOT EXISTS agent_audit_logs (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE,
    goal_id TEXT REFERENCES goals(id) ON DELETE SET NULL,
    tool_name TEXT NOT NULL,
    target_path TEXT,
    command TEXT,
    status TEXT NOT NULL,              -- 'allowed', 'denied', 'executed', 'failed', 'timeout'
    reason TEXT,                        -- Detailed reason if denied or error message
    duration_ms INTEGER,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_goal ON agent_audit_logs(goal_id);
CREATE INDEX IF NOT EXISTS idx_audit_session ON agent_audit_logs(session_id);
CREATE INDEX IF NOT EXISTS idx_audit_status ON agent_audit_logs(status);
CREATE INDEX IF NOT EXISTS idx_audit_created_at ON agent_audit_logs(created_at DESC);
