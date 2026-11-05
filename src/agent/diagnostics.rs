use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiagnosticTaskItem {
    pub file_path: String,
    pub lines: Vec<usize>,
    pub error_messages: Vec<String>,
    pub resolved: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SessionDiagnosticQueue {
    pub items: Vec<DiagnosticTaskItem>,
    pub pending_compile_cmd: Option<String>,
}

static SESSION_DIAGNOSTIC_STORE: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<String, SessionDiagnosticQueue>>> = std::sync::OnceLock::new();

fn get_diagnostic_store() -> &'static std::sync::RwLock<std::collections::HashMap<String, SessionDiagnosticQueue>> {
    SESSION_DIAGNOSTIC_STORE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}...", truncated)
    } else {
        s.to_string()
    }
}

/// Stores or replaces the diagnostic queue for a specific task session in RAM.
pub fn set_session_diagnostic_queue(session_id: &str, items: Vec<DiagnosticTaskItem>, compile_cmd: Option<String>) {
    if let Ok(mut lock) = get_diagnostic_store().write() {
        // Prevent unbounded memory growth if sessions disconnect abruptly without clean reset
        if lock.len() >= 64 && !lock.contains_key(session_id)
            && let Some(oldest_key) = lock.keys().next().cloned()
        {
            lock.remove(&oldest_key);
        }
        lock.insert(session_id.to_string(), SessionDiagnosticQueue {
            items,
            pending_compile_cmd: compile_cmd,
        });
    }
}

/// Retrieves the current diagnostic queue for a session.
pub fn get_session_diagnostic_queue(session_id: &str) -> Option<SessionDiagnosticQueue> {
    if let Ok(lock) = get_diagnostic_store().read() {
        lock.get(session_id).cloned()
    } else {
        None
    }
}

/// Marks all items associated with `file_path` as resolved (e.g. after editing the file).
pub fn mark_diagnostic_file_resolved(session_id: &str, file_path: &str) -> (usize, usize) {
    if let Ok(mut lock) = get_diagnostic_store().write()
        && let Some(queue) = lock.get_mut(session_id) {
            let norm_target = file_path.replace('\\', "/").trim_start_matches("./").to_lowercase();
            for item in &mut queue.items {
                let norm_item = item.file_path.replace('\\', "/").trim_start_matches("./").to_lowercase();
                if norm_item == norm_target
                    || norm_target.ends_with(&norm_item)
                    || norm_item.ends_with(&norm_target)
                    || (norm_target.contains('/') && norm_item.contains('/') && norm_target.rsplit('/').next() == norm_item.rsplit('/').next())
                {
                    item.resolved = true;
                }
            }
            let remaining = queue.items.iter().filter(|i| !i.resolved).count();
            return (remaining, queue.items.len());
        }
    (0, 0)
}

/// Clears the session diagnostic queue in RAM.
pub fn clear_session_diagnostic_queue(session_id: &str) {
    if let Ok(mut lock) = get_diagnostic_store().write() {
        lock.remove(session_id);
    }
}

/// Parses command output and returns structured `DiagnosticTaskItem` list grouped by file.
/// All error line numbers and error messages in the same file are aggregated into a single item
/// to enable single-pass reading and single-pass editing per file.
pub fn parse_diagnostic_items_from_output(output: &str) -> Vec<DiagnosticTaskItem> {
    let mut file_order: Vec<String> = Vec::new();
    let mut file_errors: std::collections::HashMap<String, (Vec<usize>, Vec<String>)> = std::collections::HashMap::new();
    let mut file_warnings: std::collections::HashMap<String, (Vec<usize>, Vec<String>)> = std::collections::HashMap::new();
    let mut warning_order: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some((fpath, line_no)) = parse_compiler_diagnostic_line(trimmed) {
            let lower = trimmed.to_lowercase();
            let is_warning = lower.contains("warning:") && !lower.contains("error");
            let msg = truncate_with_ellipsis(trimmed, 157);

            if is_warning {
                let entry = file_warnings.entry(fpath.clone()).or_insert_with(|| {
                    warning_order.push(fpath.clone());
                    (Vec::new(), Vec::new())
                });
                if let Some(ln) = line_no
                    && !entry.0.contains(&ln) {
                        entry.0.push(ln);
                    }
                if entry.1.len() < 5 && !entry.1.contains(&msg) {
                    entry.1.push(msg);
                }
            } else {
                let entry = file_errors.entry(fpath.clone()).or_insert_with(|| {
                    file_order.push(fpath.clone());
                    (Vec::new(), Vec::new())
                });
                if let Some(ln) = line_no
                    && !entry.0.contains(&ln) {
                        entry.0.push(ln);
                    }
                if entry.1.len() < 5 && !entry.1.contains(&msg) {
                    entry.1.push(msg);
                }
            }
        }
    }

    // Root-cause prioritization: If hard errors exist, exclude warnings
    let (target_order, mut target_map) = if !file_order.is_empty() {
        (file_order, file_errors)
    } else {
        (warning_order, file_warnings)
    };

    let mut items = Vec::new();
    for fpath in target_order {
        if let Some((mut lines, messages)) = target_map.remove(&fpath) {
            lines.sort();
            items.push(DiagnosticTaskItem {
                file_path: fpath,
                lines,
                error_messages: messages,
                resolved: false,
            });
        }
    }

    items.truncate(15);
    items
}

#[derive(Debug, Clone)]
pub struct GroupedFileDiagnostic {
    pub file_path: String,
    pub lines: Vec<usize>,
    pub error_count: usize,
    pub sample_messages: Vec<String>,
}

/// Universally parses a single compiler or linter diagnostic line into `(file_path, line_number)`.
/// Supports standard formats across all languages (Rust, TypeScript/JavaScript, Python, Go, C/C++,
/// C#, Dart, Kotlin, Swift, Zig, Java, Ruby, PHP, etc.):
/// - POSIX: `path/to/file.ext:88:11` or `path/to/file.ext:88`
/// - MSVC / C#: `path/to/file.ext(88,11)` or `path/to/file.ext(88)`
/// - Python: `File "path/to/file.py", line 45`
/// - Java / Maven: `[ERROR] /path/to/File.java:[42,15]`
pub fn parse_compiler_diagnostic_line(line: &str) -> Option<(String, Option<usize>)> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 1. Python traceback format: `File "...", line \d+`
    if let Some(file_idx) = trimmed.find("File \"") {
        let after_file = &trimmed[file_idx + 6..];
        if let Some(quote_idx) = after_file.find('"') {
            let file_path = after_file[..quote_idx].trim_start_matches("./");
            let remainder = &after_file[quote_idx + 1..];
            let mut line_no = None;
            if let Some(line_kw) = remainder.find("line ") {
                let num_part = &remainder[line_kw + 5..];
                let digits: String = num_part.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(ln) = digits.parse::<usize>() {
                    line_no = Some(ln);
                }
            }
            if !file_path.is_empty() {
                return Some((file_path.to_string(), line_no));
            }
        }
    }

    // 2. Universal compiler diagnostic pattern:
    // Matches file path with extension followed by :line[:col], (line[,col]), or :[line[,col]]
    static COMPILER_DIAG_RE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let re_opt = COMPILER_DIAG_RE.get_or_init(|| {
        regex::Regex::new(
            r#"(?m)(?:^|[^\w\./\\])([a-zA-Z0-9_\-\./\\]+\.[a-zA-Z0-9_\-]+)(?::(\d+)(?: : \d+)?|\((\d+)(?:,\d+)?\)|:\[(\d+)(?:,\d+)?\])"#
        ).ok()
    });

    // Also support strict colon without spaces
    static COMPILER_DIAG_STRICT: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let re_strict = COMPILER_DIAG_STRICT.get_or_init(|| {
        regex::Regex::new(
            r#"(?m)(?:^|[^\w\./\\])([a-zA-Z0-9_\-\./\\]+\.[a-zA-Z0-9_\-]+)(?::(\d+)(?::\d+)?|\((\d+)(?:,\d+)?\)|:\[(\d+)(?:,\d+)?\])"#
        ).ok()
    });

    let re_to_use = re_strict.as_ref().or(re_opt.as_ref());

    if let Some(re) = re_to_use
        && let Some(cap) = re.captures(trimmed)
            && let Some(file_match) = cap.get(1) {
                let raw_path = file_match.as_str().trim_matches(|c: char| c == '"' || c == '\'' || c == '(' || c == '[' || c == ']');
                let clean_path = raw_path.trim_start_matches("./");

                // Filter out URLs, protocols, or numeric/invalid extensions
                if !trimmed.contains("://")
                    && !clean_path.starts_with("//")
                    && !clean_path.starts_with("http:")
                    && !clean_path.starts_with("https:")
                    && let Some(dot_idx) = clean_path.rfind('.') {
                        let ext = &clean_path[dot_idx + 1..];
                        // Extension must have 1-8 alphanumeric chars and contain at least one letter (prevents IPs like 192.168.1.1)
                        if !ext.is_empty() && ext.len() <= 8 && ext.chars().any(|c| c.is_ascii_alphabetic()) {
                            let line_no = cap.get(2)
                                .or_else(|| cap.get(3))
                                .or_else(|| cap.get(4))
                                .and_then(|m| m.as_str().parse::<usize>().ok());
                            return Some((clean_path.to_string(), line_no));
                        }
                    }
            }

    None
}

/// Parses and groups diagnostic errors by file across any language/compiler.
/// Returns grouped summaries so the model can batch-edit entire files at once.
pub fn group_diagnostics_by_file(output: &str) -> (Vec<GroupedFileDiagnostic>, Vec<String>) {
    let mut file_map: HashMap<String, (Vec<usize>, usize, Vec<String>)> = HashMap::new();
    let mut file_order: Vec<String> = Vec::new();
    let mut general_errors: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some((fpath, line_no)) = parse_compiler_diagnostic_line(trimmed) {
            if !file_map.contains_key(&fpath) {
                file_order.push(fpath.clone());
            }
            let entry = file_map.entry(fpath).or_insert_with(|| (Vec::new(), 0, Vec::new()));
            entry.1 += 1;
            if let Some(ln) = line_no
                && !entry.0.contains(&ln) {
                    entry.0.push(ln);
                }
            let lower = trimmed.to_lowercase();
            if entry.2.len() < 3 && (lower.contains("error") || lower.contains("fail") || trimmed.contains("error[") || lower.contains("warning") || lower.contains("exception")) {
                let sample = truncate_with_ellipsis(trimmed, 117);
                entry.2.push(sample);
            }
        } else {
            let lower = trimmed.to_lowercase();
            let is_general = trimmed.starts_with("error[")
                || lower.starts_with("error:")
                || lower.starts_with("fatal:")
                || lower.starts_with("fatal error:")
                || lower.starts_with("panic:")
                || lower.ends_with("error:")
                || lower.contains("syntaxerror")
                || lower.contains("typeerror")
                || lower.contains("referenceerror")
                || lower.contains("assertionerror")
                || (lower.starts_with("found ") && lower.contains(" error"))
                || lower.starts_with("failed to compile")
                || lower.starts_with("compilation failed")
                || lower.starts_with("build failed");

            if is_general && general_errors.len() < 8 {
                general_errors.push(trimmed.to_string());
            }
        }
    }

    let grouped = file_order
        .into_iter()
        .filter_map(|fpath| {
            file_map.remove(&fpath).map(|(mut lines, count, samples)| {
                lines.sort();
                GroupedFileDiagnostic {
                    file_path: fpath,
                    lines,
                    error_count: count,
                    sample_messages: samples,
                }
            })
        })
        .collect();

    (grouped, general_errors)
}

/// Extracts diagnostic error summaries (file locations, line numbers, error descriptions)
/// from compiler, bundler, linter, or test runner output across any programming language.
pub fn extract_command_diagnostics(output: &str) -> Vec<String> {
    let mut diagnostics = Vec::new();
    let mut seen = HashSet::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let lower = trimmed.to_lowercase();
        let has_err_kw = lower.contains("error")
            || lower.contains("fail")
            || lower.contains("warning")
            || lower.contains("panic")
            || lower.contains("exception");

        let is_diag_line = parse_compiler_diagnostic_line(trimmed).is_some() && has_err_kw;

        let is_generic_error = trimmed.starts_with("error[")
            || lower.starts_with("error:")
            || lower.starts_with("fatal:")
            || lower.starts_with("fatal error:")
            || lower.starts_with("panic:")
            || lower.ends_with("error:")
            || lower.contains("syntaxerror")
            || lower.contains("typeerror")
            || lower.contains("referenceerror")
            || lower.contains("assertionerror")
            || (lower.starts_with("found ") && lower.contains(" error"))
            || lower.starts_with("failed to compile")
            || lower.starts_with("compilation failed")
            || lower.starts_with("build failed");

        if is_diag_line || is_generic_error {
            // Normalize and deduplicate
            let diag_entry = truncate_with_ellipsis(trimmed, 157);

            if seen.insert(diag_entry.clone()) {
                diagnostics.push(diag_entry);
                if diagnostics.len() >= 15 {
                    break;
                }
            }
        }
    }

    diagnostics
}

/// Checks if a tool execution output from workspace_run_command represents a compiler error / command failure.
/// Strictly scoped to command execution outputs (JSON with "command" or "exit_code") and failure banners,
/// preventing false positives from search/grep or file inspection on source code containing error strings.
pub fn is_command_output_error(output: &str) -> bool {
    // 1. Explicit tool execution failure
    if output.starts_with("Tool execution error") {
        return true;
    }

    // 2. Explicit command failure banner generated by workspace_run_command
    if output.contains("❌ COMMAND FAILED") || output.contains("❌ COMMAND TIMED OUT") {
        return true;
    }

    // 3. Inspect JSON output from workspace_run_command
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(output) {
        // Must be a command execution payload (has "command" or "exit_code")
        let is_cmd = val.get("command").is_some() || val.get("exit_code").is_some();
        if !is_cmd {
            return false;
        }

        let exit_code = val.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(0);
        let success = val.get("success").and_then(|v| v.as_bool()).unwrap_or(true);
        let status = val.get("status").and_then(|v| v.as_str()).unwrap_or("SUCCESS");

        if !success || exit_code != 0 || status == "FAILED" || status == "ERROR" || status == "TIMEOUT" {
            return true;
        }

        // Process exited cleanly with code 0.
        // Warnings, deprecation notices, or info logs in stderr MUST NOT be considered errors when exit_code is 0.
        let stderr = val.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
        if !stderr.is_empty() {
            let lower_err = stderr.to_lowercase();
            if lower_err.contains("panic:")
                || lower_err.contains("segmentation fault")
                || lower_err.contains("fatal error:")
            {
                return true;
            }
        }

        return false;
    }

    false
}

/// Determines if a command failure is primarily an external environment, network, port conflict,
/// or missing binary error rather than a source code compilation or lint failure.
pub fn is_environment_error(output: &str, grouped_diags: &[GroupedFileDiagnostic]) -> bool {
    // If there are compiler diagnostics pointing to files, it is a code error
    if !grouped_diags.is_empty() {
        return false;
    }

    let lower = output.to_lowercase();
    lower.contains("command not found")
        || lower.contains("not found")
        || lower.contains("not recognized as an internal or external command")
        || lower.contains("no such file or directory")
        || lower.contains("enoent")
        || lower.contains("econnrefused")
        || lower.contains("connection refused")
        || lower.contains("permission denied")
        || lower.contains("access is denied")
        || lower.contains("failed to connect")
        || lower.contains("network unreachable")
        || lower.contains("timed out")
        || lower.contains("timedout")
        || lower.contains("\"exit_code\": 127")
        || lower.contains("\"exit_code\":127")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_command_output_error_strictly_scoped() {
        // 1. Failed command with non-zero exit code
        let failed_cmd = serde_json::json!({
            "command": "cargo check",
            "exit_code": 1,
            "success": false,
            "stderr": "error[E0425]: cannot find value `foo` in this scope"
        }).to_string();
        assert!(is_command_output_error(&failed_cmd));

        // 2. Successful command (exit code 0)
        let success_cmd = serde_json::json!({
            "command": "npm run build",
            "exit_code": 0,
            "success": true,
            "stdout": "Build completed successfully."
        }).to_string();
        assert!(!is_command_output_error(&success_cmd));

        // 2b. Python unittest output sent via stderr by default (exit code 0, status OK)
        let python_unittest_ok = serde_json::json!({
            "command": "python3 -m unittest discover -s tests",
            "exit_code": 0,
            "success": true,
            "status": "SUCCESS",
            "stdout": "",
            "stderr": "....\n----------------------------------------------------------------------\nRan 4 tests in 0.001s\n\nOK\n"
        }).to_string();
        assert!(!is_command_output_error(&python_unittest_ok));

        // 3. Search / Grep results containing error terms (MUST NOT trigger false positive!)
        let grep_with_typeerror = serde_json::json!({
            "query": "TypeError",
            "matches_found": 1,
            "results": [{
                "file": "src/utils.ts",
                "line": 42,
                "content": "if (!valid) throw new TypeError('Invalid argument');"
            }]
        }).to_string();
        assert!(!is_command_output_error(&grep_with_typeerror));

        // 4. Raw file read with syntaxerror / typeerror (MUST NOT trigger false positive!)
        let raw_file_content = "class MyError extends TypeError {\n  constructor() { super('failed'); }\n}";
        assert!(!is_command_output_error(raw_file_content));

        // 5. Explicit error banner from tool execution
        assert!(is_command_output_error("Tool execution error: failed to edit file"));
        assert!(is_command_output_error("❌ COMMAND FAILED with exit code 2. Review stdout and stderr below."));
    }

    #[test]
    fn test_universal_compiler_diagnostic_parsing() {
        // TypeScript / JavaScript
        let ts = "src/App.tsx:88:11 - error TS2769: No overload matches this call.";
        assert_eq!(
            parse_compiler_diagnostic_line(ts),
            Some(("src/App.tsx".to_string(), Some(88)))
        );

        // Rust
        let rust = "src/main.rs:42:15: error[E0308]: mismatched types";
        assert_eq!(
            parse_compiler_diagnostic_line(rust),
            Some(("src/main.rs".to_string(), Some(42)))
        );

        // Python traceback
        let py = "  File \"simulations/python_scenario/main.py\", line 45, in <module>";
        assert_eq!(
            parse_compiler_diagnostic_line(py),
            Some(("simulations/python_scenario/main.py".to_string(), Some(45)))
        );

        // Go
        let go = "main.go:14:2: undefined: calculate_hash";
        assert_eq!(
            parse_compiler_diagnostic_line(go),
            Some(("main.go".to_string(), Some(14)))
        );

        // C / C++
        let cpp = "src/utils.cpp:102:5: error: 'vector' is not a member of 'std'";
        assert_eq!(
            parse_compiler_diagnostic_line(cpp),
            Some(("src/utils.cpp".to_string(), Some(102)))
        );

        // C# / MSVC
        let cs = "Controllers/App.cs(45,12): error CS0103: The name 'foo' does not exist";
        assert_eq!(
            parse_compiler_diagnostic_line(cs),
            Some(("Controllers/App.cs".to_string(), Some(45)))
        );

        // Dart
        let dart = "lib/foo.dart:10:5: Error: Undefined name 'bar'.";
        assert_eq!(
            parse_compiler_diagnostic_line(dart),
            Some(("lib/foo.dart".to_string(), Some(10)))
        );

        // Kotlin
        let kt = "src/Main.kt:15:10: error: unresolved reference: bar";
        assert_eq!(
            parse_compiler_diagnostic_line(kt),
            Some(("src/Main.kt".to_string(), Some(15)))
        );

        // Swift
        let swift = "Package.swift:12:5: error: cannot find 'test' in scope";
        assert_eq!(
            parse_compiler_diagnostic_line(swift),
            Some(("Package.swift".to_string(), Some(12)))
        );

        // Zig
        let zig = "src/main.zig:8:12: error: use of undeclared identifier 'x'";
        assert_eq!(
            parse_compiler_diagnostic_line(zig),
            Some(("src/main.zig".to_string(), Some(8)))
        );

        // Invalid: IP address or URL
        assert_eq!(parse_compiler_diagnostic_line("Connected to 192.168.1.1:8080"), None);
        assert_eq!(parse_compiler_diagnostic_line("GET http://example.com/api.js:80"), None);
    }

    #[test]
    fn test_group_diagnostics_by_file_multi_language() {
        let output = r#"
src/main.rs:12:5: error[E0308]: mismatched types
src/main.rs:25:9: error[E0425]: cannot find value `foo`
File "server.py", line 18, in run
Controllers/Order.cs(30,5): error CS0103: missing identifier
fatal error: build failed
"#;
        let (grouped, general) = group_diagnostics_by_file(output);
        assert_eq!(grouped.len(), 3);
        assert_eq!(grouped[0].file_path, "src/main.rs");
        assert_eq!(grouped[0].lines, vec![12, 25]);
        assert_eq!(grouped[0].error_count, 2);

        assert_eq!(grouped[1].file_path, "server.py");
        assert_eq!(grouped[1].lines, vec![18]);

        assert_eq!(grouped[2].file_path, "Controllers/Order.cs");
        assert_eq!(grouped[2].lines, vec![30]);

        assert!(general.iter().any(|g| g.contains("fatal error: build failed")));
    }

    #[test]
    fn test_extract_command_diagnostics_multi_language() {
        let output = r#"
src/app.zig:14:5: error: expected type 'i32', found 'bool'
lib/auth.dart:22:9: Error: The getter 'token' isn't defined.
Panic: runtime error: index out of range [5] with length 2
Random console log line with no errors
"#;
        let diags = extract_command_diagnostics(output);
        assert!(diags.iter().any(|d| d.contains("src/app.zig:14:5")));
        assert!(diags.iter().any(|d| d.contains("lib/auth.dart:22:9")));
        assert!(diags.iter().any(|d| d.contains("Panic: runtime error")));
        assert!(!diags.iter().any(|d| d.contains("Random console log line")));
    }

    #[test]
    fn test_multibyte_emoji_truncation_does_not_panic() {
        // String where 117th byte boundary falls inside a 4-byte emoji (like ❌)
        let prefix = "a".repeat(115);
        let error_line = format!("src/main.rs:10:5: error: {}❌ extra long error line exceeding max truncation limits", prefix);
        let (grouped, _general) = group_diagnostics_by_file(&error_line);
        assert!(!grouped.is_empty());

        let diags = extract_command_diagnostics(&error_line);
        assert!(!diags.is_empty());
    }
}