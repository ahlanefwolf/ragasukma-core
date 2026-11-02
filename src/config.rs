use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default = "default_model")]
    pub llm_model: String,
    pub system_prompt: Option<String>,
    pub memory_enabled: Option<bool>,
    pub llm_api_key: Option<String>,
    pub llm_base_url: Option<String>,
    #[serde(default = "default_user_callsign")]
    pub user_callsign: String,
    #[serde(default = "default_assistant_name")]
    pub assistant_name: String,
    #[serde(default = "default_personality_role")]
    pub personality_role: String,
}

fn default_model() -> String {
    "openai/gpt-4o".to_string()
}
fn default_user_callsign() -> String {
    "Commander".to_string()
}
fn default_assistant_name() -> String {
    "Ragasukma".to_string()
}
fn default_personality_role() -> String {
    "Autonomous Assistant".to_string()
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            llm_model: default_model(),
            system_prompt: None,
            memory_enabled: Some(true),
            llm_api_key: None,
            llm_base_url: None,
            user_callsign: default_user_callsign(),
            assistant_name: default_assistant_name(),
            personality_role: default_personality_role(),
        }
    }
}

pub fn get_app_config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(app_data) = std::env::var("APPDATA") {
            let p = PathBuf::from(app_data).join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let p = PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            let p = PathBuf::from(xdg).join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
        if let Ok(home) = std::env::var("HOME") {
            let p = PathBuf::from(home).join(".config").join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    let p = PathBuf::from(".config").join("ragasukma");
    let _ = fs::create_dir_all(&p);
    p
}

pub fn get_app_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(local_appdata) = std::env::var("LOCALAPPDATA").or_else(|_| std::env::var("APPDATA")) {
            let p = PathBuf::from(local_appdata).join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let p = PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            let p = PathBuf::from(xdg).join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
        if let Ok(home) = std::env::var("HOME") {
            let p = PathBuf::from(home).join(".local").join("share").join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    let p = PathBuf::from(".ragasukma");
    let _ = fs::create_dir_all(&p);
    p
}

pub fn get_app_cache_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
            let p = PathBuf::from(local_appdata).join("ragasukma").join("cache");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let p = PathBuf::from(home).join("Library").join("Caches").join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
            let p = PathBuf::from(xdg).join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
        if let Ok(home) = std::env::var("HOME") {
            let p = PathBuf::from(home).join(".cache").join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    let p = PathBuf::from(".cache").join("ragasukma");
    let _ = fs::create_dir_all(&p);
    p
}

pub fn load_user_config() -> UserConfig {
    let config_dir = get_app_config_dir();
    
    // Check config-ai.json first (used by voice assistant/web UI), then config.json
    for filename in &["config-ai.json", "config.json"] {
        let config_path = config_dir.join(filename);
        if config_path.exists() {
            if let Ok(content) = fs::read_to_string(&config_path) {
                if let Ok(cfg) = serde_json::from_str::<UserConfig>(&content) {
                    return cfg;
                }
            }
        }
    }

    UserConfig::default()
}
