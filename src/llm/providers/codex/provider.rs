use super::constants::*;
use super::oauth::refresh_account_token;
use super::types::{
    CodexAccount, CodexAccountInfo, CodexModelInfo, CodexQuotaResponse, current_timestamp_secs,
};
use super::v1_adapter::{
    build_codex_payload_from_messages, build_codex_payload_from_v1, parse_codex_sse_line,
};
use crate::llm::types::ChatMessage;
use anyhow::{Context, Result, bail};
use std::fs;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Mutex;

static ACCOUNTS_FILE_LOCK: Mutex<()> = Mutex::new(());

pub struct CodexProvider {
    pub accounts: Vec<CodexAccount>,
    pub current_index: usize,
    pub client: reqwest::blocking::Client,
    pub config_file: PathBuf,
}

impl Default for CodexProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexProvider {
    pub fn get_accounts_file_path() -> PathBuf {
        let candidate = crate::assets::find_file_in_candidates("codex_accounts.json");
        if candidate.exists() {
            return candidate;
        }
        crate::config::get_app_config_dir().join("codex_accounts.json")
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
            "[Codex] Initialized provider with {} account(s) loaded.",
            provider.accounts.len()
        );
        provider
    }

    pub fn new() -> Self {
        static SHARED_CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
        let client = SHARED_CLIENT
            .get_or_init(|| {
                reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(90))
                    .build()
                    .unwrap_or_default()
            })
            .clone();
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

        if let Ok(accs) = serde_json::from_str::<Vec<CodexAccount>>(trimmed) {
            self.accounts = accs;
            return Ok(());
        }

        if let Ok(token_strings) = serde_json::from_str::<Vec<String>>(trimmed) {
            let mut list = Vec::new();
            for (idx, tok) in token_strings.into_iter().enumerate() {
                if !tok.trim().is_empty() {
                    list.push(CodexAccount {
                        id: format!("cx-acc-{}", idx + 1),
                        email: None,
                        refresh_token: tok.trim().to_string(),
                        access_token: None,
                        id_token: None,
                        expires_at: 0,
                        chatgpt_account_id: None,
                        chatgpt_plan_type: None,
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
                    if let Ok(acc) = serde_json::from_value::<CodexAccount>(item.clone()) {
                        list.push(acc);
                    } else if let Some(tok) = item
                        .pointer("/credentials/refreshToken")
                        .or_else(|| item.pointer("/credentials/refresh_token"))
                        .or_else(|| item.pointer("/refreshToken"))
                        .or_else(|| item.pointer("/refresh_token"))
                        .and_then(|v| v.as_str())
                    {
                        list.push(CodexAccount {
                            id: format!("cx-acc-{}", idx + 1),
                            email: item.get("email").and_then(|v| v.as_str()).map(String::from),
                            refresh_token: tok.trim().to_string(),
                            access_token: None,
                            id_token: None,
                            expires_at: 0,
                            chatgpt_account_id: item.get("chatgptAccountId").and_then(|v| v.as_str()).map(String::from),
                            chatgpt_plan_type: item.get("planType").and_then(|v| v.as_str()).map(String::from),
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
        }

        Ok(())
    }

    pub fn save_accounts(&self) -> Result<()> {
        let _guard = ACCOUNTS_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(parent) = self.config_file.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let json = serde_json::to_string_pretty(&self.accounts)?;
        fs::write(&self.config_file, json)?;
        Ok(())
    }

    pub fn add_oauth_account(&mut self, mut account: CodexAccount) -> Result<String> {
        let _ = refresh_account_token(&self.client, &mut account);

        if let Some(existing) = self
            .accounts
            .iter_mut()
            .find(|a| a.refresh_token == account.refresh_token)
        {
            existing.is_active = true;
            existing.access_token = account.access_token;
            existing.id_token = account.id_token;
            existing.expires_at = account.expires_at;
            if account.email.is_some() {
                existing.email = account.email;
            }
            if account.chatgpt_account_id.is_some() {
                existing.chatgpt_account_id = account.chatgpt_account_id;
            }
            if account.chatgpt_plan_type.is_some() {
                existing.chatgpt_plan_type = account.chatgpt_plan_type;
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
        chatgpt_account_id: Option<String>,
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
            if chatgpt_account_id.is_some() {
                existing.chatgpt_account_id = chatgpt_account_id;
            }
            let id = existing.id.clone();
            self.save_accounts()?;
            return Ok(id);
        }

        let id = format!("cx-acc-{}", self.accounts.len() + 1);
        let mut account = CodexAccount {
            id: id.clone(),
            email,
            refresh_token: clean_token,
            access_token: None,
            id_token: None,
            expires_at: 0,
            chatgpt_account_id,
            chatgpt_plan_type: None,
            rate_limited_until: 0,
            model_rate_limits: std::collections::HashMap::new(),
            consecutive_errors: 0,
            is_active: true,
        };

        let _ = refresh_account_token(&self.client, &mut account);

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
                for path in crate::assets::find_all_file_candidates("codex_accounts.json") {
                    let _ = std::fs::write(path, &json);
                }
            }
            true
        } else {
            false
        }
    }

    pub fn list_account_infos(&self) -> Vec<CodexAccountInfo> {
        let now = current_timestamp_secs();
        self.accounts
            .iter()
            .map(|a| {
                let is_rl = a.rate_limited_until > now;
                let left = if is_rl {
                    a.rate_limited_until - now
                } else {
                    0
                };
                CodexAccountInfo {
                    id: a.id.clone(),
                    email: a.email.clone(),
                    chatgpt_account_id: a.chatgpt_account_id.clone(),
                    plan_type: a.chatgpt_plan_type.clone(),
                    is_active: a.is_active,
                    is_rate_limited: is_rl,
                    rate_limited_seconds_left: left,
                }
            })
            .collect()
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

    pub fn refresh_account_by_id(&mut self, id: &str) -> Result<()> {
        let client = self.client.clone();
        if let Some(account) = self.accounts.iter_mut().find(|a| a.id == id) {
            refresh_account_token(&client, account)?;
            let _ = self.save_accounts();
            Ok(())
        } else {
            bail!("Account not found");
        }
    }

    pub fn get_account_quota(&mut self, id: &str) -> Result<CodexQuotaResponse> {
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

    pub fn get_dynamic_available_models(&mut self) -> Result<Vec<CodexModelInfo>> {
        let total = self.accounts.len();
        if total == 0 {
            bail!("No OpenAI Codex accounts available. Please connect an account first.");
        }

        let mut last_err = None;
        for i in 0..total {
            if !self.accounts[i].is_active {
                continue;
            }

            let now = current_timestamp_secs();
            let need_token = {
                let acc = &self.accounts[i];
                acc.access_token.is_none() || acc.expires_at <= now
            };

            if need_token {
                if let Err(e) = refresh_account_token(&self.client, &mut self.accounts[i]) {
                    last_err = Some(e);
                    continue;
                }
                let _ = self.save_accounts();
            }

            let access_token = match self.accounts[i].access_token.as_deref() {
                Some(tok) if !tok.trim().is_empty() => tok,
                _ => continue,
            };

            let mut req = self
                .client
                .get(CODEX_MODELS_URL)
                .header("Authorization", format!("Bearer {access_token}"))
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
                .header("originator", CODEX_ORIGINATOR)
                .header("User-Agent", format!("codex_cli_rs/{CODEX_CLI_VERSION}"))
                .header("version", CODEX_CLI_VERSION);

            if let Some(ref act_id) = self.accounts[i].chatgpt_account_id {
                req = req.header("ChatGPT-Account-ID", act_id);
            }

            match req.send() {
                Ok(res) => {
                    let status = res.status();
                    if status.is_success() {
                        match res.json::<serde_json::Value>() {
                            Ok(val) => {
                                let items = val
                                    .get("models")
                                    .or_else(|| val.get("data"))
                                    .and_then(|v| v.as_array());

                                if let Some(arr) = items {
                                    let mut models = Vec::with_capacity(arr.len());
                                    for item in arr {
                                        let id = item
                                            .get("id")
                                            .or_else(|| item.get("slug"))
                                            .or_else(|| item.get("model"))
                                            .or_else(|| item.get("name"))
                                            .and_then(|v| v.as_str());

                                        if let Some(m_id) = id {
                                            let name = item
                                                .get("display_name")
                                                .or_else(|| item.get("displayName"))
                                                .or_else(|| item.get("name"))
                                                .and_then(|v| v.as_str())
                                                .unwrap_or(m_id);

                                            models.push(CodexModelInfo {
                                                id: format!("cx/{m_id}"),
                                                model_id: m_id.to_string(),
                                                display_name: name.to_string(),
                                                provider: "codex".to_string(),
                                            });
                                        }
                                    }
                                    if !models.is_empty() {
                                        return Ok(models);
                                    }
                                }
                            }
                            Err(e) => {
                                last_err = Some(e.into());
                            }
                        }
                    } else {
                        let err_text = res.text().unwrap_or_default();
                        last_err = Some(anyhow::anyhow!("Codex Models API returned HTTP {status}: {err_text}"));
                    }
                }
                Err(e) => {
                    last_err = Some(e.into());
                }
            }
        }

        if let Some(e) = last_err {
            bail!("Failed to fetch Codex models from active accounts: {e}");
        }
        bail!("No models returned from active Codex accounts")
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
        mut on_delta: F,
    ) -> Result<String>
    where
        F: FnMut(&str) -> Result<()>,
    {
        if self.accounts.is_empty() {
            bail!("Codex has no configured accounts. Please add an OpenAI Codex account first.");
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
                        "All Codex accounts are rate-limited or inactive for model '{}'. Please wait {} seconds.",
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

            if need_token
                && let Err(e) = refresh_account_token(&self.client, &mut self.accounts[acc_idx]) {
                    self.accounts[acc_idx].consecutive_errors += 1;
                    crate::term_err!(
                        "[Codex] Failed to refresh token for account {}: {}",
                        self.accounts[acc_idx].id,
                        e
                    );
                    continue;
                }

            let access_token = self.accounts[acc_idx]
                .access_token
                .clone()
                .unwrap_or_default();
            let chatgpt_account_id = self.accounts[acc_idx].chatgpt_account_id.clone();

            let payload = build_codex_payload_from_messages(model, messages, temperature, max_tokens);

            match self.execute_codex_stream(&access_token, chatgpt_account_id.as_deref(), &payload, &mut on_delta) {
                Ok(full_reply) => {
                    self.accounts[acc_idx].consecutive_errors = 0;
                    let _ = self.save_accounts();
                    return Ok(full_reply);
                }
                Err(e) => {
                    let err_str = e.to_string();
                    let is_rate_limit = err_str.contains("429")
                        || err_str.to_lowercase().contains("rate limit")
                        || err_str.to_lowercase().contains("capacity");

                    if is_rate_limit {
                        let cooldown = 60;
                        self.accounts[acc_idx].set_rate_limit_for_model(
                            Some(model),
                            cooldown,
                            current_timestamp_secs(),
                        );
                        crate::term_err!(
                            "[Codex] Account {} rate limited on {}. Failing over to next account.",
                            self.accounts[acc_idx].id,
                            model
                        );
                    } else if err_str.contains("401") || err_str.contains("token_revoked") || err_str.contains("invalidated oauth token") {
                        crate::term_err!(
                            "[Codex] Account {} access token revoked or expired (401). Invalidating access token for refresh...",
                            self.accounts[acc_idx].id
                        );
                        self.accounts[acc_idx].access_token = None;
                        let _ = refresh_account_token(&self.client, &mut self.accounts[acc_idx]);
                        self.accounts[acc_idx].consecutive_errors += 1;
                    } else {
                        self.accounts[acc_idx].consecutive_errors += 1;
                        crate::term_err!(
                            "[Codex] Error on account {}: {}. Failing over...",
                            self.accounts[acc_idx].id,
                            e
                        );
                    }
                    let _ = self.save_accounts();
                }
            }
        }

        bail!("All available Codex accounts failed to fulfill request.");
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
            bail!("Codex has no configured accounts.");
        }

        let total_accounts = self.accounts.len();
        let mut attempts = 0;
        let mut last_err = None;

        while attempts < total_accounts {
            let acc_idx = match self.select_available_account_index(Some(&req.model)) {
                Some(idx) => idx,
                None => {
                    bail!("All Codex accounts are rate-limited or inactive.");
                }
            };

            self.current_index = (acc_idx + 1) % total_accounts;
            attempts += 1;

            let now = current_timestamp_secs();
            let need_token = {
                let acc = &self.accounts[acc_idx];
                acc.access_token.is_none() || acc.expires_at <= now
            };

            if need_token
                && let Err(e) = refresh_account_token(&self.client, &mut self.accounts[acc_idx]) {
                    crate::term_err!(
                        "[Codex] Failed to refresh token for account {}: {}",
                        self.accounts[acc_idx].id,
                        e
                    );
                    self.accounts[acc_idx].consecutive_errors += 1;
                    last_err = Some(format!("Account {} token refresh failed: {e}", self.accounts[acc_idx].id));
                    continue;
                }

            let access_token = self.accounts[acc_idx]
                .access_token
                .clone()
                .unwrap_or_default();
            let chatgpt_account_id = self.accounts[acc_idx].chatgpt_account_id.clone();

            let payload = build_codex_payload_from_v1(req);

            match self.execute_codex_stream_internal_v1(&access_token, chatgpt_account_id.as_deref(), &payload, &mut on_event) {
                Ok((full_reply, tool_calls)) => {
                    self.accounts[acc_idx].consecutive_errors = 0;
                    let _ = self.save_accounts();
                    return Ok(crate::llm::v1_engine::V1Response {
                        content: full_reply,
                        tool_calls,
                        finish_reason: "stop".to_string(),
                    });
                }
                Err(e) => {
                    let err_str = e.to_string();
                    if err_str.contains("429") || err_str.to_lowercase().contains("capacity") {
                        self.accounts[acc_idx].set_rate_limit_for_model(
                            Some(&req.model),
                            60,
                            current_timestamp_secs(),
                        );
                    } else if err_str.contains("401") || err_str.contains("token_revoked") || err_str.contains("invalidated oauth token") {
                        crate::term_err!(
                            "[Codex] Account {} token revoked/expired (401). Invalidating access token...",
                            self.accounts[acc_idx].id
                        );
                        self.accounts[acc_idx].access_token = None;
                        let _ = refresh_account_token(&self.client, &mut self.accounts[acc_idx]);
                        self.accounts[acc_idx].consecutive_errors += 1;
                    } else {
                        self.accounts[acc_idx].consecutive_errors += 1;
                    }
                    last_err = Some(err_str);
                    let _ = self.save_accounts();
                }
            }
        }

        if let Some(err) = last_err {
            bail!("Failed to execute Codex stream request after trying available accounts: {err}");
        }
        bail!("Failed to execute Codex stream request after trying all available accounts.");
    }

    fn execute_codex_stream_internal_v1<F>(
        &self,
        access_token: &str,
        chatgpt_account_id: Option<&str>,
        payload: &serde_json::Value,
        on_event: &mut F,
    ) -> Result<(String, Option<Vec<crate::llm::v1_engine::V1ToolCall>>)>
    where
        F: FnMut(crate::llm::v1_engine::V1StreamEvent) -> Result<()>,
    {
        let mut req = self
            .client
            .post(CODEX_RESPONSES_URL)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("originator", CODEX_ORIGINATOR)
            .header("User-Agent", format!("codex_cli_rs/{}", CODEX_CLI_VERSION))
            .header("version", CODEX_CLI_VERSION);

        if let Some(account_id) = chatgpt_account_id {
            req = req.header("ChatGPT-Account-ID", account_id);
        }

        let res = req.json(payload).send().context("Failed to connect to Codex Responses API")?;

        let status = res.status();
        if !status.is_success() {
            let err_body = res.text().unwrap_or_default();
            bail!("Codex Responses API returned HTTP {}: {}", status, err_body);
        }

        let mut full_content = String::new();
        let mut collected_tool_calls: Vec<crate::llm::v1_engine::V1ToolCall> = Vec::new();
        let mut sse_parser = super::v1_adapter::CodexSseParserState::new();
        let reader = std::io::BufReader::new(res);

        for line_result in reader.lines() {
            let line = line_result?;
            let events = sse_parser.parse_line(&line);
            for ev in events {
                match &ev {
                    crate::llm::v1_engine::V1StreamEvent::ContentDelta(c) => {
                        full_content.push_str(c);
                    }
                    crate::llm::v1_engine::V1StreamEvent::ToolCallDelta { index, id, name, arguments } => {
                        let idx = *index;
                        if idx >= collected_tool_calls.len() {
                            collected_tool_calls.resize(
                                idx + 1,
                                crate::llm::v1_engine::V1ToolCall {
                                    id: id.clone().unwrap_or_else(|| format!("call_{}", idx)),
                                    call_type: "function".to_string(),
                                    function: crate::llm::v1_engine::V1ToolCallFunction {
                                        name: name.clone().unwrap_or_default(),
                                        arguments: String::new(),
                                    },
                                    thought_signature: None,
                                    extra_content: None,
                                    provider_specific_fields: None,
                                },
                            );
                        }
                        if let Some(tc_id) = id {
                            collected_tool_calls[idx].id = tc_id.clone();
                        }
                        if let Some(tc_name) = name
                            && !tc_name.is_empty() {
                                collected_tool_calls[idx].function.name = tc_name.clone();
                            }
                        collected_tool_calls[idx].function.arguments.push_str(arguments);
                    }
                    _ => {}
                }
                on_event(ev)?;
            }
        }

        let tc = if !collected_tool_calls.is_empty() {
            Some(collected_tool_calls)
        } else {
            None
        };

        Ok((full_content, tc))
    }

    fn execute_codex_stream_internal<F>(
        &self,
        access_token: &str,
        chatgpt_account_id: Option<&str>,
        payload: &serde_json::Value,
        mut on_raw_delta: F,
    ) -> Result<String>
    where
        F: FnMut(&str) -> Result<()>,
    {
        let mut req = self
            .client
            .post(CODEX_RESPONSES_URL)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("originator", CODEX_ORIGINATOR)
            .header("User-Agent", format!("codex_cli_rs/{}", CODEX_CLI_VERSION))
            .header("version", CODEX_CLI_VERSION);

        if let Some(account_id) = chatgpt_account_id {
            req = req.header("ChatGPT-Account-ID", account_id);
        }

        let res = req.json(payload).send().context("Failed to connect to Codex Responses API")?;

        let status = res.status();
        if !status.is_success() {
            let err_body = res.text().unwrap_or_default();
            bail!("Codex Responses API returned HTTP {}: {}", status, err_body);
        }

        let mut full_content = String::new();
        let reader = std::io::BufReader::new(res);

        for line_result in reader.lines() {
            let line = line_result?;
            if let Some(delta) = parse_codex_sse_line(&line)
                && !delta.is_empty() {
                    full_content.push_str(&delta);
                    on_raw_delta(&delta)?;
                }
        }

        Ok(full_content)
    }

    fn execute_codex_stream<F>(
        &self,
        access_token: &str,
        chatgpt_account_id: Option<&str>,
        payload: &serde_json::Value,
        on_delta: &mut F,
    ) -> Result<String>
    where
        F: FnMut(&str) -> Result<()>,
    {
        let mut full_reply = String::new();

        self.execute_codex_stream_internal(access_token, chatgpt_account_id, payload, |delta| {
            full_reply.push_str(delta);
            on_delta(delta)?;
            Ok(())
        })?;

        Ok(full_reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codex_get_dynamic_available_models_no_accounts() {
        let mut provider = CodexProvider {
            accounts: Vec::new(),
            current_index: 0,
            client: reqwest::blocking::Client::new(),
            config_file: PathBuf::from("/tmp/nonexistent_codex_test.json"),
        };

        let res = provider.get_dynamic_available_models();
        assert!(res.is_err(), "Should return error without fallback when no accounts exist");
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("No OpenAI Codex accounts available"));
    }
}
