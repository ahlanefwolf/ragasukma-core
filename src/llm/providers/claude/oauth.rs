use super::constants::*;
use super::types::{
    ClaudeAccount, ClaudeTokenResponse, current_timestamp_secs, generate_pseudo_uuid,
};
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
    let random_seed = format!("claude_pkce_{}_{}", now, std::process::id());
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
    let scopes = CLAUDE_SCOPES.join(" ");
    let encoded_redirect =
        form_urlencoded::byte_serialize(redirect_uri.as_bytes()).collect::<String>();
    let encoded_state = form_urlencoded::byte_serialize(state.as_bytes()).collect::<String>();
    let encoded_challenge =
        form_urlencoded::byte_serialize(code_challenge.as_bytes()).collect::<String>();
    let encoded_scopes = form_urlencoded::byte_serialize(scopes.as_bytes()).collect::<String>();

    format!(
        "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        CLAUDE_AUTH_URL,
        CLAUDE_CLIENT_ID,
        encoded_redirect,
        encoded_scopes,
        encoded_challenge,
        encoded_state
    )
}

pub fn exchange_code(
    client: &reqwest::blocking::Client,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
    state: &str,
) -> Result<ClaudeTokenResponse> {
    // Parse code if it contains state after '#' (similar to 9router / Claude OAuth callback)
    let mut auth_code = code.to_string();
    let mut code_state = state.to_string();
    if let Some((c, s)) = code.split_once('#') {
        auth_code = c.to_string();
        if !s.is_empty() {
            code_state = s.to_string();
        }
    }

    let payload = serde_json::json!({
        "grant_type": "authorization_code",
        "client_id": CLAUDE_CLIENT_ID,
        "code": auth_code,
        "redirect_uri": redirect_uri,
        "code_verifier": code_verifier,
        "state": code_state,
    });

    let res = client
        .post(CLAUDE_TOKEN_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .context("Failed to contact Claude OAuth token endpoint for code exchange")?;

    if !res.status().is_success() {
        let err = res.text().unwrap_or_default();
        bail!("Claude authorization code exchange failed: {err}");
    }

    let token_data: ClaudeTokenResponse = res
        .json()
        .context("Failed to parse Claude token response")?;
    Ok(token_data)
}

pub fn fetch_claude_profile(
    client: &reqwest::blocking::Client,
    access_token: &str,
) -> Result<(Option<String>, Option<String>)> {
    let res = client
        .get(CLAUDE_PROFILE_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header(
            "User-Agent",
            format!("claude-cli/{CLAUDE_CLI_VERSION} (external, cli)"),
        )
        .send();

    if let Ok(resp) = res
        && resp.status().is_success()
        && let Ok(json) = resp.json::<serde_json::Value>()
    {
        let email = json
            .get("account")
            .and_then(|a| a.get("email_address").or_else(|| a.get("email")))
            .or_else(|| json.get("email"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let org_id = json
            .get("organization")
            .and_then(|o| o.get("uuid").or_else(|| o.get("id")))
            .or_else(|| json.get("organization_id"))
            .and_then(|v| v.as_str())
            .map(String::from);

        return Ok((email, org_id));
    }

    Ok((None, None))
}

pub fn complete_oauth_flow(
    client: &reqwest::blocking::Client,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
    state: &str,
) -> Result<ClaudeAccount> {
    let tokens = exchange_code(client, code, redirect_uri, code_verifier, state)?;
    let refresh_token = match tokens.refresh_token {
        Some(tok) if !tok.trim().is_empty() => tok.trim().to_string(),
        _ => bail!("Anthropic did not return a refresh_token. Please re-authorize."),
    };

    let (email, org_id) =
        fetch_claude_profile(client, &tokens.access_token).unwrap_or((None, None));

    let now = current_timestamp_secs();
    let account = ClaudeAccount {
        id: generate_pseudo_uuid("cc-acc"),
        email,
        refresh_token,
        access_token: Some(tokens.access_token),
        expires_at: now + tokens.expires_in.saturating_sub(120),
        org_id,
        plan_type: Some("Claude Code".to_string()),
        rate_limited_until: 0,
        model_rate_limits: HashMap::new(),
        consecutive_errors: 0,
        is_active: true,
    };

    Ok(account)
}

pub fn refresh_account_token(
    client: &reqwest::blocking::Client,
    account: &mut ClaudeAccount,
) -> Result<String> {
    let payload = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": CLAUDE_CLIENT_ID,
        "refresh_token": account.refresh_token.as_str(),
    });

    let res = client
        .post(CLAUDE_TOKEN_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .context("Failed to contact Claude token endpoint for refresh")?;

    if !res.status().is_success() {
        let err = res.text().unwrap_or_default();
        bail!("Failed to refresh Claude token: {err}");
    }

    let token_data: ClaudeTokenResponse = res
        .json()
        .context("Failed to parse Claude refresh token response")?;

    let now = current_timestamp_secs();
    account.access_token = Some(token_data.access_token.clone());
    account.expires_at = now + token_data.expires_in.saturating_sub(120);

    if let Some(ref new_refresh) = token_data.refresh_token
        && !new_refresh.trim().is_empty()
    {
        account.refresh_token = new_refresh.trim().to_string();
    }

    if (account.email.is_none() || account.org_id.is_none())
        && let Ok((em, org)) = fetch_claude_profile(client, &token_data.access_token)
    {
        if account.email.is_none() && em.is_some() {
            account.email = em;
        }
        if account.org_id.is_none() && org.is_some() {
            account.org_id = org;
        }
    }

    Ok(token_data.access_token)
}
