use super::constants::*;
use super::oauth::{complete_oauth_flow, ensure_project_id, refresh_account_token};
use super::types::{
    AntigravityAccount, AntigravityAccountInfo, AntigravityQuotaResponse, current_timestamp_secs,
    generate_pseudo_uuid,
};
use crate::llm::types::ChatMessage;
use anyhow::{Context, Result, bail};
use std::fs;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static ACCOUNTS_FILE_LOCK: Mutex<()> = Mutex::new(());

pub struct AntigravityProvider {
    pub accounts: Vec<AntigravityAccount>,
    pub current_index: usize,
    pub client: reqwest::blocking::Client,
    pub config_file: PathBuf,
}

impl AntigravityProvider {
    pub fn get_accounts_file_path() -> PathBuf {
        let candidate = crate::assets::find_file_in_candidates("antigravity_accounts.json");
        if candidate.exists() {
            return candidate;
        }
        crate::config::get_app_config_dir().join("antigravity_accounts.json")
    }

    pub fn new_with_client(client: reqwest::blocking::Client) -> Self {
        let config_file = Self::get_accounts_file_path();

        let mut provider = Self {
            accounts: Vec::new(),
            current_index: 0,
            client,
            config_file,
        };

        let _ = provider.load_accounts();
        crate::term_log!(
            "[Antigravity] Initialized provider with {} account(s) loaded.",
            provider.accounts.len()
        );
        provider
    }

    pub fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self::new_with_client(client)
    }

    pub fn load_accounts(&mut self) -> Result<()> {
        let _guard = ACCOUNTS_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        if !self.config_file.exists() {
            self.config_file = Self::get_accounts_file_path();
            if !self.config_file.exists() {
                if let Some(parent) = self.config_file.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::write(&self.config_file, "[]\n");
                return Ok(());
            }
        }

        let content = fs::read_to_string(&self.config_file)?;
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Ok(());
        }

        if let Ok(accs) = serde_json::from_str::<Vec<AntigravityAccount>>(trimmed) {
            self.accounts = accs;
            return Ok(());
        }

        if let Ok(token_strings) = serde_json::from_str::<Vec<String>>(trimmed) {
            let mut list = Vec::new();
            for (idx, tok) in token_strings.into_iter().enumerate() {
                if !tok.trim().is_empty() {
                    list.push(AntigravityAccount {
                        id: format!("ag-acc-{}", idx + 1),
                        email: None,
                        refresh_token: tok.trim().to_string(),
                        access_token: None,
                        expires_at: 0,
                        project_id: None,
                        tier_id: None,
                        rate_limited_until: 0,
                        model_rate_limits: std::collections::HashMap::new(),
                        consecutive_errors: 0,
                        is_active: true,
                    });
                }
            }
            self.accounts = list;
            return Ok(());
        }

        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            let items = val
                .get("accounts")
                .or_else(|| val.get("tokens"))
                .or_else(|| val.get("connections"))
                .and_then(|v| v.as_array());

            if let Some(arr) = items {
                let mut list = Vec::new();
                for (idx, item) in arr.iter().enumerate() {
                    if let Ok(acc) = serde_json::from_value::<AntigravityAccount>(item.clone()) {
                        list.push(acc);
                    } else if let Some(tok) = item
                        .pointer("/credentials/refreshToken")
                        .or_else(|| item.pointer("/credentials/refresh_token"))
                        .or_else(|| item.pointer("/refreshToken"))
                        .or_else(|| item.pointer("/refresh_token"))
                        .and_then(|v| v.as_str())
                    {
                        let email = item
                            .pointer("/credentials/email")
                            .or_else(|| item.pointer("/email"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let project_id = item
                            .pointer("/credentials/projectId")
                            .or_else(|| item.pointer("/projectId"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());

                        list.push(AntigravityAccount {
                            id: format!("ag-acc-{}", idx + 1),
                            email,
                            refresh_token: tok.trim().to_string(),
                            access_token: None,
                            expires_at: 0,
                            project_id,
                            tier_id: None,
                            rate_limited_until: 0,
                            model_rate_limits: std::collections::HashMap::new(),
                            consecutive_errors: 0,
                            is_active: true,
                        });
                    }
                }
                self.accounts = list;
            }
        }

        Ok(())
    }

    pub fn save_accounts(&self) -> Result<()> {
        let _guard = ACCOUNTS_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let target_path = if self.config_file.exists() {
            self.config_file.clone()
        } else {
            Self::get_accounts_file_path()
        };
        if let Some(parent) = target_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(&self.accounts)?;
        let tmp_path = target_path.with_extension("tmp");
        if fs::write(&tmp_path, &json).is_ok() && fs::rename(&tmp_path, &target_path).is_ok() {
            return Ok(());
        }
        fs::write(&target_path, json)?;
        Ok(())
    }

    pub fn add_oauth_account(&mut self, code: &str, redirect_uri: &str) -> Result<String> {
        let account = complete_oauth_flow(&self.client, code, redirect_uri)?;

        if let Some(existing) = self
            .accounts
            .iter_mut()
            .find(|a| a.refresh_token == account.refresh_token)
        {
            existing.is_active = true;
            existing.access_token = account.access_token;
            existing.expires_at = account.expires_at;
            if account.email.is_some() {
                existing.email = account.email;
            }
            if account.project_id.is_some() {
                existing.project_id = account.project_id;
            }
            let id = existing.id.clone();
            self.save_accounts()?;
            return Ok(id);
        }

        let id = account.id.clone();
        self.accounts.push(account);
        self.save_accounts()?;
        Ok(id)
    }

    pub fn add_account(
        &mut self,
        refresh_token: &str,
        email: Option<String>,
        project_id: Option<String>,
    ) -> Result<String> {
        let clean_token = refresh_token.trim().to_string();
        if clean_token.is_empty() {
            bail!("Refresh token cannot be empty");
        }

        if let Some(existing) = self
            .accounts
            .iter_mut()
            .find(|a| a.refresh_token == clean_token)
        {
            existing.is_active = true;
            if email.is_some() {
                existing.email = email;
            }
            if project_id.is_some() {
                existing.project_id = project_id;
            }
            let id = existing.id.clone();
            self.save_accounts()?;
            return Ok(id);
        }

        let id = format!("ag-acc-{}", self.accounts.len() + 1);
        let mut account = AntigravityAccount {
            id: id.clone(),
            email,
            refresh_token: clean_token,
            access_token: None,
            expires_at: 0,
            project_id,
            tier_id: None,
            rate_limited_until: 0,
            model_rate_limits: std::collections::HashMap::new(),
            consecutive_errors: 0,
            is_active: true,
        };

        let _ = refresh_account_token(&self.client, &mut account);
        let _ = ensure_project_id(&self.client, &mut account);

        self.accounts.push(account);
        self.save_accounts()?;
        Ok(id)
    }

    pub fn remove_account(&mut self, id_or_email: &str) -> bool {
        let initial_len = self.accounts.len();
        self.accounts
            .retain(|a| a.id != id_or_email && a.email.as_deref().unwrap_or("") != id_or_email);
        if self.accounts.len() != initial_len {
            let _ = self.save_accounts();
            // Sync across all candidate file locations to avoid restoration from stale candidate paths
            if let Ok(json) = serde_json::to_string_pretty(&self.accounts) {
                for path in crate::assets::find_all_file_candidates("antigravity_accounts.json") {
                    let _ = std::fs::write(path, &json);
                }
            }
            true
        } else {
            false
        }
    }

    pub fn list_accounts(&self) -> Vec<AntigravityAccountInfo> {
        let now = current_timestamp_secs();
        self.accounts
            .iter()
            .map(|a| {
                let is_rate_limited = a.rate_limited_until > now;
                let rate_limited_seconds_left = if is_rate_limited {
                    a.rate_limited_until.saturating_sub(now)
                } else {
                    0
                };
                AntigravityAccountInfo {
                    id: a.id.clone(),
                    email: a.email.clone(),
                    project_id: a.project_id.clone(),
                    is_active: a.is_active,
                    is_rate_limited,
                    rate_limited_seconds_left,
                }
            })
            .collect()
    }

    pub fn refresh_account_by_id(&mut self, id: &str) -> Result<String> {
        let client = self.client.clone();
        if let Some(account) = self.accounts.iter_mut().find(|a| a.id == id) {
            let _ = refresh_account_token(&client, account)?;
            let _ = ensure_project_id(&client, account);
            account.rate_limited_until = 0;
            account.model_rate_limits.clear();
            let pid = account.project_id.clone().unwrap_or_default();
            let _ = self.save_accounts();
            Ok(pid)
        } else {
            bail!("Account not found");
        }
    }

    pub fn toggle_account_by_id(&mut self, id: &str) -> Result<bool> {
        if let Some(account) = self.accounts.iter_mut().find(|a| a.id == id) {
            account.is_active = !account.is_active;
            let active = account.is_active;
            let _ = self.save_accounts();
            Ok(active)
        } else {
            bail!("Account not found");
        }
    }

    pub fn get_account_quota(&mut self, id: &str) -> Result<AntigravityQuotaResponse> {
        let client = self.client.clone();
        let res = {
            let account = match self.accounts.iter_mut().find(|a| a.id == id) {
                Some(a) => a,
                None => bail!("Account with ID '{id}' not found"),
            };
            super::quota::fetch_account_quota(&client, account)?
        };
        let _ = self.save_accounts();
        Ok(res)
    }

    pub fn get_dynamic_available_models(&mut self) -> Result<Vec<super::types::ModelQuotaInfo>> {
        let client = self.client.clone();
        let total = self.accounts.len();
        if total == 0 {
            bail!("No accounts available");
        }

        let mut last_err = None;
        for i in 0..total {
            let account = &mut self.accounts[i];
            if !account.is_active {
                continue;
            }
            match super::quota::fetch_account_quota(&client, account) {
                Ok(quota_res) => {
                    let _ = self.save_accounts();
                    if !quota_res.models.is_empty() {
                        return Ok(quota_res.models);
                    }
                }
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }

        if let Some(e) = last_err {
            bail!("Failed to fetch models from any account: {e}");
        }
        bail!("No models returned from active accounts")
    }

    fn select_available_account_index(&self, model: Option<&str>) -> Option<usize> {
        let total = self.accounts.len();
        if total == 0 {
            return None;
        }

        let now = current_timestamp_secs();
        for i in 0..total {
            let idx = (self.current_index + i) % total;
            let acc = &self.accounts[idx];
            if acc.is_active && !acc.is_rate_limited_for_model(model, now) {
                return Some(idx);
            }
        }
        None
    }

    pub fn chat_stream<F>(
        &mut self,
        model: &str,
        messages: &[ChatMessage],
        temperature: f32,
        max_tokens: u32,
        mut on_sentence: F,
    ) -> Result<String>
    where
        F: FnMut(&str, &str) -> Result<()>,
    {
        if self.accounts.is_empty() {
            bail!("Antigravity has no configured accounts. Please add an account first.");
        }

        let total_accounts = self.accounts.len();
        let mut attempts = 0;
        let now = current_timestamp_secs();

        while attempts < total_accounts {
            let acc_idx = match self.select_available_account_index(Some(model)) {
                Some(idx) => idx,
                None => {
                    let min_wait = self
                        .accounts
                        .iter()
                        .filter(|a| a.is_active)
                        .filter_map(|a| a.min_rate_limit_wait(Some(model), now))
                        .min()
                        .unwrap_or(60);
                    bail!(
                        "All Antigravity accounts are rate-limited or inactive for model '{}'. Please wait {} seconds.",
                        model,
                        min_wait
                    );
                }
            };

            self.current_index = (acc_idx + 1) % total_accounts;
            attempts += 1;

            let now = current_timestamp_secs();
            let need_token = {
                let acc = &self.accounts[acc_idx];
                acc.access_token.is_none() || acc.expires_at <= now
            };

            if need_token {
                if let Err(e) = refresh_account_token(&self.client, &mut self.accounts[acc_idx]) {
                    self.accounts[acc_idx].consecutive_errors += 1;
                    crate::term_err!(
                        "[Antigravity] Failed to refresh token for account {}: {}",
                        self.accounts[acc_idx].id,
                        e
                    );
                    continue;
                }
            }

            let project_id = match ensure_project_id(&self.client, &mut self.accounts[acc_idx]) {
                Ok(pid) => pid,
                Err(_) => "cloudaicompanion-default".to_string(),
            };

            let access_token = self.accounts[acc_idx]
                .access_token
                .clone()
                .unwrap_or_default();

            let result = self.execute_stream_request(
                &access_token,
                &project_id,
                model,
                messages,
                temperature,
                max_tokens,
                &mut on_sentence,
            );

            match result {
                Ok(reply) => {
                    self.accounts[acc_idx].consecutive_errors = 0;
                    let _ = self.save_accounts();
                    return Ok(reply);
                }
                Err(err) => {
                    let err_str = err.to_string();
                    let is_rate_limit = err_str.contains("429")
                        || err_str.to_lowercase().contains("resource_exhausted")
                        || err_str.to_lowercase().contains("quota");

                    if is_rate_limit {
                        let cooldown = 15 * 60;
                        let now = current_timestamp_secs();
                        self.accounts[acc_idx].set_rate_limit_for_model(Some(model), cooldown, now);
                        crate::term_err!(
                            "[Antigravity] Account {} hit 429 quota limit for model '{}'. Rotating to next account...",
                            self.accounts[acc_idx].id,
                            model
                        );
                        let _ = self.save_accounts();
                        continue;
                    } else if err_str.contains("401") || err_str.contains("403") {
                        self.accounts[acc_idx].access_token = None;
                        self.accounts[acc_idx].expires_at = 0;
                        crate::term_err!(
                            "[Antigravity] Account {} auth error: {}. Retrying next...",
                            self.accounts[acc_idx].id,
                            err_str
                        );
                        let _ = self.save_accounts();
                        continue;
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        bail!("All Antigravity accounts failed during execution.")
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_stream_request<F>(
        &self,
        access_token: &str,
        project_id: &str,
        model: &str,
        messages: &[ChatMessage],
        temperature: f32,
        max_tokens: u32,
        on_sentence: &mut F,
    ) -> Result<String>
    where
        F: FnMut(&str, &str) -> Result<()>,
    {
        let endpoint = format!(
            "{}/v1internal:streamGenerateContent?alt=sse",
            ANTIGRAVITY_BASE_URL
        );

        let mut contents = Vec::new();
        for msg in messages {
            let (role, text) = match msg.role.as_str() {
                "assistant" => ("model", msg.content.clone()),
                "tool" => ("user", format!("[TOOL OBSERVATION]:\n{}", msg.content)),
                _ => ("user", msg.content.clone()),
            };
            contents.push(serde_json::json!({
                "role": role,
                "parts": [{ "text": text }]
            }));
        }

        let session_id = generate_pseudo_uuid("ag-sess");
        let conversation_id = generate_pseudo_uuid("ag-conv");
        let trajectory_id = generate_pseudo_uuid("ag-traj");
        let step = (messages.len() * 2).max(1);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let request_id = format!("agent/{conversation_id}/{now_ms}/{trajectory_id}/{step}");

        let clean_model = if let Some(stripped) = model.strip_prefix("ag/") {
            stripped
        } else {
            model
        };

        let payload = serde_json::json!({
            "project": project_id,
            "model": clean_model,
            "userAgent": "antigravity",
            "requestType": "agent",
            "requestId": request_id,
            "request": {
                "contents": contents,
                "generationConfig": {
                    "temperature": temperature,
                    "topP": 0.95,
                    "topK": 40,
                    "maxOutputTokens": max_tokens.min(8192)
                },
                "sessionId": session_id
            }
        });

        let response = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", "application/json")
            .header("User-Agent", ANTIGRAVITY_USER_AGENT)
            .header("X-Client-Name", "antigravity")
            .header("X-Client-Version", "2.11.0")
            .json(&payload)
            .send()
            .context("Failed to send request to Antigravity stream endpoint")?;

        let status = response.status();
        if !status.is_success() {
            let err_text = response.text().unwrap_or_default();
            bail!("Antigravity HTTP error {status}: {err_text}");
        }

        let reader = std::io::BufReader::new(response);
        let mut full_reply = String::new();
        let mut current_sentence = String::new();
        let active_emotion = "neutral";

        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();
            if let Some(data_str) = trimmed.strip_prefix("data: ") {
                if data_str.trim() == "[DONE]" {
                    break;
                }
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(data_str) {
                    let candidates = val
                        .pointer("/response/candidates")
                        .or_else(|| val.pointer("/candidates"))
                        .and_then(|c| c.as_array());

                    if let Some(cands) = candidates {
                        for cand in cands {
                            if let Some(parts) =
                                cand.pointer("/content/parts").and_then(|p| p.as_array())
                            {
                                for part in parts {
                                    if part
                                        .get("thought")
                                        .and_then(|t| t.as_bool())
                                        .unwrap_or(false)
                                    {
                                        continue;
                                    }

                                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                        full_reply.push_str(text);
                                        current_sentence.push_str(text);

                                        let has_skill_tag = current_sentence.contains("[SKILL:")
                                            || current_sentence.contains("[CMD:");
                                        let has_skill_closed =
                                            has_skill_tag && current_sentence.contains(']');

                                        if !has_skill_tag || has_skill_closed {
                                            while let Some(pos) =
                                                current_sentence.find(['.', '?', '!', '\n'])
                                            {
                                                if let Some(tag_idx) = current_sentence
                                                    .find("[SKILL:")
                                                    .or_else(|| current_sentence.find("[CMD:"))
                                                {
                                                    if pos > tag_idx {
                                                        break;
                                                    }
                                                }

                                                // Do not break in the middle of ellipses (... or ..)
                                                let mut end_pos = pos;
                                                if current_sentence.as_bytes().get(pos) == Some(&b'.') {
                                                    while end_pos + 1 < current_sentence.len()
                                                        && current_sentence.as_bytes().get(end_pos + 1) == Some(&b'.')
                                                    {
                                                        end_pos += 1;
                                                    }
                                                }

                                                let sentence = current_sentence[..=end_pos].to_string();
                                                current_sentence =
                                                    current_sentence[end_pos + 1..].to_string();

                                                let clean = sentence.trim().to_string();
                                                if !clean.is_empty() {
                                                    on_sentence(&clean, active_emotion)?;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let clean_rest = current_sentence.trim().to_string();
        if !clean_rest.is_empty() {
            on_sentence(&clean_rest, active_emotion)?;
        }

        Ok(full_reply)
    }

    pub fn chat_stream_v1<F>(
        &mut self,
        req: &crate::llm::v1_engine::V1ChatRequest,
        mut on_event: F,
    ) -> Result<crate::llm::v1_engine::V1Response>
    where
        F: FnMut(crate::llm::v1_engine::V1StreamEvent) -> Result<()>,
    {
        if self.accounts.is_empty() {
            bail!("Antigravity has no configured accounts. Please add an account first.");
        }

        let total_accounts = self.accounts.len();
        let mut attempts = 0;
        let now = current_timestamp_secs();

        while attempts < total_accounts {
            let acc_idx = match self.select_available_account_index(Some(&req.model)) {
                Some(idx) => idx,
                None => {
                    let min_wait = self
                        .accounts
                        .iter()
                        .filter(|a| a.is_active)
                        .filter_map(|a| a.min_rate_limit_wait(Some(&req.model), now))
                        .min()
                        .unwrap_or(60);
                    bail!(
                        "All Antigravity accounts are rate-limited or inactive for model '{}'. Please wait {} seconds.",
                        req.model,
                        min_wait
                    );
                }
            };

            self.current_index = (acc_idx + 1) % total_accounts;
            attempts += 1;

            let now = current_timestamp_secs();
            let need_token = {
                let acc = &self.accounts[acc_idx];
                acc.access_token.is_none() || acc.expires_at <= now
            };

            if need_token {
                if let Err(e) = refresh_account_token(&self.client, &mut self.accounts[acc_idx]) {
                    self.accounts[acc_idx].consecutive_errors += 1;
                    crate::term_err!(
                        "[Antigravity] Failed to refresh token for account {}: {}",
                        self.accounts[acc_idx].id,
                        e
                    );
                    continue;
                }
            }

            let project_id = match ensure_project_id(&self.client, &mut self.accounts[acc_idx]) {
                Ok(pid) => pid,
                Err(_) => "cloudaicompanion-default".to_string(),
            };

            let access_token = self.accounts[acc_idx]
                .access_token
                .clone()
                .unwrap_or_default();

            let result = super::v1_adapter::execute_v1_stream(
                &self.client,
                &access_token,
                &project_id,
                req,
                &mut on_event,
            );

            match result {
                Ok(reply) => {
                    self.accounts[acc_idx].consecutive_errors = 0;
                    let _ = self.save_accounts();
                    return Ok(reply);
                }
                Err(err) => {
                    let err_str = err.to_string();
                    let is_rate_limit = err_str.contains("429")
                        || err_str.to_lowercase().contains("resource_exhausted")
                        || err_str.to_lowercase().contains("quota");

                    if is_rate_limit {
                        let cooldown = 15 * 60;
                        let now = current_timestamp_secs();
                        self.accounts[acc_idx].set_rate_limit_for_model(Some(&req.model), cooldown, now);
                        crate::term_err!(
                            "[Antigravity] Account {} hit 429 quota limit for model '{}'. Rotating to next account...",
                            self.accounts[acc_idx].id,
                            req.model
                        );
                        let _ = self.save_accounts();
                        continue;
                    } else if err_str.contains("401") || err_str.contains("403") {
                        self.accounts[acc_idx].access_token = None;
                        self.accounts[acc_idx].expires_at = 0;
                        crate::term_err!(
                            "[Antigravity] Account {} auth error: {}. Retrying next...",
                            self.accounts[acc_idx].id,
                            err_str
                        );
                        let _ = self.save_accounts();
                        continue;
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        bail!("All Antigravity accounts failed during execution.")
    }
}
