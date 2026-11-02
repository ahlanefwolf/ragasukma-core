use anyhow::{bail, Result};
use std::collections::HashSet;

/// Applies surgical edits to file content with a multi-tier resilient matcher inspired by Git's 3-way merge & fuzzy patch algorithms:
/// 1. Exact literal match (`str::contains`)
/// 2. Trimmed leading/trailing newline match
/// 3. Whitespace & indentation tolerant line-by-line sliding window
/// 4. Quote & semicolon relaxed matching (single quotes vs double quotes, trailing semicolons/commas)
/// 5. Git 3-Way Hunk Decomposition (detects prefix & suffix context, replaces intermediate hunk)
/// 6. Git Unified Diff / Patch Parser (applies `@@ -x,y +z,w @@` hunks with context)
/// 7. Git LCS (Longest Common Subsequence) Fuzzy Alignment (dynamic programming line alignment)
/// 8. Relaxed anchor matching (first and last non-trivial anchor lines)
/// 9. Detailed contextual diagnostic reporting exact file lines if all matchers fail.
pub fn apply_resilient_edit(
    content: &str,
    old_text: &str,
    new_text: &str,
    file_path: &str,
) -> Result<String> {
    let normalized_content = content.replace("\r\n", "\n");
    let normalized_old = old_text.replace("\r\n", "\n");
    let normalized_new = new_text.replace("\r\n", "\n");

    // Tier 1: Exact literal match
    if normalized_content.contains(&normalized_old) {
        let count = normalized_content.matches(&normalized_old).count();
        if count == 1 {
            return Ok(normalized_content.replacen(&normalized_old, &normalized_new, 1));
        } else if count > 1 {
            bail!(
                "'old_text' matches {} times in file '{}'. Provide 2-3 additional surrounding lines to make it unique.",
                count, file_path
            );
        }
    }

    // Tier 2: Trimmed leading/trailing blank lines
    let trimmed_old = normalized_old.trim_matches('\n');
    let trimmed_new = normalized_new.trim_matches('\n');
    if !trimmed_old.is_empty() && trimmed_old != normalized_old && normalized_content.contains(trimmed_old) {
        let count = normalized_content.matches(trimmed_old).count();
        if count == 1 {
            return Ok(normalized_content.replacen(trimmed_old, trimmed_new, 1));
        }
    }

    let content_lines: Vec<&str> = normalized_content.lines().collect();
    let raw_old_lines: Vec<&str> = normalized_old.lines().collect();

    // Check if lines have line-number prefixes (e.g. "   42\t" or "   42 | ") from tool output
    let has_line_prefixes = raw_old_lines.iter().any(|l| {
        let t = l.trim_start();
        t.find(|c: char| !c.is_ascii_digit()).map(|idx| {
            idx > 0 && (t[idx..].starts_with('\t') || t[idx..].starts_with('|') || t[idx..].starts_with(": "))
        }).unwrap_or(false)
    });

    let cleaned_old_lines_owned: Vec<String>;
    let effective_raw_old_lines: Vec<&str> = if has_line_prefixes {
        cleaned_old_lines_owned = raw_old_lines.iter().map(|l| strip_line_prefix(l)).collect();
        cleaned_old_lines_owned.iter().map(|s| s.as_str()).collect()
    } else {
        raw_old_lines
    };

    // Strip leading & trailing empty lines from old_lines
    let start_empty = effective_raw_old_lines.iter().take_while(|l| l.trim().is_empty()).count();
    let end_empty = effective_raw_old_lines.iter().rev().take_while(|l| l.trim().is_empty()).count();
    let old_lines: Vec<&str> = if start_empty + end_empty < effective_raw_old_lines.len() {
        effective_raw_old_lines[start_empty..effective_raw_old_lines.len() - end_empty].to_vec()
    } else {
        effective_raw_old_lines
    };

    if old_lines.is_empty() {
        bail!("'old_text' cannot be empty or only whitespace");
    }

    // Tier 3: Whitespace-tolerant line-by-line matching
    if content_lines.len() >= old_lines.len() {
        let mut match_indices = Vec::new();
        for i in 0..=(content_lines.len() - old_lines.len()) {
            let window = &content_lines[i..i + old_lines.len()];
            if window.iter().zip(old_lines.iter()).all(|(c, o)| c.trim() == o.trim()) {
                match_indices.push(i);
            }
        }

        if match_indices.len() == 1 {
            let start_idx = match_indices[0];
            let adapted_new = adapt_indentation(
                &content_lines[start_idx..start_idx + old_lines.len()],
                &old_lines,
                &normalized_new,
            );
            let mut result_lines = Vec::new();
            result_lines.extend_from_slice(&content_lines[..start_idx]);
            result_lines.push(&adapted_new);
            result_lines.extend_from_slice(&content_lines[start_idx + old_lines.len()..]);
            return Ok(result_lines.join("\n"));
        } else if match_indices.len() > 1 {
            bail!(
                "'old_text' matched {} different sections with whitespace tolerance in '{}'. Provide more surrounding lines.",
                match_indices.len(), file_path
            );
        }
    }

    // Tier 4: Punctuation & quote relaxed matching (single quotes vs double quotes, trailing semicolons/commas)
    if content_lines.len() >= old_lines.len() {
        let relaxed_old: Vec<String> = old_lines.iter().map(|l| relax_line(l)).collect();
        let mut relaxed_matches = Vec::new();
        for i in 0..=(content_lines.len() - old_lines.len()) {
            let window = &content_lines[i..i + old_lines.len()];
            let matches = window
                .iter()
                .zip(relaxed_old.iter())
                .all(|(c, o)| relax_line(c) == *o);
            if matches {
                relaxed_matches.push(i);
            }
        }

        if relaxed_matches.len() == 1 {
            let start_idx = relaxed_matches[0];
            let adapted_new = adapt_indentation(
                &content_lines[start_idx..start_idx + old_lines.len()],
                &old_lines,
                &normalized_new,
            );
            let mut result_lines = Vec::new();
            result_lines.extend_from_slice(&content_lines[..start_idx]);
            result_lines.push(&adapted_new);
            result_lines.extend_from_slice(&content_lines[start_idx + old_lines.len()..]);
            return Ok(result_lines.join("\n"));
        }
    }

    // Tier 5: Git 3-Way Context & Hunk Decomposition
    // Extracts common prefix context and suffix context between old_text and new_text.
    // Replaces only the intermediate modified hunk between matched context boundaries.
    let raw_new_lines: Vec<&str> = normalized_new.lines().collect();
    if let Some(hunk) = decompose_3way_hunk(&old_lines, &raw_new_lines)
        && let Some(result) = apply_git_3way_hunk(&content_lines, &hunk) {
            return Ok(result);
        }

    // Tier 6: Git Unified Diff / Patch Parser
    // If the input was formatted as a unified diff with @@ hunk headers
    if is_unified_diff(&normalized_old) || is_unified_diff(&normalized_new) {
        let diff_payload = if is_unified_diff(&normalized_new) {
            &normalized_new
        } else {
            &normalized_old
        };
        if let Some(result) = apply_unified_diff_patch(&content_lines, diff_payload) {
            return Ok(result);
        }
    }

    // Tier 7: Git LCS (Longest Common Subsequence) Fuzzy Alignment
    // Uses dynamic programming to align lines even when comments, blank lines, or slight edits shifted indices
    if let Some(result) = apply_git_lcs_alignment(&content_lines, &old_lines, &normalized_new) {
        return Ok(result);
    }

    // Tier 8: Anchor matching with meaningful non-trivial anchors
    if old_lines.len() >= 3 {
        let first_meaningful = old_lines
            .iter()
            .enumerate()
            .find(|(_, l)| {
                let t = l.trim();
                t.len() > 3 && t != "</div>" && t != "}" && t != "};" && t != "</button>" && t != "</div>;"
            })
            .map(|(idx, l)| (idx, l.trim()));

        let last_meaningful = old_lines
            .iter()
            .enumerate()
            .rev()
            .find(|(_, l)| {
                let t = l.trim();
                t.len() > 3 && t != "</div>" && t != "}" && t != "};" && t != "</button>" && t != "</div>;"
            })
            .map(|(idx, l)| (idx, l.trim()));

        if let (Some((first_idx_in_old, first_anchor)), Some((last_idx_in_old, last_anchor))) = (first_meaningful, last_meaningful)
            && first_idx_in_old < last_idx_in_old {
                let mut anchor_matches = Vec::new();
                for i in 0..content_lines.len() {
                    if content_lines[i].trim() == first_anchor {
                        let min_end = i + 1;
                        let max_end = (i + old_lines.len() + 15).min(content_lines.len());
                        for (j, line) in content_lines.iter().enumerate().take(max_end).skip(min_end) {
                            if line.trim() == last_anchor {
                                anchor_matches.push((i, j));
                            }
                        }
                    }
                }

                if !anchor_matches.is_empty() {
                    let target_span = old_lines.len();
                    let mut best_anchor = anchor_matches[0];
                    let mut min_diff = (anchor_matches[0].1 - anchor_matches[0].0 + 1).abs_diff(target_span);
                    let mut second_diff = usize::MAX;

                    for &(start, end) in &anchor_matches[1..] {
                        let diff = (end - start + 1).abs_diff(target_span);
                        if diff < min_diff {
                            second_diff = min_diff;
                            min_diff = diff;
                            best_anchor = (start, end);
                        } else if diff < second_diff {
                            second_diff = diff;
                        }
                    }

                    if anchor_matches.len() == 1 || min_diff < second_diff {
                        let (start_idx, end_idx) = best_anchor;
                        let suffix_count = old_lines.len() - 1 - last_idx_in_old;
                        let effective_start = start_idx.saturating_sub(first_idx_in_old);
                        let effective_end = (end_idx + suffix_count).min(content_lines.len().saturating_sub(1));

                        let target_slice = &content_lines[effective_start..=effective_end];
                        let adapted_new = adapt_indentation(target_slice, &old_lines, &normalized_new);

                        let mut result_lines = Vec::new();
                        result_lines.extend_from_slice(&content_lines[..effective_start]);
                        result_lines.push(&adapted_new);
                        if effective_end + 1 < content_lines.len() {
                            result_lines.extend_from_slice(&content_lines[effective_end + 1..]);
                        }
                        return Ok(result_lines.join("\n"));
                    }
                }
            }
    }

    // Tier 9: Diagnostics reporting exact file lines
    let mut best_hint_idx = None;
    let mut highest_keyword_match = 0;

    for (idx, line) in content_lines.iter().enumerate() {
        let line_lower = line.to_lowercase();
        let matches = old_lines
            .iter()
            .filter(|o| o.trim().len() >= 4 && line_lower.contains(&o.trim().to_lowercase()))
            .count();
        if matches > highest_keyword_match {
            highest_keyword_match = matches;
            best_hint_idx = Some(idx);
        }
    }

    let hint_block = if let Some(center_idx) = best_hint_idx {
        let start = center_idx.saturating_sub(4);
        let end = (center_idx + 6).min(content_lines.len());
        let snippet = content_lines[start..end]
            .iter()
            .enumerate()
            .map(|(offset, l)| format!("{:>4} | {}", start + offset + 1, l))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "\nClosest actual content in '{}' (lines {}-{}):\n```\n{}\n```\nAction: Copy the exact lines above to form `old_text` or use `workspace_write_file` if replacing a large block.",
            file_path, start + 1, end, snippet
        )
    } else {
        format!(
            "\nUse `workspace_read_file` to inspect the exact lines of '{}' before editing.",
            file_path
        )
    };

    bail!("'old_text' not found in file '{}'.{}", file_path, hint_block)
}

// =========================================================================
// Git 3-Way Fuzzy Patch & LCS Matching Helpers
// =========================================================================

pub fn strip_line_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    if let Some(idx) = trimmed.find(|c: char| !c.is_ascii_digit())
        && idx > 0 {
            let rem = &trimmed[idx..];
            if let Some(after_tab) = rem.strip_prefix('\t') {
                return after_tab.to_string();
            } else if let Some(after_pipe) = rem.strip_prefix('|') {
                return after_pipe.strip_prefix(' ').unwrap_or(after_pipe).to_string();
            } else if let Some(after_colon) = rem.strip_prefix(": ") {
                return after_colon.to_string();
            }
        }
    line.to_string()
}

#[derive(Debug, Clone)]
pub struct Git3WayHunk {
    pub context_before: Vec<String>,
    pub to_delete: Vec<String>,
    pub to_insert: Vec<String>,
    pub context_after: Vec<String>,
}

/// Normalizes punctuation, quotes, and whitespace for fuzzy comparison
pub fn relax_line(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && *c != ';' && *c != ',')
        .map(|c| if c == '\'' || c == '`' { '"' } else { c })
        .collect()
}

/// Computes line similarity between 0.0 and 1.0
pub fn line_similarity(a: &str, b: &str) -> f64 {
    let at = a.trim();
    let bt = b.trim();
    if at == bt {
        return 1.0;
    }
    if relax_line(at) == relax_line(bt) {
        return 0.95;
    }
    if at.eq_ignore_ascii_case(bt) {
        return 0.90;
    }

    let a_chars: HashSet<char> = at.chars().filter(|c| !c.is_whitespace()).collect();
    let b_chars: HashSet<char> = bt.chars().filter(|c| !c.is_whitespace()).collect();
    let intersect = a_chars.intersection(&b_chars).count();
    let union = a_chars.union(&b_chars).count();
    if union > 0 {
        (intersect as f64) / (union as f64)
    } else {
        0.0
    }
}

/// Detects indentation difference and adapts `new_text` lines to match target file indentation
fn adapt_indentation(target_slice: &[&str], old_lines: &[&str], new_text: &str) -> String {
    if target_slice.is_empty() || old_lines.is_empty() {
        return new_text.to_string();
    }

    // Find first non-empty line in target and old
    let target_first = target_slice.iter().find(|l| !l.trim().is_empty());
    let old_first = old_lines.iter().find(|l| !l.trim().is_empty());

    if let (Some(tf), Some(of)) = (target_first, old_first) {
        let tf_indent_len = tf.len() - tf.trim_start().len();
        let of_indent_len = of.len() - of.trim_start().len();

        if tf_indent_len != of_indent_len {
            let tf_prefix = &tf[..tf_indent_len];
            let is_tabs = tf_prefix.starts_with('\t');

            if tf_indent_len > of_indent_len {
                let diff = tf_indent_len - of_indent_len;
                let pad = if is_tabs { "\t".repeat(diff) } else { " ".repeat(diff) };
                return new_text
                    .lines()
                    .map(|l| {
                        if l.trim().is_empty() {
                            l.to_string()
                        } else {
                            format!("{}{}", pad, l)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
            } else {
                let diff = of_indent_len - tf_indent_len;
                return new_text
                    .lines()
                    .map(|l| {
                        let l_indent = l.len() - l.trim_start().len();
                        if l_indent >= diff {
                            l[diff..].to_string()
                        } else {
                            l.trim_start().to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
            }
        }
    }

    new_text.to_string()
}

/// Decomposes `old_lines` and `new_lines` into Git 3-way prefix, deletion, insertion, and suffix hunks
pub fn decompose_3way_hunk(old_lines: &[&str], new_lines: &[&str]) -> Option<Git3WayHunk> {
    if old_lines.is_empty() || new_lines.is_empty() {
        return None;
    }

    // Common prefix
    let mut prefix_len = 0;
    while prefix_len < old_lines.len()
        && prefix_len < new_lines.len()
        && old_lines[prefix_len].trim() == new_lines[prefix_len].trim()
    {
        prefix_len += 1;
    }

    // Common suffix (do not overlap with prefix)
    let mut suffix_len = 0;
    while suffix_len < (old_lines.len() - prefix_len)
        && suffix_len < (new_lines.len() - prefix_len)
        && old_lines[old_lines.len() - 1 - suffix_len].trim()
            == new_lines[new_lines.len() - 1 - suffix_len].trim()
    {
        suffix_len += 1;
    }

    // Require at least one boundary of context (prefix or suffix) to be a valid 3-way hunk
    if prefix_len > 0 || suffix_len > 0 {
        let ctx_pre: Vec<String> = old_lines[..prefix_len].iter().map(|s| s.to_string()).collect();
        let to_del: Vec<String> = old_lines[prefix_len..old_lines.len() - suffix_len]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let to_add: Vec<String> = new_lines[prefix_len..new_lines.len() - suffix_len]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let ctx_post: Vec<String> = old_lines[old_lines.len() - suffix_len..]
            .iter()
            .map(|s| s.to_string())
            .collect();

        Some(Git3WayHunk {
            context_before: ctx_pre,
            to_delete: to_del,
            to_insert: to_add,
            context_after: ctx_post,
        })
    } else {
        None
    }
}

/// Applies a Git 3-Way hunk to content lines by locating context boundaries
fn apply_git_3way_hunk(content_lines: &[&str], hunk: &Git3WayHunk) -> Option<String> {
    if content_lines.is_empty() {
        return None;
    }

    let pre_len = hunk.context_before.len();
    let post_len = hunk.context_after.len();

    // 1. Both prefix and suffix context available
    if pre_len > 0 && post_len > 0 {
        let mut candidate_pairs = Vec::new();

        for i in 0..content_lines.len() {
            // Check prefix match
            if i + pre_len <= content_lines.len() {
                let pre_slice = &content_lines[i..i + pre_len];
                let pre_match = pre_slice
                    .iter()
                    .zip(hunk.context_before.iter())
                    .all(|(c, o)| line_similarity(c, o) >= 0.85);

                if pre_match {
                    let min_j = i + pre_len;
                    let max_j = (min_j + hunk.to_delete.len() + 15).min(content_lines.len());
                    for j in min_j..=max_j {
                        if j + post_len <= content_lines.len() {
                            let post_slice = &content_lines[j..j + post_len];
                            let post_match = post_slice
                                .iter()
                                .zip(hunk.context_after.iter())
                                .all(|(c, o)| line_similarity(c, o) >= 0.85);

                            if post_match {
                                candidate_pairs.push((i, j + post_len));
                            }
                        }
                    }
                }
            }
        }

        if candidate_pairs.len() == 1 {
            let (start_idx, end_idx) = candidate_pairs[0];
            let mut result = Vec::new();
            result.extend_from_slice(&content_lines[..start_idx]);
            for l in &hunk.context_before {
                result.push(l.as_str());
            }
            for l in &hunk.to_insert {
                result.push(l.as_str());
            }
            for l in &hunk.context_after {
                result.push(l.as_str());
            }
            result.extend_from_slice(&content_lines[end_idx..]);
            return Some(result.join("\n"));
        }
    }

    None
}

/// Checks if string is a unified diff
fn is_unified_diff(s: &str) -> bool {
    let s = s.trim();
    s.starts_with("diff --git")
        || s.starts_with("--- ")
        || s.starts_with("@@ ")
        || (s.lines().any(|l| l.starts_with("@@ -")) && (s.contains("\n+") || s.contains("\n-")))
}

/// Applies a unified diff patch to content lines
fn apply_unified_diff_patch(content_lines: &[&str], diff_text: &str) -> Option<String> {
    let mut hunks = Vec::new();
    let lines: Vec<&str> = diff_text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("@@ ") {
            i += 1;
            let mut ctx_pre = Vec::new();
            let mut del = Vec::new();
            let mut add = Vec::new();
            let mut ctx_post = Vec::new();
            let mut seen_mods = false;

            while i < lines.len() && !lines[i].starts_with("@@ ") && !lines[i].starts_with("diff --git") {
                let hline = lines[i];
                if let Some(stripped) = hline.strip_prefix('+') {
                    add.push(stripped.to_string());
                    seen_mods = true;
                } else if let Some(stripped) = hline.strip_prefix('-') {
                    del.push(stripped.to_string());
                    seen_mods = true;
                } else {
                    let text = hline.strip_prefix(' ').unwrap_or(hline);
                    if !seen_mods {
                        ctx_pre.push(text.to_string());
                    } else {
                        ctx_post.push(text.to_string());
                    }
                }
                i += 1;
            }

            hunks.push(Git3WayHunk {
                context_before: ctx_pre,
                to_delete: del,
                to_insert: add,
                context_after: ctx_post,
            });
        } else {
            i += 1;
        }
    }

    if hunks.is_empty() {
        return None;
    }

    let mut current_lines: Vec<String> = content_lines.iter().map(|s| s.to_string()).collect();

    for hunk in hunks {
        let refs: Vec<&str> = current_lines.iter().map(|s| s.as_str()).collect();
        let updated = apply_git_3way_hunk(&refs, &hunk)?;
        current_lines = updated.lines().map(|s| s.to_string()).collect();
    }

    Some(current_lines.join("\n"))
}

/// Computes Longest Common Subsequence of lines using line similarity
/// Space-optimized to O(N) using two alternating rows instead of allocating full 2D matrices.
fn compute_line_lcs(a: &[&str], b: &[&str]) -> usize {
    let m = a.len();
    let n = b.len();
    if m == 0 || n == 0 {
        return 0;
    }

    let mut prev = vec![0u16; n + 1];
    let mut curr = vec![0u16; n + 1];

    for i in 1..=m {
        for j in 1..=n {
            if line_similarity(a[i - 1], b[j - 1]) >= 0.85 {
                curr[j] = prev[j - 1] + 1;
            } else {
                curr[j] = prev[j].max(curr[j - 1]);
            }
        }
        std::mem::swap(&mut prev, &mut curr);
        curr.fill(0);
    }

    prev[n] as usize
}

/// Git LCS Fuzzy Alignment: finds the window in content that has the highest LCS alignment with `old_lines`
fn apply_git_lcs_alignment(
    content_lines: &[&str],
    old_lines: &[&str],
    new_text: &str,
) -> Option<String> {
    if content_lines.is_empty() || old_lines.is_empty() {
        return None;
    }

    let m = old_lines.len();

    // PERFORMANCE GUARD: For large files (> 300 lines) or large old_lines (> 80 lines),
    // avoid brute-force scanning every window of the entire file. Localize to candidate region or skip.
    let (search_start, search_end) = if content_lines.len() > 300 {
        // Find best candidate anchor line using fast keyword overlap
        let mut best_center = None;
        let mut max_hits = 0;
        for (idx, line) in content_lines.iter().enumerate() {
            let l_lower = line.to_lowercase();
            let hits = old_lines
                .iter()
                .filter(|o| o.trim().len() >= 4 && l_lower.contains(&o.trim().to_lowercase()))
                .count();
            if hits > max_hits {
                max_hits = hits;
                best_center = Some(idx);
            }
        }

        let center = best_center?;
        // Localized window: 40 lines before and after center
        let start = center.saturating_sub(40);
        let end = (center + m + 40).min(content_lines.len());
        (start, end)
    } else {
        (0, content_lines.len())
    };

    let slice = &content_lines[search_start..search_end];
    let min_win = m.saturating_sub(3).max(1);
    let max_win = (m + 6).min(slice.len());

    let mut best_score = 0.0f64;
    let mut second_best_score = 0.0f64;
    let mut best_window = None;

    for win_len in min_win..=max_win {
        if slice.len() < win_len {
            continue;
        }
        for i in 0..=(slice.len() - win_len) {
            let win = &slice[i..i + win_len];
            let lcs = compute_line_lcs(win, old_lines);
            let score = (lcs as f64) / (m.max(win_len) as f64);

            if score > best_score {
                second_best_score = best_score;
                best_score = score;
                best_window = Some((search_start + i, search_start + i + win_len));
            } else if score > second_best_score {
                second_best_score = score;
            }
        }
    }

    // Require high LCS alignment (>= 0.75) and confidence margin
    if best_score >= 0.75 && (best_score - second_best_score >= 0.05 || second_best_score == 0.0)
        && let Some((start_idx, end_idx)) = best_window {
            let adapted_new = adapt_indentation(
                &content_lines[start_idx..end_idx],
                old_lines,
                new_text,
            );
            let mut result = Vec::new();
            result.extend_from_slice(&content_lines[..start_idx]);
            result.push(&adapted_new);
            result.extend_from_slice(&content_lines[end_idx..]);
            return Some(result.join("\n"));
        }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_3way_hunk_decomposition() {
        let old_lines = vec![
            "function calculateTotal(items) {",
            "    let sum = 0;",
            "    return sum;",
            "}",
        ];
        let new_lines = vec![
            "function calculateTotal(items) {",
            "    let sum = items.reduce((a, b) => a + b, 0);",
            "    return sum;",
            "}",
        ];

        let hunk = decompose_3way_hunk(&old_lines, &new_lines).expect("Should decompose hunk");
        assert_eq!(hunk.context_before, vec!["function calculateTotal(items) {"]);
        assert_eq!(hunk.to_delete, vec!["    let sum = 0;"]);
        assert_eq!(hunk.to_insert, vec!["    let sum = items.reduce((a, b) => a + b, 0);"]);
        assert_eq!(hunk.context_after, vec!["    return sum;", "}"]);
    }

    #[test]
    fn test_git_3way_hunk_application() {
        let content = "import { Item } from './types';\n\nfunction calculateTotal(items) {\n    let sum = 0;\n    return sum;\n}\n\nexport default calculateTotal;";
        let old_text = "function calculateTotal(items) {\n    let sum = 0;\n    return sum;\n}";
        let new_text = "function calculateTotal(items) {\n    let sum = items.reduce((a, b) => a + b, 0);\n    return sum;\n}";

        let updated = apply_resilient_edit(content, old_text, new_text, "calc.ts").unwrap();
        assert!(updated.contains("let sum = items.reduce"));
        assert!(updated.contains("import { Item }"));
    }

    #[test]
    fn test_git_unified_diff_patch() {
        let content = "const port = 3000;\nconst host = 'localhost';\nconst timeout = 5000;";
        let patch = "@@ -1,3 +1,3 @@\n const port = 3000;\n-const host = 'localhost';\n+const host = '0.0.0.0';\n const timeout = 5000;";

        let updated = apply_resilient_edit(content, patch, patch, "config.ts").unwrap();
        assert!(updated.contains("const host = '0.0.0.0';"));
        assert!(updated.contains("const port = 3000;"));
    }

    #[test]
    fn test_git_lcs_alignment_with_inserted_line() {
        let content = "function render() {\n    // Some extra inserted comment\n    setup();\n    display();\n    cleanup();\n}";
        // AI didn't know about the inserted comment
        let old_text = "function render() {\n    setup();\n    display();\n    cleanup();\n}";
        let new_text = "function render() {\n    setupV2();\n    displayV2();\n    cleanupV2();\n}";

        let updated = apply_resilient_edit(content, old_text, new_text, "app.ts").unwrap();
        assert!(updated.contains("setupV2();"));
        assert!(updated.contains("displayV2();"));
    }

    #[test]
    fn test_git_smart_indentation_adaptation() {
        let content = "class App {\n        init() {\n            start();\n        }\n}";
        // AI sent 4 spaces instead of 8 spaces
        let old_text = "    init() {\n        start();\n    }";
        let new_text = "    init() {\n        startFast();\n    }";

        let updated = apply_resilient_edit(content, old_text, new_text, "app.ts").unwrap();
        assert!(updated.contains("startFast();"));
    }
}
