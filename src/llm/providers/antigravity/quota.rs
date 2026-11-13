use super::constants::*;
use super::oauth::{ensure_project_id, refresh_account_token};
use super::types::{
    AntigravityAccount, AntigravityQuotaResponse, ModelQuotaInfo, current_timestamp_secs,
};
use anyhow::Result;

pub fn fetch_account_quota(
    client: &reqwest::blocking::Client,
    account: &mut AntigravityAccount,
) -> Result<AntigravityQuotaResponse> {
    let now = current_timestamp_secs();
    if account.access_token.is_none() || account.expires_at <= now {
        let _ = refresh_account_token(client, account)?;
    }

    let _ = ensure_project_id(client, account);

    let access_token = account.access_token.clone().unwrap_or_default();
    let project_id = account.project_id.clone();
    let email = account.email.clone();
    let id = account.id.clone();

    let body = if let Some(ref pid) = project_id {
        serde_json::json!({ "project": pid })
    } else {
        serde_json::json!({})
    };

    // 1. Fetch available models with quota info
    let mut models_quota = Vec::new();
    let mut fetch_err: Option<String> = None;

    let models_resp = client
        .post(ANTIGRAVITY_FETCH_MODELS_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", ANTIGRAVITY_USER_AGENT)
        .header("X-Client-Name", "antigravity")
        .header("X-Client-Version", ANTIGRAVITY_IDE_VERSION)
        .json(&body)
        .send();

    match models_resp {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                if let Ok(val) = resp.json::<serde_json::Value>()
                    && let Some(models_obj) = val.get("models").and_then(|v| v.as_object())
                {
                    for (model_key, model_val) in models_obj {
                        let is_internal = model_val
                            .get("isInternal")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        if is_internal {
                            continue;
                        }

                        if let Some(q_info) = model_val.get("quotaInfo") {
                            let frac = q_info
                                .get("remainingFraction")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0);
                            let pct = (frac * 1000.0).round() / 10.0;
                            let reset_time = q_info
                                .get("resetTime")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let display_name = model_val
                                .get("displayName")
                                .and_then(|v| v.as_str())
                                .unwrap_or(model_key)
                                .to_string();

                            models_quota.push(ModelQuotaInfo {
                                model_id: model_key.clone(),
                                display_name,
                                remaining_fraction: frac,
                                remaining_percentage: pct,
                                reset_time,
                            });
                        }
                    }
                }
            } else {
                let err_txt = resp.text().unwrap_or_default();
                fetch_err = Some(format!("HTTP {status}: {err_txt}"));
            }
        }
        Err(e) => {
            fetch_err = Some(format!("Request failed: {e}"));
        }
    }

    // Sort models_quota alphabetically by model_id
    models_quota.sort_by(|a, b| a.model_id.cmp(&b.model_id));

    // 2. Fetch weekly quota summary
    let mut weekly_quota = Vec::new();
    let weekly_resp = client
        .post(ANTIGRAVITY_QUOTA_SUMMARY_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", ANTIGRAVITY_USER_AGENT)
        .header("X-Client-Name", "antigravity")
        .header("X-Client-Version", "2.11.0")
        .json(&body)
        .send();

    if let Ok(w_res) = weekly_resp
        && w_res.status().is_success()
        && let Ok(w_val) = w_res.json::<serde_json::Value>()
    {
        let groups = w_val
            .get("groups")
            .or_else(|| w_val.pointer("/quotaSummary/groups"))
            .and_then(|v| v.as_array());

        if let Some(groups_arr) = groups {
            for g in groups_arr {
                let g_name = g.get("displayName").and_then(|v| v.as_str()).unwrap_or("");
                let buckets = g.get("buckets").and_then(|v| v.as_array());
                if let Some(b_arr) = buckets {
                    for b in b_arr {
                        let b_id = b.get("bucketId").and_then(|v| v.as_str()).unwrap_or("");
                        let b_name = b.get("displayName").and_then(|v| v.as_str()).unwrap_or("");
                        let b_text = format!("{b_id} {b_name}").to_lowercase();
                        if !b_text.contains("weekly") {
                            continue;
                        }
                        if b.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false) {
                            continue;
                        }
                        if let Some(frac) = b.get("remainingFraction").and_then(|v| v.as_f64()) {
                            let d_name = if g_name.to_lowercase().contains("gemini") {
                                "Gemini (Weekly)".to_string()
                            } else if g_name.to_lowercase().contains("claude")
                                || g_name.to_lowercase().contains("gpt")
                            {
                                "Claude & GPT (Weekly)".to_string()
                            } else {
                                format!("{g_name} (Weekly)")
                            };
                            let reset_time = b
                                .get("resetTime")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let pct = (frac * 1000.0).round() / 10.0;
                            weekly_quota.push(ModelQuotaInfo {
                                model_id: b_id.to_string(),
                                display_name: d_name,
                                remaining_fraction: frac,
                                remaining_percentage: pct,
                                reset_time,
                            });
                        }
                    }
                }
            }
        }
    }

    let plan = None;

    let status = if fetch_err.is_some() && models_quota.is_empty() && weekly_quota.is_empty() {
        "error".to_string()
    } else {
        "success".to_string()
    };

    Ok(AntigravityQuotaResponse {
        status,
        account_id: id,
        email,
        project_id,
        plan,
        models: models_quota,
        weekly: weekly_quota,
        error: fetch_err,
    })
}
