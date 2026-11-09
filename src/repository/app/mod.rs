//! App-level entities (chat history, todos).

pub mod chat_history;
pub mod todo;

/// Single entry point for obtaining the database pool from app-level
/// repositories.
///
/// Mirrors [`crate::repository::agent::pool`] so that both halves of the
/// repository layer reach the database the same way. Callers should go through
/// this instead of `crate::db::get_pool()` directly, keeping the pool source
/// swappable in one place.
pub async fn pool() -> &'static sqlx::SqlitePool {
    crate::db::get_pool().await
}
