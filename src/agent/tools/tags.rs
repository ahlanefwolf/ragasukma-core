use std::fs;
use std::sync::OnceLock;
use super::fs::{resolve_allowed_path, to_workspace_folder_path};

/// Automatically parse `@file` or `@file:start-end` tags in user messages
/// and append the referenced file snippet lines directly to the turn prompt.
pub fn expand_file_tags(text: &str, allowed_roots: &[String]) -> String {
    if !text.contains('@') || allowed_roots.is_empty() {
        return text.to_string();
    }

    // Match @filepath or @filepath:start or @filepath:start-end (cached regex)
    static FILE_TAG_RE: OnceLock<Option<regex::Regex>> = OnceLock::new();
    let re_opt = FILE_TAG_RE.get_or_init(|| {
        regex::Regex::new(r#"@([a-zA-Z0-9_\-\./\\]+)(?::(\d+)(?:-(\d+))?)?"#).ok()
    });

    let re = match re_opt.as_ref() {
        Some(r) => r,
        None => return text.to_string(),
    };

    let mut attached_snippets: Vec<String> = Vec::new();
    let mut seen_keys: std::collections::HashSet<String> = std::collections::HashSet::new();

    for caps in re.captures_iter(text) {
        let raw_file = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if raw_file.is_empty() {
            continue;
        }

        // Try to resolve in allowed roots
        let resolved = match resolve_allowed_path(raw_file, allowed_roots) {
            Ok(p) => p,
            Err(_) => continue,
        };

        if !resolved.exists() {
            continue;
        }

        // If it's a directory tag (@folder), attach directory listing context
        if resolved.is_dir() {
            let snippet_key = format!("dir:{}", raw_file);
            if seen_keys.contains(&snippet_key) {
                continue;
            }
            seen_keys.insert(snippet_key);

            let mut dir_files: Vec<String> = Vec::new();
            let mut builder = ignore::WalkBuilder::new(&resolved);
            builder
                .max_depth(Some(1))
                .hidden(false)
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                .parents(true);

            let mut overrides = ignore::overrides::OverrideBuilder::new(&resolved);
            let _ = overrides.add("!.git");
            let _ = overrides.add("!.git/**");
            let _ = overrides.add("!.DS_Store");
            let _ = overrides.add("!Thumbs.db");
            if let Ok(ov) = overrides.build() {
                builder.overrides(ov);
            }

            for entry in builder.build().flatten() {
                if entry.depth() == 0 {
                    continue;
                }
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname == ".git" || fname == ".DS_Store" || fname == "Thumbs.db" {
                    continue;
                }
                let is_d = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_d {
                    dir_files.push(format!("  [dir]  {}/", fname));
                } else {
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    dir_files.push(format!("  [file] {} ({} bytes)", fname, size));
                }
            }
            dir_files.sort();

            let display_path = to_workspace_folder_path(&resolved, allowed_roots);
            attached_snippets.push(format!(
                "📁 Attached Directory Context for `@{}`:\n```\nPath: {}\nItems:\n{}\n```",
                raw_file,
                display_path,
                if dir_files.is_empty() {
                    "  (empty folder)".to_string()
                } else {
                    dir_files.join("\n")
                }
            ));
            continue;
        }

        if !resolved.is_file() {
            continue;
        }

        let start_line = caps.get(2).and_then(|m| m.as_str().parse::<usize>().ok());
        let end_line = caps.get(3).and_then(|m| m.as_str().parse::<usize>().ok());

        let snippet_key = format!(
            "{}:{}:{}",
            raw_file,
            start_line.unwrap_or(0),
            end_line.unwrap_or(0)
        );
        if seen_keys.contains(&snippet_key) {
            continue;
        }
        seen_keys.insert(snippet_key);

        if let Ok(content) = fs::read_to_string(&resolved) {
            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();

            let (sliced, range_label) = match (start_line, end_line) {
                (Some(s), Some(e)) => {
                    let s_idx = s.saturating_sub(1);
                    let e_idx = e.min(total);
                    if s_idx < e_idx {
                        let numbered = lines[s_idx..e_idx]
                            .iter()
                            .enumerate()
                            .map(|(i, l)| format!("{:4} | {}", s + i, l))
                            .collect::<Vec<_>>()
                            .join("\n");
                        (numbered, format!("(lines {}-{})", s, e_idx))
                    } else {
                        (String::new(), format!("(lines {}-{})", s, e))
                    }
                }
                (Some(s), None) => {
                    let s_idx = s.saturating_sub(1);
                    if s_idx < total {
                        (format!("{:4} | {}", s, lines[s_idx]), format!("(line {})", s))
                    } else {
                        (String::new(), format!("(line {})", s))
                    }
                }
                (None, Some(e)) => {
                    let e_idx = e.min(total);
                    let numbered = lines[..e_idx]
                        .iter()
                        .enumerate()
                        .map(|(i, l)| format!("{:4} | {}", i + 1, l))
                        .collect::<Vec<_>>()
                        .join("\n");
                    (numbered, format!("(lines 1-{})", e_idx))
                }
                (None, None) => {
                    let numbered = lines
                        .iter()
                        .take(300)
                        .enumerate()
                        .map(|(i, l)| format!("{:4} | {}", i + 1, l))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let label = if total > 300 {
                        format!("(lines 1-300 of {})", total)
                    } else {
                        format!("(all {} lines)", total)
                    };
                    (numbered, label)
                }
            };

            if !sliced.is_empty() {
                attached_snippets.push(format!(
                    "### [Referenced File Context: `{}` {}]:\n```\n{}\n```",
                    raw_file, range_label, sliced
                ));
            }
        }
    }

    if attached_snippets.is_empty() {
        text.to_string()
    } else {
        format!(
            "{}\n\n### Attached File Snippets (via @ references):\n{}",
            text,
            attached_snippets.join("\n\n")
        )
    }
}
