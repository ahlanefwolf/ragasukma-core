pub const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const CODEX_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
pub const CODEX_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
pub const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
pub const CODEX_CLI_VERSION: &str = "0.167.1";
pub const CODEX_ORIGINATOR: &str = "codex_cli_rs";

pub const CODEX_SCOPES: &[&str] = &["openid", "profile", "email", "offline_access"];

pub const CODEX_MODELS_URL: &str =
    "https://chatgpt.com/backend-api/codex/models?client_version=0.167.1";

pub const CODEX_DEFAULT_INSTRUCTIONS: &str = "You are an AI assistant in Ragasukma.";
