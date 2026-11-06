use super::constants::*;
use super::oauth::refresh_account_token;
use super::quota::fetch_account_quota;
use super::types::{
    ClaudeAccount, ClaudeAccountInfo, ClaudeModelInfo, ClaudeQuotaResponse, current_timestamp_secs,
};
use super::v1_adapter::{
    ClaudeSseParserState, build_claude_payload_from_messages, build_claude_payload_from_v1,
};
use anyhow::{Context, Result, bail};
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

static ACCOUNTS_FILE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub struct ClaudeProvider {
    pub accounts: Vec<ClaudeAccount>,
    pub current_index: usize,
    pub client: reqwest::blocking::Client,
    pub config_file: PathBuf,
}

impl Default for ClaudeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeProvider {
    pub fn get_accounts_file_path() -> PathBuf {
        let candidate = crate::assets::find_file_in_candidates("claude_accounts.json");
        if candidate.exists() {
            return candidate;
        }
        crate::config::get_app_config_dir().join("claude_accounts.json")
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
        provider
    }

    pub fn new() -> Self {
        static SHARED_CLIENT: std::sync::OnceLock<reqwest::blocking::Client> =
            std::sync::OnceLock::new();
        let client = SHARED_CLIENT
            .get_or_init(|| {
                reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(60))
                    .build()
                    .unwrap_or_default()
            })
            .clone();
        Self::new_with_client(client)
    }

    pub fn load_accounts(&mut self) -> Result<()> {
        let _lock = ACCOUNTS_FILE_LOCK
            .lock()
            .map_err(|e| anyhow::anyhow!("ACCOUNTS_FILE_LOCK poisoned: {e}"))?;
        let candidate = crate::assets::find_file_in_candidates("claude_accounts.json");
        if !candidate.exists() {
            let app_file = crate::config::get_app_config_dir().join("claude_accounts.json");
            if app_file.exists() {
                self.config_file = app_file;
            } else {
                self.config_file = candidate;
                self.accounts = Vec::new();
                return Ok(());
            }
        } else {
            self.config_file = candidate;
        }

        let content = std::fs::read_to_string(&self.config_file)
            .with_context(|| format!("Failed to read accounts file: {:?}", self.config_file))?;

        if content.trim().is_empty() {
            self.accounts = Vec::new();
            return Ok(());
        }

        let mut loaded: Vec<ClaudeAccount> = serde_json::from_str(&content).with_context(|| {
            format!(
                "Failed to parse Claude accounts JSON from {:?}",
                self.config_file
            )
        })?;

        let now = current_timestamp_secs();
        let mut dirty = false;

        for acc in &mut loaded {
            if !acc.is_active {
                continue;
            }
            if (acc.access_token.is_none() || acc.expires_at <= now)
                && refresh_account_token(&self.client, acc).is_ok()
            {
                dirty = true;
            }
        }

        self.accounts = loaded;
        if dirty && let Ok(json) = serde_json::to_string_pretty(&self.accounts) {
            for path in crate::assets::find_all_file_candidates("claude_accounts.json") {
                let _ = std::fs::write(path, &json);
            }
        }

        Ok(())
    }

    pub fn save_accounts(&self) -> Result<()> {
        let _lock = ACCOUNTS_FILE_LOCK
            .lock()
            .map_err(|e| anyhow::anyhow!("ACCOUNTS_FILE_LOCK poisoned: {e}"))?;
        let json = serde_json::to_string_pretty(&self.accounts)
            .context("Failed to serialize Claude accounts")?;

        for path in crate::assets::find_all_file_candidates("claude_accounts.json") {
            let _ = std::fs::write(path, &json);
        }

        if let Some(parent) = self.config_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.config_file, json)
            .with_context(|| format!("Failed to write accounts file: {:?}", self.config_file))?;
        Ok(())
    }

    pub fn add_oauth_account(&mut self, account: ClaudeAccount) -> Result<String> {
        let id = account.id.clone();
        if let Some(pos) = self.accounts.iter().position(|a| {
            a.refresh_token == account.refresh_token
                || (account.email.is_some() && a.email == account.email)
        }) {
            self.accounts[pos].refresh_token = account.refresh_token;
            self.accounts[pos].access_token = account.access_token;
            self.accounts[pos].expires_at = account.expires_at;
            if account.email.is_some() {
                self.accounts[pos].email = account.email;
            }
            if account.org_id.is_some() {
                self.accounts[pos].org_id = account.org_id;
            }
            self.accounts[pos].is_active = true;
            let existing_id = self.accounts[pos].id.clone();
            self.save_accounts()?;
            return Ok(existing_id);
        }

        self.accounts.push(account);
        self.save_accounts()?;
        Ok(id)
    }

    pub fn add_account(
        &mut self,
        refresh_token: &str,
        email: Option<String>,
        org_id: Option<String>,
    ) -> Result<String> {
        let clean_token = refresh_token.trim().to_string();
        if clean_token.is_empty() {
            bail!("Refresh token cannot be empty");
        }

        let mut account = ClaudeAccount {
            id: super::types::generate_pseudo_uuid("cc-acc"),
            email,
            refresh_token: clean_token,
            access_token: None,
            expires_at: 0,
            org_id,
            plan_type: Some("Claude Code".to_string()),
            rate_limited_until: 0,
            model_rate_limits: std::collections::HashMap::new(),
            consecutive_errors: 0,
            is_active: true,
        };

        refresh_account_token(&self.client, &mut account)?;
        self.add_oauth_account(account)
    }

    pub fn remove_account(&mut self, id: &str) -> bool {
        let before_len = self.accounts.len();
        self.accounts.retain(|a| a.id != id);
        let removed = self.accounts.len() < before_len;
        if removed {
            let _ = self.save_accounts();
        }
        removed
    }

    pub fn list_account_infos(&self) -> Vec<ClaudeAccountInfo> {
        let now = current_timestamp_secs();
        self.accounts
            .iter()
            .map(|a| {
                let is_rl = a.rate_limited_until > now;
                let rl_left = if is_rl {
                    a.rate_limited_until.saturating_sub(now)
                } else {
                    0
                };
                ClaudeAccountInfo {
                    id: a.id.clone(),
                    email: a.email.clone(),
                    org_id: a.org_id.clone(),
                    plan_type: a.plan_type.clone(),
                    is_active: a.is_active,
                    is_rate_limited: is_rl,
                    rate_limited_seconds_left: rl_left,
                }
            })
            .collect()
    }

    pub fn toggle_account_by_id(&mut self, id: &str) -> Result<bool> {
        if let Some(acc) = self.accounts.iter_mut().find(|a| a.id == id) {
            acc.is_active = !acc.is_active;
            let active = acc.is_active;
            self.save_accounts()?;
            Ok(active)
        } else {
            bail!("Account not found: {id}");
        }
    }

    pub fn refresh_account_by_id(&mut self, id: &str) -> Result<String> {
        if let Some(acc) = self.accounts.iter_mut().find(|a| a.id == id) {
            let token = refresh_account_token(&self.client, acc)?;
            self.save_accounts()?;
            Ok(token)
        } else {
            bail!("Account not found: {id}");
        }
    }

    pub fn get_account_quota(&mut self, id: &str) -> Result<ClaudeQuotaResponse> {
        let acc_opt = self.accounts.iter_mut().find(|a| a.id == id);
        let acc = match acc_opt {
            Some(a) => a,
            None => bail!("Account not found: {id}"),
        };

        let res = fetch_account_quota(&self.client, acc)?;
        let _ = self.save_accounts();
        Ok(res)
    }

    pub fn get_dynamic_available_models(&mut self) -> Result<Vec<ClaudeModelInfo>> {
        let total = self.accounts.len();
        if total == 0 {
            bail!("No Claude accounts available. Please connect an account first.");
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

            let req = self
                .client
                .get(CLAUDE_MODELS_URL)
                .header("Authorization", format!("Bearer {access_token}"))
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("anthropic-beta", "oauth-2025-04-20")
                .header(
                    "User-Agent",
                    format!("claude-cli/{CLAUDE_CLI_VERSION} (external, sdk-cli)"),
                );

            match req.send() {
                Ok(res) => {
                    let status = res.status();
                    if status.is_success() {
                        match res.json::<serde_json::Value>() {
                            Ok(val) => {
                                let items = val
                                    .get("data")
                                    .or_else(|| val.get("models"))
                                    .and_then(|v| v.as_array());

                                if let Some(arr) = items {
                                    let mut models = Vec::with_capacity(arr.len());
                                    for item in arr {
                                        let id = item
                                            .get("id")
                                            .or_else(|| item.get("name"))
                                            .and_then(|v| v.as_str());

                                        if let Some(m_id) = id {
                                            let name = item
                                                .get("display_name")
                                                .or_else(|| item.get("displayName"))
                                                .or_else(|| item.get("name"))
                                                .and_then(|v| v.as_str())
                                                .unwrap_or(m_id);

                                            models.push(ClaudeModelInfo {
                                                id: format!("cc/{m_id}"),
                                                model_id: m_id.to_string(),
                                                display_name: name.to_string(),
                                                provider: "claude".to_string(),
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
                        last_err = Some(anyhow::anyhow!(
                            "Claude Models API returned HTTP {status}: {err_text}"
                        ));
                    }
                }
                Err(e) => {
                    last_err = Some(e.into());
                }
            }
        }

        if let Some(e) = last_err {
            bail!("Failed to fetch Claude models from active accounts: {e}");
        }
        bail!("No models returned from active Claude accounts")
    }

    fn select_available_account_index(&self, model: Option<&str>) -> Option<usize> {
        let total = self.accounts.len();
        if total == 0 {
            return None;
        }

        let now = current_timestamp_secs();
        for offset in 0..total {
            let idx = (self.current_index + offset) % total;
            let acc = &self.accounts[idx];
            if !acc.is_active {
                continue;
            }
            if !acc.is_rate_limited_for_model(model, now) {
                return Some(idx);
            }
        }

        None
    }

    pub fn chat_stream<F>(
        &mut self,
        model: &str,
        messages: &[crate::llm::types::ChatMessage],
        temperature: f32,
        max_tokens: u32,
        mut on_delta: F,
    ) -> Result<String>
    where
        F: FnMut(&str) -> Result<()>,
    {
        let total = self.accounts.len();
        if total == 0 {
            bail!("No Claude accounts available. Please connect an account first.");
        }

        let mut attempts = 0;
        let max_attempts = total;

        while attempts < max_attempts {
            attempts += 1;
            let acc_idx = match self.select_available_account_index(Some(model)) {
                Some(idx) => idx,
                None => {
                    bail!("All active Claude accounts are currently rate-limited.");
                }
            };

            self.current_index = (acc_idx + 1) % total;

            let now = current_timestamp_secs();
            if self.accounts[acc_idx].access_token.is_none()
                || self.accounts[acc_idx].expires_at <= now
            {
                if let Err(e) = refresh_account_token(&self.client, &mut self.accounts[acc_idx]) {
                    tracing::warn!(
                        "[Claude] Token refresh error for account {}: {}",
                        self.accounts[acc_idx].id,
                        e
                    );
                    continue;
                }
                let _ = self.save_accounts();
            }

            let access_token = match self.accounts[acc_idx].access_token.clone() {
                Some(t) => t,
                None => continue,
            };

            let payload =
                build_claude_payload_from_messages(model, messages, temperature, max_tokens);

            match self.execute_claude_stream(&access_token, &payload, &mut on_delta) {
                Ok(reply) => {
                    self.accounts[acc_idx].consecutive_errors = 0;
                    let _ = self.save_accounts();
                    return Ok(reply);
                }
                Err(e) => {
                    let err_str = e.to_string();
                    tracing::error!(
                        "[Claude] Stream error on account {}: {}",
                        self.accounts[acc_idx].id,
                        err_str
                    );

                    let now = current_timestamp_secs();
                    if err_str.contains("429") || err_str.to_lowercase().contains("rate_limit") {
                        self.accounts[acc_idx].set_rate_limit_for_model(Some(model), 300, now);
                    } else if err_str.contains("401") {
                        let _ = refresh_account_token(&self.client, &mut self.accounts[acc_idx]);
                    } else {
                        self.accounts[acc_idx].consecutive_errors += 1;
                    }
                    let _ = self.save_accounts();
                }
            }
        }

        bail!("Failed to get Claude completion after trying available accounts")
    }

    pub fn chat_stream_v1<F>(
        &mut self,
        req: &crate::llm::v1_engine::V1ChatRequest,
        mut on_event: F,
    ) -> Result<crate::llm::v1_engine::V1Response>
    where
        F: FnMut(crate::llm::v1_engine::V1StreamEvent) -> Result<()>,
    {
        let total = self.accounts.len();
        if total == 0 {
            bail!("No Claude accounts available. Please connect an account first.");
        }

        let mut attempts = 0;
        let max_attempts = total;

        while attempts < max_attempts {
            attempts += 1;
            let acc_idx = match self.select_available_account_index(Some(&req.model)) {
                Some(idx) => idx,
                None => {
                    bail!("All active Claude accounts are currently rate-limited.");
                }
            };

            self.current_index = (acc_idx + 1) % total;

            let now = current_timestamp_secs();
            if self.accounts[acc_idx].access_token.is_none()
                || self.accounts[acc_idx].expires_at <= now
            {
                if let Err(e) = refresh_account_token(&self.client, &mut self.accounts[acc_idx]) {
                    tracing::warn!(
                        "[Claude V1] Token refresh error for account {}: {}",
                        self.accounts[acc_idx].id,
                        e
                    );
                    continue;
                }
                let _ = self.save_accounts();
            }

            let access_token = match self.accounts[acc_idx].access_token.clone() {
                Some(t) => t,
                None => continue,
            };

            let payload = build_claude_payload_from_v1(req);

            match self.execute_claude_stream_internal_v1(&access_token, &payload, &mut on_event) {
                Ok((content, tool_calls)) => {
                    self.accounts[acc_idx].consecutive_errors = 0;
                    let _ = self.save_accounts();
                    let finish = if tool_calls.is_some() {
                        "tool_calls".to_string()
                    } else {
                        "stop".to_string()
                    };
                    return Ok(crate::llm::v1_engine::V1Response {
                        content,
                        tool_calls,
                        finish_reason: finish,
                    });
                }
                Err(e) => {
                    let err_str = e.to_string();
                    tracing::error!(
                        "[Claude V1] Stream error on account {}: {}",
                        self.accounts[acc_idx].id,
                        err_str
                    );

                    let now = current_timestamp_secs();
                    if err_str.contains("429") || err_str.to_lowercase().contains("rate_limit") {
                        self.accounts[acc_idx].set_rate_limit_for_model(Some(&req.model), 300, now);
                    } else if err_str.contains("401") {
                        let _ = refresh_account_token(&self.client, &mut self.accounts[acc_idx]);
                    } else {
                        self.accounts[acc_idx].consecutive_errors += 1;
                    }
                    let _ = self.save_accounts();
                }
            }
        }

        bail!("Failed to get Claude v1 stream completion after trying available accounts")
    }

    fn execute_claude_stream_internal_v1<F>(
        &self,
        access_token: &str,
        payload: &serde_json::Value,
        on_event: &mut F,
    ) -> Result<(String, Option<Vec<crate::llm::v1_engine::V1ToolCall>>)>
    where
        F: FnMut(crate::llm::v1_engine::V1StreamEvent) -> Result<()>,
    {
        let req = self
            .client
            .post(format!("{CLAUDE_MESSAGES_URL}?beta=true"))
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("Anthropic-Version", ANTHROPIC_VERSION)
            .header("Anthropic-Beta", ANTHROPIC_BETA)
            .header("Anthropic-Dangerous-Direct-Browser-Access", "true")
            .header(
                "User-Agent",
                format!("claude-cli/{CLAUDE_CLI_VERSION} (external, sdk-cli)"),
            )
            .header("X-App", "cli");

        let res = req
            .json(payload)
            .send()
            .context("Failed to connect to Claude Messages API")?;

        let status = res.status();
        if !status.is_success() {
            let err_body = res.text().unwrap_or_default();
            bail!("Claude Messages API returned HTTP {}: {}", status, err_body);
        }

        let mut full_content = String::new();
        let mut collected_tool_calls: Vec<crate::llm::v1_engine::V1ToolCall> = Vec::new();
        let mut sse_parser = ClaudeSseParserState::new();
        let reader = std::io::BufReader::new(res);

        for line_result in reader.lines() {
            let line = line_result?;
            let events = sse_parser.parse_line(&line);
            for ev in events {
                match &ev {
                    crate::llm::v1_engine::V1StreamEvent::ContentDelta(c) => {
                        full_content.push_str(c);
                    }
                    crate::llm::v1_engine::V1StreamEvent::ToolCallDelta {
                        index,
                        id,
                        name,
                        arguments,
                    } => {
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
                            && !tc_name.is_empty()
                        {
                            collected_tool_calls[idx].function.name = tc_name.clone();
                        }
                        collected_tool_calls[idx]
                            .function
                            .arguments
                            .push_str(arguments);
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

    fn execute_claude_stream<F>(
        &self,
        access_token: &str,
        payload: &serde_json::Value,
        on_delta: &mut F,
    ) -> Result<String>
    where
        F: FnMut(&str) -> Result<()>,
    {
        let mut full_text = String::new();
        let mut v1_handler = |event| -> Result<()> {
            if let crate::llm::v1_engine::V1StreamEvent::ContentDelta(delta) = event {
                full_text.push_str(&delta);
                on_delta(&delta)?;
            }
            Ok(())
        };

        self.execute_claude_stream_internal_v1(access_token, payload, &mut v1_handler)?;
        Ok(full_text)
    }
}
