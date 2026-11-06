use super::diagnostics::is_command_output_error;
use super::models::Message;
use crate::llm::v1_engine::V1ToolCall;

/// Smartly truncates output keeping both the head and tail, omitting the middle.
pub fn truncate_smart(s: &str, max_head: usize, max_tail: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_head + max_tail + 100 {
        return s.to_string();
    }
    let head: String = s.chars().take(max_head).collect();
    let tail: String = s
        .chars()
        .skip(char_count.saturating_sub(max_tail))
        .collect();
    let omitted = char_count.saturating_sub(max_head + max_tail);
    format!("{head}\n\n[... {omitted} characters omitted to preserve token budget ...]\n\n{tail}")
}

/// Estimates token count from a message for budget calculations (approx 4 chars per token)
pub fn estimate_message_tokens(msg: &Message) -> usize {
    let mut total = 0;
    if let Some(ref c) = msg.content {
        total += c.len();
    }
    if let Some(ref tc) = msg.tool_calls {
        // If the message has tool calls with large arguments (like workspace_write_file or edit),
        // in historical context those arguments are compacted to a lightweight summary (~200 chars).
        let tc_len = if tc.len() > 1_000
            && (tc.contains("workspace_write_file") || tc.contains("workspace_edit_file"))
        {
            1_000
        } else {
            tc.len()
        };
        total += tc_len;
    }
    (total / 4).max(1)
}

/// Generates a rolling hierarchical semantic summary of older messages evicted from the active sliding window,
/// preserving user directives, file mutations, actions, and decisions without memory amnesia across long multi-turn sessions.
pub fn summarize_evicted_messages(evicted: &[&Message]) -> String {
    let mut user_goals = Vec::new();
    let mut modified_files = Vec::new();
    let mut executed_cmds = Vec::new();
    let mut key_read_files = Vec::new();
    let mut decisions = Vec::new();

    for msg in evicted {
        if msg.role == "user" {
            if let Some(ref c) = msg.content {
                let trimmed = c.trim();
                // Filter out synthetic or tool-injected eviction summaries
                if !trimmed.is_empty()
                    && !trimmed.starts_with("### 🧠")
                    && !trimmed.starts_with("VERIFICATION")
                {
                    let first_line = trimmed.lines().next().unwrap_or(trimmed);
                    let short: String = first_line.chars().take(120).collect();
                    if !user_goals.contains(&short) {
                        user_goals.push(short);
                    }
                }
            }
        } else if msg.role == "assistant" {
            if let Some(ref thought) = msg.thought {
                let trimmed = thought.trim();
                if !trimmed.is_empty() {
                    let first_line = trimmed.lines().next().unwrap_or(trimmed);
                    let short: String = first_line.chars().take(140).collect();
                    if !decisions.contains(&short) && decisions.len() < 4 {
                        decisions.push(short);
                    }
                }
            }
            if let Some(ref tc_raw) = msg.tool_calls
                && let Ok(calls) = serde_json::from_str::<Vec<V1ToolCall>>(tc_raw)
            {
                for call in calls {
                    if let Ok(args) =
                        serde_json::from_str::<serde_json::Value>(&call.function.arguments)
                    {
                        if call.function.name == "workspace_edit_file"
                            || call.function.name == "workspace_write_file"
                        {
                            if let Some(p) = args
                                .get("file_path")
                                .or_else(|| args.get("path"))
                                .and_then(|v| v.as_str())
                                && !modified_files.contains(&p.to_string())
                            {
                                modified_files.push(p.to_string());
                            }
                        } else if call.function.name == "workspace_run_command" {
                            if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
                                let clean_cmd = cmd.trim().to_string();
                                if !executed_cmds.contains(&clean_cmd) {
                                    executed_cmds.push(clean_cmd);
                                }
                            }
                        } else if call.function.name == "workspace_read_file"
                            && let Some(p) = args
                                .get("file_path")
                                .or_else(|| args.get("path"))
                                .and_then(|v| v.as_str())
                            && !key_read_files.contains(&p.to_string())
                        {
                            key_read_files.push(p.to_string());
                        }
                    }
                }
            }
        }
    }

    let mut summary = String::new();
    summary.push_str("### 🧠 Hierarchical Memory Distillation (Earlier Evicted Turns):\n");
    if !user_goals.is_empty() {
        summary.push_str(&format!(
            "- **User Directives**: {}\n",
            user_goals.join(" | ")
        ));
    }
    if !modified_files.is_empty() {
        summary.push_str(&format!(
            "- **Files Modified**: {}\n",
            modified_files.join(", ")
        ));
    }
    if !executed_cmds.is_empty() {
        summary.push_str(&format!(
            "- **Key Commands**: {}\n",
            executed_cmds
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join(" && ")
        ));
    }
    if !key_read_files.is_empty() {
        summary.push_str(&format!(
            "- **Inspected Files**: {}\n",
            key_read_files
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !decisions.is_empty() {
        summary.push_str(&format!(
            "- **Prior Reasoning & Decisions**: {}\n",
            decisions.join(" | ")
        ));
    }
    summary.push_str("- Retain this context and build directly upon it without repeating earlier operations.\n\n");
    summary
}

/// Sanitizes tool output for persistent storage in database.
/// If tool output is extremely large (e.g. read_file > 32,000 chars), we truncate the middle
/// to keep SQLite and frontend state lightweight, while keeping live buffer intact in memory.
pub fn sanitize_tool_output_for_db(content: &str) -> String {
    if content.len() > 32_000 {
        let prefix: String = content.chars().take(16_000).collect();
        let suffix: String = content
            .chars()
            .rev()
            .take(8_000)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!(
            "{}\n\n[... Output truncated for database efficiency ({} total bytes) ...]\n\n{}",
            prefix,
            content.len(),
            suffix
        )
    } else {
        content.to_string()
    }
}

pub fn sanitize_tool_output_for_memory(content: &str) -> String {
    if content.len() > 32_000 {
        let prefix: String = content.chars().take(16_000).collect();
        let suffix: String = content
            .chars()
            .rev()
            .take(8_000)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!(
            "{}\n\n[... Large tool output truncated for in-memory context budget ({} total bytes) ...]\n\n{}",
            prefix,
            content.len(),
            suffix
        )
    } else {
        content.to_string()
    }
}

/// Prunes tool output results to optimize context window, token cost, and LLM inference latency.
/// Preserves the live tail intact while eliding bulky older outputs.
/// Retains compiler error diagnostics up to 12,000 chars while heavily compacting successful command runs
/// and replacing historical file reads with lightweight DB cache references.
pub fn compact_tool_output(content: &str, is_recent_tail: bool) -> String {
    compact_tool_output_smart(content, None, is_recent_tail)
}

/// Compacts assistant tool_call arguments in older history.
/// In particular, historical `workspace_write_file` or `workspace_edit_file` calls that have
/// already completed on disk do not need to repeat thousands of characters of file content
/// in every subsequent turn.
pub fn compact_assistant_tool_calls_smart(
    calls: &mut [crate::llm::v1_engine::V1ToolCall],
    is_recent_tail: bool,
) {
    if is_recent_tail {
        return;
    }
    for tc in calls.iter_mut() {
        let name = tc.function.name.as_str();
        if name == "workspace_write_file" {
            if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&tc.function.arguments) {
                let file_path = val
                    .get("file_path")
                    .and_then(|p| p.as_str())
                    .unwrap_or("file")
                    .to_string();
                let content_len = val
                    .get("content")
                    .and_then(|c| c.as_str())
                    .map(|s| s.len())
                    .unwrap_or(0);
                if content_len > 200 {
                    val["content"] = serde_json::json!(format!(
                        "[Content omitted: {} chars written to `{}`]",
                        content_len, file_path
                    ));
                    if let Ok(compacted_args) = serde_json::to_string(&val) {
                        tc.function.arguments = compacted_args;
                    }
                }
            }
        } else if name == "workspace_edit_file"
            && let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
        {
            let file_path = val
                .get("file_path")
                .and_then(|p| p.as_str())
                .unwrap_or("file")
                .to_string();
            let (should_compact, edits_count, total_chars) =
                if let Some(edits) = val.get("edits").and_then(|e| e.as_array()) {
                    let total_chars: usize = edits
                        .iter()
                        .map(|e| {
                            let old_len = e
                                .get("old_text")
                                .and_then(|t| t.as_str())
                                .map(|t| t.len())
                                .unwrap_or(0);
                            let new_len = e
                                .get("new_text")
                                .and_then(|t| t.as_str())
                                .map(|t| t.len())
                                .unwrap_or(0);
                            old_len + new_len
                        })
                        .sum();
                    (total_chars > 300, edits.len(), total_chars)
                } else {
                    (false, 0, 0)
                };

            if should_compact {
                val["edits"] = serde_json::json!([
                    {
                        "summary": format!("[{} edit hunk(s) applied to `{}` ({} chars)]", edits_count, file_path, total_chars)
                    }
                ]);
                if let Ok(compacted_args) = serde_json::to_string(&val) {
                    tc.function.arguments = compacted_args;
                }
            }
        }
    }
}

/// Context-aware tool output compactor that differentiates by tool type.
pub fn compact_tool_output_smart(
    content: &str,
    tool_name: Option<&str>,
    is_recent_tail: bool,
) -> String {
    // 1. Preserve compiler/test error diagnostics up to 12,000 chars so actionable error messages are never lost
    if is_command_output_error(content) {
        if content.len() > 12_000 {
            return truncate_smart(content, 4_000, 8_000);
        }
        return content.to_string();
    }

    match tool_name {
        Some("workspace_run_command") => {
            // For successful commands (exit 0, tests passing, builds, etc.):
            if is_recent_tail {
                // Keep up to 6,000 chars (2,000 head + 4,000 tail)
                truncate_smart(content, 2_000, 4_000)
            } else {
                // Older successful command runs: retain up to 3,000 chars
                truncate_smart(content, 1_000, 2_000)
            }
        }
        Some("workspace_read_file") => {
            if is_recent_tail {
                // In active recent turns: keep up to 10,000 chars for live edits
                truncate_smart(content, 4_000, 6_000)
            } else {
                // In older historical turns: retain concise evidence summary (up to 1,500 chars)
                // The full content is safely persisted in DB evidence_cache, preventing massive prompt bloat
                truncate_smart(content, 600, 900)
            }
        }
        Some("workspace_list_dir") => {
            if is_recent_tail {
                truncate_smart(content, 1_000, 1_500)
            } else if content.len() > 300 {
                let first_line = content.lines().next().unwrap_or("").trim();
                format!(
                    "{}\n[... {} characters of directory listing omitted ...]",
                    first_line,
                    content.len()
                )
            } else {
                content.to_string()
            }
        }
        Some("workspace_grep") => {
            if is_recent_tail {
                truncate_smart(content, 1_500, 2_500)
            } else if content.len() > 400 {
                let first_line = content.lines().next().unwrap_or("").trim();
                format!(
                    "{}\n[... {} characters of grep search results omitted ...]",
                    first_line,
                    content.len()
                )
            } else {
                content.to_string()
            }
        }
        _ => {
            // General fallback
            if is_recent_tail {
                truncate_smart(content, 3_000, 5_000)
            } else if content.len() > 500 {
                let first_line = content.lines().next().unwrap_or("").trim();
                format!(
                    "{}\n[... {} characters of historical tool output compacted to preserve token budget ...]",
                    first_line,
                    content.len()
                )
            } else {
                content.to_string()
            }
        }
    }
}

/// Estimates token count with a high-accuracy, language-agnostic heuristic (similar to BPE tokenizer approximations).
/// Accounts for word boundaries, punctuation, numbers, whitespace sequences, and special characters.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    let mut tokens = 0usize;
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];
        if ch.is_whitespace() {
            // Consecutive whitespaces (spaces, tabs, newlines)
            let start = i;
            while i < len && chars[i].is_whitespace() {
                i += 1;
            }
            let ws_len = i - start;
            // Indentations or multiple spaces: roughly 1 token per 4 spaces or newlines
            tokens += ws_len.div_ceil(4);
        } else if ch.is_ascii_alphanumeric() {
            // Consecutive alphanumeric characters
            let start = i;
            while i < len && chars[i].is_ascii_alphanumeric() {
                i += 1;
            }
            let word_len = i - start;
            // Standard average BPE token is ~3.8-4 characters
            tokens += word_len.div_ceil(4);
        } else if ch.is_ascii_punctuation() {
            // Punctuation characters are usually 1 token each in BPE, or pairs
            tokens += 1;
            i += 1;
        } else {
            // Non-ASCII (Unicode, CJK, Emoji, accented chars)
            // CJK characters or complex unicode symbols typically consume 1-2 tokens per char
            tokens += 2;
            i += 1;
        }
    }

    // Safety lower bound
    tokens.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert!(estimate_tokens("hello world") >= 2);
        assert!(estimate_tokens("function test() { return true; }") >= 5);
        assert!(estimate_tokens("こんにちは世界") >= 4);
    }

    #[test]
    fn test_sanitize_tool_output_for_memory_capping() {
        let small = "{\"status\": \"ok\", \"count\": 5}";
        assert_eq!(sanitize_tool_output_for_memory(small), small);

        let huge = "X".repeat(100_000);
        let sanitized = sanitize_tool_output_for_memory(&huge);
        assert!(sanitized.len() < 35_000);
        assert!(sanitized.contains("Large tool output truncated for in-memory context budget"));
    }

    #[test]
    fn test_compact_tool_output_caps_large_error_diagnostics() {
        let huge_error = format!(
            "❌ COMMAND FAILED with exit code 1\n{}",
            "error: type mismatch\n".repeat(2_000)
        );
        assert!(huge_error.len() > 30_000);
        let compacted = compact_tool_output(&huge_error, true);
        assert!(compacted.len() <= 15_000);
    }

    #[test]
    fn test_compact_successful_command_output() {
        let successful_build = format!(
            "Compiling crates...\n{}Finished release [optimized] target(s) in 12.3s",
            "   Compiling dep v0.1\n".repeat(600)
        );
        assert!(successful_build.len() > 10_000);

        // Recent tail preserves up to 6,000 chars
        let tail_compacted =
            compact_tool_output_smart(&successful_build, Some("workspace_run_command"), true);
        assert!(tail_compacted.len() <= 7_000);
        assert!(tail_compacted.contains("characters omitted to preserve token budget"));

        // Historical turn retains up to 3,000 chars
        let hist_compacted =
            compact_tool_output_smart(&successful_build, Some("workspace_run_command"), false);
        assert!(hist_compacted.len() <= 3_500);
        assert!(hist_compacted.contains("characters omitted to preserve token budget"));
    }

    #[test]
    fn test_compact_historical_file_reads() {
        let file_content = format!(
            "// Header\n{}",
            "fn do_something() { println!(\"hello\"); }\n".repeat(200)
        );
        assert!(file_content.len() > 8_000);

        // Historical file read preserves code context (up to 4,000 chars) instead of a 1-line tombstone
        let hist_compacted =
            compact_tool_output_smart(&file_content, Some("workspace_read_file"), false);
        assert!(hist_compacted.len() <= 4_500);
        assert!(hist_compacted.contains("fn do_something()"));
        assert!(hist_compacted.contains("characters omitted to preserve token budget"));
    }

    #[test]
    fn test_compact_assistant_tool_calls_smart() {
        use crate::llm::v1_engine::{V1ToolCall, V1ToolCallFunction};

        let mut calls = vec![
            V1ToolCall {
                id: "call_write_1".to_string(),
                call_type: "function".to_string(),
                function: V1ToolCallFunction {
                    name: "workspace_write_file".to_string(),
                    arguments: serde_json::json!({
                        "file_path": "src/App.tsx",
                        "content": "A".repeat(10_000)
                    })
                    .to_string(),
                },
                thought_signature: None,
                extra_content: None,
                provider_specific_fields: None,
            },
            V1ToolCall {
                id: "call_edit_1".to_string(),
                call_type: "function".to_string(),
                function: V1ToolCallFunction {
                    name: "workspace_edit_file".to_string(),
                    arguments: serde_json::json!({
                        "file_path": "src/index.ts",
                        "edits": [
                            {"old_text": "B".repeat(200), "new_text": "C".repeat(200)}
                        ]
                    })
                    .to_string(),
                },
                thought_signature: None,
                extra_content: None,
                provider_specific_fields: None,
            },
        ];

        // Tail call is preserved verbatim
        let mut tail_calls = calls.clone();
        compact_assistant_tool_calls_smart(&mut tail_calls, true);
        assert!(tail_calls[0].function.arguments.contains(&"A".repeat(500)));

        // Historical call is compacted
        compact_assistant_tool_calls_smart(&mut calls, false);
        assert!(!calls[0].function.arguments.contains(&"A".repeat(500)));
        assert!(
            calls[0]
                .function
                .arguments
                .contains("[Content omitted: 10000 chars written to `src/App.tsx`]")
        );
        assert!(
            calls[1]
                .function
                .arguments
                .contains("[1 edit hunk(s) applied to `src/index.ts`")
        );
    }
}
