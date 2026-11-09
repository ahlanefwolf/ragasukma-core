use sqlx::SqlitePool;

// App-level entities (was: src/repository/*.rs)
pub mod app;
// Agent-domain entities (was: src/agent/repository/*.rs)
pub mod agent;

pub use app::chat_history::*;
pub use app::todo::*;
pub use app::{chat_history, todo};

// Agent submodules are exposed both as modules and flattened.
pub use agent::{audit, goals, messages, projects_and_sessions};
pub use agent::{generate_id, now_millis, now_secs, pool, testsupport};

// NOTE: the legacy SQLite-backed skill CRUD module (formerly `repository::skill`) was
// removed. Skills are now discovered dynamically from `.skill` / `.skills` folders on
// disk via `crate::agent::skills::scan_project_skills`. The old `skills` table created
// in migration `0003_todos_and_skills.sql` is intentionally left in place (dropping or
// rewriting a shipped migration would break the `_sqlx_migrations` checksum chain for
// existing installs); it is simply no longer read or written. Clean it up with a new
// forward migration if/when desired.

/// Runs all SQLx migrations embedded in ragasukma-core against the given SQLite pool.
pub async fn migrate_database(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}
