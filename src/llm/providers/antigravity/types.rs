use serde::{Deserialize, Serialize};
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
pub struct AntigravityAccount {
    #[serde(default = "default_account_id")]
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(alias = "refreshToken", alias = "token")]
    pub refresh_token: String,
    #[serde(default, alias = "accessToken")]
    pub access_token: Option<String>,
    #[serde(default, alias = "expiresAt")]
    pub expires_at: u64,
    #[serde(default, alias = "projectId")]
    pub project_id: Option<String>,
    #[serde(default, alias = "tierId")]
    pub tier_id: Option<String>,
    #[serde(default, alias = "rateLimitedUntil")]
    pub rate_limited_until: u64,
    #[serde(default, alias = "modelRateLimits")]
    pub model_rate_limits: std::collections::HashMap<String, u64>,
    #[serde(default, alias = "consecutiveErrors")]
    pub consecutive_errors: u32,
    #[serde(default = "default_true", alias = "isActive", alias = "enabled")]
    pub is_active: bool,
}

impl AntigravityAccount {
    pub fn is_rate_limited_for_model(&self, model: Option<&str>, now: u64) -> bool {
        if self.rate_limited_until > now {
            return true;
        }
        if let Some(m) = model {
            let clean_m = m.trim().trim_start_matches("ag/");
            if let Some(&until) = self.model_rate_limits.get(clean_m)
                && until > now {
                    return true;
                }
        }
        false
    }

    pub fn set_rate_limit_for_model(&mut self, model: Option<&str>, cooldown_secs: u64, now: u64) {
        let until = now + cooldown_secs;
        if let Some(m) = model {
            let clean_m = m.trim().trim_start_matches("ag/");
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
            let clean_m = m.trim().trim_start_matches("ag/");
            if let Some(&until) = self.model_rate_limits.get(clean_m)
                && until > now {
                    waits.push(until - now);
                }
        }
        waits.into_iter().min()
    }
}

fn default_account_id() -> String {
    generate_pseudo_uuid("ag-acc")
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityAccountInfo {
    pub id: String,
    pub email: Option<String>,
    pub project_id: Option<String>,
    pub is_active: bool,
    pub is_rate_limited: bool,
    pub rate_limited_seconds_left: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub expires_in: u64,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelQuotaInfo {
    pub model_id: String,
    pub display_name: String,
    pub remaining_fraction: f64,
    pub remaining_percentage: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityQuotaResponse {
    pub status: String,
    pub account_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub models: Vec<ModelQuotaInfo>,
    pub weekly: Vec<ModelQuotaInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_per_model_rate_limiting_isolation() {
        let mut account = AntigravityAccount {
            id: "test-acc".to_string(),
            email: None,
            refresh_token: "token".to_string(),
            access_token: None,
            expires_at: 0,
            project_id: None,
            tier_id: None,
            rate_limited_until: 0,
            model_rate_limits: HashMap::new(),
            consecutive_errors: 0,
            is_active: true,
        };

        let now = 1000;
        // Initially no model is rate limited
        assert!(!account.is_rate_limited_for_model(Some("gemini-3.8-flash-high"), now));
        assert!(!account.is_rate_limited_for_model(Some("claude-3-7-sonnet"), now));

        // Rate limit claude-3-7-sonnet for 60 seconds (with ag/ prefix)
        account.set_rate_limit_for_model(Some("ag/claude-3-7-sonnet"), 60, now);

        // claude-3-7-sonnet is rate limited (with or without prefix)
        assert!(account.is_rate_limited_for_model(Some("claude-3-7-sonnet"), now + 10));
        assert!(account.is_rate_limited_for_model(Some("ag/claude-3-7-sonnet"), now + 10));

        // other models on the same account are NOT rate limited
        assert!(!account.is_rate_limited_for_model(Some("gemini-3.8-flash-high"), now + 10));
        assert!(!account.is_rate_limited_for_model(Some("ag/gemini-3.8-flash-high"), now + 10));

        // After cooldown expires, claude-3-7-sonnet is available again
        assert!(!account.is_rate_limited_for_model(Some("claude-3-7-sonnet"), now + 61));

        // Global account rate limit affects all models
        account.set_rate_limit_for_model(None, 30, now);
        assert!(account.is_rate_limited_for_model(Some("gemini-3.8-flash-high"), now + 10));
        assert!(account.is_rate_limited_for_model(Some("claude-3-7-sonnet"), now + 10));
    }
}
