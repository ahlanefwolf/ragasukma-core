use anyhow::Result;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

use super::models::Goal;
use super::repository;

/// Computes a sha256 checksum of a file to detect any file changes / drifts.
pub fn compute_file_version<P: AsRef<Path>>(path: P) -> Result<String> {
    let content = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    let hash = hasher.finalize();
    Ok(format!("{:x}", hash))
}

/// Revalidates existing evidence cache against actual files on disk.
/// If file content changed or file no longer exists, marks evidence as stale.
pub async fn revalidate_evidence_cache(
    project_id: &str,
    workspace_roots: &[String],
) -> Result<usize> {
    if workspace_roots.is_empty() {
        return Ok(0);
    }

    let evidences = repository::list_evidences_by_project(project_id).await?;
    let mut stale_count = 0;

    for ev in evidences {
        if ev.is_stale {
            continue;
        }

        // Try to locate file in workspace roots using standard path resolver
        let mut matched = false;
        let mut current_version = None;

        if let Ok(full_path) =
            crate::agent::tools::fs::resolve_allowed_path(&ev.file_path, workspace_roots)
            && full_path.exists()
        {
            matched = true;
            if let Ok(hash) = compute_file_version(&full_path) {
                current_version = Some(hash);
            }
        }

        // If file disappeared or hash does not match stored file_version
        if !matched || current_version.as_deref() != Some(&ev.file_version) {
            repository::mark_evidence_stale_by_file(project_id, &ev.file_path).await?;
            stale_count += 1;
        }
    }

    Ok(stale_count)
}

/// Discovers reusable evidence strictly from the parent goal or antecedent task in the same session.
/// Never leaks evidence from unrelated tasks or foreign sessions.
///
/// The relevance of each inherited file is measured by `evidence_scoring`, which weighs a request
/// word by how rare it is among the candidates. The earlier rule divided matches by the total number
/// of request words, so a long sentence pushed every score toward zero and inherited context lost to
/// whatever was read last.
///
/// Scores only ever rise. Inheritance adds knowledge about a file; it is not evidence that the file
/// has become less relevant, and `link_evidence_to_goal` overwrites the stored value.
pub async fn reuse_evidence_from_parent(
    new_goal_id: &str,
    parent_goal_id: &str,
    keywords: &[&str],
) -> Result<usize> {
    let parent_evidences = repository::get_active_evidences_for_goal(parent_goal_id).await?;

    // Stale rows are already excluded by the query; filtering again would silently score a
    // shorter candidate list than the one the caller believes it has.
    let candidates: Vec<crate::agent::evidence_scoring::Candidate<'_>> = parent_evidences
        .iter()
        .map(|ev| crate::agent::evidence_scoring::Candidate {
            file_path: ev.file_path.as_str(),
            symbol_name: ev.symbol_name.as_deref(),
            summary: ev.summary.as_str(),
        })
        .collect();

    // Callers pass words from the request that continues the task, already split for them.
    let request: Vec<String> = keywords
        .iter()
        .flat_map(|k| crate::agent::evidence_scoring::request_words(k))
        .collect();

    let mut linked_count = 0;
    for (index, ev) in parent_evidences.iter().enumerate() {
        let score = crate::agent::evidence_scoring::score_candidate(
            &request,
            &candidates,
            candidates.len(),
            index,
        );

        // A continuation with no distinguishing words still inherits, because the parent goal is
        // explicitly linked: the work continues, whatever the wording. It receives a modest score
        // rather than being dropped, since dropping it would discard the very context the link
        // exists to carry.
        let relevance_score = if score > 0.0 {
            score
        } else if request.is_empty() {
            CONTINUATION_ONLY_SCORE
        } else {
            continue;
        };

        repository::link_evidence_to_goal(new_goal_id, &ev.id, relevance_score, true).await?;
        linked_count += 1;
    }

    Ok(linked_count)
}

/// Score for evidence inherited purely because the goal is a continuation, with no wording that
/// singles it out.
///
/// Sits above a plain read (`evidence_ranking::SCORE_READ`) because a linked parent goal is stronger
/// evidence of continued relevance than the fact that a file was once opened, and below a fresh edit
/// so it never displaces work the agent is doing right now.
pub const CONTINUATION_ONLY_SCORE: f64 = 0.7;

/// Builds a compact, high-signal summary of previous goals strictly within the SAME session/thread.
/// Prevents cross-task context pollution from other unrelated tasks.
pub async fn build_previous_goals_summary(
    session_id: &str,
    current_goal_id: Option<&str>,
) -> Result<String> {
    let session_goals = repository::list_session_goals(session_id).await?;
    let previous_goals: Vec<_> = session_goals
        .into_iter()
        .filter(|g| {
            Some(g.id.as_str()) != current_goal_id
                // A goal only belongs here once it actually ended. Treating "has any conclusion
                // text" as the signal pulled live goals in, because a conclusion was written on
                // every turn — including turns that only asked the user a clarifying question.
                // Feeding that back made an unanswered question look like completed work.
                && matches!(g.status.as_str(), "completed" | "completed_unverified" | "failed")
        })
        .take(3)
        .collect();

    if previous_goals.is_empty() {
        return Ok(String::new());
    }

    let mut out = String::new();
    out.push_str("### 📜 Prior Accomplished Steps in this Task Session:\n");

    for g in previous_goals {
        out.push_str(&format!(
            "- **Step: \"{}\"** (Status: {})\n",
            g.title, g.status
        ));
        if let Some(ref conc) = g.conclusion {
            let conc_preview = conc.lines().next().unwrap_or(conc).trim();
            out.push_str(&format!("  * Outcome: {}\n", conc_preview));
        }

        if let Ok(verifs) = repository::list_goal_verifications(&g.id).await
            && let Some(last_v) = verifs.last()
        {
            let status_str = if last_v.is_passed {
                "PASSED"
            } else {
                "FAILED"
            };
            out.push_str(&format!(
                "  * Last Verification: `{}` ({})\n",
                last_v.test_command, status_str
            ));
        }

        let evidences = repository::get_active_evidences_for_goal(&g.id)
            .await
            .unwrap_or_default();
        if !evidences.is_empty() {
            out.push_str("  * Touched Files & Locations:\n");
            for ev in evidences.iter().take(4) {
                let lines_info = match (ev.start_line, ev.end_line) {
                    (Some(s), Some(e)) => format!(" (Lines {}-{})", s, e),
                    (Some(s), None) => format!(" (Line {}+)", s),
                    _ => String::new(),
                };
                out.push_str(&format!(
                    "    - `{}`{}: {}\n",
                    ev.file_path, lines_info, ev.summary
                ));
            }
        }
    }
    out.push('\n');

    Ok(out)
}

/// Builds the dynamic Goal Context and Active Evidence block for system prompt / active prompt.
///
/// `evidence_limit` caps how many evidence entries are injected. The block is rebuilt on every
/// turn, so an unbounded list would grow with the session and re-bill the same snippets over and
/// over. Callers pass the limit derived from the active model's capabilities.
pub async fn build_goal_context_prompt(goal: &Goal, evidence_limit: usize) -> Result<String> {
    let mut prompt = String::new();

    let criteria: Vec<String> = serde_json::from_str(&goal.success_criteria).unwrap_or_default();

    prompt.push_str("=== ACTIVE GOAL OBJECTIVE ===\n");
    prompt.push_str(&format!("Goal ID: {}\n", goal.id));
    prompt.push_str(&format!("Title: {}\n", goal.title));

    // This block is re-rendered on every turn, so each distinct piece of text is emitted
    // once and only once. Older goals persisted the demand verbatim as a criterion, which
    // made the prompt repeat several kilobytes of spec per turn; that is skipped here so
    // legacy rows do not keep paying for it.
    let criteria_repeat_demand = criteria.len() == 1 && criteria[0].trim() == goal.demand.trim();
    if !criteria_repeat_demand {
        prompt.push_str(&format!("Demand: {}\n", goal.demand));
    }
    prompt.push_str(&format!(
        "Current State: {}\n\n",
        goal.status.to_uppercase()
    ));

    // Success criteria
    if !criteria.is_empty() {
        prompt.push_str("Success Criteria (MUST all be verified before conclusion):\n");
        for (i, c) in criteria.iter().enumerate() {
            prompt.push_str(&format!("  {}. {}\n", i + 1, c));
        }
        prompt.push('\n');
    }

    // Constraints
    if let Ok(constraints) = serde_json::from_str::<Vec<String>>(&goal.constraints)
        && !constraints.is_empty()
    {
        prompt.push_str("Constraints & Guardrails:\n");
        for c in &constraints {
            prompt.push_str(&format!("  - {}\n", c));
        }
        prompt.push('\n');
    }

    // Active Evidence Cache (only fresh & active evidence)
    let active_evidences = repository::get_active_evidences_for_goal(&goal.id).await?;
    if !active_evidences.is_empty() {
        let shown = active_evidences.len().min(evidence_limit);
        prompt.push_str("=== ACTIVE EVIDENCE CACHE (Verified Relevant Code) ===\n");
        if shown < active_evidences.len() {
            prompt.push_str(&format!(
                "(showing the {} most relevant of {} cached entries; read a file again to surface it here)\n",
                shown,
                active_evidences.len()
            ));
        }
        for ev in active_evidences.iter().take(shown) {
            let line_range = match (ev.start_line, ev.end_line) {
                (Some(s), Some(e)) => format!(" [Lines {}-{}]", s, e),
                (Some(s), None) => format!(" [Line {}+]", s),
                _ => String::new(),
            };
            let symbol = ev
                .symbol_name
                .as_deref()
                .map(|s| format!(" (Symbol: `{}`)", s))
                .unwrap_or_default();

            prompt.push_str(&format!(
                "- `{}`{}{}\n  Version: {}\n  Summary: {}\n",
                ev.file_path, line_range, symbol, ev.file_version, ev.summary
            ));

            if let Some(ref snippet) = ev.raw_snippet
                && !snippet.trim().is_empty()
            {
                prompt.push_str("  Snippet:\n```\n");
                let short_snippet: String = snippet.lines().take(12).collect::<Vec<_>>().join("\n");
                prompt.push_str(short_snippet.trim());
                prompt.push_str("\n```\n");
            }
            prompt.push('\n');
        }
    } else {
        prompt.push_str("=== ACTIVE EVIDENCE CACHE ===\n(No active evidence yet. Perform Discovery Loop first.)\n\n");
    }

    prompt.push_str("=== ORCHESTRATION INSTRUCTIONS ===\n");
    prompt.push_str("- Discovery: Gather and verify facts. Inspect only relevant files.\n");
    prompt.push_str("- Action: Apply surgical edits once sufficient evidence is gathered.\n");
    prompt.push_str(
        "- Verification: Run unit/integration tests to confirm every success criterion.\n",
    );
    prompt.push_str("- Conclusion: Conclude only when verified.\n");

    Ok(prompt)
}

#[cfg(test)]
mod reuse_scoring_tests {
    use crate::agent::context_builder::reuse_evidence_from_parent;
    use crate::agent::repository;

    async fn session_with_two_goals() -> (String, String, String) {
        let project = repository::create_project("Reuse Scoring", "", None, None)
            .await
            .unwrap();
        let session = repository::create_session(&project.id, "S", None, None, None)
            .await
            .unwrap();
        let parent_id = repository::generate_id("goal_parent");
        repository::create_goal(
            &parent_id,
            &project.id,
            Some(&session.id),
            None,
            "Parent",
            "build login",
            "[]",
            "[]",
        )
        .await
        .unwrap();
        let child_id = repository::generate_id("goal_child");
        repository::create_goal(
            &child_id,
            &project.id,
            Some(&session.id),
            Some(&parent_id),
            "Child",
            "continue",
            "[]",
            "[]",
        )
        .await
        .unwrap();
        (project.id, parent_id, child_id)
    }

    async fn seed_parent_evidence(project_id: &str, parent_id: &str) {
        for (path, summary) in [
            ("src/auth/login.rs", "login handler"),
            ("src/ui/oauth.rs", "oauth consent screen"),
        ] {
            repository::save_evidence_and_link(
                project_id,
                path,
                "v1",
                None,
                None,
                None,
                summary,
                Some("snippet"),
                Some((parent_id, 1.0, true)),
            )
            .await
            .unwrap();
        }
    }

    /// Reads the stored relevance straight from `goal_evidence`.
    ///
    /// `get_active_evidences_for_goal` returns evidence rows sorted by score but does not carry the
    /// score itself, and the assertions here are about the stored value, not the ordering.
    async fn stored_score(goal_id: &str, evidence_id: &str) -> f64 {
        let pool = repository::pool().await;
        sqlx::query_scalar::<_, f64>(
            "SELECT relevance_score FROM goal_evidence WHERE goal_id = ?1 AND evidence_id = ?2",
        )
        .bind(goal_id)
        .bind(evidence_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// A request full of connective words must not bury the one file it is about. Under the old
    /// rule the divisor was the whole sentence, so this file scored near zero and was dropped from
    /// the prompt whenever it filled up.
    #[tokio::test]
    async fn a_continuing_request_keeps_the_relevant_file_ranked_first() {
        let _guard = repository::testsupport::db_test_lock().await;
        let (project_id, parent_id, child_id) = session_with_two_goals().await;
        seed_parent_evidence(&project_id, &parent_id).await;

        reuse_evidence_from_parent(
            &child_id,
            &parent_id,
            &[
                "tolong", "perbaiki", "login", "yang", "masih", "error", "ya", "bro",
            ],
        )
        .await
        .unwrap();

        let inherited = repository::get_active_evidences_for_goal(&child_id)
            .await
            .unwrap();
        assert_eq!(
            inherited.len(),
            1,
            "only the relevant file should be inherited"
        );
        assert_eq!(inherited[0].file_path, "src/auth/login.rs");
    }

    /// Inheriting a file must never demote it.
    ///
    /// The request here is deliberately *weak*: it shares one word with every candidate, so the
    /// computed inheritance score is below the 1.0 the child already holds. Without that, an
    /// overwrite would happen to produce the same value and the test would pass even with the bug in
    /// place — which is exactly what happened before this fixture was tightened.
    #[tokio::test]
    async fn reusing_never_lowers_an_existing_score() {
        let _guard = repository::testsupport::db_test_lock().await;
        let (project_id, parent_id, child_id) = session_with_two_goals().await;
        seed_parent_evidence(&project_id, &parent_id).await;

        // The child already knows this file from its own work, at full relevance.
        let parent_evs = repository::get_active_evidences_for_goal(&parent_id)
            .await
            .unwrap();
        let login = parent_evs
            .iter()
            .find(|e| e.file_path == "src/auth/login.rs")
            .unwrap();
        repository::link_evidence_to_goal(&child_id, &login.id, 1.0, true)
            .await
            .unwrap();

        // Both request words are distinctive, but only one of them matches the login file, so the
        // computed inheritance score is 0.5 — genuinely below the 1.0 the child already holds.
        reuse_evidence_from_parent(&child_id, &parent_id, &["login", "oauth"])
            .await
            .unwrap();

        assert_eq!(
            stored_score(&child_id, &login.id).await,
            1.0,
            "inheritance must not overwrite a higher score"
        );
    }

    /// A continuation with nothing distinctive still inherits: the parent goal is explicitly linked,
    /// so the context is wanted even when the wording gives no clue.
    #[tokio::test]
    async fn a_bare_continuation_still_inherits() {
        let _guard = repository::testsupport::db_test_lock().await;
        let (project_id, parent_id, child_id) = session_with_two_goals().await;
        seed_parent_evidence(&project_id, &parent_id).await;

        let linked = reuse_evidence_from_parent(&child_id, &parent_id, &[])
            .await
            .unwrap();
        assert_eq!(linked, 2, "both files belong to the continued work");
        let inherited = repository::get_active_evidences_for_goal(&child_id)
            .await
            .unwrap();
        assert_eq!(inherited.len(), 2);
    }
}

#[cfg(test)]
mod conclusion_tests {
    use crate::agent::orchestrator::{GoalOrchestrator, GoalState};
    use crate::agent::repository;

    async fn goal_in_new_session() -> (String, String) {
        let project = repository::create_project("Conclusion Rules", "", None, None)
            .await
            .unwrap();
        let session = repository::create_session(&project.id, "S", None, None, None)
            .await
            .unwrap();
        let goal_id = repository::generate_id("goal_concl");
        repository::create_goal(
            &goal_id,
            &project.id,
            Some(&session.id),
            None,
            "Conclude goal",
            "demand",
            "[]",
            "[]",
        )
        .await
        .unwrap();
        (project.id, goal_id)
    }

    /// Work that nothing verified must not be labelled `completed`. That label is reserved for a
    /// goal a passing verification record actually backs.
    #[tokio::test]
    async fn work_without_verification_is_completed_unverified() {
        let _guard = repository::testsupport::db_test_lock().await;
        let (project_id, goal_id) = goal_in_new_session().await;
        let orch = GoalOrchestrator::new(&goal_id, &project_id);

        orch.conclude_after_work("implemented the feature")
            .await
            .unwrap();

        let goal = repository::get_goal(&goal_id).await.unwrap().unwrap();
        assert_eq!(goal.status, "completed_unverified");
        assert_eq!(goal.conclusion.as_deref(), Some("implemented the feature"));
    }

    /// The same entry point must still produce a fully verified `completed` when evidence exists,
    /// so the fallback never downgrades a genuinely proven result.
    #[tokio::test]
    async fn verified_work_is_completed() {
        let _guard = repository::testsupport::db_test_lock().await;
        let (project_id, goal_id) = goal_in_new_session().await;
        repository::record_verification_with_status(
            &goal_id,
            1,
            "cargo test",
            "ok",
            true,
            None,
            None,
        )
        .await
        .unwrap();

        GoalOrchestrator::new(&goal_id, &project_id)
            .conclude_after_work("tests pass")
            .await
            .unwrap();

        let goal = repository::get_goal(&goal_id).await.unwrap().unwrap();
        assert_eq!(goal.status, "completed");
    }

    /// Closing a goal straight to `completed` with no verification is exactly the write that turned
    /// a clarifying question into a finished goal. It must be rejected.
    #[tokio::test]
    async fn asserting_completion_without_a_verification_is_refused() {
        let _guard = repository::testsupport::db_test_lock().await;
        let (project_id, goal_id) = goal_in_new_session().await;
        let orch = GoalOrchestrator::new(&goal_id, &project_id);

        let res = orch
            .conclude_with_status(GoalState::Completed, "done")
            .await;
        assert!(
            res.is_err(),
            "completed must require a passing verification"
        );
        let goal = repository::get_goal(&goal_id).await.unwrap().unwrap();
        assert_ne!(goal.status, "completed");
        assert!(goal.conclusion.is_none());
    }

    /// A goal that is still running must never appear in the prior-steps summary, even when it has
    /// conclusion text.
    ///
    /// This is the shape the bug took in production: every turn wrote a conclusion, so an
    /// unanswered clarifying question was replayed to the model as completed work. Two details make
    /// the fixture able to catch a regression: the open goal is *not* the current one (otherwise it
    /// is filtered by id before status is ever consulted), and it *carries* conclusion text
    /// (otherwise the broken and the fixed filter agree).
    #[tokio::test]
    async fn an_open_goal_never_surfaces_as_a_prior_step() {
        let _guard = repository::testsupport::db_test_lock().await;
        let (_project_id, current_goal_id) = goal_in_new_session().await;
        let current = repository::get_goal(&current_goal_id)
            .await
            .unwrap()
            .unwrap();
        let session_id = current.session_id.clone().unwrap();

        let open_goal_id = repository::generate_id("goal_open");
        repository::create_goal(
            &open_goal_id,
            &current.project_id,
            Some(&session_id),
            None,
            "Still running",
            "do the work",
            "[]",
            "[]",
        )
        .await
        .unwrap();
        repository::update_goal_conclusion(&open_goal_id, "Which framework should I use?")
            .await
            .unwrap();

        let summary = crate::agent::context_builder::build_previous_goals_summary(
            &session_id,
            Some(&current_goal_id),
        )
        .await
        .unwrap();
        assert!(
            summary.is_empty(),
            "an open goal must not surface as a prior step even with conclusion text: {summary}"
        );

        // Once it really ended, it belongs in the summary regardless of how it ended.
        repository::conclude_goal(&open_goal_id, "completed_unverified", "did the work")
            .await
            .unwrap();
        let summary = crate::agent::context_builder::build_previous_goals_summary(
            &session_id,
            Some(&current_goal_id),
        )
        .await
        .unwrap();
        assert!(
            summary.contains("did the work"),
            "concluded goal must surface: {summary}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::repository;

    /// The evidence block is rebuilt on every turn, so it must respect the caller's cap instead of
    /// dumping every cached file into the prompt.
    #[tokio::test]
    async fn goal_prompt_injects_at_most_the_evidence_limit() {
        let _guard = repository::testsupport::db_test_lock().await;
        let project = repository::create_project("Evidence Cap", "", None, None)
            .await
            .unwrap();
        let session = repository::create_session(&project.id, "S", None, None, None)
            .await
            .unwrap();
        let goal_id = format!("G-cap-{}", std::process::id());
        let goal = repository::create_goal(
            &goal_id,
            &project.id,
            Some(&session.id),
            None,
            "Cap goal",
            "read 20 files",
            "[]",
            "[]",
        )
        .await
        .unwrap();

        for i in 0..20 {
            repository::save_evidence_and_link(
                &project.id,
                &format!("src/file_{i}.rs"),
                "v1",
                Some(i as i64),
                Some(i as i64 + 5),
                None,
                &format!("summary {i}"),
                Some(&format!("snippet body {i}")),
                Some((&goal.id, 1.0, true)),
            )
            .await
            .unwrap();
        }

        let capped = build_goal_context_prompt(&goal, 5).await.unwrap();
        assert_eq!(
            capped.matches("Snippet:").count(),
            5,
            "cap must limit injected snippets"
        );
        assert!(
            capped.contains("of 20 cached entries"),
            "prompt must disclose that entries were omitted"
        );

        let uncapped = build_goal_context_prompt(&goal, usize::MAX).await.unwrap();
        assert_eq!(uncapped.matches("Snippet:").count(), 20);
    }

    /// The whole point of ranking: when the cap cuts the evidence list, an edited file must
    /// survive and a merely-read file must be the one dropped.
    #[tokio::test]
    async fn edited_files_survive_the_cap_and_reads_are_dropped_first() {
        let _guard = repository::testsupport::db_test_lock().await;
        let project = repository::create_project("Rank Cap", "", None, None)
            .await
            .unwrap();
        let session = repository::create_session(&project.id, "S", None, None, None)
            .await
            .unwrap();
        let goal_id = format!("G-rank-{}", std::process::id());
        let goal = repository::create_goal(
            &goal_id,
            &project.id,
            Some(&session.id),
            None,
            "Rank goal",
            "do things",
            "[]",
            "[]",
        )
        .await
        .unwrap();

        use crate::agent::evidence_ranking::{EvidenceOrigin, rank_evidence};

        // Two files are only read; only one is actually edited.
        for i in 0..2 {
            repository::save_evidence_and_link(
                &project.id,
                &format!("src/read_{i}.rs"),
                "v1",
                None,
                None,
                None,
                "read only",
                Some("read snippet"),
                Some((&goal.id, rank_evidence(EvidenceOrigin::Read, 0), true)),
            )
            .await
            .unwrap();
        }
        repository::save_evidence_and_link(
            &project.id,
            "src/edited.rs",
            "v1",
            None,
            None,
            None,
            "the file that was changed",
            Some("edited snippet"),
            Some((&goal.id, rank_evidence(EvidenceOrigin::Modified, 0), true)),
        )
        .await
        .unwrap();

        // Cap of 1 keeps only the most relevant entry.
        let prompt = build_goal_context_prompt(&goal, 1).await.unwrap();
        assert!(
            prompt.contains("src/edited.rs"),
            "the edited file must survive the cap: {prompt}"
        );
        assert!(
            !prompt.contains("src/read_0.rs") && !prompt.contains("src/read_1.rs"),
            "read-only files must be dropped first: {prompt}"
        );
    }

    /// The goal block is rebuilt on every turn, so any text it repeats is paid for on
    /// every turn too.
    ///
    /// A real session stored the demand verbatim as the single success criterion, and
    /// because the prompt printed both sections the specification appeared twice per
    /// turn. For a 6 KB spec across 29 turns that was tens of thousands of wasted tokens.
    #[tokio::test]
    async fn a_criterion_that_merely_repeats_the_demand_is_not_printed_twice() {
        let _guard = repository::testsupport::db_test_lock().await;
        let project = repository::create_project("Duplicate Prompt", "", None, None)
            .await
            .unwrap();
        let session = repository::create_session(&project.id, "S", None, None, None)
            .await
            .unwrap();
        let demand = "Refactor the authentication module so that expired tokens are rejected";
        let goal_id = format!("G-dup-{}", std::process::id());
        let criteria = serde_json::to_string(&vec![demand.to_string()]).unwrap();
        let goal = repository::create_goal(
            &goal_id,
            &project.id,
            Some(&session.id),
            None,
            "dup",
            demand,
            &criteria,
            "[]",
        )
        .await
        .unwrap();

        let prompt = build_goal_context_prompt(&goal, usize::MAX).await.unwrap();
        assert_eq!(
            prompt.matches(demand).count(),
            1,
            "the demand must appear exactly once per turn, not once per section: {prompt}"
        );
    }

    /// Distinct criteria must still be shown, and the demand must not be swallowed along
    /// with them. The dedup above must not turn into "drop the objective when criteria exist".
    #[tokio::test]
    async fn distinct_criteria_and_demand_are_both_kept() {
        let _guard = repository::testsupport::db_test_lock().await;
        let project = repository::create_project("Distinct Prompt", "", None, None)
            .await
            .unwrap();
        let session = repository::create_session(&project.id, "S", None, None, None)
            .await
            .unwrap();
        let demand = "Add a health endpoint";
        let goal_id = format!("G-dist-{}", std::process::id());
        let criteria = serde_json::to_string(&vec![
            "GET /health returns 200".to_string(),
            "response body is JSON".to_string(),
        ])
        .unwrap();
        let goal = repository::create_goal(
            &goal_id,
            &project.id,
            Some(&session.id),
            None,
            "dist",
            demand,
            &criteria,
            "[]",
        )
        .await
        .unwrap();

        let prompt = build_goal_context_prompt(&goal, usize::MAX).await.unwrap();
        assert!(
            prompt.contains(demand),
            "demand must still be shown: {prompt}"
        );
        assert!(
            prompt.contains("GET /health returns 200"),
            "criteria must be shown: {prompt}"
        );
        assert!(
            prompt.contains("response body is JSON"),
            "criteria must be shown: {prompt}"
        );
    }
}
