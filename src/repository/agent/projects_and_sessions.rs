use super::{generate_id, now_secs};
use crate::agent::models::{McpServer, McpServerTool, Project, Session, SessionMcpServer};
use anyhow::Result;

// ============================================================================
// PROJECTS REPOSITORY
// ============================================================================

pub async fn list_projects() -> Result<Vec<Project>> {
    let pool = super::pool().await;
    list_projects_with_pool(pool).await
}

pub async fn list_projects_with_pool(pool: &sqlx::SqlitePool) -> Result<Vec<Project>> {
    let projects = sqlx::query_as::<_, Project>(
        "SELECT id, name, description, root_path, custom_instructions, created_at, updated_at \
         FROM projects ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(projects)
}

pub async fn get_project(id: &str) -> Result<Option<Project>> {
    let pool = super::pool().await;
    get_project_with_pool(pool, id).await
}

pub async fn get_project_with_pool(pool: &sqlx::SqlitePool, id: &str) -> Result<Option<Project>> {
    let project = sqlx::query_as::<_, Project>(
        "SELECT id, name, description, root_path, custom_instructions, created_at, updated_at \
         FROM projects WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(project)
}

pub async fn create_project(
    name: &str,
    description: &str,
    root_path: Option<&str>,
    custom_instructions: Option<&str>,
) -> Result<Project> {
    let pool = super::pool().await;
    create_project_with_pool(pool, name, description, root_path, custom_instructions).await
}

pub async fn create_project_with_pool(
    pool: &sqlx::SqlitePool,
    name: &str,
    description: &str,
    root_path: Option<&str>,
    custom_instructions: Option<&str>,
) -> Result<Project> {
    let id = generate_id("proj");
    let now = now_secs();

    sqlx::query(
        "INSERT INTO projects (id, name, description, root_path, custom_instructions, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(&id)
    .bind(name)
    .bind(description)
    .bind(root_path)
    .bind(custom_instructions)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(Project {
        id,
        name: name.to_string(),
        description: description.to_string(),
        root_path: root_path.map(|s| s.to_string()),
        custom_instructions: custom_instructions.map(|s| s.to_string()),
        created_at: now,
        updated_at: now,
    })
}

pub async fn update_project(
    id: &str,
    name: &str,
    description: &str,
    root_path: Option<&str>,
    custom_instructions: Option<&str>,
) -> Result<()> {
    let pool = super::pool().await;
    update_project_with_pool(pool, id, name, description, root_path, custom_instructions).await
}

pub async fn update_project_with_pool(
    pool: &sqlx::SqlitePool,
    id: &str,
    name: &str,
    description: &str,
    root_path: Option<&str>,
    custom_instructions: Option<&str>,
) -> Result<()> {
    let now = now_secs();

    sqlx::query(
        "UPDATE projects SET name = ?1, description = ?2, root_path = ?3, custom_instructions = ?4, updated_at = ?5 \
         WHERE id = ?6",
    )
    .bind(name)
    .bind(description)
    .bind(root_path)
    .bind(custom_instructions)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn delete_project(id: &str) -> Result<()> {
    let pool = super::pool().await;
    delete_project_with_pool(pool, id).await
}

pub async fn delete_project_with_pool(pool: &sqlx::SqlitePool, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM projects WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ============================================================================
// SESSIONS REPOSITORY
// ============================================================================

pub async fn list_sessions(project_id: &str) -> Result<Vec<Session>> {
    let pool = super::pool().await;
    list_sessions_with_pool(pool, project_id).await
}

pub async fn list_sessions_with_pool(
    pool: &sqlx::SqlitePool,
    project_id: &str,
) -> Result<Vec<Session>> {
    let sessions = sqlx::query_as::<_, Session>(
        "SELECT s.id, s.project_id, s.title, s.status, s.model, s.temperature, s.source_chat_id, s.metadata, s.created_at, s.updated_at, \
         COALESCE(SUM(m.token_usage_input), 0) AS total_tokens_input, \
         COALESCE(SUM(m.token_usage_output), 0) AS total_tokens_output, \
         COALESCE(SUM(CASE WHEN m.role IN ('assistant', 'tool') THEN 1 ELSE 0 END), 0) AS total_requests \
         FROM sessions s \
         LEFT JOIN messages m ON m.session_id = s.id \
         WHERE s.project_id = ?1 \
         GROUP BY s.id \
         ORDER BY s.created_at DESC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    Ok(sessions)
}

pub async fn get_session(id: &str) -> Result<Option<Session>> {
    let pool = super::pool().await;
    get_session_with_pool(pool, id).await
}

pub async fn get_session_with_pool(pool: &sqlx::SqlitePool, id: &str) -> Result<Option<Session>> {
    let session = sqlx::query_as::<_, Session>(
        "SELECT s.id, s.project_id, s.title, s.status, s.model, s.temperature, s.source_chat_id, s.metadata, s.created_at, s.updated_at, \
         COALESCE(SUM(m.token_usage_input), 0) AS total_tokens_input, \
         COALESCE(SUM(m.token_usage_output), 0) AS total_tokens_output, \
         COALESCE(SUM(CASE WHEN m.role IN ('assistant', 'tool') THEN 1 ELSE 0 END), 0) AS total_requests \
         FROM sessions s \
         LEFT JOIN messages m ON m.session_id = s.id \
         WHERE s.id = ?1 \
         GROUP BY s.id",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(session)
}

pub async fn create_session(
    project_id: &str,
    title: &str,
    model: Option<&str>,
    temperature: Option<f64>,
    source_chat_id: Option<&str>,
) -> Result<Session> {
    let pool = super::pool().await;
    create_session_with_pool(pool, project_id, title, model, temperature, source_chat_id).await
}

pub async fn create_session_with_pool(
    pool: &sqlx::SqlitePool,
    project_id: &str,
    title: &str,
    model: Option<&str>,
    temperature: Option<f64>,
    source_chat_id: Option<&str>,
) -> Result<Session> {
    let id = generate_id("sess");
    let now = now_secs();
    let temp = temperature.unwrap_or(0.1);

    sqlx::query(
        "INSERT INTO sessions (id, project_id, title, status, model, temperature, source_chat_id, metadata, created_at, updated_at) \
         VALUES (?1, ?2, ?3, 'idle', ?4, ?5, ?6, '{}', ?7, ?8)",
    )
    .bind(&id)
    .bind(project_id)
    .bind(title)
    .bind(model)
    .bind(temp)
    .bind(source_chat_id)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(Session {
        id,
        project_id: project_id.to_string(),
        title: title.to_string(),
        status: "idle".to_string(),
        model: model.map(|s| s.to_string()),
        temperature: Some(temp),
        source_chat_id: source_chat_id.map(|s| s.to_string()),
        metadata: "{}".to_string(),
        created_at: now,
        updated_at: now,
        total_tokens_input: Some(0),
        total_tokens_output: Some(0),
        total_requests: Some(0),
    })
}

pub async fn get_session_status(id: &str) -> Result<Option<String>> {
    let pool = super::pool().await;
    get_session_status_with_pool(pool, id).await
}

pub async fn get_session_status_with_pool(
    pool: &sqlx::SqlitePool,
    id: &str,
) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT status FROM sessions WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

pub async fn update_session_status(id: &str, status: &str) -> Result<()> {
    let pool = super::pool().await;
    update_session_status_with_pool(pool, id, status).await
}

pub async fn update_session_status_with_pool(
    pool: &sqlx::SqlitePool,
    id: &str,
    status: &str,
) -> Result<()> {
    let now = now_secs();
    sqlx::query("UPDATE sessions SET status = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(status)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_session_model(id: &str, model: &str) -> Result<()> {
    let pool = super::pool().await;
    update_session_model_with_pool(pool, id, model).await
}

pub async fn update_session_model_with_pool(
    pool: &sqlx::SqlitePool,
    id: &str,
    model: &str,
) -> Result<()> {
    let now = now_secs();
    sqlx::query("UPDATE sessions SET model = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(model)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Resets any sessions left in "running" state by an earlier process crash or abrupt kill.
///
/// Returns the number of sessions reset to "idle".
pub async fn reset_stale_running_sessions() -> Result<usize> {
    let pool = super::pool().await;
    reset_stale_running_sessions_with_pool(pool).await
}

pub async fn reset_stale_running_sessions_with_pool(pool: &sqlx::SqlitePool) -> Result<usize> {
    let stale: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE status = 'running'")
        .fetch_one(pool)
        .await?;
    if stale == 0 {
        return Ok(0);
    }

    sqlx::query("UPDATE sessions SET status = 'idle' WHERE status = 'running'")
        .execute(pool)
        .await?;
    tracing::warn!(
        "[Agent Turn] Reset {} session(s) left 'running' by a previous process",
        stale
    );
    Ok(stale as usize)
}

pub async fn delete_session(id: &str) -> Result<()> {
    let pool = super::pool().await;
    delete_session_with_pool(pool, id).await
}

pub async fn delete_session_with_pool(pool: &sqlx::SqlitePool, id: &str) -> Result<()> {
    // 1. Fetch all (server_id, pid) registered for this session before deleting rows
    let mcp_rows: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT server_id, pid FROM session_mcp_servers WHERE session_id = ?1",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    // 2. Stop in-memory MCP runtime and persistent stdio processes for this session
    crate::agent::mcp::SESSION_MCP_REGISTRY
        .clear_session(id)
        .await;

    // 3. Terminate any recorded OS PID that may still be alive
    for (_server_id, pid_opt) in mcp_rows {
        if let Some(pid) = pid_opt {
            crate::agent::mcp::kill_os_pid(pid).await;
        }
    }

    // Reclaim in-memory file snapshots, turn generation counters, and event broadcasters
    crate::agent::snapshot::release_session(id);
    crate::agent::executor::forget_session_generation(id);
    crate::agent::events::cleanup_session(id);

    // 4. All database deletions run inside a single transaction
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Clean up session MCP server associations
    sqlx::query("DELETE FROM session_mcp_servers WHERE session_id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    // Clean up goal verifications and goal evidence for any goal in this session
    sqlx::query("DELETE FROM goal_verifications WHERE goal_id IN (SELECT id FROM goals WHERE session_id = ?1)")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "DELETE FROM goal_evidence WHERE goal_id IN (SELECT id FROM goals WHERE session_id = ?1)",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    // Clean up goals belonging to this session
    sqlx::query("DELETE FROM goals WHERE session_id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    // Purge orphaned evidence_cache entries no longer attached to any active goal
    sqlx::query(
        "DELETE FROM evidence_cache WHERE id NOT IN (SELECT evidence_id FROM goal_evidence)",
    )
    .execute(&mut *tx)
    .await?;
    // Delete the session itself (messages cascade via foreign key)
    sqlx::query("DELETE FROM sessions WHERE id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

/// Deletes sessions that have been idle longer than `cutoff_epoch_secs`.
///
/// Unlike `delete_session`, this removes rows directly: stale-session cleanup runs on a
/// schedule and must stay cheap. Foreign keys cascade `messages`, and goals/evidence are
/// swept separately, so nothing is orphaned.
///
/// Returns the number of sessions removed.
pub async fn delete_expired_sessions(cutoff_epoch_secs: i64) -> Result<u64> {
    let pool = super::pool().await;
    delete_expired_sessions_with_pool(pool, cutoff_epoch_secs).await
}

pub async fn delete_expired_sessions_with_pool(
    pool: &sqlx::SqlitePool,
    cutoff_epoch_secs: i64,
) -> Result<u64> {
    // Snapshot the ids before deleting so the matching in-memory registries can be reclaimed
    // afterwards. The sweep otherwise leaks one `agent::events` entry (broadcaster + channel)
    // per expired session, since this path bypasses `delete_session`'s cleanup.
    let expired_ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM sessions WHERE updated_at < ?1")
            .bind(cutoff_epoch_secs)
            .fetch_all(pool)
            .await?;

    let expired_mcp_rows: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT server_id, pid FROM session_mcp_servers WHERE session_id IN (SELECT id FROM sessions WHERE updated_at < ?1)",
    )
    .bind(cutoff_epoch_secs)
    .fetch_all(pool)
    .await?;

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Remove session MCP server records belonging to expired sessions.
    sqlx::query(
        "DELETE FROM session_mcp_servers WHERE session_id IN (SELECT id FROM sessions WHERE updated_at < ?1)",
    )
    .bind(cutoff_epoch_secs)
    .execute(&mut *tx)
    .await?;

    // Remove goals (and their dependent rows) belonging to expired sessions first.
    sqlx::query(
        "DELETE FROM goal_verifications WHERE goal_id IN (SELECT id FROM goals WHERE session_id IN (SELECT id FROM sessions WHERE updated_at < ?1))",
    )
    .bind(cutoff_epoch_secs)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM goal_evidence WHERE goal_id IN (SELECT id FROM goals WHERE session_id IN (SELECT id FROM sessions WHERE updated_at < ?1))",
    )
    .bind(cutoff_epoch_secs)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM goals WHERE session_id IN (SELECT id FROM sessions WHERE updated_at < ?1)",
    )
    .bind(cutoff_epoch_secs)
    .execute(&mut *tx)
    .await?;

    let res = sqlx::query("DELETE FROM sessions WHERE updated_at < ?1")
        .bind(cutoff_epoch_secs)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    // Reclaim per-session in-memory registries and MCP processes now the rows are gone.
    for id in &expired_ids {
        crate::agent::mcp::SESSION_MCP_REGISTRY
            .clear_session(id)
            .await;
        crate::agent::snapshot::release_session(id);
        crate::agent::executor::forget_session_generation(id);
        crate::agent::events::cleanup_session(id);
    }

    for (_server_id, pid_opt) in expired_mcp_rows {
        if let Some(pid) = pid_opt {
            crate::agent::mcp::kill_os_pid(pid).await;
        }
    }

    Ok(res.rows_affected())
}

/// Deletes projects that have no sessions left at all.
///
/// A project with zero sessions carries no recoverable work: every goal, evidence row
/// and message hangs off a session. These accumulate from exploratory projects and from
/// earlier test runs that wrote to the live database, so they are swept on a schedule.
///
/// Returns the number of projects removed.
pub async fn delete_orphan_projects() -> Result<u64> {
    let pool = super::pool().await;
    delete_orphan_projects_with_pool(pool).await
}

pub async fn delete_orphan_projects_with_pool(pool: &sqlx::SqlitePool) -> Result<u64> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Evidence is project-scoped rather than session-scoped, so clear it explicitly.
    sqlx::query(
        "DELETE FROM evidence_cache WHERE project_id IN (SELECT p.id FROM projects p WHERE NOT EXISTS (SELECT 1 FROM sessions s WHERE s.project_id = p.id))",
    )
    .execute(&mut *tx)
    .await?;

    let res = sqlx::query(
        "DELETE FROM projects WHERE NOT EXISTS (SELECT 1 FROM sessions s WHERE s.project_id = projects.id)",
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(res.rows_affected())
}

// ============================================================================
// MCP SERVERS REPOSITORY
// ============================================================================

pub async fn list_mcp_servers() -> Result<Vec<McpServer>> {
    let pool = super::pool().await;
    list_mcp_servers_with_pool(pool).await
}

pub async fn list_mcp_servers_with_pool(pool: &sqlx::SqlitePool) -> Result<Vec<McpServer>> {
    let servers = sqlx::query_as::<_, McpServer>(
        "SELECT id, id as name, type, config, handshake, created_at, updated_at \
         FROM mcp_servers \
         ORDER BY id ASC",
    )
    .fetch_all(pool)
    .await?;

    Ok(servers)
}

pub async fn get_mcp_server(id: &str) -> Result<Option<McpServer>> {
    let pool = super::pool().await;
    get_mcp_server_with_pool(pool, id).await
}

pub async fn get_mcp_server_with_pool(
    pool: &sqlx::SqlitePool,
    id: &str,
) -> Result<Option<McpServer>> {
    let server = sqlx::query_as::<_, McpServer>(
        "SELECT id, id as name, type, config, handshake, created_at, updated_at \
         FROM mcp_servers \
         WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(server)
}

pub async fn create_or_update_mcp_server(
    id: &str,
    name: &str,
    config: &str,
    is_enabled: bool,
) -> Result<()> {
    let pool = super::pool().await;
    create_or_update_mcp_server_with_pool(pool, id, name, config, is_enabled).await
}

pub async fn create_or_update_mcp_server_with_pool(
    pool: &sqlx::SqlitePool,
    id: &str,
    _name: &str,
    config: &str,
    _is_enabled: bool,
) -> Result<()> {
    let now = now_secs();
    let r#type = if config.contains("\"url\"") || config.contains("'url'") {
        "http"
    } else {
        "stdio"
    };

    sqlx::query(
        "INSERT INTO mcp_servers (id, type, config, handshake, created_at, updated_at) \
         VALUES (?1, ?2, ?3, 1, ?4, ?4) \
         ON CONFLICT(id) DO UPDATE SET \
            type = excluded.type, \
            config = excluded.config, \
            updated_at = excluded.updated_at",
    )
    .bind(id)
    .bind(r#type)
    .bind(config)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn delete_mcp_server(id: &str) -> Result<()> {
    let pool = super::pool().await;
    delete_mcp_server_with_pool(pool, id).await
}

pub async fn delete_mcp_server_with_pool(pool: &sqlx::SqlitePool, id: &str) -> Result<()> {
    // 1. Fetch all (session_id, pid) pairs registered for this server across all sessions
    let session_rows: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT session_id, pid FROM session_mcp_servers WHERE server_id = ?1",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    // 2. Shutdown any active stdio process and clear SESSION_MCP_REGISTRY across all sessions
    crate::agent::mcp::shutdown_server_across_all_sessions(id).await;

    // Terminate any recorded OS PID that may still be alive
    for (_session_id, pid_opt) in session_rows {
        if let Some(pid) = pid_opt {
            crate::agent::mcp::kill_os_pid(pid).await;
        }
    }

    // 3. Explicitly remove from session_mcp_servers, mcp_server_tools (if present), and mcp_servers
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    sqlx::query("DELETE FROM session_mcp_servers WHERE server_id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    let _ = sqlx::query("DELETE FROM mcp_server_tools WHERE server_id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await;

    sqlx::query("DELETE FROM mcp_servers WHERE id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

// ============================================================================
// SESSION MCP SERVERS (DYNAMIC PER-SESSION & ENV)
// ============================================================================

/// Gracefully shuts down all MCP servers across all sessions:
/// 1. Terminates all active child processes in `STDIO_POOL` (`shutdown_all_stdio_servers()`).
/// 2. Queries all rows in `session_mcp_servers WHERE pid IS NOT NULL`, kills each OS `pid`,
///    clears `SESSION_MCP_REGISTRY`, and resets `pid = NULL` in the database.
pub async fn shutdown_all_mcp_servers() -> Result<()> {
    let pool = super::pool().await;
    shutdown_all_mcp_servers_with_pool(pool).await
}

pub async fn shutdown_all_mcp_servers_with_pool(pool: &sqlx::SqlitePool) -> Result<()> {
    crate::agent::mcp::shutdown_all_stdio_servers().await;

    let active_rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT session_id, server_id, pid FROM session_mcp_servers WHERE pid IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;

    for (session_id, server_id, pid) in active_rows {
        crate::agent::mcp::kill_os_pid(pid).await;
        crate::agent::mcp::SESSION_MCP_REGISTRY
            .remove_server(&session_id, &server_id)
            .await;
    }

    sqlx::query("UPDATE session_mcp_servers SET pid = NULL")
        .execute(pool)
        .await?;

    Ok(())
}

/// Sweeps and terminates any orphaned MCP child processes left behind in `session_mcp_servers`
/// from a previous crashed or force-closed application run, and resets all `pid` columns to `NULL`.
pub async fn cleanup_orphaned_mcp_processes_on_startup() -> Result<usize> {
    let pool = super::pool().await;
    cleanup_orphaned_mcp_processes_on_startup_with_pool(pool).await
}

pub async fn cleanup_orphaned_mcp_processes_on_startup_with_pool(
    pool: &sqlx::SqlitePool,
) -> Result<usize> {
    let orphaned_rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT session_id, server_id, pid FROM session_mcp_servers WHERE pid IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;

    let count = orphaned_rows.len();
    for (session_id, server_id, pid) in orphaned_rows {
        crate::agent::mcp::kill_os_pid(pid).await;
        crate::agent::mcp::SESSION_MCP_REGISTRY
            .remove_server(&session_id, &server_id)
            .await;
    }

    sqlx::query("UPDATE session_mcp_servers SET pid = NULL")
        .execute(pool)
        .await?;

    Ok(count)
}

/// Synchronizes in-memory `STDIO_POOL` and OS process liveness with `session_mcp_servers.pid`
/// for a given session. If a process has exited on its own, cleans it from the pool and resets `pid = NULL`.
pub async fn sync_session_mcp_runtime(session_id: &str) -> Result<()> {
    let pool = super::pool().await;
    sync_session_mcp_runtime_with_pool(pool, session_id).await
}

pub async fn sync_session_mcp_runtime_with_pool(
    pool: &sqlx::SqlitePool,
    session_id: &str,
) -> Result<()> {
    // 1. Sweep any dead processes in STDIO_POOL
    let dead_pairs = crate::agent::mcp::sweep_dead_stdio_servers().await;
    for (dead_sid, dead_srv) in dead_pairs {
        let _ = update_session_mcp_server_pid_with_pool(pool, &dead_sid, &dead_srv, None).await;
    }

    // 2. Reconcile recorded rows for this session against STDIO_POOL and OS PID liveness
    let rows: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT server_id, pid FROM session_mcp_servers WHERE session_id = ?1",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    for (server_id, pid_opt) in rows {
        if let Some(live_pid) =
            crate::agent::mcp::check_stdio_server_alive(session_id, &server_id).await
        {
            if pid_opt != Some(i64::from(live_pid)) {
                update_session_mcp_server_pid_with_pool(
                    pool,
                    session_id,
                    &server_id,
                    Some(i64::from(live_pid)),
                )
                .await?;
            }
        } else if let Some(pid) = pid_opt
            && !crate::agent::mcp::is_os_pid_alive(pid).await
        {
            update_session_mcp_server_pid_with_pool(pool, session_id, &server_id, None).await?;
        }
    }

    Ok(())
}

pub async fn list_session_mcp_servers(session_id: &str) -> Result<Vec<SessionMcpServer>> {
    let pool = super::pool().await;
    list_session_mcp_servers_with_pool(pool, session_id).await
}

pub async fn list_session_mcp_servers_with_pool(
    pool: &sqlx::SqlitePool,
    session_id: &str,
) -> Result<Vec<SessionMcpServer>> {
    let _ = sync_session_mcp_runtime_with_pool(pool, session_id).await;

    let rows = sqlx::query_as::<_, SessionMcpServer>(
        "SELECT session_id, server_id, env, is_active, pid, created_at, updated_at \
         FROM session_mcp_servers \
         WHERE session_id = ?1 \
         ORDER BY server_id ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

pub async fn get_session_active_mcp_servers(
    session_id: &str,
) -> Result<Vec<(McpServer, SessionMcpServer)>> {
    let pool = super::pool().await;
    get_session_active_mcp_servers_with_pool(pool, session_id).await
}

pub async fn get_session_active_mcp_servers_with_pool(
    pool: &sqlx::SqlitePool,
    session_id: &str,
) -> Result<Vec<(McpServer, SessionMcpServer)>> {
    let _ = sync_session_mcp_runtime_with_pool(pool, session_id).await;

    let rows = sqlx::query_as::<_, McpServer>(
        "SELECT s.id, s.type, s.config, s.handshake, s.created_at, s.updated_at \
         FROM session_mcp_servers sms \
         JOIN mcp_servers s ON s.id = sms.server_id \
         WHERE sms.session_id = ?1 AND sms.is_active = 1 \
         ORDER BY s.id ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    let session_rows = sqlx::query_as::<_, SessionMcpServer>(
        "SELECT session_id, server_id, env, is_active, pid, created_at, updated_at \
         FROM session_mcp_servers \
         WHERE session_id = ?1 AND is_active = 1 \
         ORDER BY server_id ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    let mut result = Vec::new();
    for s in rows {
        if let Some(sms) = session_rows.iter().find(|item| item.server_id == s.id) {
            result.push((s, sms.clone()));
        }
    }

    Ok(result)
}

async fn stop_and_clear_session_mcp_runtime(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    server_id: &str,
) -> Result<()> {
    let existing_pid: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT pid FROM session_mcp_servers WHERE session_id = ?1 AND server_id = ?2",
    )
    .bind(session_id)
    .bind(server_id)
    .fetch_optional(pool)
    .await?;

    crate::agent::mcp::shutdown_session_stdio_server(session_id, server_id).await;

    if let Some(Some(pid)) = existing_pid {
        crate::agent::mcp::kill_os_pid(pid).await;
    }

    crate::agent::executor::state::deactivate_session_extension(session_id, server_id);
    Ok(())
}

pub async fn upsert_session_mcp_server(
    session_id: &str,
    server_id: &str,
    env: &str,
    is_active: bool,
) -> Result<()> {
    let pool = super::pool().await;
    upsert_session_mcp_server_with_pool(pool, session_id, server_id, env, is_active).await
}

pub async fn upsert_session_mcp_server_with_pool(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    server_id: &str,
    env: &str,
    is_active: bool,
) -> Result<()> {
    if !is_active {
        stop_and_clear_session_mcp_runtime(pool, session_id, server_id).await?;
    }

    let now = now_secs();
    let active_int = if is_active { 1 } else { 0 };

    if is_active {
        sqlx::query(
            "INSERT INTO session_mcp_servers (session_id, server_id, env, is_active, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?5) \
             ON CONFLICT(session_id, server_id) DO UPDATE SET \
                env = excluded.env, \
                is_active = excluded.is_active, \
                updated_at = excluded.updated_at",
        )
        .bind(session_id)
        .bind(server_id)
        .bind(env)
        .bind(active_int)
        .bind(now)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO session_mcp_servers (session_id, server_id, env, is_active, pid, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5) \
             ON CONFLICT(session_id, server_id) DO UPDATE SET \
                env = excluded.env, \
                is_active = 0, \
                pid = NULL, \
                updated_at = excluded.updated_at",
        )
        .bind(session_id)
        .bind(server_id)
        .bind(env)
        .bind(active_int)
        .bind(now)
        .execute(pool)
        .await?;
    }

    Ok(())
}

pub async fn set_session_mcp_server_active(
    session_id: &str,
    server_id: &str,
    is_active: bool,
) -> Result<()> {
    let pool = super::pool().await;
    set_session_mcp_server_active_with_pool(pool, session_id, server_id, is_active).await
}

pub async fn set_session_mcp_server_active_with_pool(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    server_id: &str,
    is_active: bool,
) -> Result<()> {
    let now = now_secs();
    let active_int = if is_active { 1 } else { 0 };

    if !is_active {
        stop_and_clear_session_mcp_runtime(pool, session_id, server_id).await?;
        sqlx::query(
            "UPDATE session_mcp_servers SET is_active = 0, pid = NULL, updated_at = ?1 \
             WHERE session_id = ?2 AND server_id = ?3",
        )
        .bind(now)
        .bind(session_id)
        .bind(server_id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE session_mcp_servers SET is_active = ?1, updated_at = ?2 \
             WHERE session_id = ?3 AND server_id = ?4",
        )
        .bind(active_int)
        .bind(now)
        .bind(session_id)
        .bind(server_id)
        .execute(pool)
        .await?;
    }

    Ok(())
}

pub async fn remove_session_mcp_server(session_id: &str, server_id: &str) -> Result<()> {
    let pool = super::pool().await;
    remove_session_mcp_server_with_pool(pool, session_id, server_id).await
}

pub async fn remove_session_mcp_server_with_pool(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    server_id: &str,
) -> Result<()> {
    delete_session_mcp_server_with_pool(pool, session_id, server_id).await
}

pub async fn delete_session_mcp_server(session_id: &str, server_id: &str) -> Result<()> {
    let pool = super::pool().await;
    delete_session_mcp_server_with_pool(pool, session_id, server_id).await
}

pub async fn delete_session_mcp_server_with_pool(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    server_id: &str,
) -> Result<()> {
    stop_and_clear_session_mcp_runtime(pool, session_id, server_id).await?;

    sqlx::query("UPDATE session_mcp_servers SET pid = NULL WHERE session_id = ?1 AND server_id = ?2")
        .bind(session_id)
        .bind(server_id)
        .execute(pool)
        .await?;

    sqlx::query("DELETE FROM session_mcp_servers WHERE session_id = ?1 AND server_id = ?2")
        .bind(session_id)
        .bind(server_id)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn update_session_mcp_server_pid(
    session_id: &str,
    server_id: &str,
    pid: Option<i64>,
) -> Result<()> {
    let pool = super::pool().await;
    update_session_mcp_server_pid_with_pool(pool, session_id, server_id, pid).await
}

pub async fn update_session_mcp_server_pid_with_pool(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    server_id: &str,
    pid: Option<i64>,
) -> Result<()> {
    let now = now_secs();
    sqlx::query(
        "UPDATE session_mcp_servers SET pid = ?1, updated_at = ?2 \
         WHERE session_id = ?3 AND server_id = ?4",
    )
    .bind(pid)
    .bind(now)
    .bind(session_id)
    .bind(server_id)
    .execute(pool)
    .await?;

    Ok(())
}

// Deprecated compatibility stubs for tools table
pub async fn list_mcp_server_tools(_server_id: &str) -> Result<Vec<McpServerTool>> {
    Ok(Vec::new())
}

pub async fn list_mcp_server_tools_with_pool(_pool: &sqlx::SqlitePool, _server_id: &str) -> Result<Vec<McpServerTool>> {
    Ok(Vec::new())
}

pub async fn list_all_mcp_server_tools() -> Result<Vec<McpServerTool>> {
    Ok(Vec::new())
}

pub async fn list_all_mcp_server_tools_with_pool(_pool: &sqlx::SqlitePool) -> Result<Vec<McpServerTool>> {
    Ok(Vec::new())
}

pub async fn upsert_mcp_server_tool(_server_id: &str, _name: &str, _config: &str) -> Result<()> {
    Ok(())
}

pub async fn upsert_mcp_server_tool_with_pool(_pool: &sqlx::SqlitePool, _server_id: &str, _name: &str, _config: &str) -> Result<()> {
    Ok(())
}

pub async fn delete_mcp_server_tool(_server_id: &str, _name: &str) -> Result<()> {
    Ok(())
}

pub async fn delete_mcp_server_tool_with_pool(_pool: &sqlx::SqlitePool, _server_id: &str, _name: &str) -> Result<()> {
    Ok(())
}

pub async fn delete_mcp_server_tools_by_server(_server_id: &str) -> Result<()> {
    Ok(())
}

pub async fn delete_mcp_server_tools_by_server_with_pool(_pool: &sqlx::SqlitePool, _server_id: &str) -> Result<()> {
    Ok(())
}

/// Retrieve MCP servers that are active.
pub async fn get_project_active_mcp_servers(_project_id: &str) -> Result<Vec<McpServer>> {
    let pool = super::pool().await;
    list_mcp_servers_with_pool(pool).await
}

pub async fn get_project_active_mcp_servers_with_pool(
    pool: &sqlx::SqlitePool,
    _project_id: &str,
) -> Result<Vec<McpServer>> {
    list_mcp_servers_with_pool(pool).await
}

pub async fn get_project_mcp_server_ids(project_id: &str) -> Result<Vec<String>> {
    let pool = super::pool().await;
    get_project_mcp_server_ids_with_pool(pool, project_id).await
}

pub async fn get_project_mcp_server_ids_with_pool(
    pool: &sqlx::SqlitePool,
    project_id: &str,
) -> Result<Vec<String>> {
    let active_servers = get_project_active_mcp_servers_with_pool(pool, project_id).await?;
    Ok(active_servers.into_iter().map(|s| s.id).collect())
}

pub async fn set_project_mcp_servers(_project_id: &str, _active_server_ids: &[String]) -> Result<()> {
    Ok(())
}

pub async fn set_project_mcp_servers_with_pool(
    _pool: &sqlx::SqlitePool,
    _project_id: &str,
    _active_server_ids: &[String],
) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn test_delete_session_cleans_up_session_mcp_servers_and_runtime() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let proj = create_project_with_pool(&pool, "Proj", "Desc", Some("/tmp"), None)
            .await
            .unwrap();
        let sess = create_session_with_pool(&pool, &proj.id, "Sess 1", None, None, None)
            .await
            .unwrap();
        create_or_update_mcp_server_with_pool(&pool, "figcraft", "figcraft", "stdio|npx", true)
            .await
            .unwrap();

        upsert_session_mcp_server_with_pool(&pool, &sess.id, "figcraft", "{}", true)
            .await
            .unwrap();
        update_session_mcp_server_pid_with_pool(&pool, &sess.id, "figcraft", Some(999_999_999))
            .await
            .unwrap();

        crate::agent::mcp::SESSION_MCP_REGISTRY
            .set_tools(&sess.id, "figcraft", vec![])
            .await;
        assert!(
            crate::agent::mcp::SESSION_MCP_REGISTRY
                .get_tools(&sess.id, "figcraft")
                .await
                .is_some()
        );

        delete_session_with_pool(&pool, &sess.id).await.unwrap();

        let remaining = list_session_mcp_servers_with_pool(&pool, &sess.id)
            .await
            .unwrap();
        assert!(remaining.is_empty());
        assert!(
            crate::agent::mcp::SESSION_MCP_REGISTRY
                .get_tools(&sess.id, "figcraft")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_delete_expired_sessions_cleans_up_session_mcp_servers_and_runtime() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let proj = create_project_with_pool(&pool, "Proj", "Desc", Some("/tmp"), None)
            .await
            .unwrap();
        let sess = create_session_with_pool(&pool, &proj.id, "Old Sess", None, None, None)
            .await
            .unwrap();
        create_or_update_mcp_server_with_pool(&pool, "figcraft", "figcraft", "stdio|npx", true)
            .await
            .unwrap();

        upsert_session_mcp_server_with_pool(&pool, &sess.id, "figcraft", "{}", true)
            .await
            .unwrap();

        sqlx::query("UPDATE sessions SET updated_at = 100 WHERE id = ?1")
            .bind(&sess.id)
            .execute(&pool)
            .await
            .unwrap();

        crate::agent::mcp::SESSION_MCP_REGISTRY
            .set_tools(&sess.id, "figcraft", vec![])
            .await;

        let deleted = delete_expired_sessions_with_pool(&pool, 500).await.unwrap();
        assert_eq!(deleted, 1);

        let remaining = list_session_mcp_servers_with_pool(&pool, &sess.id)
            .await
            .unwrap();
        assert!(remaining.is_empty());
        assert!(
            crate::agent::mcp::SESSION_MCP_REGISTRY
                .get_tools(&sess.id, "figcraft")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_delete_mcp_server_cleans_up_all_sessions_and_runtime() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let proj = create_project_with_pool(&pool, "Proj", "Desc", Some("/tmp"), None)
            .await
            .unwrap();
        let sess1 = create_session_with_pool(&pool, &proj.id, "Sess 1", None, None, None)
            .await
            .unwrap();
        let sess2 = create_session_with_pool(&pool, &proj.id, "Sess 2", None, None, None)
            .await
            .unwrap();
        create_or_update_mcp_server_with_pool(&pool, "figcraft", "figcraft", "stdio|npx", true)
            .await
            .unwrap();

        upsert_session_mcp_server_with_pool(&pool, &sess1.id, "figcraft", "{}", true)
            .await
            .unwrap();
        upsert_session_mcp_server_with_pool(&pool, &sess2.id, "figcraft", "{}", true)
            .await
            .unwrap();
        update_session_mcp_server_pid_with_pool(&pool, &sess1.id, "figcraft", Some(999_999_999))
            .await
            .unwrap();

        crate::agent::mcp::SESSION_MCP_REGISTRY
            .set_tools(&sess1.id, "figcraft", vec![])
            .await;
        crate::agent::mcp::SESSION_MCP_REGISTRY
            .set_tools(&sess2.id, "figcraft", vec![])
            .await;

        delete_mcp_server_with_pool(&pool, "figcraft")
            .await
            .unwrap();

        let servers = list_mcp_servers_with_pool(&pool).await.unwrap();
        assert!(!servers.iter().any(|s| s.id == "figcraft"));

        let s1_mcp = list_session_mcp_servers_with_pool(&pool, &sess1.id)
            .await
            .unwrap();
        let s2_mcp = list_session_mcp_servers_with_pool(&pool, &sess2.id)
            .await
            .unwrap();
        assert!(s1_mcp.is_empty());
        assert!(s2_mcp.is_empty());

        assert!(
            crate::agent::mcp::SESSION_MCP_REGISTRY
                .get_tools(&sess1.id, "figcraft")
                .await
                .is_none()
        );
        assert!(
            crate::agent::mcp::SESSION_MCP_REGISTRY
                .get_tools(&sess2.id, "figcraft")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_sync_session_mcp_runtime_clears_dead_pid() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let proj = create_project_with_pool(&pool, "Proj", "Desc", Some("/tmp"), None)
            .await
            .unwrap();
        let sess = create_session_with_pool(&pool, &proj.id, "Sess", None, None, None)
            .await
            .unwrap();
        create_or_update_mcp_server_with_pool(&pool, "figcraft", "figcraft", "stdio|npx", true)
            .await
            .unwrap();

        upsert_session_mcp_server_with_pool(&pool, &sess.id, "figcraft", "{}", true)
            .await
            .unwrap();
        // Record a dead OS PID
        update_session_mcp_server_pid_with_pool(&pool, &sess.id, "figcraft", Some(999_999_999))
            .await
            .unwrap();

        // list_session_mcp_servers_with_pool runs sync_session_mcp_runtime_with_pool automatically
        let servers = list_session_mcp_servers_with_pool(&pool, &sess.id)
            .await
            .unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].pid, None);
    }

    #[tokio::test]
    async fn test_deactivate_and_remove_session_mcp_server_stops_runtime_and_clears_pid() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let proj = create_project_with_pool(&pool, "Proj", "Desc", Some("/tmp"), None)
            .await
            .unwrap();
        let sess = create_session_with_pool(&pool, &proj.id, "Sess", None, None, None)
            .await
            .unwrap();
        create_or_update_mcp_server_with_pool(&pool, "figcraft", "figcraft", "stdio|npx", true)
            .await
            .unwrap();

        upsert_session_mcp_server_with_pool(&pool, &sess.id, "figcraft", "{}", true)
            .await
            .unwrap();
        // Use current process PID (which is alive) so sync doesn't clear it as dead,
        // while kill_os_pid ignores current process PID safely.
        let fake_live_pid = i64::from(std::process::id());
        update_session_mcp_server_pid_with_pool(&pool, &sess.id, "figcraft", Some(fake_live_pid))
            .await
            .unwrap();
        crate::agent::mcp::SESSION_MCP_REGISTRY
            .set_tools(&sess.id, "figcraft", vec![])
            .await;

        // Deactivate via set_session_mcp_server_active_with_pool -> must clear pid & runtime registry
        set_session_mcp_server_active_with_pool(&pool, &sess.id, "figcraft", false)
            .await
            .unwrap();

        let servers = list_session_mcp_servers_with_pool(&pool, &sess.id)
            .await
            .unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].is_active, 0);
        assert_eq!(servers[0].pid, None);
        assert!(
            crate::agent::mcp::SESSION_MCP_REGISTRY
                .get_tools(&sess.id, "figcraft")
                .await
                .is_none()
        );

        // Re-activate and set PID again, then remove_session_mcp_server_with_pool
        set_session_mcp_server_active_with_pool(&pool, &sess.id, "figcraft", true)
            .await
            .unwrap();
        update_session_mcp_server_pid_with_pool(&pool, &sess.id, "figcraft", Some(fake_live_pid))
            .await
            .unwrap();
        crate::agent::mcp::SESSION_MCP_REGISTRY
            .set_tools(&sess.id, "figcraft", vec![])
            .await;

        remove_session_mcp_server_with_pool(&pool, &sess.id, "figcraft")
            .await
            .unwrap();

        let after_remove = list_session_mcp_servers_with_pool(&pool, &sess.id)
            .await
            .unwrap();
        assert!(after_remove.is_empty());
        assert!(
            crate::agent::mcp::SESSION_MCP_REGISTRY
                .get_tools(&sess.id, "figcraft")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_cleanup_orphaned_mcp_processes_on_startup_kills_pid_and_resets_null() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let proj = create_project_with_pool(&pool, "Proj", "Desc", Some("/tmp"), None)
            .await
            .unwrap();
        let sess = create_session_with_pool(&pool, &proj.id, "Sess", None, None, None)
            .await
            .unwrap();
        create_or_update_mcp_server_with_pool(&pool, "figcraft", "figcraft", "stdio|npx", true)
            .await
            .unwrap();

        upsert_session_mcp_server_with_pool(&pool, &sess.id, "figcraft", "{}", true)
            .await
            .unwrap();

        // Spawn a real background OS child process to simulate an orphaned MCP server
        let mut child = tokio::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        let orphan_pid = i64::from(child.id().unwrap());
        assert!(crate::agent::mcp::is_os_pid_alive(orphan_pid).await);

        update_session_mcp_server_pid_with_pool(&pool, &sess.id, "figcraft", Some(orphan_pid))
            .await
            .unwrap();
        crate::agent::mcp::SESSION_MCP_REGISTRY
            .set_tools(&sess.id, "figcraft", vec![])
            .await;

        let cleaned = cleanup_orphaned_mcp_processes_on_startup_with_pool(&pool)
            .await
            .unwrap();
        assert_eq!(cleaned, 1);

        // Reap the zombie child in test process so `kill -0` sees it is completely gone
        let _ = child.wait().await;
        assert!(!crate::agent::mcp::is_os_pid_alive(orphan_pid).await);

        let servers = list_session_mcp_servers_with_pool(&pool, &sess.id)
            .await
            .unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].pid, None);
        assert!(
            crate::agent::mcp::SESSION_MCP_REGISTRY
                .get_tools(&sess.id, "figcraft")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_shutdown_all_mcp_servers_kills_processes_and_resets_pid_null() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let proj = create_project_with_pool(&pool, "Proj", "Desc", Some("/tmp"), None)
            .await
            .unwrap();
        let sess1 = create_session_with_pool(&pool, &proj.id, "Sess1", None, None, None)
            .await
            .unwrap();
        let sess2 = create_session_with_pool(&pool, &proj.id, "Sess2", None, None, None)
            .await
            .unwrap();
        create_or_update_mcp_server_with_pool(&pool, "figcraft", "figcraft", "stdio|npx", true)
            .await
            .unwrap();

        upsert_session_mcp_server_with_pool(&pool, &sess1.id, "figcraft", "{}", true)
            .await
            .unwrap();
        upsert_session_mcp_server_with_pool(&pool, &sess2.id, "figcraft", "{}", true)
            .await
            .unwrap();

        let mut child1 = tokio::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        let mut child2 = tokio::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        let pid1 = i64::from(child1.id().unwrap());
        let pid2 = i64::from(child2.id().unwrap());

        update_session_mcp_server_pid_with_pool(&pool, &sess1.id, "figcraft", Some(pid1))
            .await
            .unwrap();
        update_session_mcp_server_pid_with_pool(&pool, &sess2.id, "figcraft", Some(pid2))
            .await
            .unwrap();
        crate::agent::mcp::SESSION_MCP_REGISTRY
            .set_tools(&sess1.id, "figcraft", vec![])
            .await;
        crate::agent::mcp::SESSION_MCP_REGISTRY
            .set_tools(&sess2.id, "figcraft", vec![])
            .await;

        shutdown_all_mcp_servers_with_pool(&pool).await.unwrap();

        let _ = child1.wait().await;
        let _ = child2.wait().await;
        assert!(!crate::agent::mcp::is_os_pid_alive(pid1).await);
        assert!(!crate::agent::mcp::is_os_pid_alive(pid2).await);

        let s1 = list_session_mcp_servers_with_pool(&pool, &sess1.id)
            .await
            .unwrap();
        let s2 = list_session_mcp_servers_with_pool(&pool, &sess2.id)
            .await
            .unwrap();
        assert_eq!(s1[0].pid, None);
        assert_eq!(s2[0].pid, None);
        assert!(
            crate::agent::mcp::SESSION_MCP_REGISTRY
                .get_tools(&sess1.id, "figcraft")
                .await
                .is_none()
        );
        assert!(
            crate::agent::mcp::SESSION_MCP_REGISTRY
                .get_tools(&sess2.id, "figcraft")
                .await
                .is_none()
        );
    }
}