//! Database bootstrap: the process-wide SQLite pool, the database path, and the
//! isolated test database lifecycle.
//!
//! This module owns the connection pool so that the data layer (`crate::repository`)
//! does not have to reach into `crate::llm` just to obtain a connection. `llm::usage`
//! still records usage rows through it, but that is a consumer, not the owner.

use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use std::path::PathBuf;
use tokio::sync::OnceCell;

#[allow(dead_code)]
static POOL: OnceCell<SqlitePool> = OnceCell::const_new();

/// Returns the pool used by all repository functions.
///
/// In `cfg(test)` builds this NEVER touches the user's live database file. It points at
/// a temporary WAL database with all migrations applied, so tests exercise the same
/// concurrency model as production while leaving `assets/ragasukma.db` untouched.
#[cfg(test)]
pub async fn get_pool() -> &'static SqlitePool {
    POOL.get_or_init(test_pool).await
}

/// Builds a fully-migrated isolated pool for tests.
///
/// The store lives in a unique temporary file rather than `:memory:`. An in-memory SQLite
/// database is per-connection unless shared-cache is used, and shared-cache takes table
/// locks that deadlock under `cargo test`'s parallel threads. A real WAL file gives the
/// same concurrency model production uses, so tests exercise the real code path.
#[cfg(test)]
async fn test_pool() -> SqlitePool {
    // Remove databases left behind by earlier test processes (see `reap_stale_test_dbs`).
    reap_stale_test_dbs();

    let mut path = std::env::temp_dir();
    path.push(format!(
        "ragasukma_test_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));

    // Register cleanup so the temp file does not outlive the test process.
    let _ = TEST_DB_GUARD.set(TempDbGuard(path.clone()));

    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(15));

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await
        .expect("Failed to initialize isolated test pool");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed to run migrations on isolated test pool");

    pool
}

#[cfg(test)]
const TEST_DB_PREFIX: &str = "ragasukma_test_";
#[cfg(test)]
const TEST_DB_SUFFIXES: [&str; 3] = ["", "-wal", "-shm"];

/// Decides whether a test database owned by `owner_pid` may be deleted.
///
/// A database is only reapable when its owner is a different process that is no longer running,
/// so concurrent test binaries can never delete each other's live databases.
#[cfg(test)]
fn should_reap(owner_pid: u32, current_pid: u32) -> bool {
    owner_pid != current_pid && !process_is_alive(owner_pid)
}

/// Deletes test databases whose owning process no longer exists.
///
/// `TempDbGuard` only runs on a graceful drop, but libtest can terminate the process without
/// unwinding, so a best-effort sweep runs before each new test pool is created. A file is only
/// removed when its PID is our own or can no longer be signalled, so concurrent test binaries
/// never delete each other's live databases.
#[cfg(test)]
fn reap_stale_test_dbs() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(rest) = file_name.strip_prefix(TEST_DB_PREFIX) else {
            continue;
        };
        let pid: u32 = rest
            .split('_')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        if should_reap(pid, std::process::id()) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Returns `true` if a process with `pid` still exists.
#[cfg(test)]
fn process_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // `kill -0` performs an existence/permission probe without delivering a signal.
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        // `tasklist` output is localized and its PID filter depends on shell quoting, so query the
        // process table directly. A successful handle (even without PROCESS_TERMINATE) means the
        // PID is still owned; `ERROR_INVALID_PARAMETER` means the PID is free.
        use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER};
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

        // SAFETY: `OpenProcess` only inspects the process table; the returned handle is closed below.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) };
        match handle {
            Ok(h) => {
                let _ = unsafe { CloseHandle(h) };
                true
            }
            Err(e) => {
                // `OpenProcess` reports Win32 errors wrapped in an HRESULT, so the Win32 code lives
                // in the low 16 bits (e.g. 0x8007_0057 -> ERROR_INVALID_PARAMETER).
                let code = (e.code().0 as u32) & 0xFFFF;
                // Only a definitively invalid PID is treated as dead. Access-denied and any
                // unexpected error are treated as alive so we never delete a live database.
                code != ERROR_INVALID_PARAMETER.0
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        // No portable probe; assume alive so we never delete live data.
        true
    }
}

/// Removes the temporary test database once the test process exits.
#[cfg(test)]
struct TempDbGuard(std::path::PathBuf);

#[cfg(test)]
impl Drop for TempDbGuard {
    fn drop(&mut self) {
        for suffix in TEST_DB_SUFFIXES {
            let _ = std::fs::remove_file(format!("{}{}", self.0.display(), suffix));
        }
    }
}

#[cfg(test)]
static TEST_DB_GUARD: std::sync::OnceLock<TempDbGuard> = std::sync::OnceLock::new();

#[cfg_attr(test, allow(dead_code))]
pub fn get_db_path() -> PathBuf {
    // 1. Explicit override always wins (useful for tests and power users).
    if let Ok(custom) = std::env::var("RAGASUKMA_DB")
        && !custom.trim().is_empty()
    {
        return PathBuf::from(custom);
    }

    // 2. In dev mode (debug build run from a crate root, or RAGASUKMA_DEV=1) prefer the
    //    *nearest* existing assets/ragasukma.db, walking up from the current directory.
    //    This lets `cargo run` from a workspace root pick up the real DB in a member
    //    crate's assets/ instead of silently creating an empty one next to the root.
    //    Release binaries never take this branch implicitly, so launching them from an
    //    unrelated Rust project cannot plant a database inside that project.
    if crate::assets::is_dev_mode() {
        if let Ok(cwd) = std::env::current_dir() {
            for dir in cwd.ancestors() {
                let candidate = dir.join("assets").join("ragasukma.db");
                if candidate.exists() {
                    return candidate;
                }
            }
        }
        let local_assets = PathBuf::from("assets");
        let _ = std::fs::create_dir_all(&local_assets);
        return local_assets.join("ragasukma.db");
    }

    let candidate = crate::assets::find_file_in_candidates("ragasukma.db");
    if candidate.exists() {
        return candidate;
    }

    let dir = crate::config::get_app_data_dir();
    let _ = std::fs::create_dir_all(&dir);
    dir.join("ragasukma.db")
}

#[cfg(not(test))]
pub async fn get_pool() -> &'static SqlitePool {
    POOL.get_or_init(|| async {
        let db_path = get_db_path();
        tracing::info!("[Database] Initializing SQLite pool at: {}", db_path.display());
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .expect("Failed to initialize SQLite pool");

        // Run embedded migrations at startup with automatic checksum auto-healing on development changes
        if let Err(e) = sqlx::migrate!("./migrations").run(&pool).await {
            tracing::warn!("[Migration Warning] Migration issue encountered: {}. Attempting checksum auto-heal...", e);
            // If VersionMismatch occurred because migration files were edited during development,
            // clear mismatched checksums from _sqlx_migrations and re-run cleanly
            let _ = sqlx::query("DELETE FROM _sqlx_migrations WHERE version >= 4").execute(&pool).await;
            if let Err(e2) = sqlx::migrate!("./migrations").run(&pool).await {
                tracing::warn!("[Migration Warning] Secondary migration error: {}. Continuing.", e2);
            }
        }

        // Schema sync for existing DBs if tables were created before columns were added
        let _ = sqlx::query("ALTER TABLE goals ADD COLUMN phase TEXT NOT NULL DEFAULT 'discovery'").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE goals ADD COLUMN target_files TEXT NOT NULL DEFAULT '[]'").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE goals ADD COLUMN discovery_count INTEGER NOT NULL DEFAULT 0").execute(&pool).await;

        pool
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_is_alive_detects_current_and_dead_pids() {
        assert!(process_is_alive(std::process::id()));
        assert!(!process_is_alive(0));
        // PID values this large are never allocated on supported platforms.
        assert!(!process_is_alive(u32::MAX - 1));
    }

    #[test]
    fn test_should_reap_only_touches_dead_foreign_owners() {
        let current = std::process::id();
        // Never delete a database owned by the running process.
        assert!(!should_reap(current, current));
        // A dead foreign PID is reapable.
        assert!(should_reap(u32::MAX - 1, current));
        // PID 0 is never a valid owner.
        assert!(should_reap(0, current));
    }

    #[test]
    fn test_reap_preserves_current_process_databases() {
        // End-to-end guard: the sweep must leave this process's own database file on disk.
        let mut path = std::env::temp_dir();
        path.push(format!(
            "{}{}_reap_probe.db",
            TEST_DB_PREFIX,
            std::process::id()
        ));
        std::fs::write(&path, b"live").unwrap();
        reap_stale_test_dbs();
        let survived = path.exists();
        let _ = std::fs::remove_file(&path);
        assert!(
            survived,
            "reaper removed a live test database: {}",
            path.display()
        );
    }
}
