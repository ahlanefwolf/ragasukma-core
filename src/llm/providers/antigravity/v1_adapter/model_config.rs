//! Model alias resolution and Gemini thinking-config derivation.

pub fn resolve_model_alias(input: &str) -> String {
    let clean = input.strip_prefix("ag/").unwrap_or(input).trim();
    if clean.is_empty() {
        return "gemini-3.8-flash-high".to_string();
    }
    // Strip trailing parenthesized suffix e.g. "model(low)" -> "model"
    if let Some(idx) = clean.rfind('(')
        && clean.ends_with(')')
    {
        let base = clean[..idx].trim();
        if !base.is_empty() {
            return base.to_string();
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

