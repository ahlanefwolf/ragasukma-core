use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{FromRow, Row, SqlitePool};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::OnceCell;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct LlmUsageRecord {
    pub id: String,
    pub timestamp: i64,
    pub date_str: String,
    pub time_str: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub cached_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub duration_ms: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmUsagePaginatedResponse {
    pub items: Vec<LlmUsageRecord>,
    pub total: usize,
    pub page: usize,
    pub limit: usize,
    pub total_pages: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmUsageSummary {
    pub total_requests: usize,
    pub total_input_tokens: usize,
    pub total_cached_tokens: usize,
    pub total_output_tokens: usize,
    pub total_tokens: usize,
    pub cache_hit_rate_pct: f32,
    pub estimated_saved_tokens: usize,
}

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
        return std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
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

    // 2. In dev mode (Cargo.toml exists) prefer the *nearest* existing assets/ragasukma.db,
    //    walking up from the current directory. This lets `cargo run` from a workspace root
    //    pick up the real DB in a member crate's assets/ instead of silently creating an
    //    empty one next to the root.
    if PathBuf::from("Cargo.toml").exists() {
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
            .expect("Failed to initialize SQLite pool for LLM usage");

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

#[allow(clippy::too_many_arguments)]
pub async fn record_usage_async(
    provider: &str,
    model: &str,
    input_tokens: usize,
    cached_tokens: usize,
    output_tokens: usize,
    duration_ms: u64,
    status: &str,
) -> anyhow::Result<LlmUsageRecord> {
    let pool = get_pool().await;
    record_usage_to_pool(
        pool,
        provider,
        model,
        input_tokens,
        cached_tokens,
        output_tokens,
        duration_ms,
        status,
    )
    .await
}

pub fn get_local_date_and_time() -> (String, String) {
    #[cfg(unix)]
    {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut tm).is_null() {
                return ("1970-01-01".to_string(), "00:00:00".to_string());
            }
            let date_str = format!(
                "{:04}-{:02}-{:02}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday
            );
            let time_str = format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec);
            (date_str, time_str)
        }
    }

    #[cfg(windows)]
    {
        let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        let date_str = format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay);
        let time_str = format!("{:02}:{:02}:{:02}", st.wHour, st.wMinute, st.wSecond);
        (date_str, time_str)
    }

    #[cfg(not(any(unix, windows)))]
    {
        ("1970-01-01".to_string(), "00:00:00".to_string())
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn record_usage_to_pool(
    pool: &SqlitePool,
    provider: &str,
    model: &str,
    input_tokens: usize,
    cached_tokens: usize,
    output_tokens: usize,
    duration_ms: u64,
    status: &str,
) -> anyhow::Result<LlmUsageRecord> {
    let (date_str, time_str) = get_local_date_and_time();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let id = format!("usage_{}_{}", timestamp, fastrand_hex(6));
    let total_tokens = (input_tokens + output_tokens) as i64;

    let record = LlmUsageRecord {
        id: id.clone(),
        timestamp,
        date_str: date_str.clone(),
        time_str: time_str.clone(),
        provider: provider.to_lowercase(),
        model: model.to_string(),
        input_tokens: input_tokens as i64,
        cached_tokens: cached_tokens as i64,
        output_tokens: output_tokens as i64,
        total_tokens,
        duration_ms: duration_ms as i64,
        status: status.to_lowercase(),
    };

    sqlx::query(
        r#"
        INSERT INTO usage_records (
            id, timestamp, date_str, time_str, provider, model,
            input_tokens, cached_tokens, output_tokens, total_tokens,
            duration_ms, status
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#,
    )
    .bind(&record.id)
    .bind(record.timestamp)
    .bind(&record.date_str)
    .bind(&record.time_str)
    .bind(&record.provider)
    .bind(&record.model)
    .bind(record.input_tokens)
    .bind(record.cached_tokens)
    .bind(record.output_tokens)
    .bind(record.total_tokens)
    .bind(record.duration_ms)
    .bind(&record.status)
    .execute(pool)
    .await?;

    Ok(record)
}

#[allow(clippy::too_many_arguments)]
pub fn record_usage(
    provider: &str,
    model: &str,
    input_tokens: usize,
    cached_tokens: usize,
    output_tokens: usize,
    duration_ms: u64,
    status: &str,
) {
    let provider = provider.to_string();
    let model = model.to_string();
    let status = status.to_string();

    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            let _ = record_usage_async(
                &provider,
                &model,
                input_tokens,
                cached_tokens,
                output_tokens,
                duration_ms,
                &status,
            )
            .await;
        });
    } else {
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            if let Ok(rt) = rt {
                rt.block_on(async {
                    let _ = record_usage_async(
                        &provider,
                        &model,
                        input_tokens,
                        cached_tokens,
                        output_tokens,
                        duration_ms,
                        &status,
                    )
                    .await;
                });
            }
        });
    }
}

pub async fn get_usage_summary(
    provider_filter: Option<&str>,
    date_filter: Option<&str>,
) -> LlmUsageSummary {
    let pool = get_pool().await;
    get_usage_summary_from_pool(pool, provider_filter, date_filter).await
}

pub async fn get_usage_summary_from_pool(
    pool: &SqlitePool,
    provider_filter: Option<&str>,
    date_filter: Option<&str>,
) -> LlmUsageSummary {
    let p_filter = provider_filter
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty() && s != "all");
    let d_filter = date_filter
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "all");

    let mut query = String::from(
        r#"
        SELECT
            COUNT(*) as count,
            COALESCE(SUM(input_tokens), 0) as total_in,
            COALESCE(SUM(cached_tokens), 0) as total_cached,
            COALESCE(SUM(output_tokens), 0) as total_out,
            COALESCE(SUM(total_tokens), 0) as total_all
        FROM usage_records
        WHERE 1=1
        "#,
    );

    if p_filter.is_some() {
        query.push_str(" AND provider LIKE ? ");
    }
    if d_filter.is_some() {
        query.push_str(" AND date_str = ? ");
    }

    let mut q = sqlx::query(&query);
    if let Some(ref p) = p_filter {
        q = q.bind(format!("%{}%", p));
    }
    if let Some(d) = d_filter {
        q = q.bind(d);
    }

    match q.fetch_one(pool).await {
        Ok(row) => {
            let total_requests: i64 = row.try_get("count").unwrap_or(0);
            let total_input_tokens: i64 = row.try_get("total_in").unwrap_or(0);
            let total_cached_tokens: i64 = row.try_get("total_cached").unwrap_or(0);
            let total_output_tokens: i64 = row.try_get("total_out").unwrap_or(0);
            let total_tokens: i64 = row.try_get("total_all").unwrap_or(0);

            let total_requests = total_requests.max(0) as usize;
            let total_input_tokens = total_input_tokens.max(0) as usize;
            let total_cached_tokens = total_cached_tokens.max(0) as usize;
            let total_output_tokens = total_output_tokens.max(0) as usize;
            let total_tokens = total_tokens.max(0) as usize;

            let cache_hit_rate_pct = if total_input_tokens > 0 {
                ((total_cached_tokens as f64 / (total_input_tokens + total_cached_tokens) as f64)
                    * 100.0) as f32
            } else {
                0.0
            };

            LlmUsageSummary {
                total_requests,
                total_input_tokens,
                total_cached_tokens,
                total_output_tokens,
                total_tokens,
                cache_hit_rate_pct,
                estimated_saved_tokens: total_cached_tokens,
            }
        }
        Err(_) => LlmUsageSummary::default(),
    }
}

pub async fn get_usage_records_paginated(
    provider_filter: Option<&str>,
    date_filter: Option<&str>,
    page: usize,
    limit: usize,
) -> LlmUsagePaginatedResponse {
    let pool = get_pool().await;
    get_usage_records_paginated_from_pool(pool, provider_filter, date_filter, page, limit).await
}

pub async fn get_usage_records_paginated_from_pool(
    pool: &SqlitePool,
    provider_filter: Option<&str>,
    date_filter: Option<&str>,
    page: usize,
    limit: usize,
) -> LlmUsagePaginatedResponse {
    let p_filter = provider_filter
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty() && s != "all");
    let d_filter = date_filter
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "all");

    let page = page.max(1);
    let limit = limit.clamp(1, 200);
    let offset = (page - 1) * limit;

    let mut count_query = String::from("SELECT COUNT(*) as cnt FROM usage_records WHERE 1=1");
    if p_filter.is_some() {
        count_query.push_str(" AND provider LIKE ? ");
    }
    if d_filter.is_some() {
        count_query.push_str(" AND date_str = ? ");
    }

    let mut cq = sqlx::query(&count_query);
    if let Some(ref p) = p_filter {
        cq = cq.bind(format!("%{}%", p));
    }
    if let Some(d) = d_filter {
        cq = cq.bind(d);
    }

    let total: usize = cq
        .fetch_one(pool)
        .await
        .ok()
        .and_then(|row| row.try_get::<i64, _>("cnt").ok())
        .map(|c| c.max(0) as usize)
        .unwrap_or(0);

    let total_pages = if total == 0 { 1 } else { total.div_ceil(limit) };

    let items =
        get_usage_records_from_pool(pool, provider_filter, date_filter, limit, offset).await;

    LlmUsagePaginatedResponse {
        items,
        total,
        page,
        limit,
        total_pages,
    }
}

pub async fn get_usage_records_from_pool(
    pool: &SqlitePool,
    provider_filter: Option<&str>,
    date_filter: Option<&str>,
    limit: usize,
    offset: usize,
) -> Vec<LlmUsageRecord> {
    let p_filter = provider_filter
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty() && s != "all");
    let d_filter = date_filter
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "all");

    let mut query = String::from("SELECT * FROM usage_records WHERE 1=1");

    if p_filter.is_some() {
        query.push_str(" AND provider LIKE ? ");
    }
    if d_filter.is_some() {
        query.push_str(" AND date_str = ? ");
    }

    query.push_str(" ORDER BY timestamp DESC, id DESC LIMIT ? OFFSET ?");

    let mut q = sqlx::query_as::<_, LlmUsageRecord>(&query);
    if let Some(ref p) = p_filter {
        q = q.bind(format!("%{}%", p));
    }
    if let Some(d) = d_filter {
        q = q.bind(d);
    }
    q = q.bind(limit as i64).bind(offset as i64);

    q.fetch_all(pool).await.unwrap_or_default()
}

pub async fn clear_usage() -> anyhow::Result<()> {
    let pool = get_pool().await;
    clear_usage_from_pool(pool).await
}

pub async fn clear_usage_from_pool(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM usage_records")
        .execute(pool)
        .await?;
    Ok(())
}

fn fastrand_hex(len: usize) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let s = RandomState::new().build_hasher().finish();
    let hex = format!("{:016x}", s);
    hex.chars().take(len).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sqlx_sqlite_usage_records_and_summary() {
        // Use an isolated in-memory pool so unit tests never clear or touch user's live database!
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let rec1 = record_usage_to_pool(
            &pool,
            "antigravity",
            "gemini-2.5-flash",
            1000,
            200,
            150,
            450,
            "success",
        )
        .await
        .unwrap();
        assert_eq!(rec1.total_tokens, 1150);

        let rec2 =
            record_usage_to_pool(&pool, "codex", "gpt-5-codex", 2000, 0, 300, 800, "success")
                .await
                .unwrap();
        assert_eq!(rec2.total_tokens, 2300);

        let summary = get_usage_summary_from_pool(&pool, None, None).await;
        assert_eq!(summary.total_requests, 2);
        assert_eq!(summary.total_input_tokens, 3000);
        assert_eq!(summary.total_cached_tokens, 200);
        assert_eq!(summary.total_output_tokens, 450);
        assert_eq!(summary.total_tokens, 3450);

        let ag_summary = get_usage_summary_from_pool(&pool, Some("antigravity"), None).await;
        assert_eq!(ag_summary.total_requests, 1);
        assert_eq!(ag_summary.total_input_tokens, 1000);
        assert_eq!(ag_summary.total_cached_tokens, 200);

        let records = get_usage_records_from_pool(&pool, None, None, 10, 0).await;
        assert_eq!(records.len(), 2);

        let _ = clear_usage_from_pool(&pool).await;
        let empty_summary = get_usage_summary_from_pool(&pool, None, None).await;
        assert_eq!(empty_summary.total_requests, 0);
    }

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
