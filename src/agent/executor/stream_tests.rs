use super::accumulate_tool_call_delta;
use super::{
    MAX_RETRY_DELAY_MS, RetryDecision, cancellable_sleep, is_retryable_error, parse_retry_after_ms,
    retry_delay,
};
use crate::llm::v1_engine::V1ToolCall;
use std::collections::HashMap;

#[test]
fn test_extract_synthesized_thought_tags() {
    let input_content = "<thought>\nInspect file scenario/backend/src/main.rs due to Todo struct initializer error.\n</thought>\nI will inspect main.rs first.";
    let mut clean_content = String::new();
    let mut extracted_thoughts = Vec::new();
    let mut remaining = input_content;

    while let Some(open_idx) = remaining.find("<thought>") {
        clean_content.push_str(&remaining[..open_idx]);
        let after_open = &remaining[open_idx + "<thought>".len()..];
        if let Some(close_idx) = after_open.find("</thought>") {
            let inner = after_open[..close_idx].trim();
            if !inner.is_empty() {
                extracted_thoughts.push(inner.to_string());
            }
            remaining = &after_open[close_idx + "</thought>".len()..];
        } else {
            let inner = after_open.trim();
            if !inner.is_empty() {
                extracted_thoughts.push(inner.to_string());
            }
            remaining = "";
            break;
        }
    }
    clean_content.push_str(remaining);

    assert_eq!(extracted_thoughts.len(), 1);
    assert!(extracted_thoughts[0].contains("Inspect file scenario/backend/src/main.rs"));
    assert_eq!(clean_content.trim(), "I will inspect main.rs first.");
}

#[test]
fn test_accumulate_ignores_blank_name_after_real_name() {
    // Exact wire pattern from cbai/deepseek-v4.1-flash via 9Router:
    // delta 1 carries the name, every later delta repeats `"name": ""`.
    // The blank must never overwrite the real name.
    let mut collected = empty_call();
    let mut seen = HashMap::new();

    accumulate_tool_call_delta(
        &mut collected,
        &mut seen,
        0,
        Some("call_00_BlwK".into()),
        Some("workspace_list_dir".into()),
        "",
    );
    accumulate_tool_call_delta(
        &mut collected,
        &mut seen,
        0,
        Some("call_00_BlwK".into()),
        Some(String::new()),
        "{\"path",
    );
    accumulate_tool_call_delta(
        &mut collected,
        &mut seen,
        0,
        Some("call_00_BlwK".into()),
        Some(String::new()),
        "\":\"src/agent\"}",
    );

    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].function.name, "workspace_list_dir");
    assert_eq!(collected[0].function.arguments, "{\"path\":\"src/agent\"}");
}

#[test]
fn test_accumulate_whitespace_name_is_ignored() {
    let mut collected = empty_call();
    let mut seen = HashMap::new();

    accumulate_tool_call_delta(
        &mut collected,
        &mut seen,
        0,
        Some("c".into()),
        Some("workspace_grep".into()),
        "",
    );
    accumulate_tool_call_delta(&mut collected, &mut seen, 0, None, Some("   ".into()), "{}");

    assert_eq!(collected[0].function.name, "workspace_grep");
}

fn empty_call() -> Vec<V1ToolCall> {
    Vec::new()
}

#[test]
fn test_accumulate_keeps_name_when_first_delta_has_no_name() {
    // Regression for `Unknown tool ''`: the name must survive when a later
    // delta carries it, and arguments must not be lost.
    let mut collected = empty_call();
    let mut seen = HashMap::new();

    accumulate_tool_call_delta(
        &mut collected,
        &mut seen,
        0,
        Some("call_1".into()),
        None,
        "",
    );
    accumulate_tool_call_delta(
        &mut collected,
        &mut seen,
        0,
        None,
        Some("workspace_list_dir".into()),
        "{\"path\":\"ragasukma/src\"}",
    );

    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].function.name, "workspace_list_dir");
    assert_eq!(
        collected[0].function.arguments,
        "{\"path\":\"ragasukma/src\"}"
    );
}

#[test]
fn test_accumulate_does_not_duplicate_repeated_fragment() {
    // Regression for payloads like {"path":"a"}{"path":"a"}.
    let mut collected = empty_call();
    let mut seen = HashMap::new();
    let fragment = "{\"path\":\"a\"}";

    accumulate_tool_call_delta(
        &mut collected,
        &mut seen,
        0,
        Some("c".into()),
        Some("workspace_grep".into()),
        fragment,
    );
    accumulate_tool_call_delta(
        &mut collected,
        &mut seen,
        0,
        None,
        Some("workspace_grep".into()),
        fragment,
    );

    assert_eq!(collected[0].function.arguments, fragment);
}

#[test]
fn test_accumulate_joins_partial_fragments() {
    let mut collected = empty_call();
    let mut seen = HashMap::new();

    accumulate_tool_call_delta(
        &mut collected,
        &mut seen,
        0,
        Some("c".into()),
        Some("workspace_read_file".into()),
        "{\"path\":",
    );
    accumulate_tool_call_delta(&mut collected, &mut seen, 0, None, None, "\"src/main.rs\"}");

    assert_eq!(collected[0].function.name, "workspace_read_file");
    assert_eq!(
        collected[0].function.arguments,
        "{\"path\":\"src/main.rs\"}"
    );
}

#[test]
fn test_accumulate_resets_on_restarted_slot() {
    // A retry can reuse index 0 with a new id/name; stale arguments must not leak.
    let mut collected = empty_call();
    let mut seen = HashMap::new();

    accumulate_tool_call_delta(
        &mut collected,
        &mut seen,
        0,
        Some("call_A".into()),
        Some("workspace_list_dir".into()),
        "{\"path\":\"old\"}",
    );
    accumulate_tool_call_delta(
        &mut collected,
        &mut seen,
        0,
        Some("call_B".into()),
        Some("workspace_list_dir".into()),
        "{\"path\":\"new\"}",
    );

    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].id, "call_B");
    assert_eq!(collected[0].function.arguments, "{\"path\":\"new\"}");
}

#[test]
fn test_accumulate_handles_parallel_indices() {
    let mut collected = empty_call();
    let mut seen = HashMap::new();

    accumulate_tool_call_delta(
        &mut collected,
        &mut seen,
        1,
        Some("b".into()),
        Some("workspace_grep".into()),
        "{}",
    );
    assert_eq!(collected.len(), 2);
    assert_eq!(collected[1].function.name, "workspace_grep");
    assert!(collected[0].function.name.is_empty());
}

fn wait_of(decision: RetryDecision) -> std::time::Duration {
    match decision {
        RetryDecision::Wait(d) => d,
        other => panic!("expected Wait, got {other:?}"),
    }
}

#[test]
fn retry_delay_honours_retry_after_header() {
    let d = wait_of(retry_delay(
        1,
        "429 Too Many Requests, Retry-After: 4",
        None,
    ));
    assert_eq!(d, std::time::Duration::from_millis(4_000));
}

#[test]
fn retry_delay_honours_server_wait_above_backoff_cap() {
    // 60s > MAX_RETRY_DELAY_MS (30s) but within the server ceiling: must NOT be shortened.
    let d = wait_of(retry_delay(1, "429 Retry-After: 60", None));
    assert_eq!(d, std::time::Duration::from_secs(60));
}

#[test]
fn retry_delay_gives_up_when_server_wait_too_long() {
    assert_eq!(
        retry_delay(1, "503 Retry-After: 99999", None),
        RetryDecision::GiveUp {
            server_wait: std::time::Duration::from_secs(99_999)
        }
    );
}

#[test]
fn retry_delay_backs_off_and_never_exceeds_cap() {
    for attempt in 1..=6 {
        let d = wait_of(retry_delay(attempt, "503 service unavailable", None));
        assert!(d.as_millis() > 0, "attempt {attempt} must wait");
        assert!(
            d.as_millis() <= MAX_RETRY_DELAY_MS as u128,
            "attempt {attempt} exceeded cap"
        );
    }
}

#[test]
fn retry_delay_jitters_between_calls() {
    // Full jitter: repeated calls at the same attempt should not be byte-identical.
    let samples: Vec<u128> = (0..16)
        .map(|_| wait_of(retry_delay(3, "502 bad gateway", None)).as_millis())
        .collect();
    assert!(
        samples.windows(2).any(|w| w[0] != w[1]),
        "no jitter observed"
    );
}

#[test]
fn retry_delay_uses_structured_retry_after_header() {
    // When a structured Retry-After header value is provided, it takes precedence
    // over any value parsed from the error string.
    let d = wait_of(retry_delay(1, "429 Too Many Requests", Some("10")));
    assert_eq!(d, std::time::Duration::from_secs(10));
}

#[test]
fn retry_delay_structured_header_fractional() {
    let d = wait_of(retry_delay(1, "429", Some("2.5")));
    assert_eq!(d, std::time::Duration::from_millis(2_500));
}

#[test]
fn parse_retry_after_ignores_absent_header() {
    assert_eq!(parse_retry_after_ms("connection closed"), None);
}

#[test]
fn parse_retry_after_supports_common_formats() {
    assert_eq!(parse_retry_after_ms("Retry-After: 5"), Some(5_000));
    assert_eq!(parse_retry_after_ms(r#"{"retry-after": "7"}"#), Some(7_000));
    assert_eq!(parse_retry_after_ms("retry-after=1.5"), Some(1_500));
    assert_eq!(parse_retry_after_ms("Retry-After-Ms: 500"), Some(500));
}

#[test]
fn parse_retry_after_does_not_grab_unrelated_numbers() {
    // HTTP-date is unsupported -> None, rather than picking up "21" or "2015".
    assert_eq!(
        parse_retry_after_ms("Retry-After: Wed, 21 Oct 2015 07:28:00 GMT, 14290 tokens"),
        None
    );
    // Header name with no value must not consume a later number.
    assert_eq!(
        parse_retry_after_ms("retry-after missing; used 8000 tokens"),
        None
    );
}

#[test]
fn retryable_error_matches_status_codes_as_whole_numbers() {
    assert!(is_retryable_error("HTTP 429 Too Many Requests"));
    assert!(is_retryable_error("status: 503"));
    assert!(is_retryable_error("Anthropic 529 overloaded_error"));
    assert!(is_retryable_error(
        "Failed to send request: connection reset"
    ));
    assert!(is_retryable_error("Error reading stream endpoint"));
    // Numbers that merely contain a status code must not trigger a retry.
    assert!(!is_retryable_error("prompt is 14290 tokens, max is 8192"));
    assert!(!is_retryable_error(
        "HTTP 400 Bad Request: invalid tool schema"
    ));
    assert!(!is_retryable_error("401 Unauthorized"));
}

#[test]
fn cancellable_sleep_stops_on_cancel() {
    let flag = std::sync::atomic::AtomicBool::new(true);
    let start = std::time::Instant::now();
    assert!(!cancellable_sleep(
        std::time::Duration::from_secs(30),
        &flag
    ));
    assert!(start.elapsed() < std::time::Duration::from_secs(1));

    let flag = std::sync::atomic::AtomicBool::new(false);
    assert!(cancellable_sleep(
        std::time::Duration::from_millis(10),
        &flag
    ));
}
