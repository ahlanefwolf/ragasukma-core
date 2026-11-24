pub const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const CLAUDE_REDIRECT_URI: &str = "http://localhost:54321/callback";
pub const CLAUDE_AUTH_URL: &str = "https://claude.ai/oauth/authorize";
pub const CLAUDE_TOKEN_URL: &str = "https://api.anthropic.com/v1/oauth/token";
pub const CLAUDE_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
pub const CLAUDE_MODELS_URL: &str = "https://api.anthropic.com/v1/models";
pub const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const CLAUDE_PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";

pub const CLAUDE_CLI_VERSION: &str = "2.1.322";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const ANTHROPIC_BETA: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,context-management-2025-06-27,prompt-caching-scope-2026-01-05,advanced-tool-use-2025-11-20,effort-2025-11-24,structured-outputs-2025-12-15,fast-mode-2026-02-01,redact-thinking-2026-02-12,token-efficient-tools-2026-03-28";

pub const CLAUDE_SCOPES: &[&str] = &["org:create_api_key", "user:profile", "user:inference"];

// Anthropic SDK (Stainless) fingerprint headers. 9router sends these verbatim
// from its `claude` provider registry so the request looks like it came from the
// official JS SDK rather than a hand-rolled client. Keep the values identical to
// the registry snapshot (open-sse/providers/registry/claude.js) for 1:1 parity.
pub const STAINLESS_HEADERS: &[(&str, &str)] = &[
    ("X-Stainless-Helper-Method", "stream"),
    ("X-Stainless-Retry-Count", "0"),
    ("X-Stainless-Runtime-Version", "v24.14.0"),
    ("X-Stainless-Package-Version", "0.80.0"),
    ("X-Stainless-Runtime", "node"),
    ("X-Stainless-Lang", "js"),
    ("X-Stainless-Arch", "arm64"),
    ("X-Stainless-Os", "MacOS"),
    ("X-Stainless-Timeout", "600"),
];

/// OAuth access tokens issued to Claude Code carry this prefix. Used to gate the
/// session-id header sync so plain API keys are never treated as OAuth tokens.
pub const CLAUDE_OAUTH_TOKEN_PREFIX: &str = "sk-ant-oat";

pub const CLAUDE_DEFAULT_SYSTEM_PROMPT: &str = "You are an AI assistant in Ragasukma.";

/// 9router parity: `claude.oauth.refreshLeadMs` (4 h). Anthropic OAuth tokens
/// live ~8 h, so refresh proactively within this window before expiry.
pub const CLAUDE_REFRESH_LEAD_SECS: u64 = 14_400;

/// Safety net: refresh proactively after this age even if not yet expired.
pub const CLAUDE_MAX_REFRESH_AGE_SECS: u64 = 691_200;
