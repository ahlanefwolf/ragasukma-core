use super::constants::*;
use super::types::generate_pseudo_uuid;
use crate::llm::v1_engine::{
    V1ChatRequest, V1Response, V1StreamEvent, V1ToolCall, V1ToolCallFunction, extract_text_content,
    sanitize_function_name,
};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::io::BufRead;
use std::sync::{LazyLock, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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
    if let Ok(store) = THOUGHT_SIGNATURE_STORE.lock() {
        if let Some((sig, entry_fam, exp)) = store.get(clean_id) {
            if *exp > Instant::now() && is_signature_compatible(entry_fam.as_deref(), target_family) {
                return sanitize_thought_signature(sig);
            }
        }
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

pub fn resolve_model_alias(input: &str) -> String {
    let clean = input.strip_prefix("ag/").unwrap_or(input).trim();
    if clean.is_empty() {
        return "gemini-3.8-flash-high".to_string();
    }
    // Strip trailing parenthesized suffix e.g. "model(low)" -> "model"
    if let Some(idx) = clean.rfind('(') {
        if clean.ends_with(')') {
            let base = clean[..idx].trim();
            if !base.is_empty() {
                return base.to_string();
            }
        }
    }
    clean.to_string()
}

/// Resolves the Gemini thinking configuration (thinkingLevel, thinkingBudget, includeThoughts).
/// Dynamic tier mapping ported from 9router / standard web LLM provider spec:
/// - Low / (low): thinkingLevel: "low", 1024 tokens (fast ~1-2s turnarounds for autonomous coding agents)
/// - Minimal / (minimal): thinkingLevel: "minimal", 512 tokens
/// - Medium / (medium): thinkingLevel: "medium", 4096 tokens (balanced)
/// - High / (high): thinkingLevel: "high", -1 (unconstrained reasoning)
/// - None / Off / (none): thinkingLevel: "minimal", 0 tokens (includeThoughts = false)
/// - Unspecified suffix: defaults to "medium", 4096 tokens
pub fn resolve_gemini_thinking_config(model: &str) -> (String, i64, bool) {
    let lower = model.to_lowercase();
    let trimmed = lower.trim();

    if trimmed.contains("(none)")
        || trimmed.ends_with("-none")
        || trimmed.contains("(off)")
        || trimmed.ends_with("-off")
    {
        ("minimal".to_string(), 0, false)
    } else if trimmed.contains("(minimal)") || trimmed.ends_with("-minimal") {
        ("minimal".to_string(), 512, true)
    } else if trimmed.contains("(low)") || trimmed.ends_with("-low") {
        ("low".to_string(), 1024, true)
    } else if trimmed.contains("(medium)") || trimmed.ends_with("-medium") {
        ("medium".to_string(), 4096, true)
    } else if trimmed.contains("(high)") || trimmed.ends_with("-high") {
        ("high".to_string(), -1, true)
    } else {
        ("medium".to_string(), 4096, true)
    }
}

#[allow(dead_code)]
pub fn resolve_gemini_thinking_budget(model: &str) -> (i64, bool) {
    let (_, budget, include_thoughts) = resolve_gemini_thinking_config(model);
    (budget, include_thoughts)
}

fn sanitize_schema_recursive(val: &mut serde_json::Value) {
    let Some(map) = val.as_object_mut() else {
        return;
    };

    // 1. Convert const to enum
    if let Some(c) = map.remove("const") {
        if !map.contains_key("enum") {
            map.insert("enum".to_string(), serde_json::json!([c]));
        }
    }

    // 2. Convert enum values to strings & ensure type is string
    if let Some(enm) = map.get_mut("enum").and_then(|e| e.as_array_mut()) {
        for item in enm.iter_mut() {
            if !item.is_string() {
                *item = serde_json::Value::String(match item {
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    _ => item.to_string(),
                });
            }
        }
        if !map.contains_key("type") {
            map.insert("type".to_string(), serde_json::json!("string"));
        }
    }

    // 3. Flatten allOf
    if let Some(all_of) = map.remove("allOf") {
        if let Some(items) = all_of.as_array() {
            let mut merged_props = serde_json::Map::new();
            let mut merged_req = Vec::new();
            for item in items {
                if let Some(item_map) = item.as_object() {
                    if let Some(props) = item_map.get("properties").and_then(|p| p.as_object()) {
                        for (pk, pv) in props {
                            merged_props.insert(pk.clone(), pv.clone());
                        }
                    }
                    if let Some(reqs) = item_map.get("required").and_then(|r| r.as_array()) {
                        for r in reqs {
                            if !merged_req.contains(r) {
                                merged_req.push(r.clone());
                            }
                        }
                    }
                }
            }
            if !merged_props.is_empty() {
                let props_entry = map
                    .entry("properties".to_string())
                    .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                if let Some(props_map) = props_entry.as_object_mut() {
                    for (k, v) in merged_props {
                        props_map.entry(k).or_insert(v);
                    }
                }
            }
            if !merged_req.is_empty() {
                let req_entry = map
                    .entry("required".to_string())
                    .or_insert_with(|| serde_json::Value::Array(Vec::new()));
                if let Some(req_arr) = req_entry.as_array_mut() {
                    for r in merged_req {
                        if !req_arr.contains(&r) {
                            req_arr.push(r);
                        }
                    }
                }
            }
        }
    }

    // 4. Flatten anyOf / oneOf
    let any_of_candidate = map.remove("anyOf").or_else(|| map.remove("oneOf"));
    if let Some(union_val) = any_of_candidate {
        if let Some(variants) = union_val.as_array() {
            let mut has_null = false;
            let mut best_variant: Option<serde_json::Value> = None;
            let mut best_score = -1;

            for var in variants {
                let is_null = var.get("type").and_then(|t| t.as_str()) == Some("null")
                    || var.as_str() == Some("null");
                if is_null {
                    has_null = true;
                    continue;
                }

                let var_type = var.get("type").and_then(|t| t.as_str()).unwrap_or("");
                let score = if var_type == "object" || var.get("properties").is_some() {
                    3
                } else if var_type == "array" || var.get("items").is_some() {
                    2
                } else if !var_type.is_empty() {
                    1
                } else {
                    0
                };

                if score > best_score {
                    best_score = score;
                    best_variant = Some(var.clone());
                }
            }

            if has_null {
                map.insert("nullable".to_string(), serde_json::Value::Bool(true));
            }

            if let Some(selected) = best_variant {
                if let Some(sel_map) = selected.as_object() {
                    for (k, v) in sel_map {
                        if !map.contains_key(k) {
                            map.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
        }
    }

    // 5. Flatten type array: e.g. ["string", "null"] -> "string" + nullable: true
    if let Some(type_val) = map.get("type").cloned() {
        if let Some(type_arr) = type_val.as_array() {
            let mut has_null = false;
            let mut chosen_type = None;
            for t in type_arr {
                if let Some(t_str) = t.as_str() {
                    if t_str == "null" {
                        has_null = true;
                    } else if chosen_type.is_none() {
                        chosen_type = Some(t_str.to_string());
                    }
                }
            }
            if has_null {
                map.insert("nullable".to_string(), serde_json::Value::Bool(true));
            }
            let final_type = chosen_type.unwrap_or_else(|| "string".to_string());
            map.insert("type".to_string(), serde_json::Value::String(final_type));
        }
    }

    // Normalize type string aliases if any (e.g. "int" -> "integer", "bool" -> "boolean")
    if let Some(t_str) = map.get("type").and_then(|t| t.as_str()) {
        let lower = t_str.to_lowercase();
        let normalized = match lower.as_str() {
            "int" => "integer",
            "bool" => "boolean",
            "float" | "double" => "number",
            other => other,
        };
        if normalized != t_str {
            map.insert("type".to_string(), serde_json::json!(normalized));
        }
    }

    // 6. Infer missing type=object when properties exist
    if map.contains_key("properties") && !map.contains_key("type") {
        map.insert("type".to_string(), serde_json::json!("object"));
    }

    // 7. Ensure array has items
    if map.get("type").and_then(|t| t.as_str()) == Some("array") {
        if let Some(items_val) = map.get("items") {
            if let Some(arr) = items_val.as_array() {
                if let Some(first) = arr.first() {
                    map.insert("items".to_string(), first.clone());
                } else {
                    map.insert("items".to_string(), serde_json::json!({ "type": "string" }));
                }
            }
        } else {
            map.insert("items".to_string(), serde_json::json!({ "type": "string" }));
        }
    }

    // 8. Remove unsupported JSON Schema keywords for Gemini Protobuf
    const UNSUPPORTED_KEYWORDS: &[&str] = &[
        "$schema",
        "$id",
        "$ref",
        "$comment",
        "$defs",
        "definitions",
        "title",
        "default",
        "examples",
        "additionalProperties",
        "propertyNames",
        "patternProperties",
        "unevaluatedProperties",
        "unevaluatedItems",
        "minLength",
        "maxLength",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "minItems",
        "maxItems",
        "uniqueItems",
        "multipleOf",
        "contains",
        "prefixItems",
        "additionalItems",
        "format",
        "contentSchema",
        "contentMediaType",
        "contentEncoding",
        "deprecated",
        "readOnly",
        "writeOnly",
        "dependencies",
        "dependentSchemas",
        "dependentRequired",
        "not",
        "if",
        "then",
        "else",
    ];
    for key in UNSUPPORTED_KEYWORDS {
        map.remove(*key);
    }

    // 9. Recurse into properties
    if let Some(props) = map.get_mut("properties").and_then(|p| p.as_object_mut()) {
        for (_, prop_val) in props.iter_mut() {
            sanitize_schema_recursive(prop_val);
        }
    }

    // 10. Recurse into items
    if let Some(items) = map.get_mut("items") {
        sanitize_schema_recursive(items);
    }

    // 11. Cleanup required fields: only keep fields that exist in properties
    let valid_keys: Option<std::collections::HashSet<String>> = map
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|p| p.keys().cloned().collect());

    if let Some(valid_keys) = valid_keys {
        let mut remove_required = false;
        if let Some(req_val) = map.get_mut("required") {
            if let Some(req_arr) = req_val.as_array_mut() {
                req_arr.retain(|r| r.as_str().map(|k| valid_keys.contains(k)).unwrap_or(false));
                if req_arr.is_empty() {
                    remove_required = true;
                }
            }
        }
        if remove_required {
            map.remove("required");
        }
    }
}

pub fn clean_parameters_schema(param: &serde_json::Value) -> serde_json::Value {
    let mut obj = param.clone();
    if !obj.is_object() {
        return serde_json::json!({
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "Brief explanation of why you are calling this tool"
                }
            },
            "required": ["reason"]
        });
    }

    sanitize_schema_recursive(&mut obj);

    if let Some(map) = obj.as_object_mut() {
        // Gemini strictly requires the root parameters schema to have type: "object"
        map.insert("type".to_string(), serde_json::json!("object"));

        // If properties is missing or empty, add placeholder
        let needs_placeholder = match map.get("properties") {
            Some(p) => p.as_object().map(|o| o.is_empty()).unwrap_or(true),
            None => true,
        };

        if needs_placeholder {
            let mut props = serde_json::Map::new();
            props.insert(
                "reason".to_string(),
                serde_json::json!({
                    "type": "string",
                    "description": "Brief explanation of why you are calling this tool"
                }),
            );
            map.insert("properties".to_string(), serde_json::Value::Object(props));
            map.insert("required".to_string(), serde_json::json!(["reason"]));
        }
    }

    obj
}

pub fn sanitize_system_prompt(prompt: &str) -> String {
    // Rewrites competing client branding to avoid 429 quota exhaustion flag from Google
    let mut res = prompt.replace(
        "You are a Claude agent, built on Anthropic's Claude Agent SDK.",
        "",
    );
    res = res.replace("OpenCode", "Antigravity");
    res = res.replace("opencode", "antigravity");
    res = res.replace("OPENCODE", "ANTIGRAVITY");
    res
}

pub fn build_gemini_payload(req: &V1ChatRequest, project_id: &str) -> serde_json::Value {
    let clean_model = resolve_model_alias(&req.model);
    let family = signature_family(&req.model);

    let mut system_prompts = Vec::new();
    let mut tool_call_names: HashMap<String, String> = HashMap::new();

    // First pass: index tool_calls IDs to function names
    for msg in &req.messages {
        if let Some(ref tcs) = msg.tool_calls {
            for tc in tcs {
                let fn_name = sanitize_function_name(&tc.function.name);
                tool_call_names.insert(tc.id.clone(), fn_name.clone());
                if let Some((base, _)) = tc.id.split_once("__thought__") {
                    tool_call_names.insert(base.to_string(), fn_name);
                }
            }
        }
    }

    let mut raw_contents: Vec<serde_json::Value> = Vec::new();

    for msg in &req.messages {
        let text = extract_text_content(&msg.content);
        match msg.role.as_str() {
            "system" => {
                if !text.trim().is_empty() {
                    system_prompts.push(sanitize_system_prompt(&text));
                }
            }
            "assistant" | "model" => {
                let mut parts = Vec::new();
                if !text.trim().is_empty() {
                    parts.push(serde_json::json!({ "text": text }));
                }
                if let Some(ref tcs) = msg.tool_calls {
                    for tc in tcs {
                        let fn_name = sanitize_function_name(&tc.function.name);
                        let args_val: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments)
                                .unwrap_or_else(|_| serde_json::json!({}));

                        let clean_tc_id = tc.id.split_once("__thought__").map(|(b, _)| b).unwrap_or(&tc.id);

                        let direct_sig = tc
                            .thought_signature
                            .as_deref()
                            .and_then(sanitize_thought_signature)
                            .or_else(|| {
                                tc.extra_content
                                    .as_ref()
                                    .and_then(|ec| {
                                        ec.pointer("/google/thought_signature")
                                            .or_else(|| ec.pointer("/google/thoughtSignature"))
                                    })
                                    .and_then(|v| v.as_str())
                                    .and_then(sanitize_thought_signature)
                            })
                            .or_else(|| {
                                tc.provider_specific_fields
                                    .as_ref()
                                    .and_then(|ps| {
                                        ps.get("thought_signature")
                                            .or_else(|| ps.get("thoughtSignature"))
                                    })
                                    .and_then(|v| v.as_str())
                                    .and_then(sanitize_thought_signature)
                            });

                        let cached_sig = direct_sig.or_else(|| resolve_call_signature_scoped(&tc.id, Some(family)));
                        if let Some(ref s) = cached_sig {
                            store_thought_signature_scoped(clean_tc_id, s, Some(family));
                        }
                        let call_sig = cached_sig;

                        let mut part = serde_json::json!({
                            "functionCall": {
                                "id": clean_tc_id,
                                "name": fn_name,
                                "args": args_val
                            }
                        });

                        if let Some(sig) = call_sig.as_deref().and_then(sanitize_thought_signature)
                        {
                            part["thoughtSignature"] = serde_json::Value::String(sig);
                        }

                        parts.push(part);
                    }
                }
                if !parts.is_empty() {
                    raw_contents.push(serde_json::json!({
                        "role": "model",
                        "parts": parts
                    }));
                }
            }
            "tool" => {
                let tc_id = msg.tool_call_id.as_deref().unwrap_or("");
                let clean_tc_id = tc_id.split_once("__thought__").map(|(b, _)| b).unwrap_or(tc_id);
                let fn_name = tool_call_names
                    .get(tc_id)
                    .or_else(|| tool_call_names.get(clean_tc_id))
                    .cloned()
                    .or_else(|| msg.name.clone().map(|n| sanitize_function_name(&n)))
                    .unwrap_or_else(|| "tool".to_string());

                let parsed_val: serde_json::Value = serde_json::from_str(&text)
                    .unwrap_or_else(|_| serde_json::json!({ "result": text }));

                let res_obj = match parsed_val {
                    serde_json::Value::Object(map) => serde_json::Value::Object(map),
                    other => serde_json::json!({ "result": other }),
                };

                let mut fr_obj = serde_json::json!({
                    "name": fn_name,
                    "response": res_obj
                });
                if !clean_tc_id.is_empty() {
                    fr_obj["id"] = serde_json::Value::String(clean_tc_id.to_string());
                }

                raw_contents.push(serde_json::json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": fr_obj
                    }]
                }));
            }
            _ => {
                // "user" or other
                if !text.trim().is_empty() {
                    raw_contents.push(serde_json::json!({
                        "role": "user",
                        "parts": [{ "text": text }]
                    }));
                }
            }
        }
    }

    // Turn Normalization: Merge adjacent same-role messages without violating Gemini schema
    let mut normalized_contents: Vec<serde_json::Value> = Vec::new();
    for item in raw_contents {
        let role = item.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let parts = item
            .get("parts")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default();

        if parts.is_empty() {
            continue;
        }

        if let Some(last) = normalized_contents.last_mut() {
            let last_role = last.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if last_role == role {
                if role == "user" {
                    let last_has_fr = last
                        .get("parts")
                        .and_then(|p| p.as_array())
                        .map(|arr| arr.iter().any(|part| part.get("functionResponse").is_some()))
                        .unwrap_or(false);
                    let curr_has_fr = parts.iter().any(|part| part.get("functionResponse").is_some());

                    let last_has_text = last
                        .get("parts")
                        .and_then(|p| p.as_array())
                        .map(|arr| arr.iter().any(|part| part.get("text").is_some()))
                        .unwrap_or(false);
                    let curr_has_text = parts.iter().any(|part| part.get("text").is_some());

                    // Gemini strictly rejects mixing functionResponse and text in the same user turn.
                    if (last_has_fr && curr_has_text) || (last_has_text && curr_has_fr) {
                        // Insert a synthetic model acknowledgment turn to preserve strict user <-> model turn alternation
                        normalized_contents.push(serde_json::json!({
                            "role": "model",
                            "parts": [{ "text": "Tool execution completed." }]
                        }));
                        normalized_contents.push(serde_json::json!({
                            "role": role,
                            "parts": parts
                        }));
                        continue;
                    }

                    // Safe to merge (both are functionResponse or both are text)
                    if let Some(last_parts) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                        last_parts.extend(parts);
                        continue;
                    }
                } else if let Some(last_parts) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                    last_parts.extend(parts);
                    continue;
                }
            }
        }

        normalized_contents.push(serde_json::json!({
            "role": role,
            "parts": parts
        }));
    }

    // Ensure all model turns adhere to Gemini 3+ thoughtSignature rules (ported from 9router)
    for item in &mut normalized_contents {
        let is_model = item.get("role").and_then(|r| r.as_str()) == Some("model");
        if is_model {
            if let Some(parts) = item.get_mut("parts").and_then(|p| p.as_array_mut()) {
                // Strip thought-only parts, keep thoughtSignature on functionCall parts
                parts.retain(|p| {
                    let is_thought = p.get("thought").and_then(|t| t.as_bool()).unwrap_or(false);
                    let has_fc = p.get("functionCall").is_some();
                    let has_text = p
                        .get("text")
                        .and_then(|t| t.as_str())
                        .map(|t| !t.is_empty())
                        .unwrap_or(false);
                    let has_sig =
                        p.get("thoughtSignature").is_some() || p.get("thought_signature").is_some();

                    if is_thought && !has_fc {
                        return false;
                    }
                    if has_sig && !has_fc && !has_text {
                        return false;
                    }
                    true
                });

                let mut first_fc_seen = false;
                for p in parts.iter_mut() {
                    if let Some(fc) = p.get("functionCall") {
                        let call_id = fc.get("id").and_then(|i| i.as_str()).unwrap_or("");
                        let existing_sig = p
                            .get("thoughtSignature")
                            .or_else(|| p.get("thought_signature"))
                            .and_then(|s| s.as_str())
                            .and_then(sanitize_thought_signature);

                        let cached_sig = if !call_id.is_empty() {
                            resolve_call_signature_scoped(call_id, Some(family))
                        } else {
                            None
                        };

                        // Segregated by model family (aligned with 9router specification):
                        // - If an existing or cached signature exists and is compatible with target family, use it.
                        // - If missing: for Claude models on Antigravity, inject DEFAULT_THINKING_CLAUDE_SIGNATURE on first functionCall.
                        // - For Gemini models: do not inject synthetic signatures when none exists to prevent Google API 400 TYPE_BYTES / Corrupted signature errors.
                        // - Sibling function calls remain unsigned unless they explicitly have a cached signature.
                        let call_sig = existing_sig.or(cached_sig).or_else(|| {
                            if !first_fc_seen && family == "claude" {
                                Some(DEFAULT_THINKING_CLAUDE_SIGNATURE.to_string())
                            } else {
                                None
                            }
                        });
                        first_fc_seen = true;

                        if let Some(sig) = call_sig.as_deref().and_then(sanitize_thought_signature)
                        {
                            if let Some(obj) = p.as_object_mut() {
                                obj.insert(
                                    "thoughtSignature".to_string(),
                                    serde_json::Value::String(sig),
                                );
                            }
                        } else if let Some(obj) = p.as_object_mut() {
                            // Strip invalid/corrupted thoughtSignature so Google does not reject request
                            obj.remove("thoughtSignature");
                            obj.remove("thought_signature");
                        }
                    }
                }
            }
        }
    }

    // Ensure the first turn is user
    if normalized_contents.is_empty() {
        normalized_contents.push(serde_json::json!({
            "role": "user",
            "parts": [{ "text": "Hello" }]
        }));
    } else if normalized_contents[0].get("role").and_then(|r| r.as_str()) != Some("user") {
        normalized_contents.insert(
            0,
            serde_json::json!({
                "role": "user",
                "parts": [{ "text": "..." }]
            }),
        );
    }

    // Ensure the last turn is NOT a model turn (Gemini API returns 400 "Requests ending with a model turn are not supported")
    if let Some(last) = normalized_contents.last() {
        if last.get("role").and_then(|r| r.as_str()) == Some("model") {
            normalized_contents.push(serde_json::json!({
                "role": "user",
                "parts": [{ "text": "Please continue and complete the remaining requested actions." }]
            }));
        }
    }

    // Tools conversion
    let mut tools_payload = None;
    let mut tool_config_payload = None;
    if let Some(ref tools) = req.tools {
        let mut declarations = Vec::new();
        for t in tools {
            if let Some(fn_obj) = t.get("function") {
                let name = fn_obj.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let desc = fn_obj
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("");
                let params = fn_obj.get("parameters").unwrap_or(&serde_json::Value::Null);

                declarations.push(serde_json::json!({
                    "name": sanitize_function_name(name),
                    "description": desc,
                    "parameters": clean_parameters_schema(params)
                }));
            } else if let Some(name) = t.get("name").and_then(|n| n.as_str()) {
                // Claude style tool definition
                let desc = t.get("description").and_then(|d| d.as_str()).unwrap_or("");
                let schema = t.get("input_schema").unwrap_or(&serde_json::Value::Null);

                declarations.push(serde_json::json!({
                    "name": sanitize_function_name(name),
                    "description": desc,
                    "parameters": clean_parameters_schema(schema)
                }));
            }
        }

        if !declarations.is_empty() {
            tools_payload = Some(serde_json::json!([{
                "functionDeclarations": declarations
            }]));
            tool_config_payload = Some(serde_json::json!({
                "functionCallingConfig": {
                    "mode": "AUTO"
                }
            }));
        }
    }

    let session_id = generate_pseudo_uuid("ag-v1-sess");
    let conversation_id = generate_pseudo_uuid("ag-v1-conv");
    let trajectory_id = generate_pseudo_uuid("ag-v1-traj");
    let step = (normalized_contents.len() * 2).max(1);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let request_id = format!("agent/{conversation_id}/{now_ms}/{trajectory_id}/{step}");

    let mut gen_config = serde_json::json!({
        "temperature": req.temperature.unwrap_or(0.7),
        "topP": 0.95,
        "topK": 40,
        "maxOutputTokens": req.max_tokens.unwrap_or(64000).min(64000)
    });

    if family == "gemini" {
        let (thinking_level, thinking_budget, include_thoughts) = resolve_gemini_thinking_config(&req.model);
        let mut tc = serde_json::json!({
            "thinkingLevel": thinking_level,
            "includeThoughts": include_thoughts
        });
        if thinking_budget >= 0 {
            tc["thinkingBudget"] = serde_json::json!(thinking_budget);
        } else {
            tc["thinkingBudget"] = serde_json::json!(-1);
        }
        gen_config["thinkingConfig"] = tc;
    }

    let mut inner_request = serde_json::json!({
        "contents": normalized_contents,
        "generationConfig": gen_config,
        "sessionId": session_id
    });

    if !system_prompts.is_empty() {
        let joined_sys = system_prompts.join("\n\n");
        inner_request["systemInstruction"] = serde_json::json!({
            "parts": [{ "text": joined_sys }]
        });
    }

    if let Some(t) = tools_payload {
        inner_request["tools"] = t;
    }
    if let Some(tc) = tool_config_payload {
        inner_request["toolConfig"] = tc;
    }

    serde_json::json!({
        "project": project_id,
        "model": clean_model,
        "userAgent": "antigravity",
        "requestId": request_id,
        "request": inner_request
    })
}

#[allow(clippy::too_many_arguments)]
pub fn execute_v1_stream<F>(
    client: &reqwest::blocking::Client,
    access_token: &str,
    project_id: &str,
    req: &V1ChatRequest,
    on_event: &mut F,
) -> Result<V1Response>
where
    F: FnMut(V1StreamEvent) -> Result<()>,
{
    let endpoint = format!(
        "{}/v1internal:streamGenerateContent?alt=sse",
        ANTIGRAVITY_BASE_URL
    );
    let payload = build_gemini_payload(req, project_id);

    let response = client
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
    let mut full_content = String::new();
    let mut tool_calls = Vec::new();
    let mut function_index = 0;
    let mut finish_reason = "stop".to_string();
    let mut pending_thought_signature: Option<String> = None;
    let target_family = signature_family(&req.model);

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
                        if let Some(f_reason) = cand.get("finishReason").and_then(|r| r.as_str()) {
                            if f_reason == "MAX_TOKENS" {
                                finish_reason = "length".to_string();
                            }
                        }

                        if let Some(parts) =
                            cand.pointer("/content/parts").and_then(|p| p.as_array())
                        {
                            for part in parts {
                                let part_thought_sig = part
                                    .get("thoughtSignature")
                                    .or_else(|| part.get("thought_signature"))
                                    .and_then(|s| s.as_str())
                                    .filter(|s| !s.trim().is_empty())
                                    .map(|s| s.to_string());

                                if let Some(ref sig) = part_thought_sig {
                                    pending_thought_signature = Some(sig.clone());
                                }

                                let is_thought = part
                                    .get("thought")
                                    .and_then(|t| t.as_bool())
                                    .unwrap_or(false);

                                let has_fc = part.get("functionCall").is_some();
                                let has_text = part
                                    .get("text")
                                    .and_then(|t| t.as_str())
                                    .map(|t| !t.is_empty())
                                    .unwrap_or(false);

                                if part_thought_sig.is_some() && !has_fc && !has_text {
                                    continue;
                                }

                                if is_thought {
                                    if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                        if !txt.is_empty() {
                                            on_event(V1StreamEvent::ThoughtDelta(txt.to_string()))?;
                                        }
                                    }
                                    // Keep client connection active during thinking phase
                                    on_event(V1StreamEvent::KeepAlive)?;
                                    continue;
                                }

                                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                    if !text.is_empty() {
                                        full_content.push_str(text);
                                        on_event(V1StreamEvent::ContentDelta(text.to_string()))?;
                                    }
                                }

                                if let Some(fc) = part.get("functionCall") {
                                    let fc_thought_sig = fc
                                        .get("thoughtSignature")
                                        .or_else(|| fc.get("thought_signature"))
                                        .and_then(|s| s.as_str())
                                        .filter(|s| !s.trim().is_empty())
                                        .map(|s| s.to_string());

                                    let sig = fc_thought_sig
                                        .or(part_thought_sig)
                                        .or_else(|| pending_thought_signature.clone());

                                    let name = fc
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let empty_args = serde_json::json!({});
                                    let args_val = fc.get("args").unwrap_or(&empty_args);
                                    let args_str = serde_json::to_string(args_val)
                                        .unwrap_or_else(|_| "{}".to_string());
                                    let base_id = fc
                                        .get("id")
                                        .and_then(|i| i.as_str())
                                        .map(|s| s.to_string())
                                        .unwrap_or_else(|| {
                                            format!(
                                                "call_{}_{}",
                                                generate_pseudo_uuid("fc"),
                                                function_index
                                            )
                                        });

                                    let call_id = if let Some(ref s) = sig {
                                        if let Some(valid_sig) = sanitize_thought_signature(s) {
                                            store_thought_signature_scoped(&base_id, &valid_sig, Some(target_family));
                                            let embedded_id = format!("{base_id}__thought__{valid_sig}");
                                            store_thought_signature_scoped(&embedded_id, &valid_sig, Some(target_family));
                                            embedded_id
                                        } else {
                                            base_id
                                        }
                                    } else {
                                        base_id
                                    };

                                    let tc = V1ToolCall {
                                        id: call_id.clone(),
                                        call_type: "function".to_string(),
                                        function: V1ToolCallFunction {
                                            name: name.clone(),
                                            arguments: args_str.clone(),
                                        },
                                        thought_signature: sig.clone(),
                                        extra_content: None,
                                        provider_specific_fields: None,
                                    };
                                    tool_calls.push(tc);

                                    on_event(V1StreamEvent::ToolCallDelta {
                                        index: function_index,
                                        id: Some(call_id),
                                        name: Some(name),
                                        arguments: args_str,
                                    })?;
                                    function_index += 1;
                                    finish_reason = "tool_calls".to_string();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if !tool_calls.is_empty() {
        finish_reason = "tool_calls".to_string();
    }

    on_event(V1StreamEvent::Finish {
        finish_reason: finish_reason.clone(),
    })?;

    Ok(V1Response {
        content: full_content,
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        finish_reason,
    })
}

#[cfg(test)]
#[path = "v1_adapter_tests.rs"]
mod tests;
