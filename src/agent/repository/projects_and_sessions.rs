use anyhow::Result;
use crate::agent::models::{McpServer, Project, Session};
use super::{generate_id, now_secs};

// ============================================================================
// PROJECTS REPOSITORY
// ============================================================================

pub async fn list_projects() -> Result<Vec<Project>> {
    let pool = super::pool().await;
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
    let sessions = sqlx::query_as::<_, Session>(
        "SELECT s.id, s.project_id, s.title, s.status, s.model, s.temperature, s.source_chat_id, s.metadata, s.created_at, s.updated_at, \
         COALESCE(SUM(m.token_usage_input), 0) AS total_tokens_input, \
         COALESCE(SUM(m.token_usage_output), 0) AS total_tokens_output \
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
    let session = sqlx::query_as::<_, Session>(
        "SELECT s.id, s.project_id, s.title, s.status, s.model, s.temperature, s.source_chat_id, s.metadata, s.created_at, s.updated_at, \
         COALESCE(SUM(m.token_usage_input), 0) AS total_tokens_input, \
         COALESCE(SUM(m.token_usage_output), 0) AS total_tokens_output \
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
    })
}

pub async fn get_session_status(id: &str) -> Result<Option<String>> {
    let pool = super::pool().await;
    let row: Option<(String,)> = sqlx::query_as("SELECT status FROM sessions WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

pub async fn update_session_status(id: &str, status: &str) -> Result<()> {
    let pool = super::pool().await;
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
    let now = now_secs();
    sqlx::query("UPDATE sessions SET model = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(model)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_session(id: &str) -> Result<()> {
    let pool = super::pool().await;

    // All five statements are one logical deletion. Running them outside a transaction
    // means a mid-way failure leaves a session with goals but no evidence, or evidence
    // pointing at goals that no longer exist.
    let mut tx = pool.begin().await?;

    // 1. Clean up goal verifications and goal evidence for any goal in this session
    sqlx::query("DELETE FROM goal_verifications WHERE goal_id IN (SELECT id FROM goals WHERE session_id = ?1)")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM goal_evidence WHERE goal_id IN (SELECT id FROM goals WHERE session_id = ?1)")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    // 2. Clean up goals belonging to this session
    sqlx::query("DELETE FROM goals WHERE session_id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    // 3. Purge orphaned evidence_cache entries no longer attached to any active goal
    sqlx::query("DELETE FROM evidence_cache WHERE id NOT IN (SELECT evidence_id FROM goal_evidence)")
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

    let mut tx = pool.begin().await?;

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

    let mut tx = pool.begin().await?;

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
    let servers = sqlx::query_as::<_, McpServer>(
        "SELECT id, name, config, is_enabled, created_at \
         FROM mcp_servers ORDER BY name ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(servers)
}

pub async fn create_or_update_mcp_server(
    id: &str,
    name: &str,
    config: &str,
    is_enabled: bool,
) -> Result<()> {
    let pool = super::pool().await;
    let now = now_secs();
    let enabled_int = if is_enabled { 1 } else { 0 };

    sqlx::query(
        "INSERT INTO mcp_servers (id, name, config, is_enabled, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(id) DO UPDATE SET \
            name = excluded.name, \
            config = excluded.config, \
            is_enabled = excluded.is_enabled",
    )
    .bind(id)
    .bind(name)
    .bind(config)
    .bind(enabled_int)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn delete_mcp_server(id: &str) -> Result<()> {
    let pool = super::pool().await;
    sqlx::query("DELETE FROM mcp_servers WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Retrieve MCP servers that are active for a specific project.
/// If no custom project-level configuration exists yet, defaults to all globally enabled MCP servers.
pub async fn get_project_active_mcp_servers(project_id: &str) -> Result<Vec<McpServer>> {
    let pool = super::pool().await;

    // Check if this project has any explicit rows configured in project_mcp_servers
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM project_mcp_servers WHERE project_id = ?1",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    if count == 0 {
        // Default: Return all globally enabled MCP servers
        let servers = sqlx::query_as::<_, McpServer>(
            "SELECT id, name, config, is_enabled, created_at \
             FROM mcp_servers WHERE is_enabled = 1 ORDER BY name ASC",
        )
        .fetch_all(pool)
        .await?;
        return Ok(servers);
    }

    // Explicit project selection: only where project_mcp_servers.is_enabled = 1 AND mcp_servers.is_enabled = 1
    let servers = sqlx::query_as::<_, McpServer>(
        "SELECT ms.id, ms.name, ms.config, ms.is_enabled, ms.created_at \
         FROM mcp_servers ms \
         JOIN project_mcp_servers pms ON ms.id = pms.server_id \
         WHERE pms.project_id = ?1 AND pms.is_enabled = 1 AND ms.is_enabled = 1 \
         ORDER BY ms.name ASC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;

    Ok(servers)
}

/// Retrieve the list of server IDs currently active for a project.
/// If not customized yet, returns all globally enabled server IDs.
pub async fn get_project_mcp_server_ids(project_id: &str) -> Result<Vec<String>> {
    let active_servers = get_project_active_mcp_servers(project_id).await?;
    Ok(active_servers.into_iter().map(|s| s.id).collect())
}

/// Update active MCP servers for a project.
/// Explicitly marks each registered server as enabled (1) or disabled (0) for this project.
pub async fn set_project_mcp_servers(project_id: &str, active_server_ids: &[String]) -> Result<()> {
    let pool = super::pool().await;
    let all_servers = list_mcp_servers().await?;

    sqlx::query("DELETE FROM project_mcp_servers WHERE project_id = ?1")
        .bind(project_id)
        .execute(pool)
        .await?;

    for s in all_servers {
        let is_active = if active_server_ids.contains(&s.id) { 1 } else { 0 };
        let _ = sqlx::query(
            "INSERT INTO project_mcp_servers (project_id, server_id, is_enabled) \
             VALUES (?1, ?2, ?3)",
        )
        .bind(project_id)
        .bind(&s.id)
        .bind(is_active)
        .execute(pool)
        .await;
    }

    Ok(())
}
