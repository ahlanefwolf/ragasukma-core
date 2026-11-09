//! Thought-signature cache: stores and resolves Gemini/Claude thought signatures.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

type ThoughtSignatureEntry = (String, Option<String>, Instant);

static THOUGHT_SIGNATURE_STORE: LazyLock<Mutex<HashMap<String, ThoughtSignatureEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Model family that produced / will consume a signature. Antigravity serves Gemini and Claude
/// models behind the same API, and each backend only accepts its own signatures: a Claude
/// signature replayed to Gemini fails with 400 "Corrupted thought signature." (and vice versa).
pub fn signature_family(model: &str) -> &'static str {
    let m = model.to_lowercase();
    if m.contains("claude") {
        "claude"
    } else {
        "gemini"
    }
}

fn is_signature_compatible(entry_family: Option<&str>, target_family: Option<&str>) -> bool {
    match (entry_family, target_family) {
        (Some(ef), Some(tf)) => ef.eq_ignore_ascii_case(tf),
        _ => true,
    }
}

/// Validate if a string is a valid standard or URL-safe Base64 encoded string
pub fn is_valid_base64(s: &str) -> bool {
    let clean = s.trim();
    if clean.is_empty() {
        return false;
    }
    let mut normalized = clean.to_string();
    match normalized.len() % 4 {
        2 => normalized.push_str("=="),
        3 => normalized.push('='),
        1 => return false,
        _ => {}
    }

    let bytes = normalized.as_bytes();
    let len = bytes.len();
    if !len.is_multiple_of(4) {
        return false;
    }

    let mut pad_len = 0;
    if bytes.ends_with(b"==") {
        pad_len = 2;
    } else if bytes.ends_with(b"=") {
        pad_len = 1;
    }

    let data_part = &bytes[..len - pad_len];
    if data_part.is_empty() {
        return false;
    }

    for &b in data_part {
        if !(b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'-' || b == b'_') {
            return false;
        }
    }
    true
}

/// Sanitize and validate thought signature, returning a strictly valid padded base64 string
pub fn sanitize_thought_signature(sig: &str) -> Option<String> {
    let clean = sig.trim();
    if clean.is_empty() {
        return None;
    }
    let mut normalized = clean.to_string();
    match normalized.len() % 4 {
        2 => normalized.push_str("=="),
        3 => normalized.push('='),
        1 => return None,
        _ => {}
    }

    if is_valid_base64(&normalized) {
        Some(normalized)
    } else {
        None
    }
}

/// Store a thoughtSignature for a tool call ID with optional model family and a 2-hour TTL
pub fn store_thought_signature_scoped(call_id: &str, signature: &str, family: Option<&str>) {
    let clean_id = call_id.trim();
    let clean_sig = sanitize_thought_signature(signature);
    let Some(valid_sig) = clean_sig else {
        return;
    };
    if clean_id.is_empty() {
        return;
    }
    let family_str = family.map(|f| f.to_lowercase());
    if let Ok(mut store) = THOUGHT_SIGNATURE_STORE.lock() {
        if store.len() >= 5000 {
            let now = Instant::now();
            store.retain(|_, (_, _, exp)| *exp > now);
            if store.len() >= 5000 {
                let keys_to_remove: Vec<String> = store.keys().take(1000).cloned().collect();
                for k in keys_to_remove {
                    store.remove(&k);
                }
            }
        }
        let expires_at = Instant::now() + std::time::Duration::from_secs(2 * 3600);
        store.insert(clean_id.to_string(), (valid_sig, family_str, expires_at));
    }
}

/// Store a Gemini 3+ thoughtSignature for a tool call ID with a 2-hour TTL (unscoped legacy)
#[allow(dead_code)]
pub fn store_thought_signature(call_id: &str, signature: &str) {
    store_thought_signature_scoped(call_id, signature, None);
}

/// Retrieve a cached thoughtSignature by tool call ID scoped to a target model family
pub fn get_thought_signature_scoped(call_id: &str, target_family: Option<&str>) -> Option<String> {
    let clean_id = call_id.trim();
    if clean_id.is_empty() {
        return None;
    }
    if let Ok(store) = THOUGHT_SIGNATURE_STORE.lock()
        && let Some((sig, entry_fam, exp)) = store.get(clean_id)
        && *exp > Instant::now()
        && is_signature_compatible(entry_fam.as_deref(), target_family)
    {
        return sanitize_thought_signature(sig);
    }
    None
}

/// Retrieve a cached thoughtSignature by tool call ID
#[allow(dead_code)]
pub fn get_thought_signature(call_id: &str) -> Option<String> {
    get_thought_signature_scoped(call_id, None)
}

/// Resolve thoughtSignature from embedded ID (e.g. `call_123__thought__<sig>`) or cache with family scoping
pub fn resolve_call_signature_scoped(call_id: &str, target_family: Option<&str>) -> Option<String> {
    let clean_id = call_id.trim();
    if clean_id.is_empty() {
        return None;
    }
    if let Some((base, sig)) = clean_id.split_once("__thought__") {
        if let Some(valid_sig) = sanitize_thought_signature(sig) {
            return Some(valid_sig);
        }
        if let Some(cached) = get_thought_signature_scoped(base, target_family) {
            return Some(cached);
        }
    }
    get_thought_signature_scoped(clean_id, target_family)
}

/// Resolve thoughtSignature from embedded ID (e.g. `call_123__thought__<sig>`) or cache
#[allow(dead_code)]
pub fn resolve_call_signature(call_id: &str) -> Option<String> {
    resolve_call_signature_scoped(call_id, None)
}
