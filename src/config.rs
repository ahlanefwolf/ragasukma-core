use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

fn default_vad_threshold() -> f32 {
    0.15
}

fn default_true() -> bool {
    true
}

fn default_lang() -> String {
    "id".to_string()
}

fn default_voice() -> String {
    "F4".to_string()
}

fn default_tts_volume() -> f32 {
    1.6
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default = "default_assistant_name", alias = "assistantName")]
    pub assistant_name: String,
    #[serde(default = "default_user_callsign", alias = "userCallsign")]
    pub user_callsign: String,
    #[serde(default = "default_personality_role", alias = "personalityRole")]
    pub personality_role: String,
    #[serde(default, alias = "assistantGender")]
    pub assistant_gender: String,
    #[serde(default = "default_model", alias = "llmModel")]
    pub llm_model: String,
    #[serde(default = "default_lang")]
    pub lang: String,
    #[serde(default = "default_voice", alias = "ttsVoiceName", alias = "voiceName")]
    pub tts_voice_name: String,
    #[serde(default, alias = "ttsSpeed")]
    pub tts_speed: f32,
    #[serde(
        default = "default_tts_volume",
        alias = "ttsVolume",
        alias = "voiceVolume"
    )]
    pub tts_volume: f32,
    #[serde(default, alias = "avatarType")]
    pub avatar_type: String,
    #[serde(default = "default_true", alias = "alwaysOnTop")]
    pub always_on_top: bool,
    #[serde(default, alias = "voiceInterruption")]
    pub voice_interruption: bool,
    #[serde(default = "default_vad_threshold", alias = "vadThreshold")]
    pub vad_threshold: f32,
    #[serde(default = "default_true", alias = "vadNoiseSuppression")]
    pub vad_noise_suppression: bool,

    // Core agent / engine specific fields
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub memory_enabled: Option<bool>,
    #[serde(default)]
    pub llm_api_key: Option<String>,
    #[serde(default)]
    pub llm_base_url: Option<String>,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            assistant_name: default_assistant_name(),
            user_callsign: default_user_callsign(),
            personality_role: default_personality_role(),
            assistant_gender: "female".to_string(),
            llm_model: default_model(),
            lang: default_lang(),
            tts_voice_name: default_voice(),
            tts_speed: 1.0,
            tts_volume: default_tts_volume(),
            avatar_type: "2d".to_string(),
            always_on_top: true,
            voice_interruption: false,
            vad_threshold: default_vad_threshold(),
            vad_noise_suppression: true,
            system_prompt: None,
            memory_enabled: Some(true),
            llm_api_key: None,
            llm_base_url: None,
        }
    }
}

pub fn get_app_config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(local_appdata) =
            std::env::var("LOCALAPPDATA").or_else(|_| std::env::var("APPDATA"))
        {
            let p = PathBuf::from(local_appdata).join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let legacy_p = PathBuf::from(&home).join(".config").join("ragasukma");
            if legacy_p.exists() {
                return legacy_p;
            }
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
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
            && !xdg.trim().is_empty()
        {
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
        if let Ok(local_appdata) =
            std::env::var("LOCALAPPDATA").or_else(|_| std::env::var("APPDATA"))
        {
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
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
            && !xdg.trim().is_empty()
        {
            let p = PathBuf::from(xdg).join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
        if let Ok(home) = std::env::var("HOME") {
            let p = PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    get_app_config_dir()
}

pub fn get_app_cache_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(local_appdata) =
            std::env::var("LOCALAPPDATA").or_else(|_| std::env::var("APPDATA"))
        {
            let p = PathBuf::from(local_appdata).join("ragasukma").join("cache");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let p = PathBuf::from(home)
                .join("Library")
                .join("Caches")
                .join("ragasukma");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
            && !xdg.trim().is_empty()
        {
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

pub fn get_music_dir() -> PathBuf {
    if PathBuf::from("Cargo.toml").exists() {
        let local = PathBuf::from("assets").join("music");
        if local.exists() {
            return local;
        }
        let parent_local = PathBuf::from("../assets").join("music");
        if parent_local.exists() {
            return parent_local;
        }
    }

    let user_music = get_app_data_dir().join("music");
    if user_music.exists() {
        return user_music;
    }

    let legacy_music = get_app_config_dir().join("assets").join("music");
    if legacy_music.exists() {
        return legacy_music;
    }

    let _ = fs::create_dir_all(&user_music);
    user_music
}

pub fn get_config_file_path() -> PathBuf {
    for filename in &["config-ai.json", "config.json"] {
        let candidate = crate::assets::find_file_in_candidates(filename);
        if candidate.exists() {
            return candidate;
        }
    }

    let p = get_app_config_dir().join("config-ai.json");
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    p
}

pub fn load_user_config() -> UserConfig {
    for filename in &["config-ai.json", "config.json"] {
        let candidate = crate::assets::find_file_in_candidates(filename);
        if candidate.exists()
            && let Ok(content) = fs::read_to_string(&candidate)
            && let Ok(mut cfg) = serde_json::from_str::<UserConfig>(&content)
        {
            if cfg.lang.trim().is_empty() {
                cfg.lang = "id".to_string();
            }
            if cfg.tts_voice_name.trim().is_empty() || cfg.tts_voice_name.contains("Google") {
                cfg.tts_voice_name = "F4".to_string();
            }
            return cfg;
        }
    }

    let config_dir = get_app_config_dir();
    for filename in &["config-ai.json", "config.json"] {
        let config_path = config_dir.join(filename);
        if config_path.exists()
            && let Ok(content) = fs::read_to_string(&config_path)
            && let Ok(mut cfg) = serde_json::from_str::<UserConfig>(&content)
        {
            if cfg.lang.trim().is_empty() {
                cfg.lang = "id".to_string();
            }
            if cfg.tts_voice_name.trim().is_empty() || cfg.tts_voice_name.contains("Google") {
                cfg.tts_voice_name = "F4".to_string();
            }
            return cfg;
        }
    }

    UserConfig::default()
}

pub fn get_config_modified_time() -> Option<std::time::SystemTime> {
    let path = get_config_file_path();
    fs::metadata(&path).and_then(|m| m.modified()).ok()
}

pub fn save_user_config(cfg: &UserConfig) -> Result<(), String> {
    let path = get_config_file_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;

    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, &json).map_err(|e| e.to_string())?;

    let mut rename_success = false;
    for _ in 0..5 {
        if fs::rename(&tmp_path, &path).is_ok() {
            rename_success = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !rename_success {
        fs::write(&path, &json).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(&tmp_path);
    }

    Ok(())
}
