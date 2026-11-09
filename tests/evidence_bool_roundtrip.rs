//! Integration tests for the SQLite <-> Rust `bool` round-trip (PR 2 lock-in).
//!
//! SQLite has no native boolean type: columns like `is_stale`, `is_active` and
//! `is_passed` are declared `INTEGER` and store `1`/`0`. The model structs decode
//! these into Rust `bool` via `sqlx::FromRow`. These tests exercise the *real*
//! schema (through the embedded migrations) rather than a hand-rolled table, so a
//! regression in either the migration or the model decode shows up here.
//!
//! They are deliberately black-box: they only touch `pub` API
//! (`ragasukma_core::repository::migrate_database` + the public model structs).

use ragasukma_core::agent::{EvidenceCache, GoalEvidence, GoalVerification};
use ragasukma_core::repository::migrate_database;
use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;

/// Spins up a brand-new, migrated in-memory SQLite pool owned by one test.
///
/// `max_connections(1)` is intentional: SQLite `:memory:` databases are
/// per-connection, so all statements must share the single connection to see
/// the migrated schema.
async fn fresh_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    migrate_database(&pool)
        .await
        .expect("run embedded migrations");
    pool
}

/// Inserts a minimal project + goal so the FK-bearing evidence tables accept rows.
async fn seed_project_and_goal(pool: &SqlitePool) {
    let now = 1_700_000_000_i64;
    sqlx::query(
        "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p1', 'proj', ?1, ?1)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("insert project");

    sqlx::query(
        "INSERT INTO goals (id, project_id, title, demand, created_at, updated_at) \
         VALUES ('g1', 'p1', 'goal', 'demand', ?1, ?1)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("insert goal");
}

#[tokio::test]
async fn evidence_cache_decodes_is_stale_both_ways() {
    let pool = fresh_pool().await;
    seed_project_and_goal(&pool).await;

    // Insert one stale (1) and one fresh (0) row straight through SQL.
    for (id, path, is_stale) in [
        ("e_stale", "src/stale.rs", 1_i64),
        ("e_fresh", "src/fresh.rs", 0_i64),
    ] {
        sqlx::query(
            "INSERT INTO evidence_cache \
             (id, project_id, file_path, file_version, summary, is_stale, created_at, updated_at) \
             VALUES (?1, 'p1', ?2, 'v1', 'sum', ?3, 1, 1)",
        )
        .bind(id)
        .bind(path)
        .bind(is_stale)
        .execute(&pool)
        .await
        .expect("insert evidence_cache");
    }

    let stale: EvidenceCache = sqlx::query_as("SELECT * FROM evidence_cache WHERE id = 'e_stale'")
        .fetch_one(&pool)
        .await
        .expect("fetch stale");
    let fresh: EvidenceCache = sqlx::query_as("SELECT * FROM evidence_cache WHERE id = 'e_fresh'")
        .fetch_one(&pool)
        .await
        .expect("fetch fresh");

    assert!(stale.is_stale, "INTEGER 1 must decode to bool true");
    assert!(!fresh.is_stale, "INTEGER 0 must decode to bool false");
}

#[tokio::test]
async fn goal_evidence_decodes_is_active_both_ways() {
    let pool = fresh_pool().await;
    seed_project_and_goal(&pool).await;

    // goal_evidence FKs to an evidence_cache row; create one active link only.
    sqlx::query(
        "INSERT INTO evidence_cache \
         (id, project_id, file_path, file_version, summary, is_stale, created_at, updated_at) \
         VALUES ('e1', 'p1', 'src/x.rs', 'v1', 'sum', 0, 1, 1)",
    )
    .execute(&pool)
    .await
    .expect("insert evidence_cache");

    for (evidence_id, is_active) in [("e1", 1_i64)] {
        sqlx::query(
            "INSERT INTO goal_evidence (goal_id, evidence_id, relevance_score, is_active) \
             VALUES ('g1', ?1, 0.9, ?2)",
        )
        .bind(evidence_id)
        .bind(is_active)
        .execute(&pool)
        .await
        .expect("insert goal_evidence");
    }
    // A second, inactive mapping to the same goal needs a second evidence row
    // (the PK is (goal_id, evidence_id)).
    sqlx::query(
        "INSERT INTO evidence_cache \
         (id, project_id, file_path, file_version, summary, is_stale, created_at, updated_at) \
         VALUES ('e2', 'p1', 'src/y.rs', 'v1', 'sum', 0, 1, 1)",
    )
    .execute(&pool)
    .await
    .expect("insert evidence_cache e2");
    sqlx::query(
        "INSERT INTO goal_evidence (goal_id, evidence_id, relevance_score, is_active) \
         VALUES ('g1', 'e2', 0.1, 0)",
    )
    .execute(&pool)
    .await
    .expect("insert inactive goal_evidence");

    let active: GoalEvidence =
        sqlx::query_as("SELECT * FROM goal_evidence WHERE goal_id = 'g1' AND evidence_id = 'e1'")
            .fetch_one(&pool)
            .await
            .expect("fetch active");
    let inactive: GoalEvidence =
        sqlx::query_as("SELECT * FROM goal_evidence WHERE goal_id = 'g1' AND evidence_id = 'e2'")
            .fetch_one(&pool)
            .await
            .expect("fetch inactive");

    assert!(active.is_active, "INTEGER 1 must decode to bool true");
    assert!(!inactive.is_active, "INTEGER 0 must decode to bool false");
}

#[tokio::test]
async fn goal_verification_decodes_is_passed_both_ways() {
    let pool = fresh_pool().await;
    seed_project_and_goal(&pool).await;

    for (id, is_passed) in [("v_pass", 1_i64), ("v_fail", 0_i64)] {
        sqlx::query(
            "INSERT INTO goal_verifications \
             (id, goal_id, iteration, test_command, test_output, is_passed, created_at) \
             VALUES (?1, 'g1', 1, 'cargo test', 'out', ?2, 1)",
        )
        .bind(id)
        .bind(is_passed)
        .execute(&pool)
        .await
        .expect("insert goal_verification");
    }

    let passed: GoalVerification =
        sqlx::query_as("SELECT * FROM goal_verifications WHERE id = 'v_pass'")
            .fetch_one(&pool)
            .await
            .expect("fetch passed");
    let failed: GoalVerification =
        sqlx::query_as("SELECT * FROM goal_verifications WHERE id = 'v_fail'")
            .fetch_one(&pool)
            .await
            .expect("fetch failed");

    assert!(passed.is_passed, "INTEGER 1 must decode to bool true");
    assert!(!failed.is_passed, "INTEGER 0 must decode to bool false");
}
