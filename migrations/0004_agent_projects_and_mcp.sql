-- 0004_agent_projects_and_mcp.sql
-- Agent Workspace: Projects, MCP Support, Goals, Evidence Cache, & Verification History
-- Note: Skills are dynamically scanned from `.skills/**/*.md` or `.skill/**/*.md` in workspace projects.

PRAGMA foreign_keys = ON;

-- ============================================================================
-- 1. MCP SERVERS (Registry MCP Configuration - Raw JSON)
-- ============================================================================
CREATE TABLE IF NOT EXISTS mcp_servers (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    config TEXT NOT NULL DEFAULT '{}',           -- Raw JSON configuration (stdio, sse, http, headers, env, args, etc.)
    is_enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);

-- ============================================================================
-- 2. PROJECTS (Workspace-level isolation)
-- ============================================================================
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    root_path TEXT,                              -- Optional: local directory path for workspace filesystem & .skills scanning
    custom_instructions TEXT,                    -- Additional system prompt instructions specific to this project
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_projects_created_at ON projects(created_at DESC);

-- Many-to-Many Relationship: Project <-> MCP Servers
CREATE TABLE IF NOT EXISTS project_mcp_servers (
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    server_id TEXT NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (project_id, server_id)
);

CREATE INDEX IF NOT EXISTS idx_proj_mcp_proj ON project_mcp_servers(project_id);

-- ============================================================================
-- 3. SESSIONS (Task Execution / Thread-specific Context)
-- ============================================================================
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'idle',         -- 'idle', 'running', 'waiting_tool', 'completed', 'failed'
    model TEXT,
    temperature REAL DEFAULT 0.7,
    source_chat_id TEXT,                         -- Optional: reference to chat_history(id) on-demand
    metadata TEXT NOT NULL DEFAULT '{}',         -- Flexible JSON configuration
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_project_id ON sessions(project_id);
CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);
CREATE INDEX IF NOT EXISTS idx_sessions_created_at ON sessions(created_at DESC);

-- ============================================================================
-- 4. MESSAGES (Agent message logs, tool calls, tool responses & reasoning)
-- ============================================================================
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    sequence_num INTEGER NOT NULL,
    role TEXT NOT NULL,                          -- 'user', 'assistant', 'system', 'tool'
    content TEXT,
    thought TEXT,                                -- Chain of thought / reasoning model
    tool_calls TEXT,                             -- JSON Array format MCP/OpenAI/Anthropic tool calls
    tool_call_id TEXT,                           -- Reference to tool_calls when role = 'tool'
    status TEXT NOT NULL DEFAULT 'success',      -- 'streaming', 'success', 'error'
    token_usage_input INTEGER DEFAULT 0,
    token_usage_output INTEGER DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_session_seq ON messages(session_id, sequence_num);
CREATE INDEX IF NOT EXISTS idx_messages_session_created ON messages(session_id, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_messages_role ON messages(role);
CREATE INDEX IF NOT EXISTS idx_messages_tool_call_id ON messages(tool_call_id);

-- ============================================================================
-- 5. GOALS (Goal Store: Demand, Criteria, Status & Phase Transitions)
-- ============================================================================
CREATE TABLE IF NOT EXISTS goals (
    id TEXT PRIMARY KEY NOT NULL,                                     -- e.g. 'G1', 'G2', or UUID
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE,       -- Relates to active thread session
    parent_goal_id TEXT REFERENCES goals(id) ON DELETE SET NULL,     -- Hierarchy / antecedent demand if any
    title TEXT NOT NULL,                                              -- Short title
    demand TEXT NOT NULL,                                             -- Original user demand
    success_criteria TEXT NOT NULL DEFAULT '[]',                      -- JSON Array of criteria
    constraints TEXT NOT NULL DEFAULT '[]',                           -- JSON Array of constraints
    phase TEXT NOT NULL DEFAULT 'discovery',                          -- 'discovery', 'action', 'verification', 'concluded'
    status TEXT NOT NULL DEFAULT 'in_progress',                       -- 'in_progress', 'completed', 'failed'
    target_files TEXT NOT NULL DEFAULT '[]',                          -- JSON Array of locked target files
    discovery_count INTEGER NOT NULL DEFAULT 0,                       -- Discovery calls counter (budget 3-4)
    conclusion TEXT,                                                  -- Summary of why goal criteria met/failed
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_goals_project_id ON goals(project_id);
CREATE INDEX IF NOT EXISTS idx_goals_session_id ON goals(session_id);
CREATE INDEX IF NOT EXISTS idx_goals_parent_id ON goals(parent_goal_id);
CREATE INDEX IF NOT EXISTS idx_goals_phase ON goals(phase);
CREATE INDEX IF NOT EXISTS idx_goals_status ON goals(status);
CREATE INDEX IF NOT EXISTS idx_goals_created_at ON goals(created_at DESC);

-- ============================================================================
-- 6. EVIDENCE CACHE (Workspace / file exploration cache across demands)
-- ============================================================================
CREATE TABLE IF NOT EXISTS evidence_cache (
    id TEXT PRIMARY KEY NOT NULL,                                     -- UUID
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    file_path TEXT NOT NULL,                                          -- Project file path, e.g. 'src/auth/service.rs'
    file_version TEXT NOT NULL,                                       -- SHA256 hash or file mtime when read
    start_line INTEGER,                                               -- Start line range
    end_line INTEGER,                                                 -- End line range
    symbol_name TEXT,                                                 -- Symbol name, e.g. 'validate_token' / class / function
    summary TEXT NOT NULL,                                            -- Summary of file / symbol findings
    raw_snippet TEXT,                                                 -- Relevant code snippet (optional)
    is_stale INTEGER NOT NULL DEFAULT 0,                              -- 1 if the underlying file has changed (requires re-validation)
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_evidence_proj_file ON evidence_cache(project_id, file_path);
CREATE INDEX IF NOT EXISTS idx_evidence_is_stale ON evidence_cache(is_stale);

-- ============================================================================
-- 7. GOAL EVIDENCE (Context Builder: Mapping & relevance filtering per goal)
-- ============================================================================
CREATE TABLE IF NOT EXISTS goal_evidence (
    goal_id TEXT NOT NULL REFERENCES goals(id) ON DELETE CASCADE,
    evidence_id TEXT NOT NULL REFERENCES evidence_cache(id) ON DELETE CASCADE,
    relevance_score REAL DEFAULT 1.0,                                 -- Relevance score for this goal
    is_active INTEGER NOT NULL DEFAULT 1,                             -- 1 = included in LLM prompt, 0 = excluded
    PRIMARY KEY (goal_id, evidence_id)
);

CREATE INDEX IF NOT EXISTS idx_goal_evidence_active ON goal_evidence(goal_id, is_active);

-- ============================================================================
-- 8. GOAL VERIFICATIONS (Verification loop history & test audit results)
-- ============================================================================
CREATE TABLE IF NOT EXISTS goal_verifications (
    id TEXT PRIMARY KEY NOT NULL,                                     -- UUID
    goal_id TEXT NOT NULL REFERENCES goals(id) ON DELETE CASCADE,
    iteration INTEGER NOT NULL DEFAULT 1,                             -- Loop iteration counter (n-th)
    test_command TEXT NOT NULL,                                       -- Command, e.g. 'cargo test test_login'
    test_output TEXT NOT NULL,                                        -- Execution output
    is_passed INTEGER NOT NULL,                                       -- 1 = passed criteria, 0 = failed
    feedback TEXT,                                                    -- Failure evaluation / guidance for subsequent discovery or action phase
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_goal_verif_goal_iter ON goal_verifications(goal_id, iteration);
