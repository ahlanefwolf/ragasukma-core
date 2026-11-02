use super::constants::*;
use super::types::{
    AntigravityAccount, TokenResponse, current_timestamp_secs, generate_pseudo_uuid,
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct OAuthTokenResponse {
    pub access_token: String,
    pub expires_in: u64,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UserInfo {
    #[serde(default)]
    pub email: Option<String>,
}

pub fn build_auth_url(redirect_uri: &str, state: &str) -> String {
    let scopes = ANTIGRAVITY_SCOPES.join(" ");
    let encoded_scopes = form_urlencoded::byte_serialize(scopes.as_bytes()).collect::<String>();
    let encoded_redirect =
        form_urlencoded::byte_serialize(redirect_uri.as_bytes()).collect::<String>();
    let encoded_state = form_urlencoded::byte_serialize(state.as_bytes()).collect::<String>();

    format!(
        "{}?client_id={}&response_type=code&redirect_uri={}&scope={}&state={}&access_type=offline&prompt=consent",
        ANTIGRAVITY_AUTH_URL,
        ANTIGRAVITY_CLIENT_ID,
        encoded_redirect,
        encoded_scopes,
        encoded_state
    )
}

pub fn exchange_code(
    client: &reqwest::blocking::Client,
    code: &str,
    redirect_uri: &str,
) -> Result<OAuthTokenResponse> {
    let params = [
        ("grant_type", "authorization_code"),
        ("client_id", ANTIGRAVITY_CLIENT_ID),
        ("client_secret", ANTIGRAVITY_CLIENT_SECRET),
        ("code", code),
        ("redirect_uri", redirect_uri),
    ];

    let res = client
        .post(ANTIGRAVITY_TOKEN_URL)
        .form(&params)
        .send()
        .context("Failed to contact Google OAuth token endpoint for code exchange")?;

    if !res.status().is_success() {
        let err = res.text().unwrap_or_default();
        bail!("OAuth authorization code exchange failed: {err}");
    }

    let token_data: OAuthTokenResponse =
        res.json().context("Failed to parse OAuth token response")?;
    Ok(token_data)
}

pub fn get_user_info(client: &reqwest::blocking::Client, access_token: &str) -> Result<UserInfo> {
    let res = client
        .get(format!("{}?alt=json", ANTIGRAVITY_USER_INFO_URL))
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .context("Failed to fetch Google user info")?;

    if !res.status().is_success() {
        let err = res.text().unwrap_or_default();
        bail!("User info fetch failed: {err}");
    }

    let user_info: UserInfo = res.json().context("Failed to parse user info")?;
    Ok(user_info)
}

pub fn complete_oauth_flow(
    client: &reqwest::blocking::Client,
    code: &str,
    redirect_uri: &str,
) -> Result<AntigravityAccount> {
    let tokens = exchange_code(client, code, redirect_uri)?;
    let refresh_token = match tokens.refresh_token {
        Some(tok) if !tok.trim().is_empty() => tok.trim().to_string(),
        _ => bail!(
            "Google OAuth did not return a refresh_token. Please revoke app access or ensure prompt=consent was used."
        ),
    };

    let user_info = get_user_info(client, &tokens.access_token).ok();
    let email = user_info.and_then(|u| u.email);

    let now = current_timestamp_secs();
    let mut account = AntigravityAccount {
        id: generate_pseudo_uuid("ag-acc"),
        email,
        refresh_token,
        access_token: Some(tokens.access_token),
        expires_at: now + tokens.expires_in.saturating_sub(300),
        project_id: None,
        tier_id: None,
        rate_limited_until: 0,
        model_rate_limits: std::collections::HashMap::new(),
        consecutive_errors: 0,
        is_active: true,
    };

    let _ = ensure_project_id(client, &mut account);
    Ok(account)
}

pub fn refresh_account_token(
    client: &reqwest::blocking::Client,
    account: &mut AntigravityAccount,
) -> Result<String> {
    let params = [
        ("client_id", ANTIGRAVITY_CLIENT_ID),
        ("client_secret", ANTIGRAVITY_CLIENT_SECRET),
        ("grant_type", "refresh_token"),
        ("refresh_token", &account.refresh_token),
    ];

    let res = client
        .post(ANTIGRAVITY_TOKEN_URL)
        .form(&params)
        .send()
        .context("Failed to contact Google OAuth token endpoint")?;

    if !res.status().is_success() {
        let err = res.text().unwrap_or_default();
        bail!("Google OAuth token refresh failed: {err}");
    }

    let token_data: TokenResponse = res.json().context("Failed to parse token response")?;
    let now = current_timestamp_secs();
    account.access_token = Some(token_data.access_token.clone());
    account.expires_at = now + token_data.expires_in.saturating_sub(300);
    if let Some(new_refresh) = token_data.refresh_token
        && !new_refresh.is_empty() {
            account.refresh_token = new_refresh;
        }
    account.consecutive_errors = 0;
    Ok(token_data.access_token)
}

pub fn ensure_project_id(
    client: &reqwest::blocking::Client,
    account: &mut AntigravityAccount,
) -> Result<String> {
    if let Some(ref pid) = account.project_id
        && !pid.trim().is_empty() {
            return Ok(pid.clone());
        }

    let access_token = match account.access_token.as_deref() {
        Some(tok) => tok.to_string(),
        None => refresh_account_token(client, account)?,
    };

    let metadata = serde_json::json!({
        "ideType": 9,
        "platform": 5,
        "pluginType": 2
    });

    let body = serde_json::json!({
        "metadata": metadata
    });

    let res = client
        .post(ANTIGRAVITY_LOAD_CODE_ASSIST_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", ANTIGRAVITY_USER_AGENT)
        .header("x-request-source", "local")
        .json(&body)
        .send();

    if let Ok(response) = res
        && response.status().is_success()
            && let Ok(val) = response.json::<serde_json::Value>() {
                let project_id = val
                    .pointer("/cloudaicompanionProject/id")
                    .or_else(|| val.pointer("/cloudaicompanionProject"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "cloudaicompanion-default".to_string());

                let mut default_tier = "legacy-tier".to_string();
                if let Some(tiers) = val.pointer("/allowedTiers").and_then(|v| v.as_array()) {
                    for t in tiers {
                        if t.get("isDefault")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                            && let Some(tid) = t.get("id").and_then(|v| v.as_str()) {
                                default_tier = tid.to_string();
                                break;
                            }
                    }
                }

                account.project_id = Some(project_id.clone());
                account.tier_id = Some(default_tier.clone());

                // Fire onboarding call
                let _ = client
                    .post(ANTIGRAVITY_ONBOARD_USER_URL)
                    .header("Authorization", format!("Bearer {access_token}"))
                    .header("Content-Type", "application/json")
                    .header("User-Agent", ANTIGRAVITY_USER_AGENT)
                    .header("x-request-source", "local")
                    .json(&serde_json::json!({
                        "tierId": default_tier,
                        "metadata": metadata
                    }))
                    .send();

                return Ok(project_id);
            }

    let fallback_pid = format!("project-{}", &generate_pseudo_uuid("p")[..16]);
    account.project_id = Some(fallback_pid.clone());
    Ok(fallback_pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_auth_url_structure() {
        let redirect_uri = "http://localhost:42128/api/llm/antigravity/oauth/callback";
        let state = "test_state_123";
        let url = build_auth_url(redirect_uri, state);

        assert!(url.starts_with(ANTIGRAVITY_AUTH_URL));
        assert!(url.contains(&format!("client_id={}", ANTIGRAVITY_CLIENT_ID)));
        assert!(url.contains("state=test_state_123"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
    }
}
