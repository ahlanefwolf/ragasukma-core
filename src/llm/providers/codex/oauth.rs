use super::constants::*;
use super::types::{CodexAccount, CodexTokenResponse, current_timestamp_secs, generate_pseudo_uuid};
use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

/// Temporary store for OAuth PKCE code_verifiers mapped by `state` (TTL: 10 minutes)
static OAUTH_STATE_STORE: LazyLock<Mutex<HashMap<String, (String, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn generate_pkce() -> (String, String) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let random_seed = format!("codex_pkce_{}_{}", now, std::process::id());
    let mut hasher = Sha256::new();
    hasher.update(random_seed.as_bytes());
    let hash = hasher.finalize();
    let code_verifier = URL_SAFE_NO_PAD.encode(hash);

    let mut verifier_hasher = Sha256::new();
    verifier_hasher.update(code_verifier.as_bytes());
    let challenge_hash = verifier_hasher.finalize();
    let code_challenge = URL_SAFE_NO_PAD.encode(challenge_hash);

    (code_verifier, code_challenge)
}

pub fn store_oauth_state(state: &str, code_verifier: &str) {
    if let Ok(mut store) = OAUTH_STATE_STORE.lock() {
        let now = Instant::now();
        store.retain(|_, (_, exp)| *exp > now);
        let expires_at = now + std::time::Duration::from_secs(600);
        store.insert(state.to_string(), (code_verifier.to_string(), expires_at));
    }
}

pub fn take_oauth_state(state: &str) -> Option<String> {
    if let Ok(mut store) = OAUTH_STATE_STORE.lock() {
        store.remove(state).map(|(v, _)| v)
    } else {
        None
    }
}

pub fn build_auth_url(redirect_uri: &str, state: &str, code_challenge: &str) -> String {
    let scopes = CODEX_SCOPES.join("%20");
    let encoded_redirect =
        form_urlencoded::byte_serialize(redirect_uri.as_bytes()).collect::<String>();
    let encoded_state = form_urlencoded::byte_serialize(state.as_bytes()).collect::<String>();
    let encoded_challenge =
        form_urlencoded::byte_serialize(code_challenge.as_bytes()).collect::<String>();

    format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&id_token_add_organizations=true&codex_cli_simplified_flow=true&originator={}&state={}",
        CODEX_AUTH_URL,
        CODEX_CLIENT_ID,
        encoded_redirect,
        scopes,
        encoded_challenge,
        CODEX_ORIGINATOR,
        encoded_state
    )
}

pub fn exchange_code(
    client: &reqwest::blocking::Client,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<CodexTokenResponse> {
    let params = [
        ("grant_type", "authorization_code"),
        ("client_id", CODEX_CLIENT_ID),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", code_verifier),
    ];

    let res = client
        .post(CODEX_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .form(&params)
        .send()
        .context("Failed to contact OpenAI OAuth token endpoint for code exchange")?;

    if !res.status().is_success() {
        let err = res.text().unwrap_or_default();
        bail!("OpenAI Codex authorization code exchange failed: {err}");
    }

    let token_data: CodexTokenResponse =
        res.json().context("Failed to parse Codex token response")?;
    Ok(token_data)
}

pub fn extract_jwt_claims(jwt: &str) -> Option<serde_json::Value> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload_b64 = parts[1];
    let decoded = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    serde_json::from_slice(&decoded).ok()
}

pub fn extract_codex_account_info(
    id_token: Option<&str>,
    access_token: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    let mut email = None;
    let mut chatgpt_account_id = None;
    let mut plan_type = None;

    if let Some(id_tok) = id_token
        && let Some(claims) = extract_jwt_claims(id_tok) {
            email = claims.get("email").and_then(|v| v.as_str()).map(String::from);

            // Check top-level or nested claims
            if let Some(act_id) = claims.get("chatgpt_account_id").and_then(|v| v.as_str()) {
                chatgpt_account_id = Some(act_id.to_string());
            } else if let Some(sub_claim) = claims.get("https://api.openai.com/auth") {
                if let Some(act_id) = sub_claim.get("chatgpt_account_id").and_then(|v| v.as_str()) {
                    chatgpt_account_id = Some(act_id.to_string());
                }
                if let Some(plan) = sub_claim.get("chatgpt_plan_type").and_then(|v| v.as_str()) {
                    plan_type = Some(plan.to_string());
                }
            }

            if plan_type.is_none()
                && let Some(plan) = claims.get("chatgpt_plan_type").and_then(|v| v.as_str()) {
                    plan_type = Some(plan.to_string());
                }
        }

    if email.is_none()
        && let Some(claims) = extract_jwt_claims(access_token) {
            email = claims.get("email").and_then(|v| v.as_str()).map(String::from);
            if chatgpt_account_id.is_none()
                && let Some(act_id) = claims.get("chatgpt_account_id").and_then(|v| v.as_str()) {
                    chatgpt_account_id = Some(act_id.to_string());
                }
        }

    (email, chatgpt_account_id, plan_type)
}

pub fn complete_oauth_flow(
    client: &reqwest::blocking::Client,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<CodexAccount> {
    let tokens = exchange_code(client, code, redirect_uri, code_verifier)?;
    let refresh_token = match tokens.refresh_token {
        Some(tok) if !tok.trim().is_empty() => tok.trim().to_string(),
        _ => bail!("OpenAI did not return a refresh_token. Please re-authorize."),
    };

    let (email, chatgpt_account_id, plan_type) =
        extract_codex_account_info(tokens.id_token.as_deref(), &tokens.access_token);

    let now = current_timestamp_secs();
    let account = CodexAccount {
        id: generate_pseudo_uuid("cx-acc"),
        email,
        refresh_token,
        access_token: Some(tokens.access_token),
        id_token: tokens.id_token,
        expires_at: now + tokens.expires_in.saturating_sub(120),
        chatgpt_account_id,
        chatgpt_plan_type: plan_type,
        rate_limited_until: 0,
        model_rate_limits: HashMap::new(),
        consecutive_errors: 0,
        is_active: true,
    };

    Ok(account)
}

pub fn refresh_account_token(
    client: &reqwest::blocking::Client,
    account: &mut CodexAccount,
) -> Result<String> {
    let params = [
        ("grant_type", "refresh_token"),
        ("client_id", CODEX_CLIENT_ID),
        ("refresh_token", account.refresh_token.as_str()),
        ("scope", "openid profile email offline_access"),
    ];

    let res = client
        .post(CODEX_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .form(&params)
        .send()
        .context("Failed to contact OpenAI token endpoint for refresh")?;

    if !res.status().is_success() {
        let err = res.text().unwrap_or_default();
        bail!("Failed to refresh OpenAI Codex token: {err}");
    }

    let token_data: CodexTokenResponse =
        res.json().context("Failed to parse refresh token response")?;

    let now = current_timestamp_secs();
    account.access_token = Some(token_data.access_token.clone());
    account.expires_at = now + token_data.expires_in.saturating_sub(120);

    if let Some(new_ref) = token_data.refresh_token
        && !new_ref.trim().is_empty() {
            account.refresh_token = new_ref;
        }
    if let Some(new_id) = token_data.id_token {
        let (email, act_id, plan) = extract_codex_account_info(Some(&new_id), &token_data.access_token);
        if email.is_some() {
            account.email = email;
        }
        if act_id.is_some() {
            account.chatgpt_account_id = act_id;
        }
        if plan.is_some() {
            account.chatgpt_plan_type = plan;
        }
        account.id_token = Some(new_id);
    }

    Ok(token_data.access_token)
}
