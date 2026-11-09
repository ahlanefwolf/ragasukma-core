use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, RwLock};

use crate::agent::models::{Message, Project, Session};
use crate::agent::repository;

static SESSION_ACTIVATED_EXTENSIONS: LazyLock<RwLock<HashMap<String, HashSet<String>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Checks if an extension/MCP server has been activated for a session.
pub fn is_extension_active(session_id: &str, server_name: &str) -> bool {
    let guard = SESSION_ACTIVATED_EXTENSIONS
        .read()
        .unwrap_or_else(|e| e.into_inner());
    guard
        .get(session_id)
        .map(|set| set.contains(&server_name.to_lowercase()))
        .unwrap_or(false)
}

/// Activates an extension/MCP server for a session.
pub fn activate_session_extension(session_id: &str, server_name: &str) -> bool {
    let mut guard = SESSION_ACTIVATED_EXTENSIONS
        .write()
        .unwrap_or_else(|e| e.into_inner());
    guard
        .entry(session_id.to_string())
        .or_default()
        .insert(server_name.to_lowercase())
}

/// Retrieves all active extensions for a session.
pub fn get_session_active_extensions(session_id: &str) -> HashSet<String> {
    let guard = SESSION_ACTIVATED_EXTENSIONS
        .read()
        .unwrap_or_else(|e| e.into_inner());
    guard.get(session_id).cloned().unwrap_or_default()
}

pub struct TurnState {
    pub goal_id: String,
    pub current_phase: String,
    pub locked_targets: Vec<String>,
    pub iteration: usize,
    pub max_safety_iterations: usize,
    /// Iteration count at which the agent is told its budget is running low.
    /// Derived from `max_safety_iterations` so the advice always matches reality.
    pub inspect_soft_limit: usize,
    /// Consecutive inspections without a code change before nudging the agent to
    /// act. Derived from the turn budget so it scales automatically.
    pub inspection_streak_limit: usize,
    pub ephemeral_directive: Option<String>,
    pub modified_files: Vec<String>,
    pub executed_commands: Vec<String>,
    pub consecutive_command_count: usize,
    pub last_executed_command: Option<String>,
    pub consecutive_read_same_file: HashMap<String, usize>,
    pub recent_read_slices: HashSet<(String, Option<i64>, Option<i64>)>,
    pub global_inspection_streak: usize,
    pub last_command_had_error: bool,
    pub last_command_is_environment_error: bool,
    pub last_tool_had_error: bool,
    pub pending_error_files: HashSet<String>,
    pub files_read_in_full: HashSet<String>,
    pub verified_subdirs: HashSet<String>,
    pub force_final_text_only: bool,
    pub circuit_breaker_triggered: bool,
    pub has_passed_verification: bool,
    pub verification_attempts: usize,
    pub verification_prompted: bool,
    pub consecutive_empty_turns: usize,
    pub consecutive_tool_errors: usize,
    pub last_tool_error_signature: Option<String>,
    pub repeated_tool_error_count: usize,
    pub has_taken_any_action: bool,
    pub intent_without_action_nudges: usize,
}

impl TurnState {
    /// Single source of truth for the per-turn tool execution budget.
    /// The prompt and the runtime loop both read this, so they can never disagree.
    pub const MAX_SAFETY_ITERATIONS: usize = 50;

    pub fn new(
        goal_id: String,
        current_phase: String,
        locked_targets: Vec<String>,
        prior_verifs_count: usize,
    ) -> Self {
        Self {
            goal_id,
            current_phase,
            locked_targets,
            iteration: 0,
            max_safety_iterations: Self::MAX_SAFETY_ITERATIONS,
            inspect_soft_limit: Self::MAX_SAFETY_ITERATIONS / 2,
            inspection_streak_limit: Self::MAX_SAFETY_ITERATIONS / 5,
            ephemeral_directive: None,
            modified_files: Vec::new(),
            executed_commands: Vec::new(),
            consecutive_command_count: 0,
            last_executed_command: None,
            consecutive_read_same_file: HashMap::new(),
            recent_read_slices: HashSet::new(),
            global_inspection_streak: 0,
            last_command_had_error: false,
            last_command_is_environment_error: false,
            last_tool_had_error: false,
            pending_error_files: HashSet::new(),
            files_read_in_full: HashSet::new(),
            verified_subdirs: HashSet::new(),
            force_final_text_only: false,
            circuit_breaker_triggered: false,
            has_passed_verification: false,
            verification_attempts: prior_verifs_count,
            verification_prompted: false,
            consecutive_empty_turns: 0,
            consecutive_tool_errors: 0,
            last_tool_error_signature: None,
            repeated_tool_error_count: 0,
            has_taken_any_action: false,
            intent_without_action_nudges: 0,
        }
    }

    pub fn trip_circuit_breaker(&mut self, directive_msg: &str) {
        self.circuit_breaker_triggered = true;
        self.force_final_text_only = true;
        self.ephemeral_directive = Some(directive_msg.to_string());
    }

    pub fn on_successful_edit(&mut self, file_path: &str) {
        self.last_tool_had_error = false;
        self.last_command_is_environment_error = false;
        self.has_passed_verification = false;
        self.verification_prompted = false;
        self.global_inspection_streak = 0;
        self.consecutive_command_count = 0;
        self.last_executed_command = None;
        self.recent_read_slices.clear();
        self.consecutive_read_same_file.clear();

        let clean = file_path.trim_start_matches("./");
        self.pending_error_files
            .retain(|ef| ef != clean && !clean.ends_with(ef) && !ef.ends_with(clean));
        if !self.modified_files.iter().any(|f| f == file_path) {
            self.modified_files.push(file_path.to_string());
        }
        if !self.locked_targets.iter().any(|f| f == file_path) {
            self.locked_targets.push(file_path.to_string());
        }
        self.current_phase = "verification".to_string();
    }
}

pub async fn resolve_or_create_goal(
    session: &Session,
    project: &Project,
    db_messages: &[Message],
    allowed_folders: &[String],
) -> Result<(String, String, Vec<String>)> {
    let last_user_prompt = db_messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.as_deref())
        .unwrap_or("")
        .to_string();

    let session_goals = repository::list_session_goals(&session.id)
        .await
        .unwrap_or_default();
    let latest_existing_goal = session_goals.first().cloned();

    // Check if the latest goal was already completed or failed; if so, create a new one
    let goal_id = match latest_existing_goal {
        Some(ref g)
            if g.status != "completed"
                && g.status != "completed_unverified"
                && g.status != "failed" =>
        {
            // Re-use current active goal if still in progress and user is continuing the task
            g.id.clone()
        }
        _ => {
            // New distinct goal for this user request with sub-second entropy
            let new_gid = repository::generate_id(&format!("goal_{}", session.id));
            let title = if last_user_prompt.chars().count() > 60 {
                format!(
                    "{}...",
                    last_user_prompt.chars().take(57).collect::<String>()
                )
            } else if !last_user_prompt.trim().is_empty() {
                last_user_prompt.trim().to_string()
            } else {
                session.title.clone()
            };
            // Passed whole: `evidence_scoring` tokenizes and decides which words can distinguish a
            // file, so trimming by word length here would only hide information from it (a short
            // but distinctive word, like an identifier, matters most).
            let prompt_keywords: Vec<&str> = last_user_prompt.split_whitespace().collect();

            // Success criteria stay empty unless something actually states them.
            //
            // Copying the demand into this field used to look harmless, but the two are
            // different roles: the demand is what the user asked for, a criterion is a
            // checkable condition derived from it. Duplicating one into the other made
            // the prompt carry the same multi-kilobyte text twice on every single turn,
            // and a checklist entry that merely restates the request cannot be verified.
            // An empty list is honest; a fake one is not.
            let success_criteria: Vec<String> = Vec::new();

            let _ = crate::agent::orchestrator::GoalOrchestrator::initialize_demand(
                &new_gid,
                &project.id,
                Some(&session.id),
                latest_existing_goal.as_ref().map(|g| g.id.as_str()),
                &title,
                &last_user_prompt,
                &success_criteria,
                &[],
                &prompt_keywords,
                allowed_folders,
            )
            .await?;
            new_gid
        }
    };

    // Revalidate evidence cache on disk with degradation recording
    if let Err(e) =
        crate::agent::context_builder::revalidate_evidence_cache(&project.id, allowed_folders).await
    {
        let err_str = e.to_string();
        tracing::warn!(
            "[Agent Context Warning] Evidence cache revalidation failed for goal {}: {}. Operating in degraded mode.",
            goal_id,
            err_str
        );
        let _ = repository::update_goal_context_health(&goal_id, "degraded", Some(&err_str)).await;
    }

    // Only reuse prior evidence if this goal is explicitly a continuation child of a parent goal
    let active_goal_opt = repository::get_goal(&goal_id).await.unwrap_or(None);
    if let Some(ref g) = active_goal_opt
        && let Some(ref parent_gid) = g.parent_goal_id
    {
        // Passed whole; see the note on the call above. Scoring decides what matters.
        let prompt_keywords: Vec<&str> = last_user_prompt.split_whitespace().collect();
        if let Err(e) = crate::agent::context_builder::reuse_evidence_from_parent(
            &goal_id,
            parent_gid,
            &prompt_keywords,
        )
        .await
        {
            let err_str = e.to_string();
            tracing::warn!(
                "[Agent Context Warning] Parent evidence reuse failed for goal {} from {}: {}",
                goal_id,
                parent_gid,
                err_str
            );
            let _ =
                repository::update_goal_context_health(&goal_id, "degraded", Some(&err_str)).await;
        }
    }

    // A fresh user turn MUST ALWAYS start with active execution phase ("discovery" or "action").
    let current_phase = active_goal_opt
        .as_ref()
        .map(|g| g.phase.as_str())
        .filter(|p| *p != "concluded")
        .unwrap_or("discovery")
        .to_string();
    let locked_targets: Vec<String> = active_goal_opt
        .as_ref()
        .and_then(|g| serde_json::from_str::<Vec<String>>(&g.target_files).ok())
        .unwrap_or_default();

    Ok((goal_id, current_phase, locked_targets))
}

pub async fn resolve_model(session: &Session, model_override: Option<&str>) -> (String, bool) {
    let model = model_override
        .filter(|m| !m.trim().is_empty())
        .map(|m| m.to_string())
        .or_else(|| {
            session
                .model
                .as_ref()
                .filter(|m| !m.trim().is_empty())
                .cloned()
        })
        .unwrap_or_else(|| {
            let u = crate::config::load_user_config();
            if !u.llm_model.trim().is_empty() {
                u.llm_model
            } else {
                crate::config::default_model()
            }
        });

    let _ = repository::update_session_model(&session.id, &model).await;
    let is_codex = model.starts_with("cx/");
    (model, is_codex)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelProviderFamily {
    Antigravity,
    Codex,
    Claude,
    Custom,
}

/// Token budget for replayed history on providers with large context windows.
const LARGE_CONTEXT_HISTORY_BUDGET: usize = 48_000;

/// Token budget for replayed history on OpenAI-compatible gateways, whose real
/// limits vary too much to assume a large window.
const STANDARD_CONTEXT_HISTORY_BUDGET: usize = 12_000;

/// What the selected provider can do natively. Derived from the provider family,
/// never from a hardcoded model name, so new models inherit the right behaviour
/// automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelCapabilities {
    /// The provider emits a separate hidden reasoning channel (e.g.
    /// `reasoning_content`), so the prompt must NOT ask for `<thought>` tags.
    pub native_reasoning: bool,
    /// The provider reliably emits multiple tool calls in one assistant message.
    pub parallel_tool_calls: bool,
    /// Token budget for replayed conversation history in one turn.
    ///
    /// Families differ by orders of magnitude in usable context, so a single global
    /// constant either wastes a large window or overflows a small one.
    pub history_token_budget: usize,
    /// Maximum number of evidence entries injected into the system prompt.
    ///
    /// The evidence block is rebuilt on every turn of a session, so an unbounded list keeps
    /// growing as the agent reads more files. Derived from `history_token_budget` rather than
    /// fixed, so a model with a small window automatically gets a smaller evidence slice.
    pub evidence_prompt_limit: usize,
}

/// Fraction of the history budget the evidence block is allowed to occupy.
///
/// Evidence is valuable context but it is not the conversation, so it gets a minority share of
/// the window; the rest stays available for the actual dialogue and tool output.
const EVIDENCE_BUDGET_SHARE: usize = 4;

/// Tokens one evidence entry is assumed to cost: path, version, summary, and a short snippet.
///
/// Used only to size `evidence_prompt_limit` from the token budget, so it stays an estimate of
/// the formatting done in `build_goal_context_prompt`.
const EVIDENCE_ENTRY_TOKEN_COST: usize = 200;

/// Smallest evidence slice worth injecting. A model with a tiny budget still needs to see
/// something it just read, otherwise the cache is pure write cost with no benefit.
const MIN_EVIDENCE_PROMPT_LIMIT: usize = 8;

fn evidence_prompt_limit_for(history_token_budget: usize) -> usize {
    (history_token_budget / EVIDENCE_BUDGET_SHARE / EVIDENCE_ENTRY_TOKEN_COST)
        .max(MIN_EVIDENCE_PROMPT_LIMIT)
}

pub fn model_capabilities(family: ModelProviderFamily) -> ModelCapabilities {
    match family {
        // These adapters stream a dedicated thought channel to the UI; asking for
        // inline <thought> tags on top of that produces duplicated, conflicting output.
        ModelProviderFamily::Antigravity
        | ModelProviderFamily::Claude
        | ModelProviderFamily::Codex => {
            let history_token_budget = LARGE_CONTEXT_HISTORY_BUDGET;
            ModelCapabilities {
                native_reasoning: true,
                parallel_tool_calls: true,
                history_token_budget,
                evidence_prompt_limit: evidence_prompt_limit_for(history_token_budget),
            }
        }
        // OpenAI-compatible gateways vary in how they split reasoning vs. content;
        // inline <thought> tags are the portable fallback that always works. They also
        // advertise wildly different windows, so stay within the conservative band.
        ModelProviderFamily::Custom => {
            let history_token_budget = STANDARD_CONTEXT_HISTORY_BUDGET;
            ModelCapabilities {
                native_reasoning: false,
                parallel_tool_calls: true,
                history_token_budget,
                evidence_prompt_limit: evidence_prompt_limit_for(history_token_budget),
            }
        }
    }
}

/// In-memory RAM cache for resolved model capabilities.
/// Thread-safe (RwLock) allowing concurrent read access across multiple parallel sessions.
static MODEL_CAPABILITIES_CACHE: LazyLock<RwLock<HashMap<String, ModelCapabilities>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Returns cached model capabilities from RAM if already resolved.
pub fn get_cached_capabilities(model: &str) -> Option<ModelCapabilities> {
    MODEL_CAPABILITIES_CACHE.read().ok()?.get(model).copied()
}

/// Sets cached model capabilities into RAM.
pub fn set_cached_capabilities(model: &str, capabilities: ModelCapabilities) {
    if let Ok(mut cache) = MODEL_CAPABILITIES_CACHE.write() {
        cache.insert(model.to_string(), capabilities);
    }
}

/// Clears the in-memory RAM cache of model capabilities.
pub fn clear_capabilities_cache() {
    if let Ok(mut cache) = MODEL_CAPABILITIES_CACHE.write() {
        cache.clear();
    }
}

/// Resolves model capabilities dynamically and agnostically based on model identifier,
/// prefix, discovered endpoint metadata in RAM, account settings, or heuristic architecture hints.
/// Cached in RAM for concurrent $O(1)$ access across parallel sessions (same or different models).
pub fn resolve_model_capabilities(model: &str) -> ModelCapabilities {
    if let Some(cached) = get_cached_capabilities(model) {
        return cached;
    }

    let caps = compute_model_capabilities(model);
    set_cached_capabilities(model, caps);
    caps
}

fn compute_model_capabilities(model: &str) -> ModelCapabilities {
    let family = detect_provider_family(model);
    match family {
        ModelProviderFamily::Antigravity
        | ModelProviderFamily::Claude
        | ModelProviderFamily::Codex => {
            let history_token_budget = LARGE_CONTEXT_HISTORY_BUDGET;
            ModelCapabilities {
                native_reasoning: true,
                parallel_tool_calls: true,
                history_token_budget,
                evidence_prompt_limit: evidence_prompt_limit_for(history_token_budget),
            }
        }
        ModelProviderFamily::Custom => {
            // Check for explicit custom account configuration first
            let prefix = model.split('/').next().unwrap_or("");
            let custom_accounts = crate::llm::providers::custom::load_accounts();
            let account = custom_accounts
                .iter()
                .find(|a| a.prefix.eq_ignore_ascii_case(prefix));

            // Check discovered metadata in RAM (from /models discovery or endpoint introspection)
            let discovered_meta = crate::llm::providers::custom::get_discovered_meta(model);

            // 1. Resolve native reasoning capability
            // Priority: Explicit Account Config > Discovered Metadata in RAM > Generic Capability Keywords
            let native_reasoning = if let Some(Some(nr)) = account.map(|a| a.native_reasoning) {
                nr
            } else if let Some(Some(nr)) = discovered_meta.as_ref().map(|m| m.native_reasoning) {
                nr
            } else {
                detect_generic_reasoning(model)
            };

            // 2. Resolve history token budget (context window scaling)
            // Priority: Explicit Account Config > Discovered Metadata in RAM > Generic Context Window Suffix > Baseline Standard
            let parsed_cw = parse_generic_context_window(model);
            let history_token_budget = if let Some(Some(cw)) = account.map(|a| a.context_window) {
                budget_for_context_window(cw)
            } else if let Some(Some(cw)) = discovered_meta.as_ref().map(|m| m.context_window) {
                budget_for_context_window(cw)
            } else if let Some(cw) = parsed_cw {
                budget_for_context_window(cw)
            } else {
                STANDARD_CONTEXT_HISTORY_BUDGET
            };

            ModelCapabilities {
                native_reasoning,
                parallel_tool_calls: true,
                history_token_budget,
                evidence_prompt_limit: evidence_prompt_limit_for(history_token_budget),
            }
        }
    }
}

/// Extracts generic context window size (in tokens) from universal suffix patterns
/// like "128k", "64k", "32k", "8k", "1m", "200k" without relying on brand names.
fn parse_generic_context_window(model: &str) -> Option<usize> {
    let lower = model.to_ascii_lowercase();
    for part in lower.split(|c: char| !c.is_alphanumeric()) {
        if let Some(num_str) = part.strip_suffix('k')
            && let Ok(num) = num_str.parse::<usize>()
            && (1..=4096).contains(&num)
        {
            return Some(num * 1024);
        } else if let Some(num_str) = part.strip_suffix('m')
            && let Ok(num) = num_str.parse::<usize>()
            && (1..=32).contains(&num)
        {
            return Some(num * 1024 * 1024);
        }
    }
    None
}

/// Dynamically scales history token budget based on context window size.
fn budget_for_context_window(cw: usize) -> usize {
    if cw <= 16_000 {
        (cw * 60 / 100).max(4_000)
    } else {
        (cw * 60 / 100).clamp(STANDARD_CONTEXT_HISTORY_BUDGET, 96_000)
    }
}

/// Detects if model identifier contains universal reasoning capability keywords
/// without hardcoding specific vendor/model brands.
fn detect_generic_reasoning(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower.contains("reason") || lower.contains("think") || lower.contains("thought")
}

/// Asynchronously discovers and resolves model capabilities.
/// If not already in RAM, it discovers provider metadata with a fast timeout, caches the result in RAM,
/// and returns immediately. Safe for parallel sessions running different or identical models concurrently.
pub async fn resolve_model_capabilities_async(model: &str) -> ModelCapabilities {
    if let Some(cached) = get_cached_capabilities(model) {
        return cached;
    }

    if detect_provider_family(model) == ModelProviderFamily::Custom {
        let _ = crate::llm::providers::custom::discover_model_meta(model).await;
    }

    resolve_model_capabilities(model)
}

pub fn detect_provider_family(model: &str) -> ModelProviderFamily {
    if model.starts_with("cc/") {
        ModelProviderFamily::Claude
    } else if model.starts_with("cx/") {
        ModelProviderFamily::Codex
    } else if model.starts_with("ag/") {
        ModelProviderFamily::Antigravity
    } else {
        ModelProviderFamily::Custom
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn every_family_declares_a_positive_history_budget() {
        for family in [
            ModelProviderFamily::Antigravity,
            ModelProviderFamily::Claude,
            ModelProviderFamily::Codex,
            ModelProviderFamily::Custom,
        ] {
            let caps = model_capabilities(family);
            assert!(
                caps.history_token_budget > 0,
                "{family:?} must declare a usable history budget"
            );
        }
    }

    #[test]
    fn large_context_families_get_a_bigger_window_than_gateways() {
        let claude = model_capabilities(ModelProviderFamily::Claude);
        let custom = model_capabilities(ModelProviderFamily::Custom);
        assert!(claude.history_token_budget > custom.history_token_budget);
        assert!(claude.evidence_prompt_limit > custom.evidence_prompt_limit);
    }

    #[test]
    fn evidence_limit_scales_with_the_budget_and_stays_bounded() {
        // A tiny budget still yields a usable slice rather than zero entries.
        assert_eq!(evidence_prompt_limit_for(0), MIN_EVIDENCE_PROMPT_LIMIT);
        // A larger window yields a proportionally larger slice.
        assert!(evidence_prompt_limit_for(48_000) > evidence_prompt_limit_for(12_000));
        // The slice never eats the whole window.
        for budget in [12_000usize, 48_000, 200_000, 1_000_000] {
            let tokens = evidence_prompt_limit_for(budget) * EVIDENCE_ENTRY_TOKEN_COST;
            assert!(
                tokens <= budget / EVIDENCE_BUDGET_SHARE + EVIDENCE_ENTRY_TOKEN_COST,
                "evidence slice for budget {budget} must stay within its share"
            );
        }
    }

    #[test]
    fn budget_is_derived_from_family_not_model_name() {
        // Two different model strings in the same family must yield the same capabilities.
        let a = model_capabilities(detect_provider_family("9r/cbai/deepseek-v4.1-flash"));
        let b = model_capabilities(detect_provider_family("9r/other/whatever-model"));
        assert_eq!(a.history_token_budget, b.history_token_budget);
    }

    #[test]
    fn resolve_model_capabilities_intelligently_scales_and_detects_reasoning() {
        // Claude / Codex / Antigravity
        let claude = resolve_model_capabilities("cc/claude-3-7-sonnet");
        assert!(claude.native_reasoning);
        assert_eq!(claude.history_token_budget, LARGE_CONTEXT_HISTORY_BUDGET);

        // Generic reasoning indicator (no brand hardcoded)
        let reasoner = resolve_model_capabilities("custom/deepseek-reasoner");
        assert!(reasoner.native_reasoning);

        let thinking = resolve_model_capabilities("ollama/model-thinking:32b");
        assert!(thinking.native_reasoning);

        // Generic token-scaled models without brand hardcoding
        let scaled_128k = resolve_model_capabilities("vllm/custom-model-128k");
        assert!(!scaled_128k.native_reasoning);
        assert_eq!(scaled_128k.history_token_budget, 128 * 1024 * 60 / 100);

        // Small context model dynamically scaled
        let small = resolve_model_capabilities("custom/my-model-8k");
        assert_eq!(small.history_token_budget, 8 * 1024 * 60 / 100);
    }

    #[test]
    fn discovered_meta_in_ram_scales_budget_and_reasoning() {
        clear_capabilities_cache();
        let model = "test-provider/custom-discovered-model";

        // Inject discovered metadata into RAM (as if discovered from /models)
        crate::llm::providers::custom::set_discovered_meta(
            model,
            crate::llm::providers::custom::DiscoveredModelMeta {
                context_window: Some(65_536),
                native_reasoning: Some(true),
            },
        );

        let caps = resolve_model_capabilities(model);
        assert!(caps.native_reasoning);
        // 60% of 65_536 is 39_321
        assert_eq!(caps.history_token_budget, 65_536 * 60 / 100);

        // Subsequent call hits RAM cache directly
        assert_eq!(get_cached_capabilities(model), Some(caps));
    }

    #[test]
    fn parallel_sessions_can_run_concurrently_with_same_and_different_models() {
        clear_capabilities_cache();

        // Simulate 20 parallel session tasks running simultaneously across threads
        // Some with the same model, some with different models
        std::thread::scope(|s| {
            let mut handles = Vec::new();

            // 5 sessions running Model A (Generic reasoning)
            for _ in 0..5 {
                handles.push(s.spawn(|| {
                    let caps = resolve_model_capabilities("parallel-custom/model-reasoner-shared");
                    assert!(caps.native_reasoning);
                }));
            }

            // 5 sessions running Model B (Generic 128k context)
            for _ in 0..5 {
                handles.push(s.spawn(|| {
                    let caps = resolve_model_capabilities("parallel-custom/generic-128k");
                    assert!(!caps.native_reasoning);
                    assert_eq!(caps.history_token_budget, 128 * 1024 * 60 / 100);
                }));
            }

            // 5 sessions running Model C (Claude 3.7)
            for _ in 0..5 {
                handles.push(s.spawn(|| {
                    let caps = resolve_model_capabilities("cc/claude-3-7-sonnet");
                    assert!(caps.native_reasoning);
                    assert_eq!(caps.history_token_budget, LARGE_CONTEXT_HISTORY_BUDGET);
                }));
            }

            // 5 sessions running Model D (Small 8k)
            for _ in 0..5 {
                handles.push(s.spawn(|| {
                    let caps = resolve_model_capabilities("parallel-custom/small-8k-model");
                    assert_eq!(caps.history_token_budget, 8 * 1024 * 60 / 100);
                }));
            }

            for handle in handles {
                handle.join().unwrap();
            }
        });

        // Verify all 4 distinct models are cached in RAM
        assert!(get_cached_capabilities("parallel-custom/model-reasoner-shared").is_some());
        assert!(get_cached_capabilities("parallel-custom/generic-128k").is_some());
        assert!(get_cached_capabilities("cc/claude-3-7-sonnet").is_some());
        assert!(get_cached_capabilities("parallel-custom/small-8k-model").is_some());
    }
}

/// A goal created from a fresh user turn must not invent success criteria.
///
/// The previous behaviour copied the whole demand into the single criterion. Downstream
/// that meant the prompt rendered the same multi-kilobyte text under both `Demand:` and
/// `Success Criteria:` on every turn, and any report based on "all criteria met" was
/// judging the user's own sentence back at them.
#[cfg(test)]
mod goal_criteria_tests {
    use crate::agent::repository;

    #[tokio::test]
    async fn a_new_goal_does_not_copy_its_demand_into_success_criteria() {
        let _guard = repository::testsupport::db_test_lock().await;
        let project = repository::create_project("Criteria Origin", "", None, None)
            .await
            .unwrap();
        let session = repository::create_session(&project.id, "S", None, None, None)
            .await
            .unwrap();
        let demand = "Buat landing page untuk produk di folder kerja sekarang";

        let (goal_id, _, _) = super::resolve_or_create_goal(
            &session,
            &project,
            &[crate::agent::models::Message {
                id: "m1".to_string(),
                session_id: session.id.clone(),
                sequence_num: 1,
                role: "user".to_string(),
                content: Some(demand.to_string()),
                thought: None,
                tool_calls: None,
                tool_call_id: None,
                status: "success".to_string(),
                token_usage_input: None,
                token_usage_output: None,
                created_at: 0,
            }],
            &[],
        )
        .await
        .unwrap();

        let goal = repository::get_goal(&goal_id).await.unwrap().unwrap();
        assert_eq!(goal.demand, demand, "the demand is still recorded verbatim");
        let criteria: Vec<String> = serde_json::from_str(&goal.success_criteria).unwrap();
        assert!(
            criteria.is_empty(),
            "nothing stated checkable criteria, so the list must stay empty, got {criteria:?}"
        );
    }
}
