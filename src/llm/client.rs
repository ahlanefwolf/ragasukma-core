use super::providers::antigravity::AntigravityProvider;
use super::providers::claude::ClaudeProvider;
use super::providers::codex::CodexProvider;
use super::providers::custom::CustomProvider;
use super::types::{ChatMessage, ChatRequest};
use anyhow::{bail, Result};

pub struct LlmClient {
    pub model: String,
    pub client: reqwest::blocking::Client,
    pub history: Vec<ChatMessage>,
    pub default_system_prompt: String,
    pub antigravity: AntigravityProvider,
    pub codex: CodexProvider,
    pub claude: ClaudeProvider,
    pub custom: CustomProvider,
}

impl Default for LlmClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmClient {
    pub fn new() -> Self {
        let config = crate::config::load_user_config();
        let model = config.llm_model;

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();

        let default_system_prompt = "You are a helpful AI assistant.".to_string();

        let history = vec![ChatMessage {
            role: "system".to_string(),
            content: default_system_prompt.clone(),
        }];

        let antigravity = AntigravityProvider::new_with_client(client.clone());
        let codex = CodexProvider::new_with_client(client.clone());
        let claude = ClaudeProvider::new_with_client(client.clone());
        let custom = CustomProvider::new_with_client(client.clone());

        Self {
            model,
            client,
            history,
            default_system_prompt,
            antigravity,
            codex,
            claude,
            custom,
        }
    }

    pub fn set_system_prompt(&mut self, prompt: String) {
        self.default_system_prompt = prompt.clone();
        if let Some(first) = self.history.first_mut()
            && first.role == "system" {
                first.content = prompt;
                return;
            }
        self.history.insert(0, ChatMessage {
            role: "system".to_string(),
            content: prompt,
        });
    }

    pub fn reset_history(&mut self) {
        self.history = vec![ChatMessage {
            role: "system".to_string(),
            content: self.default_system_prompt.clone(),
        }];
    }

    pub fn prune_history(&mut self) {
        // Hemat token in RAM/API: save system prompt + maksimal 6 conversation terakhir
        if self.history.len() > 7 {
            let keep_start = self.history.len() - 6;
            let mut pruned = vec![self.history[0].clone()];
            pruned.extend_from_slice(&self.history[keep_start..]);
            self.history = pruned;
        }
    }

    pub fn chat_stream<F>(&mut self, user_input: &str, on_delta: F) -> Result<String>
    where
        F: FnMut(&str) -> Result<()>,
    {
        self.chat_stream_with_role("user", user_input, on_delta)
    }

    pub fn chat_stream_with_role<F>(
        &mut self,
        role: &str,
        input_text: &str,
        on_delta: F,
    ) -> Result<String>
    where
        F: FnMut(&str) -> Result<()>,
    {
        self.chat_stream_with_role_and_prompt(role, input_text, None, on_delta)
    }

    pub fn chat_stream_with_role_and_prompt<F>(
        &mut self,
        role: &str,
        input_text: &str,
        custom_system_prompt: Option<&str>,
        mut on_delta: F,
    ) -> Result<String>
    where
        F: FnMut(&str) -> Result<()>,
    {
        if input_text.trim().is_empty() {
            return Ok(String::new());
        }

        if let Some(sys_prompt) = custom_system_prompt
            && let Some(first_msg) = self.history.first_mut()
                && first_msg.role == "system" {
                    first_msg.content = sys_prompt.to_string();
                }

        self.history.push(ChatMessage {
            role: role.to_string(),
            content: input_text.to_string(),
        });

        // Always reload accounts from disk for real-time synchronization if an account was newly added / removed
        let _ = self.antigravity.load_accounts();
        let _ = self.codex.load_accounts();
        let _ = self.claude.load_accounts();

        let is_claude_engine = self.model.starts_with("cc/");

        if is_claude_engine && !self.claude.accounts.is_empty() {
            let start_time = std::time::Instant::now();

            let in_tokens = self.history.iter().map(|m| m.content.len() / 4).sum::<usize>().max(1);
            match self.claude.chat_stream(
                &self.model,
                &self.history,
                0.7,
                8192,
                &mut on_delta,
            ) {
                Ok(reply) => {
                    let duration_ms = start_time.elapsed().as_millis() as u64;
                    let out_tokens = (reply.len() / 4).max(1);
                    crate::llm::usage::record_usage(
                        "claude",
                        &self.model,
                        in_tokens,
                        0,
                        out_tokens,
                        duration_ms,
                        "success",
                    );

                    self.history.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: reply.clone(),
                    });
                    self.prune_history();

                    let u = crate::config::load_user_config();
                    crate::llm::history::log_conversation_turn(
                        &u.user_callsign,
                        &u.assistant_name,
                        &u.personality_role,
                        input_text,
                        &reply,
                    );
                    return Ok(reply);
                }
                Err(e) => {
                    let duration_ms = start_time.elapsed().as_millis() as u64;
                    crate::llm::usage::record_usage(
                        "claude",
                        &self.model,
                        in_tokens,
                        0,
                        0,
                        duration_ms,
                        "error",
                    );
                    crate::term_err!("[Claude Engine Error]: {}. Falling back to default provider...", e);
                }
            }
        }

        let is_codex_engine = self.model.starts_with("cx/");

        if is_codex_engine && !self.codex.accounts.is_empty() {
            let start_time = std::time::Instant::now();
            let in_tokens = self.history.iter().map(|m| m.content.len() / 4).sum::<usize>().max(1);
            match self.codex.chat_stream(
                &self.model,
                &self.history,
                0.7,
                8192,
                &mut on_delta,
            ) {
                Ok(reply) => {
                    let duration_ms = start_time.elapsed().as_millis() as u64;
                    let out_tokens = (reply.len() / 4).max(1);
                    crate::llm::usage::record_usage(
                        "codex",
                        &self.model,
                        in_tokens,
                        0,
                        out_tokens,
                        duration_ms,
                        "success",
                    );

                    self.history.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: reply.clone(),
                    });
                    self.prune_history();

                    let u = crate::config::load_user_config();
                    crate::llm::history::log_conversation_turn(
                        &u.user_callsign,
                        &u.assistant_name,
                        &u.personality_role,
                        input_text,
                        &reply,
                    );
                    return Ok(reply);
                }
                Err(e) => {
                    let duration_ms = start_time.elapsed().as_millis() as u64;
                    crate::llm::usage::record_usage(
                        "codex",
                        &self.model,
                        in_tokens,
                        0,
                        0,
                        duration_ms,
                        "error",
                    );
                    crate::term_err!("[Codex Engine Error]: {}. Falling back to default provider...", e);
                }
            }
        }

        let is_antigravity_model = self.model.starts_with("ag/");
        if is_antigravity_model {
            if !self.antigravity.accounts.is_empty() {
                let raw_model = self.model.strip_prefix("ag/").unwrap_or(&self.model);

                let ag_model = crate::llm::providers::antigravity::v1_adapter::resolve_model_alias(raw_model);
                let start_time = std::time::Instant::now();
                let in_tokens = self.history.iter().map(|m| m.content.len() / 4).sum::<usize>().max(1);

                match self.antigravity.chat_stream(
                    &ag_model,
                    &self.history,
                    0.7,
                    8192,
                    &mut on_delta,
                ) {
                    Ok(reply) => {
                        let duration_ms = start_time.elapsed().as_millis() as u64;
                        let out_tokens = (reply.len() / 4).max(1);
                        crate::llm::usage::record_usage(
                            "antigravity",
                            &ag_model,
                            in_tokens,
                            0,
                            out_tokens,
                            duration_ms,
                            "success",
                        );

                        self.history.push(ChatMessage {
                            role: "assistant".to_string(),
                            content: reply.clone(),
                        });
                        self.prune_history();

                        let u = crate::config::load_user_config();
                        crate::llm::history::log_conversation_turn(
                            &u.user_callsign,
                            &u.assistant_name,
                            &u.personality_role,
                            input_text,
                            &reply,
                        );
                        return Ok(reply);
                    }
                    Err(e) => {
                        let duration_ms = start_time.elapsed().as_millis() as u64;
                        crate::llm::usage::record_usage(
                            "antigravity",
                            &ag_model,
                            in_tokens,
                            0,
                            0,
                            duration_ms,
                            "error",
                        );
                        crate::term_err!("[LLM Client] Antigravity error: {e}");
                        return Err(e);
                    }
                }
            } else {
                return Err(anyhow::anyhow!("No Antigravity account is connected"));
            }
        }

        // Route user-defined prefix/model requests through the matching OpenAI-compatible account.
        let start_time = std::time::Instant::now();
        let in_tokens = self.history.iter().map(|m| m.content.len() / 4).sum::<usize>().max(1);

        match self.custom.chat_stream(
            &self.model,
            &self.history,
            0.7,
            8192,
            &mut on_delta,
        ) {
            Ok(reply) => {
                let duration_ms = start_time.elapsed().as_millis() as u64;
                let out_tokens = (reply.len() / 4).max(1);
                crate::llm::usage::record_usage(
                    "custom",
                    &self.model,
                    in_tokens,
                    0,
                    out_tokens,
                    duration_ms,
                    "success",
                );

                self.history.push(ChatMessage {
                    role: "assistant".to_string(),
                    content: reply.clone(),
                });
                self.prune_history();

                let u = crate::config::load_user_config();
                crate::llm::history::log_conversation_turn(
                    &u.user_callsign,
                    &u.assistant_name,
                    &u.personality_role,
                    input_text,
                    &reply,
                );
                return Ok(reply);
            }
            Err(e) => {
                let duration_ms = start_time.elapsed().as_millis() as u64;
                crate::llm::usage::record_usage(
                    "custom",
                    &self.model,
                    in_tokens,
                    0,
                    0,
                    duration_ms,
                    "error",
                );
                crate::term_err!("[LLM Client] Custom provider error: {e}");
            }
        }

        bail!("Selected model '{}' is not supported or misconfigured.", self.model);
    }

    /// Complete a prompt or message sequence silently without triggering audio playback or modifying chat history.
    pub fn complete_messages_silent(&mut self, messages: &[ChatMessage]) -> Result<String> {
        let _ = self.antigravity.load_accounts();
        let _ = self.claude.load_accounts();
        let _ = self.codex.load_accounts();

        let is_claude_engine = self.model.starts_with("cc/");
        if is_claude_engine && !self.claude.accounts.is_empty() {
            let mut dummy_cb = |_delta: &str| -> Result<()> { Ok(()) };
            if let Ok(reply) = self.claude.chat_stream(
                &self.model,
                messages,
                0.2,
                4096,
                &mut dummy_cb,
            ) {
                return Ok(reply);
            }
        }

        let is_codex_engine = self.model.starts_with("cx/");
        if is_codex_engine && !self.codex.accounts.is_empty() {
            let mut dummy_cb = |_delta: &str| -> Result<()> { Ok(()) };
            if let Ok(reply) = self.codex.chat_stream(
                &self.model,
                messages,
                0.2,
                4096,
                &mut dummy_cb,
            ) {
                return Ok(reply);
            }
        }

        let is_antigravity_model = self.model.starts_with("ag/");
        if is_antigravity_model && !self.antigravity.accounts.is_empty() {
            let raw_model = self.model.strip_prefix("ag/").unwrap_or(&self.model);
            let ag_model = crate::llm::providers::antigravity::v1_adapter::resolve_model_alias(raw_model);
            let mut dummy_cb = |_delta: &str| -> Result<()> { Ok(()) };
            if let Ok(reply) = self.antigravity.chat_stream(
                &ag_model,
                messages,
                0.2, // Low temperature for deterministic planning
                4096,
                &mut dummy_cb,
            ) {
                return Ok(reply);
            }
        }

        let custom_account = self.model.split_once('/').and_then(|(prefix, model)| {
            if model.trim().is_empty() {
                return None;
            }
            crate::llm::providers::custom::load_accounts()
                .into_iter()
                .find(|account| account.enabled && account.prefix.eq_ignore_ascii_case(prefix))
                .map(|account| (account, model.to_string()))
        });

        let (account, request_model) = custom_account
            .ok_or_else(|| anyhow::anyhow!("No enabled provider account found for model '{}'", self.model))?;

        let custom_endpoint = if account.base_url.ends_with("/chat/completions") {
            account.base_url
        } else {
            format!("{}/chat/completions", account.base_url.trim_end_matches('/'))
        };

        let req_body = ChatRequest {
            model: request_model,
            messages: messages.to_vec(),
            temperature: 0.2,
            max_tokens: 4096,
            stream: false,
            response_format: Some(serde_json::json!({ "type": "json_object" })),
        };

        let mut req_builder = self
            .client
            .post(&custom_endpoint)
            .header("Content-Type", "application/json");

        if !account.api_key.trim().is_empty() {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", account.api_key));
        }

        let response = req_builder.json(&req_body).send()?;
        if response.status().is_success() {
            let res_json = response.json::<serde_json::Value>()?;
            if let Some(reply) = res_json["choices"][0]["message"]["content"].as_str() {
                return Ok(reply.to_string());
            }
        }

        anyhow::bail!("Failed silent completion via {custom_endpoint}")
    }
}