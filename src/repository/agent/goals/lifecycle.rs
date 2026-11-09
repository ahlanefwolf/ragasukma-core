use super::super::now_secs;
use crate::agent::models::Goal;
use anyhow::Result;

#[allow(clippy::too_many_arguments)]
pub async fn create_goal(
    id: &str,
    project_id: &str,
    session_id: Option<&str>,
    parent_goal_id: Option<&str>,
    title: &str,
    demand: &str,
    success_criteria_json: &str,
    constraints_json: &str,
) -> Result<Goal> {
    let pool = crate::repository::agent::pool().await;
    create_goal_with_pool(
        pool,
        id,
        project_id,
        session_id,
        parent_goal_id,
        title,
        demand,
        success_criteria_json,
        constraints_json,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn create_goal_with_pool(
    pool: &sqlx::SqlitePool,
    id: &str,
    project_id: &str,
    session_id: Option<&str>,
    parent_goal_id: Option<&str>,
    title: &str,
    demand: &str,
    success_criteria_json: &str,
    constraints_json: &str,
) -> Result<Goal> {
    let now = now_secs();

    // Check if goal already exists. If it exists and is in_progress, prevent unintended state overwrite.
    let existing = sqlx::query_as::<_, Goal>(
        "SELECT id, project_id, session_id, parent_goal_id, title, demand, success_criteria, constraints, phase, status, target_files, discovery_count, conclusion, created_at, updated_at, version, context_status, context_error \
         FROM goals WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    if let Some(mut existing_goal) = existing {
        if existing_goal.status == "in_progress" {
            // Already actively running; do not clobber running state, return existing
            return Ok(existing_goal);
        }

        // If completed or failed, allow explicit restart with incremented version
        let new_version = existing_goal.version + 1;
        let res = sqlx::query(
            "UPDATE goals SET \
                project_id = ?1, \
                session_id = ?2, \
                parent_goal_id = ?3, \
                title = ?4, \
                demand = ?5, \
                success_criteria = ?6, \
                constraints = ?7, \
                phase = 'discovery', \
                status = 'in_progress', \
                target_files = '[]', \
                discovery_count = 0, \
                conclusion = NULL, \
                context_status = 'healthy', \
                context_error = NULL, \
                version = ?8, \
                updated_at = ?9 \
             WHERE id = ?10 AND version = ?11",
        )
        .bind(project_id)
        .bind(session_id)
        .bind(parent_goal_id)
        .bind(title)
        .bind(demand)
        .bind(success_criteria_json)
        .bind(constraints_json)
        .bind(new_version)
        .bind(now)
        .bind(id)
        .bind(existing_goal.version)
        .execute(pool)
        .await?;

        if res.rows_affected() == 0 {
            anyhow::bail!(
                "Concurrent goal modification detected while updating goal '{}'",
                id
            );
        }

        existing_goal.project_id = project_id.to_string();
        existing_goal.session_id = session_id.map(|s| s.to_string());
        existing_goal.parent_goal_id = parent_goal_id.map(|s| s.to_string());
        existing_goal.title = title.to_string();
        existing_goal.demand = demand.to_string();
        existing_goal.success_criteria = success_criteria_json.to_string();
        existing_goal.constraints = constraints_json.to_string();
        existing_goal.phase = "discovery".to_string();
        existing_goal.status = "in_progress".to_string();
        existing_goal.target_files = "[]".to_string();
        existing_goal.discovery_count = 0;
        existing_goal.conclusion = None;
        existing_goal.version = new_version;
        existing_goal.context_status = "healthy".to_string();
        existing_goal.context_error = None;
        existing_goal.updated_at = now;
        return Ok(existing_goal);
    }

    sqlx::query(
        "INSERT INTO goals (id, project_id, session_id, parent_goal_id, title, demand, success_criteria, constraints, phase, status, target_files, discovery_count, created_at, updated_at, version, context_status, context_error) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'discovery', 'in_progress', '[]', 0, ?9, ?10, 1, 'healthy', NULL)",
    )
    .bind(id)
    .bind(project_id)
    .bind(session_id)
    .bind(parent_goal_id)
    .bind(title)
    .bind(demand)
    .bind(success_criteria_json)
    .bind(constraints_json)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(Goal {
        id: id.to_string(),
        project_id: project_id.to_string(),
        session_id: session_id.map(|s| s.to_string()),
        parent_goal_id: parent_goal_id.map(|s| s.to_string()),
        title: title.to_string(),
        demand: demand.to_string(),
        success_criteria: success_criteria_json.to_string(),
        constraints: constraints_json.to_string(),
        phase: "discovery".to_string(),
        status: "in_progress".to_string(),
        target_files: "[]".to_string(),
        discovery_count: 0,
        conclusion: None,
        created_at: now,
        updated_at: now,
        version: 1,
        context_status: "healthy".to_string(),
        context_error: None,
    })
}

pub async fn get_goal(goal_id: &str) -> Result<Option<Goal>> {
    let pool = crate::repository::agent::pool().await;
    get_goal_with_pool(pool, goal_id).await
}

pub async fn get_goal_with_pool(pool: &sqlx::SqlitePool, goal_id: &str) -> Result<Option<Goal>> {
    let goal = sqlx::query_as::<_, Goal>(
        "SELECT id, project_id, session_id, parent_goal_id, title, demand, success_criteria, constraints, phase, status, target_files, discovery_count, conclusion, created_at, updated_at, version, context_status, context_error \
         FROM goals WHERE id = ?1",
    )
    .bind(goal_id)
    .fetch_optional(pool)
    .await?;
    Ok(goal)
}

pub async fn list_session_goals(session_id: &str) -> Result<Vec<Goal>> {
    let pool = crate::repository::agent::pool().await;
    list_session_goals_with_pool(pool, session_id).await
}

pub async fn list_session_goals_with_pool(
    pool: &sqlx::SqlitePool,
    session_id: &str,
) -> Result<Vec<Goal>> {
    let goals = sqlx::query_as::<_, Goal>(
        "SELECT id, project_id, session_id, parent_goal_id, title, demand, success_criteria, constraints, phase, status, target_files, discovery_count, conclusion, created_at, updated_at, version, context_status, context_error \
         FROM goals WHERE session_id = ?1 ORDER BY created_at DESC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(goals)
}

pub async fn list_project_goals(project_id: &str) -> Result<Vec<Goal>> {
    let pool = crate::repository::agent::pool().await;
    list_project_goals_with_pool(pool, project_id).await
}

pub async fn list_project_goals_with_pool(
    pool: &sqlx::SqlitePool,
    project_id: &str,
) -> Result<Vec<Goal>> {
    let goals = sqlx::query_as::<_, Goal>(
        "SELECT id, project_id, session_id, parent_goal_id, title, demand, success_criteria, constraints, phase, status, target_files, discovery_count, conclusion, created_at, updated_at, version, context_status, context_error \
         FROM goals WHERE project_id = ?1 ORDER BY created_at DESC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    Ok(goals)
}

pub async fn update_goal_phase(goal_id: &str, phase: &str) -> Result<()> {
    let pool = crate::repository::agent::pool().await;
    update_goal_phase_with_pool(pool, goal_id, phase).await
}

pub async fn update_goal_phase_with_pool(
    pool: &sqlx::SqlitePool,
    goal_id: &str,
    phase: &str,
) -> Result<()> {
    let now = now_secs();
    let res = sqlx::query(
        "UPDATE goals SET phase = ?1, version = version + 1, updated_at = ?2 WHERE id = ?3",
    )
    .bind(phase)
    .bind(now)
    .bind(goal_id)
    .execute(pool)
    .await?;

    if res.rows_affected() == 0 {
        anyhow::bail!("Goal '{}' not found for phase update", goal_id);
    }
    Ok(())
}

pub async fn transition_goal_phase(
    goal_id: &str,
    expected_phase: &str,
    new_phase: &str,
) -> Result<()> {
    let pool = crate::repository::agent::pool().await;
    transition_goal_phase_with_pool(pool, goal_id, expected_phase, new_phase).await
}

pub async fn transition_goal_phase_with_pool(
    pool: &sqlx::SqlitePool,
    goal_id: &str,
    expected_phase: &str,
    new_phase: &str,
) -> Result<()> {
    let now = now_secs();
    let res = sqlx::query(
        "UPDATE goals SET phase = ?1, version = version + 1, updated_at = ?2 \
         WHERE id = ?3 AND phase = ?4",
    )
    .bind(new_phase)
    .bind(now)
    .bind(goal_id)
    .bind(expected_phase)
    .execute(pool)
    .await?;

    if res.rows_affected() == 0 {
        anyhow::bail!(
            "Atomic transition rejected: Goal '{}' is not in expected phase '{}' or concurrent modification occurred",
            goal_id,
            expected_phase
        );
    }
    Ok(())
}

pub async fn lock_goal_targets(goal_id: &str, target_files_json: &str) -> Result<()> {
    let pool = crate::repository::agent::pool().await;
    lock_goal_targets_with_pool(pool, goal_id, target_files_json).await
}

pub async fn lock_goal_targets_with_pool(
    pool: &sqlx::SqlitePool,
    goal_id: &str,
    target_files_json: &str,
) -> Result<()> {
    let now = now_secs();
    let res = sqlx::query(
        "UPDATE goals SET target_files = ?1, phase = 'action', version = version + 1, updated_at = ?2 \
         WHERE id = ?3 AND phase = 'discovery'",
    )
    .bind(target_files_json)
    .bind(now)
    .bind(goal_id)
    .execute(pool)
    .await?;

    if res.rows_affected() == 0 {
        // If already in action or verification, ensure idempotent lock update without phase violation
        let fallback = sqlx::query(
            "UPDATE goals SET target_files = ?1, version = version + 1, updated_at = ?2 WHERE id = ?3",
        )
        .bind(target_files_json)
        .bind(now)
        .bind(goal_id)
        .execute(pool)
        .await?;

        if fallback.rows_affected() == 0 {
            anyhow::bail!("Goal '{}' not found to lock targets", goal_id);
        }
    }
    Ok(())
}

pub async fn update_goal_status(goal_id: &str, status: &str) -> Result<()> {
    let pool = crate::repository::agent::pool().await;
    update_goal_status_with_pool(pool, goal_id, status).await
}

pub async fn update_goal_status_with_pool(
    pool: &sqlx::SqlitePool,
    goal_id: &str,
    status: &str,
) -> Result<()> {
    let now = now_secs();
    let res = sqlx::query(
        "UPDATE goals SET status = ?1, version = version + 1, updated_at = ?2 WHERE id = ?3",
    )
    .bind(status)
    .bind(now)
    .bind(goal_id)
    .execute(pool)
    .await?;

    if res.rows_affected() == 0 {
        anyhow::bail!("Goal '{}' not found for status update", goal_id);
    }
    Ok(())
}

pub async fn update_goal_context_health(
    goal_id: &str,
    context_status: &str,
    context_error: Option<&str>,
) -> Result<()> {
    let pool = crate::repository::agent::pool().await;
    update_goal_context_health_with_pool(pool, goal_id, context_status, context_error).await
}

pub async fn update_goal_context_health_with_pool(
    pool: &sqlx::SqlitePool,
    goal_id: &str,
    context_status: &str,
    context_error: Option<&str>,
) -> Result<()> {
    let now = now_secs();
    sqlx::query(
        "UPDATE goals SET context_status = ?1, context_error = ?2, version = version + 1, updated_at = ?3 WHERE id = ?4",
    )
    .bind(context_status)
    .bind(context_error)
    .bind(now)
    .bind(goal_id)
    .execute(pool)
    .await?;
    Ok(())
}
