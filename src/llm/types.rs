use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Structured HTTP error from LLM providers
// ---------------------------------------------------------------------------

/// A structured error that preserves the HTTP status code and optional
/// `Retry-After` header value from a provider response.  The retry logic in
/// `stream.rs` can downcast `anyhow::Error` to this type and make decisions
/// based on the numeric status code instead of parsing error message strings.
#[derive(Debug)]
pub struct LlmHttpError {
    /// The provider name or endpoint label (e.g. "Claude Messages API").
    pub provider: String,
    /// HTTP status code (e.g. 429, 503).
    pub status_code: u16,
    /// Raw response body (may be JSON or plain text).
    pub body: String,
    /// Value of the `Retry-After` header, if the server sent one.
    pub retry_after_header: Option<String>,
}

impl std::fmt::Display for LlmHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} returned HTTP {}: {}",
            self.provider, self.status_code, self.body
        )?;
        if let Some(ref ra) = self.retry_after_header {
            write!(f, ", Retry-After: {ra}")?;
        }
        Ok(())
    }
}

impl std::error::Error for LlmHttpError {}

impl LlmHttpError {
    /// Build an `LlmHttpError` from a `reqwest::blocking::Response` that has a
    /// non-success status.  Consumes the response to read the body.
    pub fn from_blocking_response(
        provider: impl Into<String>,
        resp: reqwest::blocking::Response,
    ) -> Self {
        let status_code = resp.status().as_u16();
        let retry_after_header = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let body = resp.text().unwrap_or_default();
        Self {
            provider: provider.into(),
            status_code,
            body,
            retry_after_header,
        }
    }

    /// Convenience: is this a status code we should retry?
    pub fn is_retryable(&self) -> bool {
        matches!(self.status_code, 429 | 500 | 502 | 503 | 529)
    }
}
