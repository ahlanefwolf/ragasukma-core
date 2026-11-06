use super::constants::*;
use super::types::{CodexAccount, CodexQuotaResponse, CodexQuotaWindow};
use anyhow::{Context, Result};

pub fn fetch_account_quota(
    client: &reqwest::blocking::Client,
    account: &mut CodexAccount,
) -> Result<CodexQuotaResponse> {
    let now = super::types::current_timestamp_secs();
    if account.access_token.is_none() || account.expires_at <= now {
        super::oauth::refresh_account_token(client, account)?;
    }

    let token = account.access_token.as_deref().unwrap_or_default();
    let mut req = client
        .get(CODEX_USAGE_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("originator", CODEX_ORIGINATOR)
        .header("User-Agent", format!("codex_cli_rs/{}", CODEX_CLI_VERSION))
        .header("version", CODEX_CLI_VERSION);

    if let Some(ref act_id) = account.chatgpt_account_id {
        req = req.header("ChatGPT-Account-ID", act_id.as_str());
    }

    let res = req
        .send()
        .context("Failed to send usage request to Codex WHAM endpoint")?;

    if !res.status().is_success() {
        let status = res.status();
        let err_body = res.text().unwrap_or_default();
        return Ok(CodexQuotaResponse {
            status: "error".to_string(),
            account_id: account.id.clone(),
            email: account.email.clone(),
            chatgpt_account_id: account.chatgpt_account_id.clone(),
            plan: account.chatgpt_plan_type.clone(),
            limit_reached: false,
            session_quota: None,
            weekly_quota: None,
            reset_credits: None,
            error: Some(format!("HTTP {status}: {err_body}")),
        });
    }

    let json: serde_json::Value = res.json().context("Failed to parse WHAM usage JSON")?;

    let plan = json
        .get("plan_type")
        .or_else(|| json.pointer("/summary/plan"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| account.chatgpt_plan_type.clone());

    if plan.is_some() {
        account.chatgpt_plan_type = plan.clone();
    }

    let rate_limit = json
        .get("rate_limit")
        .or_else(|| json.get("rate_limits"))
        .or_else(|| json.pointer("/rate_limits_by_limit_id/codex"))
        .unwrap_or(&json);

    let limit_reached = rate_limit
        .get("limit_reached")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let parse_window = |val: Option<&serde_json::Value>| -> Option<CodexQuotaWindow> {
        let v = val?;
        let used = v
            .get("used_percent")
            .or_else(|| v.get("percent_used"))
            .and_then(|p| p.as_f64())
            .unwrap_or(0.0)
            .clamp(0.0, 100.0);
        let reset_at = v
            .get("reset_at")
            .or_else(|| v.get("resets_at"))
            .and_then(|s| s.as_str())
            .map(String::from);
        Some(CodexQuotaWindow {
            used,
            total: 100.0,
            remaining: (100.0 - used).max(0.0),
            reset_at,
        })
    };

    let session_quota = parse_window(
        rate_limit
            .get("primary_window")
            .or_else(|| rate_limit.get("primary"))
            .or_else(|| json.get("primary_window")),
    );

    let weekly_quota = parse_window(
        rate_limit
            .get("secondary_window")
            .or_else(|| rate_limit.get("secondary"))
            .or_else(|| json.get("secondary_window")),
    );

    let reset_credits = json
        .pointer("/rate_limit_reset_credits/available_count")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);

    Ok(CodexQuotaResponse {
        status: "success".to_string(),
        account_id: account.id.clone(),
        email: account.email.clone(),
        chatgpt_account_id: account.chatgpt_account_id.clone(),
        plan,
        limit_reached,
        session_quota,
        weekly_quota,
        reset_credits,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codex_account_quota_live() {
        let accounts_json = std::fs::read_to_string("assets/codex_accounts.json")
            .unwrap_or_else(|_| "[]".to_string());
        let accounts: Vec<CodexAccount> = serde_json::from_str(&accounts_json).unwrap_or_default();
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();

        if let Some(mut acc) = accounts.into_iter().next() {
            let res = fetch_account_quota(&client, &mut acc);
            assert!(res.is_ok());
        }
    }
}
