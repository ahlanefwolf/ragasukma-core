use anyhow::Result;
use serde_json::json;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceFileInfo {
    pub relative_path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Execute workspace grep across files with line-length capping and total output bounding.
pub fn run_workspace_grep(
    resolved: &Path,
    query: &str,
    allowed_roots: &[String],
) -> Result<serde_json::Value> {
    let query_lower = query.to_lowercase();
    let re_opt = regex::RegexBuilder::new(query)
        .case_insensitive(true)
        .build()
        .ok();

    let max_results = 25;
    let max_total_chars = 16_000;
    let mut current_total_chars = 0;
    let mut matches = Vec::new();
    let mut truncated = false;

    let check_line_match = |line: &str| -> bool {
        if let Some(ref re) = re_opt {
            re.is_match(line)
        } else {
            line.to_lowercase().contains(&query_lower)
        }
    };

    if resolved.is_file() {
        if let Ok(content) = fs::read_to_string(resolved) {
            let display_file = super::fs::to_workspace_folder_path(resolved, allowed_roots);
            for (line_idx, line) in content.lines().enumerate() {
                if check_line_match(line) {
                    let trimmed = line.trim();
                    let content_snippet = if trimmed.chars().count() > 600 {
                        let prefix: String = trimmed.chars().take(600).collect();
                        format!("{} ... [truncated]", prefix)
                    } else {
                        trimmed.to_string()
                    };

                    current_total_chars += content_snippet.len();
                    matches.push(json!({
                        "file": display_file,
                        "line": line_idx + 1,
                        "content": content_snippet
                    }));

                    if matches.len() >= max_results || current_total_chars >= max_total_chars {
                        truncated = true;
                        break;
                    }
                }
            }
        }
        return Ok(json!({
            "query": query,
            "matches_found": matches.len(),
            "truncated": truncated,
            "results": matches
        }));
    }

    let mut builder = ignore::WalkBuilder::new(resolved);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .max_depth(Some(12));

    let mut overrides = ignore::overrides::OverrideBuilder::new(resolved);
    let _ = overrides.add("!.git");
    let _ = overrides.add("!.git/**");
    let _ = overrides.add("!node_modules");
    let _ = overrides.add("!node_modules/**");
    let _ = overrides.add("!target");
    let _ = overrides.add("!target/**");
    let _ = overrides.add("!dist");
    let _ = overrides.add("!dist/**");
    let _ = overrides.add("!build");
    let _ = overrides.add("!build/**");
    let _ = overrides.add("!.venv");
    let _ = overrides.add("!.venv/**");
    let _ = overrides.add("!venv");
    let _ = overrides.add("!venv/**");
    let _ = overrides.add("!__pycache__");
    let _ = overrides.add("!__pycache__/**");
    let _ = overrides.add("!.cache");
    let _ = overrides.add("!.cache/**");
    let _ = overrides.add("!*.min.js");
    let _ = overrides.add("!*.min.css");
    let _ = overrides.add("!*.map");
    let _ = overrides.add("!*.bundle.js");
    let _ = overrides.add("!.DS_Store");
    let _ = overrides.add("!Thumbs.db");
    if let Ok(ov) = overrides.build() {
        builder.overrides(ov);
    }

    for result in builder.build() {
        if matches.len() >= max_results || current_total_chars >= max_total_chars {
            truncated = true;
            break;
        }

        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };

        if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            let path = entry.path();
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            // Skip binary or oversized files (> 2 MB)
            if metadata.len() > 2_000_000 {
                continue;
            }

            let file_name = entry.file_name().to_string_lossy();
            if file_name.ends_with(".min.js")
                || file_name.ends_with(".min.css")
                || file_name.ends_with(".map")
                || file_name.ends_with(".bundle.js")
            {
                continue;
            }

            if let Ok(content) = fs::read_to_string(path) {
                let display_file = super::fs::to_workspace_folder_path(path, allowed_roots);
                for (line_idx, line) in content.lines().enumerate() {
                    if check_line_match(line) {
                        let trimmed = line.trim();
                        let content_snippet = if trimmed.chars().count() > 600 {
                            let prefix: String = trimmed.chars().take(600).collect();
                            format!("{} ... [truncated]", prefix)
                        } else {
                            trimmed.to_string()
                        };

                        current_total_chars += content_snippet.len();
                        matches.push(json!({
                            "file": display_file,
                            "line": line_idx + 1,
                            "content": content_snippet
                        }));

                        if matches.len() >= max_results || current_total_chars >= max_total_chars {
                            truncated = true;
                            break;
                        }
                    }
                }
            }
        }
    }

    Ok(json!({
        "query": query,
        "matches_found": matches.len(),
        "truncated": truncated,
        "results": matches
    }))
}

/// Directory names that are never walked (pruned before descending) because they
/// are build artifact / dependency caches. Pruning early keeps the walk O(source)
/// instead of O(everything) — this is the single biggest RAM + latency win.
const PRUNE_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "coverage",
    ".git",
    ".hg",
    ".svn",
    ".venv",
    "venv",
    "env",
    "__pycache__",
    ".cache",
    ".next",
    ".nuxt",
    ".output",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".gradle",
    ".terraform",
    ".idea",
    ".vscode",
    "vendor",
    "Pods",
    "DerivedData",
];

/// Hard ceiling on deep nesting — avoids pathological symlink trees.
const MAX_WALK_DEPTH: usize = 32;

/// Global budget of *file* entries (directories are always kept intact so the
/// tree structure never looks broken). 20k is far more than any human scrolls,
/// yet bounded so a huge monorepo can't blow up the payload.
const MAX_FILE_ENTRIES: usize = 20_000;

// NOTE: line counts are deliberately NOT computed while listing the tree.
// Counting lines requires opening and reading every file, so listing a large
// repo meant one `open()` + full read per entry (up to MAX_FILE_ENTRIES) on
// every project switch, with no caching. The count is pure UI decoration, and
// the exact number is already returned by the file-content endpoint
// (`total_lines`) the moment a file is actually opened. The UI therefore fills
// these in lazily as the user browses. The field was removed from
// `WorkspaceFileInfo` entirely once no consumer remained.

fn is_pruned_dir(entry: &ignore::DirEntry) -> bool {
    if entry.depth() == 0 {
        return false;
    }
    let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
    if !is_dir {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    PRUNE_DIRS.iter().any(|d| name.eq_ignore_ascii_case(d))
}

/// Recursively list files across all allowed root folders in the project,
/// strictly respecting .gitignore, git global/exclude rules, and filtering out heavy build artifacts.
///
/// Design goals (keep the workspace tree complete + cheap):
/// * every directory is always emitted, so no folder ever renders as "empty";
/// * only the *file* entries have a global budget (`MAX_FILE_ENTRIES`);
/// * ignored/heavy directories are pruned before descending, not filtered after;
/// * no file is opened while listing (line counts are resolved lazily by the UI).
pub fn list_workspace_files(allowed_roots: &[String]) -> Result<Vec<WorkspaceFileInfo>> {
    let mut files = Vec::new();
    let mut file_budget = MAX_FILE_ENTRIES;
    for root in allowed_roots {
        let r_path = Path::new(root);
        if !r_path.exists() || !r_path.is_dir() {
            continue;
        }

        let folder_name = r_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        // Also add the root folder itself to the file list so UI/agent sees it
        if !folder_name.is_empty() {
            files.push(WorkspaceFileInfo {
                relative_path: folder_name.clone(),
                name: folder_name.clone(),
                is_dir: true,
                size: 0,
            });
        }

        let mut builder = ignore::WalkBuilder::new(r_path);
        builder
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            .threads(1)
            .max_depth(Some(MAX_WALK_DEPTH));

        // `.gitignore` is the source of truth for what the project considers
        // noise, so we only override OS cruft here and let the git rules do the
        // rest. Heavy dirs are pruned structurally via `filter_entry` below,
        // which is much cheaper than post-filtering every descendant.
        let mut overrides = ignore::overrides::OverrideBuilder::new(r_path);
        let _ = overrides.add("!.DS_Store");
        let _ = overrides.add("!Thumbs.db");
        let _ = overrides.add("!*.swp");
        let _ = overrides.add("!*.swo");
        if let Ok(ov) = overrides.build() {
            builder.overrides(ov);
        }

        // Prune artifact/cache directories *before* descending (never walked).
        builder.filter_entry(|entry| !is_pruned_dir(entry));

        for result in builder.build() {
            let entry = match result {
                Ok(e) => e,
                Err(_) => continue,
            };

            if entry.depth() == 0 {
                continue;
            }

            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if name == ".git" || name == ".DS_Store" || name == "Thumbs.db" {
                continue;
            }

            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);

            // Directories are ALWAYS kept (complete tree); only files are budgeted.
            if !is_dir {
                if file_budget == 0 {
                    continue;
                }
                file_budget -= 1;
            }

            let inner_rel = path
                .strip_prefix(r_path)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| name.clone().replace('\\', "/"));

            let rel_path = if folder_name.is_empty() {
                inner_rel
            } else if inner_rel.is_empty() {
                folder_name.clone()
            } else {
                format!("{}/{}", folder_name, inner_rel)
            };

            let size = if is_dir {
                0
            } else {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            };

            files.push(WorkspaceFileInfo {
                relative_path: rel_path,
                name,
                is_dir,
                size,
            });
        }
    }

    files.sort_by(|a, b| {
        if a.is_dir == b.is_dir {
            a.relative_path.cmp(&b.relative_path)
        } else if a.is_dir {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });

    Ok(files)
}
