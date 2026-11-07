-- 0010_mcp_server_tools.sql
-- Table for storing discovered tools associated with an MCP server.
-- Tools and instructions are formatted in Markdown for semantic consumption by LLM runners.

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS mcp_server_tools (
    server_id TEXT NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    config TEXT NOT NULL,
    PRIMARY KEY (server_id, name)
);

CREATE INDEX IF NOT EXISTS idx_mcp_server_tools_server_id ON mcp_server_tools(server_id);
