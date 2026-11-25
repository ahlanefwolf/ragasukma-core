use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DiscoveredModelMeta {
    pub context_window: Option<usize>,
    pub native_reasoning: Option<bool>,
}

static DISCOVERED_META_CACHE: LazyLock<RwLock<HashMap<String, DiscoveredModelMeta>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn get_discovered_meta(model_id: &str) -> Option<DiscoveredModelMeta> {
    DISCOVERED_META_CACHE.read().ok()?.get(model_id).cloned()
}

pub fn set_discovered_meta(model_id: &str, meta: DiscoveredModelMeta) {
    if let Ok(mut cache) = DISCOVERED_META_CACHE.write() {
        cache.insert(model_id.to_string(), meta);
    }
}

pub fn clear_discovered_meta() {
    if let Ok(mut cache) = DISCOVERED_META_CACHE.write() {
        cache.clear();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CustomLlmAccount {
    pub prefix: String,
    pub name: String,
    #[serde(alias = "baseUrl")]
    pub base_url: String,
    #[serde(alias = "apiKey")]
    pub api_key: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "contextWindow"
    )]
    pub context_window: Option<usize>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "nativeReasoning"
    )]
    pub native_reasoning: Option<bool>,
}

