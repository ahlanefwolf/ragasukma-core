use super::constants::*;
use super::types::{ClaudeAccount, ClaudeQuotaResponse, ClaudeQuotaWindow};
use anyhow::{Context, Result};

pub fn fetch_account_quota(
    client: &reqwest::blocking::Client,
    account: &mut ClaudeAccount,
) -> Result<ClaudeQuotaResponse> {
    let now = super::types::current_timestamp_secs();
    if account.access_token.is_none() || account.expires_at <= now {
        super::oauth::refresh_account_token(client, account)?;
    }

    let token = account.access_token.as_deref().unwrap_or_default();
    let url = format!("{CLAUDE_USAGE_URL}?cedar_ember=1");
    let req = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header(
            "User-Agent",
            format!("claude-cli/{CLAUDE_CLI_VERSION} (external, cli)"),
        );

    let res = req
        .send()
        .context("Failed to send usage request to Claude OAuth usage endpoint")?;

    if !res.status().is_success() {
        let status = res.status();
        let err_body = res.text().unwrap_or_default();
        return Ok(ClaudeQuotaResponse {
            status: "error".to_string(),
            account_id: account.id.clone(),
            email: account.email.clone(),
            org_id: account.org_id.clone(),
            plan: account.plan_type.clone(),
            limit_reached: false,
            session_quota: None,
            weekly_quota: None,
            reset_credits: None,
            error: Some(format!("HTTP {status}: {err_body}")),
        });
    }

    let json: serde_json::Value = res.json().context("Failed to parse Claude usage JSON")?;

    let parse_window = |val: Option<&serde_json::Value>| -> Option<ClaudeQuotaWindow> {
        let v = val?;
        let used = v
            .get("utilization")
            .or_else(|| v.get("used_percent"))
            .or_else(|| v.get("percent_used"))
            .and_then(|p| p.as_f64())
            .unwrap_or(0.0)
            .clamp(0.0, 100.0);
        let reset_at = v
            .get("resets_at")
            .or_else(|| v.get("reset_at"))
            .and_then(|s| s.as_str())
            .map(String::from);
        Some(ClaudeQuotaWindow {
            used,
            total: 100.0,
            remaining: (100.0 - used).max(0.0),
            reset_at,
        })
    };

    let session_quota = parse_window(json.get("five_hour"));
    let weekly_quota = parse_window(json.get("seven_day"));

    let limit_reached = session_quota
        .as_ref()
        .map(|s| s.remaining <= 0.0)
        .unwrap_or(false);

    let reset_credits = json
        .pointer("/cedar_ember/grants")
        .and_then(|v| v.as_array())
        .map(|grants| {
            grants
                .iter()
                .filter_map(|g| g.get("resets_left").and_then(|r| r.as_u64()))
                .sum::<u64>() as u32
        });

    Ok(ClaudeQuotaResponse {
        status: "success".to_string(),
        account_id: account.id.clone(),
        email: account.email.clone(),
        org_id: account.org_id.clone(),
        plan: None,
        limit_reached,
        session_quota,
        weekly_quota,
        reset_credits,
        error: None,
    })
}
