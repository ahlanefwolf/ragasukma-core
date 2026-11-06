use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomLlmAccount {
    pub prefix: String,
    pub name: String,
    #[serde(alias = "baseUrl")]
    pub base_url: String,
    #[serde(alias = "apiKey")]
    pub api_key: String,
    #[serde(default)]
    pub enabled: bool,
}

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
                    models.push((model_id, account.name.clone()));
                }
            }
        }
    }
    models
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

pub struct CustomProvider {
    pub client: reqwest::blocking::Client,
}

impl Default for CustomProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl CustomProvider {
    pub fn new_with_client(client: reqwest::blocking::Client) -> Self {
        Self { client }
    }

    pub fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self::new_with_client(client)
    }

    pub fn chat_stream<F>(
        &mut self,
        model: &str,
        messages: &[crate::llm::types::ChatMessage],
        temperature: f32,
        max_tokens: u32,
        mut on_delta: F,
    ) -> anyhow::Result<String>
    where
        F: FnMut(&str) -> anyhow::Result<()>,
    {
        use std::io::BufRead;

        let accounts = load_accounts();
        let (account, stripped_model) = accounts
            .into_iter()
            .filter(|a| a.enabled)
            .find_map(|a| {
                let prefix_with_slash = format!("{}/", a.prefix.trim());
                if model.starts_with(&prefix_with_slash) {
                    let stripped = model[prefix_with_slash.len()..].to_string();
                    Some((a, stripped))
                } else if model.eq_ignore_ascii_case(a.prefix.trim()) {
                    Some((a, model.to_string()))
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No enabled custom provider found matching model prefix for '{model}'"
                )
            })?;

        let endpoint = format!(
            "{}/chat/completions",
            account.base_url.trim_end_matches('/')
        );
        let payload = serde_json::json!({
            "model": stripped_model,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": max_tokens,
            "stream": true,
        });

        let mut req = self
            .client
            .post(&endpoint)
            .header("Content-Type", "application/json");

        if !account.api_key.trim().is_empty() {
            req = req.header(
                "Authorization",
                format!("Bearer {}", account.api_key.trim()),
            );
        }

        let resp = req
            .json(&payload)
            .send()
            .map_err(|e| anyhow::anyhow!("Custom provider request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Custom provider returned error {status}: {body}"
            ));
        }

        let reader = std::io::BufReader::new(resp);
        let mut full_reply = String::new();

        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with(':') {
                continue;
            }

            if let Some(data) = trimmed.strip_prefix("data:") {
                let data = data.trim();
                if data == "[DONE]" {
                    break;
                }

                if let Ok(val) = serde_json::from_str::<serde_json::Value>(data)
                    && let Some(delta) = val
                        .pointer("/choices/0/delta/content")
                        .or_else(|| val.pointer("/choices/0/message/content"))
                        .and_then(|v| v.as_str())
                {
                    if delta.is_empty() {
                        continue;
                    }
                    full_reply.push_str(delta);
                    on_delta(delta)?;
                }
            }
        }

        Ok(full_reply)
    }

    pub fn chat_stream_v1<F>(
        &mut self,
        req: &crate::llm::v1_engine::V1ChatRequest,
        mut callback: F,
    ) -> anyhow::Result<crate::llm::v1_engine::V1Response>
    where
        F: FnMut(crate::llm::v1_engine::V1StreamEvent) -> anyhow::Result<()>,
    {
        use crate::llm::v1_engine::{V1StreamEvent, V1ToolCall, V1ToolCallFunction};

        let accounts = load_accounts();
        let (account, stripped_model) = accounts
            .into_iter()
            .filter(|a| a.enabled)
            .find_map(|a| {
                let prefix_with_slash = format!("{}/", a.prefix.trim());
                if req.model.starts_with(&prefix_with_slash) {
                    let stripped = req.model[prefix_with_slash.len()..].to_string();
                    Some((a, stripped))
                } else if req.model.eq_ignore_ascii_case(a.prefix.trim()) {
                    Some((a, req.model.clone()))
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No enabled custom LLM account found matching prefix for model '{}'",
                    req.model
                )
            })?;

        let base = account.base_url.trim().trim_end_matches('/');
        let url = if base.ends_with("/chat/completions") {
            base.to_string()
        } else if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        };

        let mut body = serde_json::to_value(req)?;
        if let Some(obj) = body.as_object_mut() {
            obj.insert(
                "model".to_string(),
                serde_json::Value::String(stripped_model),
            );
            obj.insert("stream".to_string(), serde_json::Value::Bool(true));
        }

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()?;

        let mut request = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body);

        let api_key = account.api_key.trim();
        if !api_key.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let resp = request.send()?;
        let status = resp.status();
        if !status.is_success() {
            let err_text = resp.text().unwrap_or_default();
            anyhow::bail!("Custom LLM HTTP error {}: {}", status, err_text);
        }

        use std::io::BufRead;
        let reader = std::io::BufReader::new(resp);

        let mut final_content = String::new();
        let mut final_thought = String::new();
        let mut tool_calls_map: std::collections::BTreeMap<usize, V1ToolCall> =
            std::collections::BTreeMap::new();

        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();

            if trimmed.is_empty() || trimmed.starts_with(':') {
                continue;
            }

            if let Some(data) = trimmed.strip_prefix("data:") {
                let data = data.trim();
                if data == "[DONE]" {
                    break;
                }

                let chunk: serde_json::Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let choice = chunk.get("choices").and_then(|c| c.get(0));
                let delta = choice.and_then(|c| c.get("delta"));

                if let Some(delta) = delta {
                    if let Some(thought) = delta
                        .get("reasoning_content")
                        .or_else(|| delta.get("thought"))
                        .and_then(|t| t.as_str())
                        && !thought.is_empty()
                    {
                        final_thought.push_str(thought);
                        callback(V1StreamEvent::ThoughtDelta(thought.to_string()))?;
                    }

                    if let Some(content) = delta.get("content").and_then(|c| c.as_str())
                        && !content.is_empty()
                    {
                        final_content.push_str(content);
                        callback(V1StreamEvent::ContentDelta(content.to_string()))?;
                    }

                    if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc_val in tcs {
                            let idx =
                                tc_val.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                            let entry = tool_calls_map.entry(idx).or_insert_with(|| V1ToolCall {
                                id: format!("call_{}", idx),
                                call_type: "function".to_string(),
                                function: V1ToolCallFunction {
                                    name: String::new(),
                                    arguments: String::new(),
                                },
                                thought_signature: None,
                                extra_content: None,
                                provider_specific_fields: None,
                            });

                            let id = tc_val
                                .get("id")
                                .and_then(|s| s.as_str())
                                .map(|s| s.to_string());
                            let func = tc_val.get("function");
                            let name = func
                                .and_then(|f| f.get("name"))
                                .and_then(|s| s.as_str())
                                .map(|s| s.to_string());
                            let args = func
                                .and_then(|f| f.get("arguments"))
                                .and_then(|s| s.as_str())
                                .unwrap_or("");

                            if let Some(ref tc_id) = id
                                && !tc_id.trim().is_empty()
                            {
                                entry.id = tc_id.clone();
                            }
                            // DeepSeek (via 9Router) sends `"name": ""` on every delta
                            // after the first one. Only a non-blank name may overwrite,
                            // otherwise the tool call ends up unnamed and the agent
                            // fails with `Unknown tool ''`.
                            if let Some(ref tc_name) = name
                                && !tc_name.trim().is_empty()
                            {
                                entry.function.name = tc_name.clone();
                            }
                            // Arguments are streamed in fragments; the empty-string
                            // re-sends carry no information, so skip them.
                            if !args.is_empty() {
                                entry.function.arguments.push_str(args);
                            }

                            callback(V1StreamEvent::ToolCallDelta {
                                index: idx,
                                id,
                                name,
                                arguments: args.to_string(),
                            })?;
                        }
                    }
                }
            }
        }

        // Drop unnamed calls: forwarding them would surface as `Unknown tool ''`.
        let tool_calls: Vec<_> = tool_calls_map
            .into_values()
            .filter(|tc| !tc.function.name.trim().is_empty())
            .collect();
        let tool_calls_opt = if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        };

        let finish_reason = if tool_calls_opt.is_some() {
            "tool_calls".to_string()
        } else {
            "stop".to_string()
        };

        Ok(crate::llm::v1_engine::V1Response {
            content: final_content,
            tool_calls: tool_calls_opt,
            finish_reason,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn model_id_adds_prefix_without_repeating_it() {
        fn with_prefix(prefix: &str, id: &str) -> String {
            if id
                .split_once('/')
                .is_some_and(|(existing, _)| existing.eq_ignore_ascii_case(prefix))
            {
                id.to_string()
            } else {
                format!("{prefix}/{id}")
            }
        }

        assert_eq!(with_prefix("or", "cc/opus-5.5"), "or/cc/opus-5.5");
        assert_eq!(with_prefix("or", "or/model"), "or/model");
    }
}
