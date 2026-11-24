//! Request payload construction for the Antigravity v1 API.

use super::model_config::{resolve_gemini_thinking_config, resolve_model_alias};
use super::signature::{
    resolve_call_signature_scoped, sanitize_thought_signature, signature_family,
    store_thought_signature_scoped,
};
use crate::llm::providers::antigravity::constants::*;
use crate::llm::v1_engine::{V1ChatRequest, extract_text_content};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

fn sanitize_schema_recursive(val: &mut serde_json::Value) {
    let Some(map) = val.as_object_mut() else {
        return;
    };

    // 1. Convert const to enum
    if let Some(c) = map.remove("const")
        && !map.contains_key("enum")
    {
        map.insert("enum".to_string(), serde_json::json!([c]));
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
    if let Some(all_of) = map.remove("allOf")
        && let Some(items) = all_of.as_array()
    {
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

    // 4. Flatten anyOf / oneOf
    let any_of_candidate = map.remove("anyOf").or_else(|| map.remove("oneOf"));
    if let Some(union_val) = any_of_candidate
        && let Some(variants) = union_val.as_array()
    {
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

        if let Some(selected) = best_variant
            && let Some(sel_map) = selected.as_object()
        {
            for (k, v) in sel_map {
                if !map.contains_key(k) {
                    map.insert(k.clone(), v.clone());
                }
            }
        }
    }

    // 5. Flatten type array: e.g. ["string", "null"] -> "string" + nullable: true
    if let Some(type_val) = map.get("type").cloned()
        && let Some(type_arr) = type_val.as_array()
    {
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
        if let Some(req_val) = map.get_mut("required")
            && let Some(req_arr) = req_val.as_array_mut()
        {
            req_arr.retain(|r| r.as_str().map(|k| valid_keys.contains(k)).unwrap_or(false));
            if req_arr.is_empty() {
                remove_required = true;
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

/// Greatest common divisor, used to reduce an image size to an aspect ratio.
fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Deterministic v4-shaped UUID derived from a seed.
///
/// Antigravity fingerprints a trajectory from `sessionId` / `conversationId`, so every
/// turn of the same conversation must map to the same ids. A random id per request makes
/// the backend treat each turn as a brand new trajectory.
pub fn uuid_from_seed(seed: &str) -> String {
    let mut bytes = [0u8; 16];
    for (i, chunk) in bytes.chunks_mut(4).enumerate() {
        // FNV-1a, salted per word.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        h ^= i as u64;
        for b in seed.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // final avalanche so neighbouring seeds do not share a prefix
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        chunk.copy_from_slice(&(h as u32).to_be_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 1

    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Image-generation models need an explicit `imageConfig` instead of a text response.
pub fn is_image_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("image") || m.contains("imagen")
}

/// Parses the `-<w>x<h>` suffix of an image model name into an aspect ratio.
///
/// Falls back to `1:1` when the model name carries no size hint.
pub fn parse_image_config(model: &str) -> serde_json::Value {
    let lower = model.to_ascii_lowercase();
    let aspect = lower
        .rsplit_once('-')
        .and_then(|(_, tail)| {
            let (w, h) = tail.split_once('x')?;
            let w: u32 = w.parse().ok()?;
            let h: u32 = h.parse().ok()?;
            if w == 0 || h == 0 {
                return None;
            }
            let g = gcd(w, h);
            Some(format!("{}:{}", w / g, h / g))
        })
        .unwrap_or_else(|| "1:1".to_string());

    serde_json::json!({ "aspectRatio": aspect })
}

/// Antigravity tool-name sanitiser.
///
/// This deliberately delegates to the engine's `sanitize_function_name` rather
/// than keeping a provider-local copy: the engine already allows `.`, `:` and
/// `-`, already prefixes a leading non-letter with `_`, and already truncates to
/// 64 chars — exactly the rules Antigravity needs. A duplicate here would drift
/// the moment the engine's rules change, so the alias exists only to document
/// the provider-specific *intent* at the call site.
pub fn sanitize_ag_function_name(name: &str) -> String {
    crate::llm::v1_engine::sanitize_function_name(name)
}

pub fn supports_long_decode(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("gemini-4")
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
                let fn_name = sanitize_ag_function_name(&tc.function.name);
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
                    system_prompts.push(text.trim().to_string());
                }
            }
            "assistant" | "model" => {
                let mut parts = Vec::new();
                if !text.trim().is_empty() {
                    parts.push(serde_json::json!({ "text": text }));
                }
                if let Some(ref tcs) = msg.tool_calls {
                    for tc in tcs {
                        let fn_name = sanitize_ag_function_name(&tc.function.name);
                        let args_val: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments)
                                .unwrap_or_else(|_| serde_json::json!({}));

                        let clean_tc_id = tc
                            .id
                            .split_once("__thought__")
                            .map(|(b, _)| b)
                            .unwrap_or(&tc.id);

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

                        let cached_sig = direct_sig
                            .or_else(|| resolve_call_signature_scoped(&tc.id, Some(family)));
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
                let clean_tc_id = tc_id
                    .split_once("__thought__")
                    .map(|(b, _)| b)
                    .unwrap_or(tc_id);
                let fn_name = tool_call_names
                    .get(tc_id)
                    .or_else(|| tool_call_names.get(clean_tc_id))
                    .cloned()
                    .or_else(|| msg.name.clone().map(|n| sanitize_ag_function_name(&n)))
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
                let mut parts: Vec<serde_json::Value> = Vec::new();
                if !text.trim().is_empty() {
                    parts.push(serde_json::json!({ "text": text }));
                }
                if let Some(serde_json::Value::Array(arr)) = &msg.content {
                    for item in arr {
                        if item.get("type").and_then(|t| t.as_str()) == Some("image_url")
                            && let Some(url) = item
                                .get("image_url")
                                .and_then(|iu| iu.get("url"))
                                .and_then(|u| u.as_str())
                            && let Some(rest) = url.strip_prefix("data:")
                            && let Some((meta, b64)) = rest.split_once(";base64,")
                            && !b64.is_empty()
                        {
                            parts.push(serde_json::json!({
                                "inlineData": {
                                    "mimeType": meta,
                                    "data": b64
                                }
                            }));
                        }
                    }
                }
                if !parts.is_empty() {
                    raw_contents.push(serde_json::json!({
                        "role": "user",
                        "parts": parts
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
                        .map(|arr| {
                            arr.iter()
                                .any(|part| part.get("functionResponse").is_some())
                        })
                        .unwrap_or(false);
                    let curr_has_fr = parts
                        .iter()
                        .any(|part| part.get("functionResponse").is_some());

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
                } else if let Some(last_parts) =
                    last.get_mut("parts").and_then(|p| p.as_array_mut())
                {
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
        if is_model && let Some(parts) = item.get_mut("parts").and_then(|p| p.as_array_mut()) {
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

                    if let Some(sig) = call_sig.as_deref().and_then(sanitize_thought_signature) {
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
    if let Some(last) = normalized_contents.last()
        && last.get("role").and_then(|r| r.as_str()) == Some("model")
    {
        normalized_contents.push(serde_json::json!({
            "role": "user",
            "parts": [{ "text": "Please continue and complete the remaining requested actions." }]
        }));
    }

    // ─── Tools conversion ───
    //
    // 9router's `cloakTools` (the `_ide` suffix + Antigravity decoy tools) is DEAD
    // CODE upstream — its only call site is commented out (`// Antigravity cloaking
    // disabled`). The path that actually runs is the one below: dedupe by sanitized
    // name, merge every group into a single `functionDeclarations`, and fall back to
    // a `reason` parameter when a tool ships no schema. Porting the disabled version
    // would have renamed tools the caller never gets back, so it is not 1:1.
    let mut tools_payload = None;
    let mut tool_config_payload = None;

    if let Some(ref tools) = req.tools {
        let mut declarations: Vec<serde_json::Value> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for t in tools {
            let (raw_name, desc, params) = if let Some(fn_obj) = t.get("function") {
                (
                    fn_obj.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                    fn_obj
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or(""),
                    fn_obj.get("parameters"),
                )
            } else if let Some(name) = t.get("name").and_then(|n| n.as_str()) {
                // Claude style tool definition
                (
                    name,
                    t.get("description").and_then(|d| d.as_str()).unwrap_or(""),
                    t.get("input_schema"),
                )
            } else {
                continue;
            };

            let name = sanitize_ag_function_name(raw_name);
            if name.is_empty() || !seen.insert(name.clone()) {
                continue;
            }

            // Gemini rejects a missing schema, so absent parameters become a
            // single required `reason` field (1:1 with 9router).
            let parameters = match params {
                Some(p) if !p.is_null() => clean_parameters_schema(p),
                _ => serde_json::json!({
                    "type": "object",
                    "properties": {
                        "reason": {
                            "type": "string",
                            "description": "Brief explanation"
                        }
                    },
                    "required": ["reason"]
                }),
            };

            declarations.push(serde_json::json!({
                "name": name,
                "description": desc,
                "parameters": parameters
            }));
        }

        if !declarations.is_empty() {
            tools_payload = Some(serde_json::json!([{
                "functionDeclarations": declarations
            }]));
            // `VALIDATED` (not `AUTO`) matches what the Antigravity IDE sends.
            tool_config_payload = Some(serde_json::json!({
                "functionCallingConfig": {
                    "mode": "VALIDATED"
                }
            }));
        }
    }

    // ─── Deterministic session / conversation / trajectory ids (1:1 with `buildIdeRequestId`) ───
    //
    // A random id per request makes the backend treat every turn as a brand new
    // trajectory. Deriving them from the conversation seed keeps the session stable.
    let seed = req
        .messages
        .first()
        .map(|m| {
            crate::llm::v1_engine::extract_text_content(&m.content)
                .chars()
                .take(64)
                .collect::<String>()
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "anonymous".to_string());

    let session_id = uuid_from_seed(&format!("antigravity:session:{seed}"));
    let conversation_id = uuid_from_seed(&format!("antigravity:conversation:{seed}"));
    let trajectory_id = uuid_from_seed(&format!(
        "antigravity:trajectory:{seed}:{clean_model}:agent"
    ));
    // `Math.max(1, contentCount * 2 - 1)` in 9router.
    let step = (normalized_contents.len() * 2).saturating_sub(1).max(1);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let request_id = format!("agent/{conversation_id}/{now_ms}/{trajectory_id}/{step}");

    // Long-decode models keep the caller's budget up to the 1M ceiling; every other
    // model is clamped to the plain 64K cap (1:1 with 9router `MAX_ANTIGRAVITY_OUTPUT_TOKENS`).
    let output_cap = if supports_long_decode(&clean_model) {
        GEMINI_LONG_DECODE_MAX_OUTPUT
    } else {
        MAX_ANTIGRAVITY_OUTPUT_TOKENS
    };
    let max_output = req.max_tokens.unwrap_or(output_cap).min(output_cap);

    let mut gen_config = serde_json::json!({
        "temperature": req.temperature.unwrap_or(0.7),
        "topP": 0.95,
        "topK": 40,
        "maxOutputTokens": max_output
    });

    if family == "gemini" {
        let (thinking_level, thinking_budget, include_thoughts) =
            resolve_gemini_thinking_config(&req.model);
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

    // Official Antigravity identity is always sent ahead of the caller's system prompt
    // (1:1 with 9router, which prepends ANTIGRAVITY_DEFAULT_SYSTEM to every request).
    let mut sys_parts: Vec<String> = vec![ANTIGRAVITY_DEFAULT_SYSTEM.to_string()];
    sys_parts.extend(system_prompts.into_iter().filter(|s| !s.trim().is_empty()));
    let joined_sys = sys_parts.join("\n\n").trim().to_string();
    if !joined_sys.is_empty() {
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
