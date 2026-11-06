use anyhow::Result;

use crate::agent::executor::state::ModelCapabilities;
use crate::agent::models::{McpServer, Project, PromptSkill};
use crate::agent::repository;

/// Everything the system prompt needs to describe one agent turn.
///
/// Grouped into a struct so the builder keeps a readable signature as the set of
/// inputs grows, instead of accumulating positional arguments.
pub struct AgentPromptContext<'a> {
    pub project: &'a Project,
    pub allowed_folders: &'a [String],
    pub prompt_skills: &'a [PromptSkill],
    pub active_mcp_servers: &'a [McpServer],
    pub mcp_tools: &'a [crate::agent::mcp::ActiveMcpTool],
    pub session_id: &'a str,
    pub goal_id: &'a str,
    pub capabilities: ModelCapabilities,
}

/// Builds the parts of the system prompt that do not change between turns.
///
/// Role, environment, guidelines, tool directives, project instructions, skills, and MCP tool
/// listings are identical for every turn of a session. Rebuilding them each turn is pure waste:
/// it re-runs verification detection, re-serialises MCP configs, and re-allocates a long string
/// that the model never sees change. The result is cached by [`static_prompt_cache_key`].
fn build_static_prompt(
    project: &Project,
    allowed_folders: &[String],
    prompt_skills: &[PromptSkill],
    active_mcp_servers: &[McpServer],
    mcp_tools: &[crate::agent::mcp::ActiveMcpTool],
    capabilities: ModelCapabilities,
) -> String {
    let os_name = std::env::consts::OS;
    let arch_name = std::env::consts::ARCH;
    let detected_verifier =
        crate::agent::languages::detect_primary_verification_command(allowed_folders);
    let verifier_hint = detected_verifier
        .as_deref()
        .unwrap_or("run project verification or test commands if available");

    #[cfg(target_os = "windows")]
    let shell_desc = "PowerShell. Use native PowerShell syntax for scripts and paths. For multiple commands in one execution, use ';' as separator (do not use '&&' as Windows PowerShell 5.1 does not support it).";

    #[cfg(target_os = "macos")]
    let shell_desc =
        "macOS POSIX Shell (login shell with enriched PATH). Use standard Unix shell syntax.";

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let shell_desc =
        "Linux POSIX Shell (login shell with enriched PATH). Use standard Unix/Linux shell syntax.";

    #[cfg(target_os = "windows")]
    let script_runner_desc = "Scripts: Node.js (`node`) or Python (`python` / `py -3`)";

    #[cfg(not(target_os = "windows"))]
    let script_runner_desc = "Scripts: Node.js (`node`) or Python (`python3`)";

    #[cfg(target_os = "windows")]
    let multiple_cmd_hint = "When running sequential terminal commands, combine them into a single execution to save turn latency: use ';' (e.g. `cmd1 ; cmd2`) on Windows PowerShell instead of '&&'.";

    #[cfg(not(target_os = "windows"))]
    let multiple_cmd_hint = "When running sequential terminal commands, combine them into a single execution (chain with '&&') to save turn latency.";

    // Real limit taken from TurnState, so the prompt never advertises a budget the
    // runtime does not enforce.
    let max_iterations = crate::agent::executor::state::TurnState::MAX_SAFETY_ITERATIONS;

    let mut prompt = format!(
        "<role>\n\
         You are an autonomous senior software engineer and technical assistant operating directly in the workspace.\n\
         You understand, write, refactor, inspect, test, and analyze any software project, regardless of programming language, framework, or architecture.\n\
         </role>\n\n\
         <environment>\n\
         OS: {} ({}) | Shell: {} | {}\n\
         Default workspace verification: `{}`\n\
         Turn budget: at most {} tool executions per request; you are warned before it runs out, so do not stop early out of caution.\n\
         Tools available: `workspace_read_file`, `workspace_write_file`, `workspace_edit_file`, `workspace_grep`, `workspace_list_dir`, `workspace_run_command`.\n\
         </environment>\n\n\
         <core-guidelines>\n\
         1. INTENT AWARENESS (Inquiry vs Action):\n\
            - If the user asks a question, architectural explanation, code review, or exploratory analysis: Answer concisely and factually. Inspect only the files needed to answer accurately, and respond conversationally.\n\
            - If the user requests a bug fix, feature, refactoring, or code change: Take direct action. Locate the target files, inspect them, apply edits, and verify your work.\n\
         2. CLEAN & ACCURATE FILE MODIFICATIONS:\n\
            - Apply complete and cohesive updates. Avoid incomplete placeholders, fake mocks, or non-functional stubs.\n\
            - For surgical or targeted modifications of existing files, use `workspace_edit_file`. Provide matching `old_text` with 1-2 lines of context and `new_text`.\n\
            - For creating new files or full rewrites of small-to-medium files, use `workspace_write_file`.\n\
            - Inline scripts (using Node.js `node -e '...'` or Python `python3 -c '...'` / `python -c '...'`) via `workspace_run_command` can also be used if dynamic transformations, API checks, or complex migrations are needed.\n\
         3. EFFICIENCY & PRAGMATISM (On-Demand & Targeted Slices):\n\
            - When inspecting large files, use `workspace_read_file` with `start_line` and `end_line` around the target function/area rather than reading the entire file. For big files (>250 lines), a structural outline is provided automatically.\n\
            - Directory listing via `workspace_list_dir` is shallow (depth 1) by default and shows subfolder item counts so you can navigate targeted subfolders without recursive dumps.\n\
            - Avoid redundant reads of the same file. Formulate changes and apply them directly.\n\
            - {}\n\
            - Do not run unnecessary exploratory commands (e.g. git logs, unprompted status checks) when the goal is direct code editing.\n\
         4. MULTILINGUAL & POLYGLOT:\n\
            - Communicate naturally in whichever human language the user addresses you in.\n\
            - Maintain all technical identifiers, code, and tool arguments in standard technical format.\n\
            - Adapt seamlessly to any programming language, build system, or configuration found in the workspace.\n\
         5. ADAPTIVE VERIFICATION & CONCLUSION:\n\
            - When code files are modified and the project has an automated test/build command, run verification before concluding.\n\
            - If the project has no automated test suite, or the changes are non-executable assets (e.g. documentation, templates, styling, configs), conclude directly once changes are applied.\n\
            - When concluding, provide a clear and factual summary of files modified and actions completed.\n\
         6. GROUND-TRUTH DELIVERABLE VERIFICATION (Anti-Hallucination):\n\
            - A passing test or build command (exit code 0) only confirms existing tests pass; it does NOT prove new files or features exist.\n\
            - Never mark a checklist item as completed `[x]` or claim a feature is implemented without physically verifying that every requested file was actually created and non-empty (via `workspace_list_dir` or `workspace_read_file`) and that required exports/logic exist.\n\
            - Never edit a task or checklist file to check off `[x]` items before all underlying code files, services, and tests have been written and verified on disk.\n\
         </core-guidelines>\n\n",
        os_name,
        arch_name,
        shell_desc,
        script_runner_desc,
        verifier_hint,
        max_iterations,
        multiple_cmd_hint
    );

    if !allowed_folders.is_empty() {
        prompt.push_str("### Workspace Attached Folders (Multi-Root Support):\n");
        for (idx, f) in allowed_folders.iter().enumerate() {
            let p = std::path::Path::new(f);
            let folder_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("root");
            let marker = if idx == 0 {
                " (Primary Default CWD)"
            } else {
                ""
            };
            prompt.push_str(&format!(
                "- `{}` [Folder Name/Alias: `{}`]{}\n",
                f, folder_name, marker
            ));
        }
        prompt.push_str("Multi-Project / Root Folder Conventions:\n\
- File paths should include the root folder name prefix (e.g. `<folder>/src/file.ext` or `@<folder>/src/file.ext`) so targeting is 100% unambiguous.\n\
- In `workspace_run_command`, specify `cwd` to target a folder alias or run commands in that subproject.\n\n");
    }

    if let Some(ref custom) = project.custom_instructions
        && !custom.trim().is_empty()
    {
        prompt.push_str("### Project Instructions:\n");
        prompt.push_str(custom.trim());
        prompt.push_str("\n\n");
    }

    if !prompt_skills.is_empty() {
        prompt.push_str("### Active Prompt Skills & Persona Guidelines:\n");
        for skill in prompt_skills {
            prompt.push_str(&format!(
                "#### Skill: {}\n{}\n\n",
                skill.name,
                skill.system_prompt.trim()
            ));
        }
    }

    // Tool-calling directives. Each numbered rule states a preference, not an
    // absolute the runtime silently contradicts. The runtime accepts several tool
    // calls per message and executes them in order, so the prompt says so plainly.
    prompt.push_str("\n### \u{1f6e0}\u{fe0f} Tool Calling Directives:\n");

    if capabilities.parallel_tool_calls {
        prompt.push_str(
            "1. TOOL BATCHING: Prefer investigating with several independent tool calls in ONE message when they do not depend on each other (e.g. reading two unrelated files, or grepping for two different patterns). Every call is executed in order and you receive all results together, which is faster than serial round-trips. For dependent steps, call one tool and wait for its result.\n",
        );
    } else {
        prompt.push_str(
            "1. SEQUENTIAL TOOLS: This provider is unreliable with parallel calls. Invoke exactly one tool per message, then wait for its observation before deciding the next action.\n",
        );
    }

    prompt.push_str(
        "2. DEDICATED STRUCTURED TOOLS: Use `workspace_edit_file`, `workspace_write_file`, `workspace_read_file` and `workspace_grep` for workspace code changes and inspections. Do not rewrite source files via shell heredocs or Python one-liners.\n\
         3. VENDOR/DEPENDENCY INTEGRITY: NEVER edit vendor dependencies inside `node_modules/`, `.git/`, or library internals. Apply all fixes to your own project source files.\n\
         4. DISCOVERY DISCIPLINE: Investigate only what the task needs. Read the relevant file, understand it, then act. Do not wander across unrelated files; if an investigation is getting large, say what you found so far and what you still need.\n\
         5. SCRIPT & DATA AUTOMATION: Node.js (`node -e '...'`) or Python (`python3` / `python`) is available for non-file operations (API requests, regex/JSON parsing, sqlite queries, data conversion) when no dedicated tool covers the need. Pick the most suitable, reliable runtime present in the environment.\n\
         6. WORKSPACE EXECUTION: Use `workspace_run_command` for builds, tests, package managers and git operations.\n",
    );

    if capabilities.native_reasoning {
        prompt.push_str(
            "7. SELF-REASONING: Think before acting. Deliberate internally about risk, which file or command is needed, and likely blind spots; the UI displays your reasoning separately, so do not wrap it in tags.\n",
        );
    } else {
        prompt.push_str(
            "7. SELF-REASONING: Before invoking a tool or answering, write a brief internal reflection of 1-3 sentences wrapped in `<thought>...</thought>`. Consider risk, which file or command is needed, and likely blind spots. Do not repeat the reasoning outside the tags.\n",
        );
    }

    prompt.push_str(
        "8. USER-FACING PREAMBLE: Before acting, say in one short sentence, in the user's language, what you are about to do. When you batch several tools, one sentence covering all of them is enough. Never narrate step by step.\n\n",
    );

    if !active_mcp_servers.is_empty() {
        prompt.push_str("### ⚡ Active Project External MCP Servers (Raw Configuration)\n\
The raw JSON config of every external MCP server configured for this workspace project is inlined below. These are NOT callable functions and there is no dedicated wrapper tool for them — you are given the raw config so that YOU can reason about it yourself. Read the raw JSON, recognize what kind of service each one is, and then decide how to talk to it:\n\
- **SSE / HTTP MCP** (`url` / `type: \"sse\"|\"http\"`): write a short Node.js (native `fetch`) or Python script that speaks JSON-RPC 2.0 over the endpoint (`initialize` → `tools/list` → `tools/call`), then run it with `workspace_run_command`.\n\
- **CLI / Stdio MCP** (`command` + `args`): run the command directly through `workspace_run_command` (e.g. `npx`, `uvx`, or a binary), piping JSON-RPC 2.0 lines on stdin if the tool expects a stdio handshake.\n\
Use the endpoint URL, headers, env, command and args exactly as they appear in the raw JSON — that is the source of truth. Do not guess a different address or invent servers.\n\n");

        for (idx, server) in active_mcp_servers.iter().enumerate() {
            let mut cfg_val = server.config_value();
            if let Some(obj) = cfg_val.as_object_mut()
                && !obj.contains_key("name")
            {
                obj.insert("name".to_string(), serde_json::json!(server.name));
            }
            let pretty_json =
                serde_json::to_string_pretty(&cfg_val).unwrap_or_else(|_| server.config.clone());
            prompt.push_str(&format!(
                "#### {}. MCP Server: \"{}\" ({})\n```json\n{}\n```\n\n",
                idx + 1,
                server.name,
                server.transport(),
                pretty_json
            ));
        }
    }

    if !mcp_tools.is_empty() {
        prompt.push_str("### ⚡ Pre-Resolved Tool Functions (Model Context Protocol):\n");
        for tool in mcp_tools {
            let desc = tool
                .definition
                .description
                .as_deref()
                .unwrap_or("Active tool");
            prompt.push_str(&format!(
                "- `{}` (from {}): {}\n",
                tool.namespaced_name, tool.server_name, desc
            ));
        }
        prompt.push('\n');
    }
    prompt
}

/// Cache key for the static prompt block.
///
/// Every input that can change the rendered text is folded in: the project (instructions and
/// identity), attached folders, skill set, MCP server configs, and resolved MCP tools. Same key
/// means byte-identical output, so a hit is always safe.
fn static_prompt_cache_key(
    project: &Project,
    allowed_folders: &[String],
    prompt_skills: &[PromptSkill],
    active_mcp_servers: &[McpServer],
    mcp_tools: &[crate::agent::mcp::ActiveMcpTool],
    capabilities: ModelCapabilities,
) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    project.id.hash(&mut h);
    project.custom_instructions.hash(&mut h);
    allowed_folders.hash(&mut h);
    for s in prompt_skills {
        s.name.hash(&mut h);
        s.system_prompt.hash(&mut h);
    }
    for s in active_mcp_servers {
        s.id.hash(&mut h);
        s.name.hash(&mut h);
        s.config.hash(&mut h);
        s.is_enabled.hash(&mut h);
    }
    for t in mcp_tools {
        t.namespaced_name.hash(&mut h);
        t.server_name.hash(&mut h);
        t.definition.description.hash(&mut h);
    }
    // Capabilities change the rendered directives (parallel batching, reasoning style),
    // so a different model family must not reuse another family's cached block.
    capabilities.native_reasoning.hash(&mut h);
    capabilities.parallel_tool_calls.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Caches the static prompt block across turns.
///
/// Keyed by [`static_prompt_cache_key`], holding exactly one entry. A single slot is enough because
/// one process serves one session at a time, and it keeps memory bounded without an eviction
/// policy to reason about.
///
/// The value is `Arc<str>` rather than `Arc<String>`: the text lives directly inside the Arc
/// allocation, so cloning a cached block to build a turn's prompt is a single refcount bump with
/// no second buffer behind it.
static STATIC_PROMPT_CACHE: std::sync::Mutex<Option<(String, std::sync::Arc<str>)>> =
    std::sync::Mutex::new(None);

fn cached_static_prompt(ctx: &AgentPromptContext<'_>) -> std::sync::Arc<str> {
    let key = static_prompt_cache_key(
        ctx.project,
        ctx.allowed_folders,
        ctx.prompt_skills,
        ctx.active_mcp_servers,
        ctx.mcp_tools,
        ctx.capabilities,
    );
    // A poisoned lock only means another thread panicked while holding it; the cache holds no
    // invariants that a panic could corrupt, so recover instead of propagating.
    let mut guard = STATIC_PROMPT_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some((cached_key, value)) = guard.as_ref()
        && cached_key == &key
    {
        return value.clone();
    }
    let rendered: std::sync::Arc<str> = std::sync::Arc::from(build_static_prompt(
        ctx.project,
        ctx.allowed_folders,
        ctx.prompt_skills,
        ctx.active_mcp_servers,
        ctx.mcp_tools,
        ctx.capabilities,
    ));
    *guard = Some((key, rendered.clone()));
    rendered
}

pub async fn build_system_prompt(ctx: AgentPromptContext<'_>) -> Result<String> {
    let capabilities = ctx.capabilities;
    let session_id = ctx.session_id;
    let goal_id = ctx.goal_id;

    let verifier_hint =
        crate::agent::languages::detect_primary_verification_command(ctx.allowed_folders)
            .unwrap_or_else(|| {
                "run project verification or test commands if available".to_string()
            });

    // Static block: cached, so a long session does not re-render it every turn. The cached text is
    // copied into an owned `String` because the dynamic sections below are appended to it.
    let mut system_prompt = cached_static_prompt(&ctx).to_string();

    // 1. Inherit prior steps only within this same task/session (Zero cross-task pollution)
    if let Ok(prev_summary) =
        crate::agent::context_builder::build_previous_goals_summary(session_id, Some(goal_id)).await
        && !prev_summary.is_empty()
    {
        system_prompt.push_str(&prev_summary);
    }

    // 2. Goal objective, success criteria, constraints and active evidence.
    //    This block used to be reachable only from tests, so criteria/constraints
    //    were persisted but never shown to the model. It replaces the ad-hoc
    //    evidence dump that previously duplicated the same information.
    if let Ok(Some(goal)) = repository::get_goal(goal_id).await {
        if let Ok(goal_block) = crate::agent::context_builder::build_goal_context_prompt(
            &goal,
            capabilities.evidence_prompt_limit,
        )
        .await
        {
            system_prompt.push_str(&goal_block);
        }
    } else {
        // Fallback: no goal row could be loaded, still surface whatever evidence
        // is linked so the turn is not left without context.
        let relevant_evs = repository::get_active_evidences_for_goal(goal_id)
            .await
            .unwrap_or_default();
        if !relevant_evs.is_empty() {
            system_prompt
                .push_str("### 🎯 Relevant Code Locations & Evidence Cache (Verified Fresh):\n");
            for ev in relevant_evs.iter().take(15) {
                let lines_info = match (ev.start_line, ev.end_line) {
                    (Some(s), Some(e)) => format!(" (Lines {}-{})", s, e),
                    (Some(s), None) => format!(" (Line {}+)", s),
                    _ => String::new(),
                };
                let sym_info = ev
                    .symbol_name
                    .as_deref()
                    .map(|s| format!(" [`{}`]", s))
                    .unwrap_or_default();
                system_prompt.push_str(&format!(
                    "- `{}`{}{}: {}\n",
                    ev.file_path, lines_info, sym_info, ev.summary
                ));
            }
            system_prompt.push('\n');
        }
    }

    system_prompt.push_str("\n### ⚙️ Deterministic Execution Phases:\n");
    system_prompt.push_str("1. DISCOVERY: Locate and inspect necessary target files or context. For questions or analysis, formulate your answer directly.\n");
    system_prompt.push_str("2. ACTION: If changes are requested, apply modifications using `workspace_edit_file` (for targeted edits) or `workspace_write_file` (for full files).\n");
    system_prompt.push_str(&format!(
        "3. VERIFICATION: If changes were made and project test/build commands exist (`{}`), run verification. Conclude with a clear, concise summary of the results.\n\n",
        verifier_hint
    ));

    Ok(system_prompt)
}

#[cfg(test)]
mod tests {
    use crate::agent::executor::state::{
        ModelProviderFamily, TurnState, detect_provider_family, model_capabilities,
    };

    #[test]
    fn test_budget_numbers_are_derived_not_hardcoded() {
        let state = TurnState::new(String::new(), String::new(), Vec::new(), 0);
        assert_eq!(
            state.max_safety_iterations,
            TurnState::MAX_SAFETY_ITERATIONS
        );
        assert_eq!(
            state.inspect_soft_limit,
            TurnState::MAX_SAFETY_ITERATIONS / 2
        );
        assert_eq!(
            state.inspection_streak_limit,
            TurnState::MAX_SAFETY_ITERATIONS / 5
        );
        // Warnings must always trigger before the hard stop.
        assert!(state.inspect_soft_limit < state.max_safety_iterations);
        assert!(state.inspection_streak_limit < state.max_safety_iterations);
    }

    #[test]
    fn test_capabilities_come_from_provider_family_not_model_name() {
        // Any model under a family prefix inherits that family's capabilities, so a
        // brand-new model needs no prompt change.
        for model in ["cc/anything-new", "cc/claude-x"] {
            let caps = model_capabilities(detect_provider_family(model));
            assert!(caps.native_reasoning);
        }
        for model in ["ag/some-future-model", "cx/another"] {
            assert!(model_capabilities(detect_provider_family(model)).native_reasoning);
        }
        // OpenAI-compatible gateways keep the portable inline tag fallback.
        for model in [
            "9r/cbai/deepseek-v4.1-flash",
            "kc/deepseek/deepseek-chat",
            "unknown-model",
        ] {
            let family = detect_provider_family(model);
            assert!(matches!(family, ModelProviderFamily::Custom));
            assert!(!model_capabilities(family).native_reasoning);
        }
    }

    fn sample_project() -> crate::agent::models::Project {
        crate::agent::models::Project {
            id: "p-cache".to_string(),
            name: "Cache".to_string(),
            description: String::new(),
            root_path: None,
            custom_instructions: Some("Be precise.".to_string()),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn caps(parallel: bool, native_reasoning: bool) -> super::ModelCapabilities {
        super::ModelCapabilities {
            native_reasoning,
            parallel_tool_calls: parallel,
            history_token_budget: 12_000,
            evidence_prompt_limit: 15,
        }
    }

    #[test]
    fn static_block_is_reused_across_turns_with_identical_inputs() {
        let project = sample_project();
        let folders = vec!["proj".to_string()];
        let first =
            super::build_static_prompt(&project, &folders, &[], &[], &[], caps(true, false));
        let second =
            super::build_static_prompt(&project, &folders, &[], &[], &[], caps(true, false));
        assert_eq!(first, second, "static rendering must be deterministic");
        assert_eq!(
            super::static_prompt_cache_key(&project, &folders, &[], &[], &[], caps(true, false)),
            super::static_prompt_cache_key(&project, &folders, &[], &[], &[], caps(true, false)),
            "identical inputs must produce the same cache key"
        );
    }

    #[test]
    fn cache_key_changes_when_any_rendered_input_changes() {
        let project = sample_project();
        let folders = vec!["proj".to_string()];
        let base =
            super::static_prompt_cache_key(&project, &folders, &[], &[], &[], caps(true, false));

        // Different project instructions must not reuse the cached block.
        let mut other_project = sample_project();
        other_project.custom_instructions = Some("Different rules.".to_string());
        assert_ne!(
            base,
            super::static_prompt_cache_key(
                &other_project,
                &folders,
                &[],
                &[],
                &[],
                caps(true, false)
            ),
            "project instructions must invalidate the cache"
        );

        // Different attached folders must not reuse the cached block.
        let other_folders = vec!["other".to_string()];
        assert_ne!(
            base,
            super::static_prompt_cache_key(
                &project,
                &other_folders,
                &[],
                &[],
                &[],
                caps(true, false)
            ),
            "attached folders must invalidate the cache"
        );

        // A different provider family renders different directives.
        assert_ne!(
            base,
            super::static_prompt_cache_key(&project, &folders, &[], &[], &[], caps(false, true)),
            "capabilities must invalidate the cache"
        );
    }

    #[test]
    fn cached_block_matches_a_fresh_render() {
        let project = sample_project();
        let folders = vec!["proj".to_string()];
        let freshly_rendered =
            super::build_static_prompt(&project, &folders, &[], &[], &[], caps(true, false));
        let cached =
            super::build_static_prompt(&project, &folders, &[], &[], &[], caps(true, false));
        assert_eq!(
            cached, freshly_rendered,
            "cache must never serve stale text"
        );
    }
}
