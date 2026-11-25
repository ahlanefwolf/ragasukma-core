use std::path::PathBuf;

use super::types::*;

fn accounts_path() -> PathBuf {
    let candidate = crate::assets::find_file_in_candidates("api_llm_accounts.json");
    if candidate.exists() {
        candidate
    } else {
        crate::config::get_app_config_dir().join("api_llm_accounts.json")
    }
}

pub fn load_accounts() -> Vec<CustomLlmAccount> {
    let paths = crate::assets::find_all_file_candidates("api_llm_accounts.json");
    for path in paths {
        if let Ok(bytes) = std::fs::read(path)
            && let Ok(accounts) = serde_json::from_slice(&bytes)
        {
            return accounts;
        }
    }
    std::fs::read(accounts_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save_accounts(accounts: &[CustomLlmAccount]) -> anyhow::Result<()> {
    let existing = load_accounts();
    let merged: Vec<CustomLlmAccount> = accounts
        .iter()
        .map(|incoming| {
            let mut acc = incoming.clone();
            let key = acc.api_key.trim();
            if (key.is_empty() || key.chars().all(|c| c == '*'))
                && let Some(old) = existing
                    .iter()
                    .find(|e| e.prefix.eq_ignore_ascii_case(&acc.prefix))
            {
                acc.api_key = old.api_key.clone();
            }
            acc
        })
        .collect();

    let paths = crate::assets::find_all_file_candidates("api_llm_accounts.json");
    let path = paths
        .into_iter()
        .find(|path| path.exists())
        .unwrap_or_else(accounts_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(&merged)?)?;
    Ok(())
}

pub fn fetch_models() -> Vec<(String, String)> {
    let accounts = load_accounts();
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(_) => return Vec::new(),
    };
    let mut models = Vec::new();
    for account in accounts.into_iter().filter(|account| account.enabled) {
        let base = account.base_url.trim().trim_end_matches('/');
        let url = if base.ends_with("/v1") {
            format!("{base}/models")
        } else {
            format!("{base}/v1/models")
        };
        let mut request = client.get(url);
        if !account.api_key.trim().is_empty() {
            request = request.bearer_auth(account.api_key.trim());
        }
        let Ok(response) = request.send() else {
            continue;
        };
        let Ok(body) = response.error_for_status() else {
            continue;
        };
        let Ok(value) = body.json::<serde_json::Value>() else {
            continue;
        };
        if let Some(items) = value.get("data").and_then(serde_json::Value::as_array) {
            for item in items {
                if let Some(id) = item.get("id").and_then(serde_json::Value::as_str) {
                    let id = id.trim();
                    if id.is_empty() {
                        continue;
                    }
                    let prefix = account.prefix.trim();
                    let model_id = if id.split_once('/').is_some_and(|(existing_prefix, _)| {
                        existing_prefix.eq_ignore_ascii_case(prefix)
                    }) {
                        id.to_string()
                    } else {
                        format!("{prefix}/{id}")
                    };

                    let context_window = item
                        .get("context_length")
                        .or_else(|| item.get("max_model_len"))
                        .or_else(|| item.get("context_window"))
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize);

                    let native_reasoning = item
                        .get("architecture")
                        .and_then(|a| a.get("instruct_type"))
                        .and_then(|s| s.as_str())
                        .map(|s| s.contains("reasoner") || s.contains("thinking"));

                    if context_window.is_some() || native_reasoning.is_some() {
                        set_discovered_meta(
                            &model_id,
                            DiscoveredModelMeta {
                                context_window,
                                native_reasoning,
                            },
                        );
                    }

                    models.push((model_id, account.name.clone()));
                }
            }
        }
    }
    models
}

/// Queries the provider endpoint to introspect model metadata if not yet cached in RAM.
/// Designed for parallel sessions: fast timeout (3s) so parallel turns are never blocked.
pub async fn discover_model_meta(model_id: &str) -> Option<DiscoveredModelMeta> {
    // 1. Fast path: check in-memory RAM cache
    if let Some(cached) = get_discovered_meta(model_id) {
        return Some(cached);
    }

    let prefix = model_id.split('/').next().unwrap_or("");
    let raw_model = if let Some(stripped) = model_id.strip_prefix(&format!("{prefix}/")) {
        stripped
    } else {
        model_id
    };

    let accounts = load_accounts();
    let account = accounts
        .into_iter()
        .find(|a| a.enabled && a.prefix.eq_ignore_ascii_case(prefix))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok()?;

    let base = account.base_url.trim_end_matches('/');
    let url = if base.ends_with("/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    };

    let mut request = client.get(&url);
    if !account.api_key.trim().is_empty() {
        request = request.bearer_auth(account.api_key.trim());
    }

    let response = request.send().await.ok()?;
    let value = response.json::<serde_json::Value>().await.ok()?;

    if let Some(items) = value.get("data").and_then(serde_json::Value::as_array) {
        for item in items {
            let id = item.get("id").and_then(serde_json::Value::as_str)?;
            if id.eq_ignore_ascii_case(raw_model) || id.eq_ignore_ascii_case(model_id) {
                let context_window = item
                    .get("context_length")
                    .or_else(|| item.get("max_model_len"))
                    .or_else(|| item.get("context_window"))
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);

                let native_reasoning = item
                    .get("architecture")
                    .and_then(|a| a.get("instruct_type"))
                    .and_then(|s| s.as_str())
                    .map(|s| s.contains("reasoner") || s.contains("thinking"));

                let meta = DiscoveredModelMeta {
                    context_window,
                    native_reasoning,
                };
                set_discovered_meta(model_id, meta.clone());
                return Some(meta);
            }
        }
    }

    None
}

pub fn validate_account(account: &CustomLlmAccount) -> anyhow::Result<()> {
    let prefix = account.prefix.trim();
    if prefix.is_empty()
        || !prefix
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        anyhow::bail!("Prefix must contain only letters, numbers, '-' or '_'");
    }
    if ["ag", "cx", "cc"]
        .iter()
        .any(|reserved| prefix.eq_ignore_ascii_case(reserved))
    {
        anyhow::bail!("Prefix '{prefix}' is reserved");
    }
    if account.name.trim().is_empty() || account.base_url.trim().is_empty() {
        anyhow::bail!("Name and base URL are required");
    }
    Ok(())
}

