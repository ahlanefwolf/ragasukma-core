use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static REQUEST_COUNTER: AtomicUsize = AtomicUsize::new(1);

pub fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn generate_pseudo_uuid(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let count = REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}-{:x}-{:x}", prefix, now, count)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAccount {
    #[serde(default = "default_account_id")]
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(alias = "refreshToken", alias = "token")]
    pub refresh_token: String,
    #[serde(default, alias = "accessToken")]
    pub access_token: Option<String>,
    #[serde(default, alias = "idToken")]
    pub id_token: Option<String>,
    #[serde(default, alias = "expiresAt")]
    pub expires_at: u64,
    #[serde(
        default,
        alias = "chatgptAccountId",
        alias = "accountId",
        alias = "workspaceId"
    )]
    pub chatgpt_account_id: Option<String>,
    #[serde(default, alias = "chatgptPlanType", alias = "planType", alias = "plan")]
    pub chatgpt_plan_type: Option<String>,
    #[serde(default, alias = "rateLimitedUntil")]
    pub rate_limited_until: u64,
    #[serde(default, alias = "modelRateLimits")]
    pub model_rate_limits: HashMap<String, u64>,
    #[serde(default, alias = "consecutiveErrors")]
    pub consecutive_errors: u32,
    #[serde(default = "default_true", alias = "isActive", alias = "enabled")]
    pub is_active: bool,
}

impl CodexAccount {
    pub fn is_rate_limited_for_model(&self, model: Option<&str>, now: u64) -> bool {
        if self.rate_limited_until > now {
            return true;
        }
        if let Some(m) = model {
            let clean_m = m.trim().trim_start_matches("cx/");
            if let Some(&until) = self.model_rate_limits.get(clean_m)
                && until > now
            {
                return true;
            }
        }
        false
    }

    pub fn set_rate_limit_for_model(&mut self, model: Option<&str>, cooldown_secs: u64, now: u64) {
        let until = now + cooldown_secs;
        if let Some(m) = model {
            let clean_m = m.trim().trim_start_matches("cx/");
            if !clean_m.is_empty() {
                self.model_rate_limits.insert(clean_m.to_string(), until);
                return;
            }
        }
        self.rate_limited_until = until;
    }

    pub fn min_rate_limit_wait(&self, model: Option<&str>, now: u64) -> Option<u64> {
        let mut waits = Vec::new();
        if self.rate_limited_until > now {
            waits.push(self.rate_limited_until - now);
        }
        if let Some(m) = model {
            let clean_m = m.trim().trim_start_matches("cx/");
            if let Some(&until) = self.model_rate_limits.get(clean_m)
                && until > now
            {
                waits.push(until - now);
            }
        }
        waits.into_iter().min()
    }
}

fn default_account_id() -> String {
    generate_pseudo_uuid("cx-acc")
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAccountInfo {
    pub id: String,
    pub email: Option<String>,
    pub chatgpt_account_id: Option<String>,
    pub plan_type: Option<String>,
    pub is_active: bool,
    pub is_rate_limited: bool,
    pub rate_limited_seconds_left: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CodexTokenResponse {
    pub access_token: String,
    pub token_type: Option<String>,
    pub expires_in: u64,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodexQuotaWindow {
    pub used: f64,
    pub total: f64,
    pub remaining: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexQuotaResponse {
    pub status: String,
    pub account_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chatgpt_account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub limit_reached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_quota: Option<CodexQuotaWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weekly_quota: Option<CodexQuotaWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_credits: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexModelInfo {
    pub id: String,
    pub model_id: String,
    pub display_name: String,
    pub provider: String,
}
