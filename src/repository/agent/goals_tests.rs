use super::*;

async fn seed_goal() -> (String, String) {
    let project = super::super::create_project("TxTest", "", None, None)
        .await
        .unwrap();
    let session = super::super::create_session(&project.id, "S", None, None, None)
        .await
        .unwrap();
    let goal_id = generate_id("goal_tx");
    create_goal(
        &goal_id,
        &project.id,
        Some(&session.id),
        None,
        "Tx goal",
        "demand",
        "[]",
        "[]",
    )
    .await
    .unwrap();
    (project.id, goal_id)
}

#[tokio::test]
async fn verification_and_status_commit_together() {
    let _guard = crate::repository::agent::testsupport::db_test_lock().await;
    let (_pid, goal_id) = seed_goal().await;

    record_verification_with_status(&goal_id, 1, "cargo test", "ok", true, None, Some("action"))
        .await
        .unwrap();

    let verifs = list_goal_verifications(&goal_id).await.unwrap();
    assert_eq!(verifs.len(), 1);
    assert!(verifs[0].is_passed);

    let goal = get_goal(&goal_id).await.unwrap().unwrap();
    assert_eq!(goal.status, "action", "status must follow the verification");
}

#[tokio::test]
async fn verification_rolls_back_when_goal_is_missing() {
    let _guard = crate::repository::agent::testsupport::db_test_lock().await;
    // The insert would succeed on its own; the failed status update must undo it.
    let ghost = generate_id("ghost_goal");
    let res =
        record_verification_with_status(&ghost, 1, "cargo test", "ok", true, None, Some("action"))
            .await;
    assert!(res.is_err(), "expected failure for missing goal");

    let leftovers = list_goal_verifications(&ghost).await.unwrap();
    assert!(
        leftovers.is_empty(),
        "no verification row may survive a rolled-back transaction"
    );
}

#[tokio::test]
async fn conclude_requires_a_passing_verification() {
    let _guard = crate::repository::agent::testsupport::db_test_lock().await;
    let (_pid, goal_id) = seed_goal().await;

    let res = conclude_goal_if_verified(&goal_id, "completed", "done").await;
    assert!(res.is_err(), "concluding without verification must fail");

    let goal = get_goal(&goal_id).await.unwrap().unwrap();
    assert_ne!(goal.status, "completed");
}

#[tokio::test]
async fn conclude_succeeds_after_passing_verification() {
    let _guard = crate::repository::agent::testsupport::db_test_lock().await;
    let (_pid, goal_id) = seed_goal().await;
    record_verification_with_status(&goal_id, 1, "cargo test", "ok", true, None, None)
        .await
        .unwrap();

    conclude_goal_if_verified(&goal_id, "completed", "done")
        .await
        .unwrap();

    let goal = get_goal(&goal_id).await.unwrap().unwrap();
    assert_eq!(goal.status, "completed");
    assert_eq!(goal.phase, "concluded");
}

#[tokio::test]
async fn evidence_and_link_are_written_together() {
    let _guard = crate::repository::agent::testsupport::db_test_lock().await;
    let (project_id, goal_id) = seed_goal().await;

    let ev = save_evidence_and_link(
        &project_id,
        "src/a.rs",
        "v1",
        Some(1),
        Some(9),
        None,
        "summary",
        None,
        Some((&goal_id, 1.0, true)),
    )
    .await
    .unwrap();

    let linked = get_active_evidences_for_goal(&goal_id).await.unwrap();
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0].id, ev.id);
}

/// Re-reading an edited file must not downgrade its rank, otherwise the prompt cap could drop
/// the very file the agent changed.
#[tokio::test]
async fn rereading_an_edited_file_keeps_its_higher_rank() {
    let _guard = crate::repository::agent::testsupport::db_test_lock().await;
    let (project_id, goal_id) = seed_goal().await;
    let modified = crate::agent::evidence_ranking::rank_evidence(
        crate::agent::evidence_ranking::EvidenceOrigin::Modified,
        0,
    );
    let read = crate::agent::evidence_ranking::rank_evidence(
        crate::agent::evidence_ranking::EvidenceOrigin::Read,
        0,
    );

    save_evidence_and_link(
        &project_id,
        "src/a.rs",
        "v1",
        None,
        None,
        None,
        "edited",
        None,
        Some((&goal_id, modified, true)),
    )
    .await
    .unwrap();
    // Agent reads the same file again later in the session.
    save_evidence_and_link(
        &project_id,
        "src/a.rs",
        "v2",
        None,
        None,
        None,
        "read again",
        None,
        Some((&goal_id, read, true)),
    )
    .await
    .unwrap();

    let score: f64 = sqlx::query_scalar(
        "SELECT ge.relevance_score FROM goal_evidence ge              JOIN evidence_cache e ON e.id = ge.evidence_id              WHERE ge.goal_id = ?1 AND e.file_path = 'src/a.rs'",
    )
    .bind(&goal_id)
    .fetch_one(super::super::pool().await)
    .await
    .unwrap();

    assert_eq!(
        score, modified,
        "rank must never regress when a file is re-read"
    );
}

/// Recency must actually reach the production read path, not just the pure ranking helper.
///
/// The fixture gives the *older* file a **higher stored score** so the `updated_at` tie-break
/// cannot produce the expected order by itself: only an applied recency factor can push the
/// recent-but-lower-ranked file to the top.
#[tokio::test]
async fn stale_evidence_is_ordered_below_recently_touched_evidence() {
    let _guard = crate::repository::agent::testsupport::db_test_lock().await;
    let (project_id, goal_id) = seed_goal().await;
    let pool = super::super::pool().await;

    // Old file: stored score 1.0, but aged past the recency floor.
    save_evidence_and_link(
        &project_id,
        "old.rs",
        "v1",
        None,
        None,
        None,
        "s",
        None,
        Some((&goal_id, 1.0, true)),
    )
    .await
    .unwrap();
    // New file: stored score 0.8, freshly observed.
    save_evidence_and_link(
        &project_id,
        "new.rs",
        "v1",
        None,
        None,
        None,
        "s",
        None,
        Some((&goal_id, 0.8, true)),
    )
    .await
    .unwrap();

    // Age the old entry well past the decay window so its factor sits at the floor (0.5).
    sqlx::query("UPDATE evidence_cache SET updated_at = updated_at - 864000 WHERE project_id = ?1 AND file_path = 'old.rs'")
        .bind(&project_id)
        .execute(pool)
        .await
        .unwrap();

    let ordered = get_active_evidences_for_goal(&goal_id).await.unwrap();
    let paths: Vec<&str> = ordered.iter().map(|e| e.file_path.as_str()).collect();
    // Without recency the order would be old.rs (1.0) then new.rs (0.8).
    assert_eq!(
        paths,
        vec!["new.rs", "old.rs"],
        "a recently touched file must outrank an old file with a higher static score"
    );
}
