//! Shared classification of OAuth token-refresh failures.
//!
//! Mirrors 9router `isUnrecoverableRefreshError`: some refresh failures are
//! *permanent* — the refresh token was rotated elsewhere, revoked, or the
//! grant was invalidated. Retrying those on every request will never succeed
//! (and, for providers that rotate refresh tokens, can keep the session dead).
//!
//! The Rust port keeps `anyhow::Result` signatures but marks permanent
//! failures by `context`-labeling them with [`UNRECOVERABLE_TAG`], so callers
//! can detect them without changing every signature.

/// Marker embedded in the error message of a permanent refresh failure.
pub const UNRECOVERABLE_TAG: &str = "unrecoverable_refresh_error";

/// OAuth refresh-failure markers that indicate a refresh can never succeed
/// again. Exact set copied from 9router `classifyOAuthRefreshError`.
const UNRECOVERABLE_CODES: &[&str] = &[
    "refresh_token_expired",
    "refresh_token_reused",
    "refresh_token_invalidated",
    "invalid_grant",
];

/// Extract the OAuth `error` field from a raw token-endpoint error body.
///
/// Mirrors 9router `classifyOAuthRefreshError`: it parses the body as JSON and
/// reads `error.code`, `error`, or `error_code` (in that order). Non-JSON
/// bodies yield no code — the caller still gets the raw text as description,
/// which is what the reference substring-matches against.
pub fn extract_error_code(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }

    let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;

    // parsed?.error?.code || parsed?.error || parsed?.error_code || ""
    let code = parsed
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_str())
        .or_else(|| parsed.get("error").and_then(|e| e.as_str()))
        .or_else(|| parsed.get("error_code").and_then(|c| c.as_str()))
        .unwrap_or_default();

    if code.is_empty() {
        None
    } else {
        Some(code.to_string())
    }
}

/// True when the given OAuth `error` code is permanent.
pub fn is_unrecoverable_code(code: &str) -> bool {
    let lowered = code.to_lowercase();
    UNRECOVERABLE_CODES
        .iter()
        .any(|marker| lowered.contains(marker))
}

/// True when a raw error body represents a permanent refresh failure.
///
/// Reference behavior: combine `code` and `description` (falling back to the
/// raw body), lowercase, then substring-match any permanent marker.
pub fn is_unrecoverable_body(body: &str) -> bool {
    let code = extract_error_code(body).unwrap_or_default();
    let description = serde_json::from_str::<serde_json::Value>(body.trim())
        .ok()
        .and_then(|v| {
            v.get("error_description")
                .and_then(|d| d.as_str())
                .or_else(|| v.get("message").and_then(|m| m.as_str()))
                .map(|s| s.to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| body.to_string());

    let combined = format!("{code} {description}").to_lowercase();
    UNRECOVERABLE_CODES
        .iter()
        .any(|marker| combined.contains(marker))
}

/// True when a surfaced [`anyhow::Error`] is a permanent refresh failure.
pub fn is_unrecoverable_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        let msg = cause.to_string();
        msg.contains(UNRECOVERABLE_TAG) || is_unrecoverable_body(&msg)
    })
}

/// Build the user-facing message for a failed token refresh, tagging it when
/// the failure is permanent so callers can deactivate the account.
pub fn refresh_failure_message(provider: &str, status: u16, body: &str) -> String {
    if is_unrecoverable_body(body) {
        let code = extract_error_code(body).unwrap_or_else(|| "unknown".to_string());
        format!(
            "{UNRECOVERABLE_TAG}: failed to refresh {provider} token (HTTP {status}, error: {code}) — \
             the refresh token was revoked or already rotated; please re-authenticate this account"
        )
    } else {
        format!("Failed to refresh {provider} token (HTTP {status}): {body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_json_invalid_grant() {
        let body =
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#;
        assert_eq!(extract_error_code(body).as_deref(), Some("invalid_grant"));
        assert!(is_unrecoverable_body(body));
    }

    /// Non-JSON bodies yield no code (reference only JSON-parses), but the raw
    /// text still participates in the marker substring-match.
    #[test]
    fn non_json_body_only_matches_via_description() {
        let form = "error=refresh_token_reused&error_description=already+used";
        assert_eq!(extract_error_code(form), None);
        assert!(is_unrecoverable_body(form));

        assert_eq!(extract_error_code("invalid_grant"), None);
        assert!(is_unrecoverable_body("invalid_grant"));
    }

    /// `parsed.error.code` takes precedence over `parsed.error`.
    #[test]
    fn prefers_nested_error_code() {
        let body = r#"{"error":{"code":"invalid_grant","message":"x"}}"#;
        assert_eq!(extract_error_code(body).as_deref(), Some("invalid_grant"));
        assert!(is_unrecoverable_body(body));
    }

    #[test]
    fn ignores_error_code_field_when_empty() {
        let body = r#"{"error_description":"boom"}"#;
        assert_eq!(extract_error_code(body), None);
        assert!(!is_unrecoverable_body(body));
    }

    /// Reference marker set is exactly these four; `invalid_request` /
    /// `invalid_client` must NOT be treated as permanent.
    #[test]
    fn only_reference_markers_are_permanent() {
        for code in [
            "refresh_token_expired",
            "refresh_token_reused",
            "refresh_token_invalidated",
            "invalid_grant",
        ] {
            assert!(is_unrecoverable_code(code), "{code} should be permanent");
        }
        for code in ["invalid_request", "invalid_client", "server_error"] {
            assert!(!is_unrecoverable_code(code), "{code} should be transient");
        }
    }

    #[test]
    fn treats_server_error_as_transient() {
        let body = r#"{"error":"server_error"}"#;
        assert_eq!(extract_error_code(body).as_deref(), Some("server_error"));
        assert!(!is_unrecoverable_body(body));
    }

    #[test]
    fn empty_and_html_bodies_are_transient() {
        assert!(!is_unrecoverable_body(""));
        assert!(!is_unrecoverable_body("<html>502 Bad Gateway</html>"));
    }

    #[test]
    fn tagged_error_is_detected_through_anyhow() {
        let err = anyhow::anyhow!("{}", refresh_failure_message("Codex", 400, "invalid_grant"));
        assert!(is_unrecoverable_error(&err));
    }

    #[test]
    fn transient_error_is_not_tagged() {
        let err = anyhow::anyhow!(
            "{}",
            refresh_failure_message("Codex", 503, "<html>unavailable</html>")
        );
        assert!(!is_unrecoverable_error(&err));
    }
}
