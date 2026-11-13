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
    // The session is going away, so its in-memory file backups must go with it. This is the
    // release point for the snapshot store: a turn boundary must keep the backups (the
    // rollback contract is per session), a deletion must not.
    crate::agent::snapshot::release_session(id);

    // Same reasoning for the per-session turn generation counter: no turn can still be in
    // flight once the session is deleted, so this is the only safe point to reclaim it.
    // `is_latest_turn` deliberately never prunes the entry, because a stale cleanup must not
    // disown a newer turn that is still running.
    crate::agent::executor::forget_session_generation(id);

    // Reclaim the per-session process-global registries owned by `agent::events`: the event
    // broadcaster, the running-task handle, and the cancellation flag. Those maps are populated
    // lazily on first use, so without this a long-running host leaks one entry (plus a
    // capacity-2048 broadcast channel) per deleted session forever. Deletion is the only point
    // where no turn can still legitimately reference the session, so it is the safe release site.
    crate::agent::events::cleanup_session(id);

    let pool = super::pool().await;
    delete_session_with_pool(pool, id).await
}

pub async fn delete_session_with_pool(pool: &sqlx::SqlitePool, id: &str) -> Result<()> {
    // All five statements are one logical deletion. Running them outside a transaction
    // means a mid-way failure leaves a session with goals but no evidence, or evidence
    // pointing at goals that no longer exist.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // 1. Clean up goal verifications and goal evidence for any goal in this session
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
    // 2. Clean up goals belonging to this session
    sqlx::query("DELETE FROM goals WHERE session_id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    // 3. Purge orphaned evidence_cache entries no longer attached to any active goal
    sqlx::query(
        "DELETE FROM evidence_cache WHERE id NOT IN (SELECT evidence_id FROM goal_evidence)",
    )
    .execute(&mut *tx)
    .await?;
    // 4. Delete the session itself (messages cascade via foreign key)
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

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

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

    // Reclaim per-session in-memory registries now the rows are gone.
    for id in &expired_ids {
        crate::agent::executor::forget_session_generation(id);
        crate::agent::events::cleanup_session(id);
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
    sqlx::query("DELETE FROM mcp_servers WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ============================================================================
// SESSION MCP SERVERS (DYNAMIC PER-SESSION & ENV)
// ============================================================================

pub async fn list_session_mcp_servers(session_id: &str) -> Result<Vec<SessionMcpServer>> {
    let pool = super::pool().await;
    list_session_mcp_servers_with_pool(pool, session_id).await
}

pub async fn list_session_mcp_servers_with_pool(
    pool: &sqlx::SqlitePool,
    session_id: &str,
) -> Result<Vec<SessionMcpServer>> {
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
    let now = now_secs();
    let active_int = if is_active { 1 } else { 0 };

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

    Ok(())
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