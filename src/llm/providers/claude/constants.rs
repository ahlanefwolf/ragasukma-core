pub const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const CLAUDE_REDIRECT_URI: &str = "http://localhost:54321/callback";
pub const CLAUDE_AUTH_URL: &str = "https://claude.ai/oauth/authorize";
pub const CLAUDE_TOKEN_URL: &str = "https://api.anthropic.com/v1/oauth/token";
pub const CLAUDE_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
pub const CLAUDE_MODELS_URL: &str = "https://api.anthropic.com/v1/models";
pub const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const CLAUDE_PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";

pub const CLAUDE_CLI_VERSION: &str = "2.1.280";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const ANTHROPIC_BETA: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,context-management-2025-06-27,prompt-caching-scope-2026-01-05,advanced-tool-use-2025-11-20,effort-2025-11-24,structured-outputs-2025-12-15,fast-mode-2026-02-01,redact-thinking-2026-02-12,token-efficient-tools-2026-03-28";

pub const CLAUDE_SCOPES: &[&str] = &["org:create_api_key", "user:profile", "user:inference"];

pub const CLAUDE_DEFAULT_SYSTEM_PROMPT: &str = "You are an AI assistant in Ragasukma.";
