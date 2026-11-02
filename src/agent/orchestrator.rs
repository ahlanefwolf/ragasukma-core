use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::context_builder::{build_goal_context_prompt, revalidate_evidence_cache, reuse_evidence_from_parent};
use super::models::{Goal, GoalVerification};
use super::repository;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalState {
    Initialized,
    Discovery,
    Action,
    Verification,
    /// Work is done and a passing verification record proves it.
    Completed,
    /// Work is done but nothing could verify it.
    ///
    /// Kept separate from `Completed` on purpose. A project with no test suite, or a change to
    /// non-executable assets, has nothing to run — the turn genuinely ended, but no evidence backs
    /// it. Folding both meanings into `completed` made the status unreadable: you could not tell an
    /// audited result from an optimistic one.
    CompletedUnverified,
    Failed,
}

impl GoalState {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalState::Initialized => "initialized",
            GoalState::Discovery => "discovery",
            GoalState::Action => "action",
            GoalState::Verification => "verification",
            GoalState::Completed => "completed",
            GoalState::CompletedUnverified => "completed_unverified",
            GoalState::Failed => "failed",
        }
    }
}

impl std::str::FromStr for GoalState {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "discovery" => GoalState::Discovery,
            "action" => GoalState::Action,
            "verification" => GoalState::Verification,
            "completed" => GoalState::Completed,
            "completed_unverified" => GoalState::CompletedUnverified,
            "failed" => GoalState::Failed,
            _ => GoalState::Initialized,
        })
    }
}

pub struct GoalOrchestrator {
    pub goal_id: String,
    pub project_id: String,
    pub max_verification_loops: usize,
}

impl GoalOrchestrator {
    pub fn new(goal_id: &str, project_id: &str) -> Self {
        Self {
            goal_id: goal_id.to_string(),
            project_id: project_id.to_string(),
            max_verification_loops: 5,
        }
    }

    /// Step 1: Initialize Goal and prepare initial reusable context
    #[allow(clippy::too_many_arguments)]
    pub async fn initialize_demand(
        goal_id: &str,
        project_id: &str,
        session_id: Option<&str>,
        parent_goal_id: Option<&str>,
        title: &str,
        demand: &str,
        success_criteria: &[String],
        constraints: &[String],
        reuse_keywords: &[&str],
        workspace_roots: &[String],
    ) -> Result<Goal> {
        let sc_json = serde_json::to_string(success_criteria)?;
        let const_json = serde_json::to_string(constraints)?;

        let goal = repository::create_goal(
            goal_id,
            project_id,
            session_id,
            parent_goal_id,
            title,
            demand,
            &sc_json,
            &const_json,
        )
        .await?;

        // 1. Revalidate any stale cache on disk with degradation tracking
        match revalidate_evidence_cache(project_id, workspace_roots).await {
            Ok(_stale_count) => {}
            Err(e) => {
                let err_str = e.to_string();
                eprintln!("[Agent Context Warning] Evidence cache revalidation failed for goal {}: {}. Continuing in degraded mode.", goal_id, err_str);
                let _ = repository::update_goal_context_health(goal_id, "degraded", Some(&err_str)).await;
            }
        }

        // 2. Reuse fresh evidence strictly if parent goal is linked
        //
        // Inheritance is unconditional. It used to require at least one reuse keyword, which meant a
        // vague continuation ("lanjut") inherited nothing — the exact case where carrying the parent's
        // context matters most. Scoring already decides how strongly each file is carried over, and a
        // wordless continuation has a defined result (`CONTINUATION_ONLY_SCORE`), so the guard no longer
        // protects against anything. Keywords remain optional input that sharpens the score.
        if let Some(parent_id) = parent_goal_id
            && let Err(e) = reuse_evidence_from_parent(goal_id, parent_id, reuse_keywords).await {
                let err_str = e.to_string();
                eprintln!("[Agent Context Warning] Failed to reuse evidence from parent goal {} for {}: {}", parent_id, goal_id, err_str);
                let _ = repository::update_goal_context_health(goal_id, "degraded", Some(&err_str)).await;
            }

        // 3. Move state to discovery
        repository::update_goal_status(goal_id, GoalState::Discovery.as_str()).await?;
        let mut goal = goal;
        goal.status = GoalState::Discovery.as_str().to_string();

        Ok(goal)
    }

    /// Step 5: Verification Loop execution & evaluation
    pub async fn verify_iteration(
        &self,
        iteration: i64,
        test_command: &str,
        test_output: &str,
        is_passed: bool,
        feedback: Option<&str>,
    ) -> Result<GoalVerification> {
        // Decide the follow-up state first, then persist verification and the status
        // transition in one transaction so history and goal state cannot diverge.
        let next_status: Option<&str> = if is_passed {
            None
        } else if iteration < self.max_verification_loops as i64 {
            // Loop back to Action by default, or Discovery if feedback implies missing facts
            let next_state = if feedback
                .map(|f| f.contains("not found") || f.contains("unknown"))
                .unwrap_or(false)
            {
                GoalState::Discovery
            } else {
                GoalState::Action
            };
            Some(next_state.as_str())
        } else {
            // Reached max retries
            Some(GoalState::Failed.as_str())
        };

        let verif = repository::record_verification_with_status(
            &self.goal_id,
            iteration,
            test_command,
            test_output,
            is_passed,
            feedback,
            next_status,
        )
        .await?;

        Ok(verif)
    }

    /// Step 6: Conclusion - marks goal completed only if verified
    ///
    /// The "is it verified?" check and the status update are evaluated inside one
    /// transaction, so a goal can never be concluded on the strength of a verification
    /// that was rolled back (or miss one that landed concurrently).
    pub async fn conclude(
        &self,
        conclusion_summary: &str,
    ) -> Result<()> {
        repository::conclude_goal_if_verified(
            &self.goal_id,
            GoalState::Completed.as_str(),
            conclusion_summary,
        )
        .await
    }

    /// Step 6b: Close a goal whose turn ended with a failure or with no work done.
    ///
    /// `Completed` is deliberately rejected here. A failing turn closing itself as completed is
    /// never truthful, and every other status means "nothing is proven", so the unguarded write
    /// is harmless for them. Calls that want `Completed` must go through [`Self::conclude`],
    /// which demands a passing verification.
    pub async fn conclude_with_status(
        &self,
        status: GoalState,
        conclusion_summary: &str,
    ) -> Result<()> {
        if status == GoalState::Completed {
            anyhow::bail!(
                "A goal cannot be closed as completed on assertion alone; use conclude() so a passing verification is required"
            );
        }
        repository::conclude_goal(&self.goal_id, status.as_str(), conclusion_summary).await?;
        Ok(())
    }

    /// Step 6c: Close a goal whose work finished without a verification record to back it.
    ///
    /// Falls back to [`Self::conclude`] when verification does exist, so a goal is never labelled
    /// unverified when the evidence says otherwise. When nothing verifies it, the conclusion is
    /// written and the goal is marked `completed_unverified` in the same write.
    pub async fn conclude_after_work(
        &self,
        conclusion_summary: &str,
    ) -> Result<()> {
        match self.conclude(conclusion_summary).await {
            Ok(()) => Ok(()),
            Err(e) => {
                // Only the "no passing verification" rejection is a signal to fall back; any other
                // error is a real storage problem and must not be disguised as success.
                if !e.to_string().contains("No passing verification") {
                    return Err(e);
                }
                repository::conclude_goal(
                    &self.goal_id,
                    GoalState::CompletedUnverified.as_str(),
                    conclusion_summary,
                )
                .await
            }
        }
    }

    /// Compiles the current active prompt for this goal.
    ///
    /// `evidence_limit` is passed in rather than defaulted, because the right cap depends on the
    /// active model's window (`ModelCapabilities::evidence_prompt_limit`).
    pub async fn get_current_prompt(&self, evidence_limit: usize) -> Result<String> {
        let goal = repository::get_goal(&self.goal_id)
            .await?
            .context("Goal not found")?;
        build_goal_context_prompt(&goal, evidence_limit).await
    }
}
#[cfg(test)]
mod inheritance_tests {
    use crate::agent::orchestrator::GoalOrchestrator;
    use crate::agent::repository;

    async fn project_and_session() -> (String, String) {
        let project = repository::create_project("Api Inheritance", "", None, None)
            .await
            .unwrap();
        let session = repository::create_session(&project.id, "S", None, None, None)
            .await
            .unwrap();
        (project.id, session.id)
    }

    async fn seed_parent(project_id: &str, session_id: &str) -> String {
        let parent_id = repository::generate_id("goal_parent");
        repository::create_goal(
            &parent_id,
            project_id,
            Some(session_id),
            None,
            "Parent",
            "build the login screen",
            "[]",
            "[]",
        )
        .await
        .unwrap();
        repository::save_evidence_and_link(
            project_id,
            "src/auth/login.rs",
            "v1",
            None,
            None,
            None,
            "login handler",
            Some("snippet"),
            Some((&parent_id, 1.0, true)),
        )
        .await
        .unwrap();
        parent_id
    }

    /// The API path must inherit on a wordless continuation, exactly like the chat path does.
    ///
    /// A guard used to skip inheritance whenever `reuse_keywords` was empty. That is the case where
    /// the user says only "lanjut" — the clearest possible signal that work continues — so the guard
    /// discarded context precisely when it was needed. This drives the production entry point rather
    /// than `reuse_evidence_from_parent`, because the guard lived in the caller.
    #[tokio::test]
    async fn a_wordless_continuation_still_inherits_via_the_api_path() {
        let _guard = repository::testsupport::db_test_lock().await;
        let (project_id, session_id) = project_and_session().await;
        let parent_id = seed_parent(&project_id, &session_id).await;

        let child_id = repository::generate_id("goal_child");
        GoalOrchestrator::initialize_demand(
            &child_id,
            &project_id,
            Some(&session_id),
            Some(&parent_id),
            "Child",
            "lanjut",
            &[],
            &[],
            &[],
            &[],
        )
        .await
        .unwrap();

        let inherited = repository::get_active_evidences_for_goal(&child_id).await.unwrap();
        assert!(
            inherited.iter().any(|e| e.file_path == "src/auth/login.rs"),
            "a wordless continuation must still inherit the parent's evidence"
        );
    }

    /// A goal with no parent inherits nothing, whatever the wording. Removing the keyword guard must
    /// not make every goal start from the whole project's evidence.
    #[tokio::test]
    async fn a_goal_without_a_parent_inherits_nothing() {
        let _guard = repository::testsupport::db_test_lock().await;
        let (project_id, session_id) = project_and_session().await;
        seed_parent(&project_id, &session_id).await;

        let standalone_id = repository::generate_id("goal_standalone");
        GoalOrchestrator::initialize_demand(
            &standalone_id,
            &project_id,
            Some(&session_id),
            None,
            "Standalone",
            "login",
            &[],
            &[],
            &["login"],
            &[],
        )
        .await
        .unwrap();

        let inherited = repository::get_active_evidences_for_goal(&standalone_id).await.unwrap();
        assert!(
            inherited.is_empty(),
            "without a parent goal there is nothing to inherit, got {inherited:?}"
        );
    }
}
