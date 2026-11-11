-- 0011_dynamic_session_mcp.sql
-- Migration to Dynamic Session MCP & In-Memory Tool Registry
-- Deprecates mcp_server_tools and project_mcp_servers
-- Refactors mcp_servers into a global blueprint catalog (removes is_enabled, adds type and handshake)
-- Introduces session_mcp_servers for session-level state and isolated environment secrets

PRAGMA foreign_keys = OFF;

CREATE TABLE IF NOT EXISTS mcp_servers_new (
    id TEXT PRIMARY KEY NOT NULL,
    type TEXT NOT NULL DEFAULT 'stdio',
    config TEXT NOT NULL,
    handshake INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

INSERT OR IGNORE INTO mcp_servers_new (id, type, config, handshake, created_at, updated_at)
SELECT id, 'stdio', config, 1, created_at, created_at FROM mcp_servers;

DROP TABLE IF EXISTS mcp_servers;
ALTER TABLE mcp_servers_new RENAME TO mcp_servers;

DROP TABLE IF EXISTS mcp_server_tools;
DROP TABLE IF EXISTS project_mcp_servers;

CREATE TABLE IF NOT EXISTS session_mcp_servers (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    server_id TEXT NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    env TEXT NOT NULL DEFAULT '{}',
    is_active INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    PRIMARY KEY (session_id, server_id)
);

CREATE INDEX IF NOT EXISTS idx_session_mcp_servers_session ON session_mcp_servers(session_id);

PRAGMA foreign_keys = ON;
