use super::{generate_id, now_secs};
use crate::agent::models::Message;
use anyhow::Result;

// ============================================================================
// MESSAGES REPOSITORY
// ============================================================================

pub async fn list_messages(session_id: &str) -> Result<Vec<Message>> {
    let pool = super::pool().await;
    list_messages_with_pool(pool, session_id).await
}

pub async fn list_messages_with_pool(
    pool: &sqlx::SqlitePool,
    session_id: &str,
) -> Result<Vec<Message>> {
    list_messages_paginated_with_pool(pool, session_id, None, None, None).await
}

pub async fn list_messages_paginated(
    session_id: &str,
    limit: Option<i64>,
    offset: Option<i64>,
    before_seq: Option<i64>,
) -> Result<Vec<Message>> {
    let pool = super::pool().await;
    list_messages_paginated_with_pool(pool, session_id, limit, offset, before_seq).await
}

pub async fn list_messages_paginated_with_pool(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    limit: Option<i64>,
    offset: Option<i64>,
    before_seq: Option<i64>,
) -> Result<Vec<Message>> {
    let messages = match (limit, before_seq) {
        (Some(lim), Some(b_seq)) => {
            let mut list = sqlx::query_as::<_, Message>(
                "SELECT id, session_id, sequence_num, role, content, thought, tool_calls, tool_call_id, \
                 status, token_usage_input, token_usage_output, created_at \
                 FROM messages WHERE session_id = ?1 AND sequence_num < ?2 \
                 ORDER BY sequence_num DESC LIMIT ?3",
            )
            .bind(session_id)
            .bind(b_seq)
            .bind(lim)
            .fetch_all(pool)
            .await?;
            list.reverse();
            list
        }
        (Some(lim), None) => {
            let off = offset.unwrap_or(0);
            let mut list = sqlx::query_as::<_, Message>(
                "SELECT id, session_id, sequence_num, role, content, thought, tool_calls, tool_call_id, \
                 status, token_usage_input, token_usage_output, created_at \
                 FROM messages WHERE session_id = ?1 \
                 ORDER BY sequence_num DESC LIMIT ?2 OFFSET ?3",
            )
            .bind(session_id)
            .bind(lim)
            .bind(off)
            .fetch_all(pool)
            .await?;
            list.reverse();
            list
        }
        (None, _) => {
            sqlx::query_as::<_, Message>(
                "SELECT id, session_id, sequence_num, role, content, thought, tool_calls, tool_call_id, \
                 status, token_usage_input, token_usage_output, created_at \
                 FROM messages WHERE session_id = ?1 ORDER BY sequence_num ASC",
            )
            .bind(session_id)
            .fetch_all(pool)
            .await?
        }
    };
    Ok(messages)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_message(
    session_id: &str,
    role: &str,
    content: Option<&str>,
    thought: Option<&str>,
    tool_calls: Option<&str>,
    tool_call_id: Option<&str>,
    status: &str,
    token_usage_input: Option<i64>,
    token_usage_output: Option<i64>,
) -> Result<Message> {
    let pool = super::pool().await;
    create_message_with_pool(
        pool,
        session_id,
        role,
        content,
        thought,
        tool_calls,
        tool_call_id,
        status,
        token_usage_input,
        token_usage_output,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn create_message_with_pool(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    role: &str,
    content: Option<&str>,
    thought: Option<&str>,
    tool_calls: Option<&str>,
    tool_call_id: Option<&str>,
    status: &str,
    token_usage_input: Option<i64>,
    token_usage_output: Option<i64>,
) -> Result<Message> {
    let now = now_secs();

    // Atomic subquery + transaction + retry loop prevents UNIQUE(session_id, sequence_num) race conditions
    let mut attempts = 0;
    loop {
        attempts += 1;
        let id = generate_id("msg");
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

        let res: Result<(i64,), sqlx::Error> = sqlx::query_as(
            "INSERT INTO messages (id, session_id, sequence_num, role, content, thought, tool_calls, tool_call_id, \
             status, token_usage_input, token_usage_output, created_at) \
             VALUES (?1, ?2, (SELECT COALESCE(MAX(sequence_num), 0) + 1 FROM messages WHERE session_id = ?2), \
             ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
             RETURNING sequence_num",
        )
        .bind(&id)
        .bind(session_id)
        .bind(role)
        .bind(content)
        .bind(thought)
        .bind(tool_calls)
        .bind(tool_call_id)
        .bind(status)
        .bind(token_usage_input)
        .bind(token_usage_output)
        .bind(now)
        .fetch_one(&mut *tx)
        .await;

        match res {
            Ok((sequence_num,)) => {
                tx.commit().await?;
                return Ok(Message {
                    id,
                    session_id: session_id.to_string(),
                    sequence_num,
                    role: role.to_string(),
                    content: content.map(|s| s.to_string()),
                    thought: thought.map(|s| s.to_string()),
                    tool_calls: tool_calls.map(|s| s.to_string()),
                    tool_call_id: tool_call_id.map(|s| s.to_string()),
                    status: status.to_string(),
                    token_usage_input,
                    token_usage_output,
                    created_at: now,
                });
            }
            Err(e) => {
                let _ = tx.rollback().await;
                if attempts >= 3 {
                    return Err(e.into());
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
}

// ============================================================================
// RETENTION
// ============================================================================

/// Deletes messages older than `cutoff_epoch_secs` and returns how many rows were removed.
///
/// The cutoff is supplied by the caller so the retention window stays a runtime policy
/// instead of being hardcoded into the query.
pub async fn delete_expired_messages(cutoff_epoch_secs: i64) -> Result<u64> {
    let pool = super::pool().await;
    delete_expired_messages_with_pool(pool, cutoff_epoch_secs).await
}

/// Pool-injected variant of [`delete_expired_messages`] for testability.
pub async fn delete_expired_messages_with_pool(
    pool: &sqlx::SqlitePool,
    cutoff_epoch_secs: i64,
) -> Result<u64> {
    let res = sqlx::query("DELETE FROM messages WHERE created_at < ?1")
        .bind(cutoff_epoch_secs)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_message_atomic_sequences() {
        let _guard = crate::repository::agent::testsupport::db_test_lock().await;
        let pool = super::super::pool().await;
        // Prepare test session
        let test_session_id = format!(
            "test_session_seq_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        // Timestamps must reflect "now": a retroactively old `updated_at` makes these rows
        // look expired to the retention sweep running concurrently in another test.
        let now = super::super::now_secs();
        sqlx::query("INSERT INTO projects (id, name, description, created_at, updated_at) VALUES (?1, 'P', 'Desc', ?2, ?2)")
            .bind(&test_session_id)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sessions (id, project_id, title, status, created_at, updated_at) VALUES (?1, ?2, 'S', 'idle', ?3, ?3)")
            .bind(&test_session_id)
            .bind(&test_session_id)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();

        let m1 = create_message(
            &test_session_id,
            "user",
            Some("Hello"),
            None,
            None,
            None,
            "success",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(m1.sequence_num, 1);

        let m2 = create_message(
            &test_session_id,
            "assistant",
            Some("Hi"),
            None,
            None,
            None,
            "success",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(m2.sequence_num, 2);

        let m3 = create_message(
            &test_session_id,
            "tool",
            Some("output"),
            None,
            None,
            Some("call_1"),
            "success",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(m3.sequence_num, 3);

        let listed = list_messages(&test_session_id).await.unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].sequence_num, 1);
        assert_eq!(listed[1].sequence_num, 2);
        assert_eq!(listed[2].sequence_num, 3);
    }
}
