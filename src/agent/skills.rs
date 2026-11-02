use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use crate::agent::models::PromptSkill;

/// Check if a directory name matches any hidden skills folder variation (case-insensitive, strictly starting with dot: .skills, .skill)
pub fn is_skill_dir_name(name: &str) -> bool {
    let trimmed = name.trim();
    if !trimmed.starts_with('.') {
        return false;
    }
    let lower = trimmed.to_lowercase();
    lower == ".skills" || lower == ".skill"
}

/// Backward compatibility alias for is_skill_dir_name
pub fn is_agent_dir_name(name: &str) -> bool {
    is_skill_dir_name(name)
}

/// Locate global skill directories (e.g. ~/.skills or ~/.skill)
pub fn get_global_agent_skill_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var("HOME").ok().or_else(|| std::env::var("USERPROFILE").ok());
    if let Some(h) = home {
        let p = PathBuf::from(h);
        for name in [".skills", ".skill"] {
            let candidate_skills = p.join(name).join("skills");
            if candidate_skills.is_dir() {
                dirs.push(candidate_skills);
            }
            let candidate_root = p.join(name);
            if candidate_root.is_dir() {
                dirs.push(candidate_root);
            }
        }
    }
    dirs
}

/// Discover all hidden skill folders (.skills, .skill) inside a given base path
pub fn find_agent_dirs_in_path(base: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    if !base.is_dir() {
        return results;
    }

    // Check if base itself is already a skill folder
    if let Some(name) = base.file_name().and_then(|n| n.to_str())
        && is_skill_dir_name(name) {
            results.push(base.to_path_buf());
            return results;
        }

    // Check immediate subdirectories of base
    if let Ok(entries) = fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
                    && is_skill_dir_name(name) {
                        results.push(path);
                    }
        }
    }

    results
}

/// Recursively find strictly .md files in a directory (all other files/folders are ignored)
fn walk_md_files<F: FnMut(&Path)>(dir: &Path, cb: &mut F) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_md_files(&path, cb);
            } else if path.is_file()
                && let Some(ext) = path.extension().and_then(|e| e.to_str())
                    && ext.eq_ignore_ascii_case("md") {
                        cb(&path);
                    }
        }
    }
}

/// Scan all skills from a skill folder (and its subfolders containing .md files)
pub fn scan_skills_in_agent_dir(agent_dir: &Path) -> Vec<PromptSkill> {
    let mut skills = Vec::new();
    if !agent_dir.is_dir() {
        return skills;
    }

    walk_md_files(agent_dir, &mut |file_path| {
        if let Some(skill) = parse_skill_file(file_path) {
            skills.push(skill);
        }
    });

    skills
}

/// Parse a markdown file into a `PromptSkill`
pub fn parse_skill_file(path: &Path) -> Option<PromptSkill> {
    let content = fs::read_to_string(path).ok()?;
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let parent_name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // Default slug: if file is "SKILL.md", derive from parent folder unless parent is "skills" or "agents"
    let default_slug = if stem.eq_ignore_ascii_case("skill")
        && !parent_name.is_empty()
        && !parent_name.eq_ignore_ascii_case("skills")
        && !is_agent_dir_name(&parent_name)
    {
        slugify(&parent_name)
    } else {
        slugify(&stem)
    };

    let default_name = title_case(&default_slug);
    let (frontmatter_map, body) = parse_frontmatter(&content);

    let name = frontmatter_map
        .get("name")
        .cloned()
        .unwrap_or_else(|| extract_first_h1(&body).unwrap_or(default_name));

    let slug = frontmatter_map
        .get("slug")
        .cloned()
        .unwrap_or(default_slug);

    let description = frontmatter_map
        .get("description")
        .cloned()
        .unwrap_or_else(|| extract_first_paragraph(&body));

    let tags = frontmatter_map
        .get("tags")
        .cloned()
        .unwrap_or_else(|| "[]".to_string());

    let system_prompt = if !body.trim().is_empty() {
        body.trim().to_string()
    } else {
        content.trim().to_string()
    };

    let id = format!("skill_{}", slug);

    Some(PromptSkill {
        id,
        name,
        slug,
        description,
        system_prompt,
        tags,
        file_path: Some(path.to_string_lossy().to_string()),
        is_active: 1,
        created_at: 0,
        updated_at: 0,
    })
}

/// Convert a string to a URL-friendly slug
pub fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for c in input.chars() {
        if c.is_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if (c == ' ' || c == '_' || c == '-') && !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "custom-skill".to_string()
    } else {
        out
    }
}

/// Convert a slug or hyphenated string into Title Case
pub fn title_case(input: &str) -> String {
    input
        .split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Extract frontmatter (YAML/JSON style) and body from markdown
fn parse_frontmatter(content: &str) -> (HashMap<String, String>, String) {
    let mut map = HashMap::new();
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (map, content.to_string());
    }

    let after_first = &trimmed[3..];
    if let Some(end_idx) = after_first.find("\n---") {
        let frontmatter_str = &after_first[..end_idx];
        let body = &after_first[end_idx + 4..];

        for line in frontmatter_str.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim().to_lowercase();
                let val = v.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
                map.insert(key, val);
            }
        }
        (map, body.to_string())
    } else {
        (map, content.to_string())
    }
}

/// Extract the first `# Header` in markdown body
fn extract_first_h1(body: &str) -> Option<String> {
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(stripped) = trimmed.strip_prefix('#')
            && !stripped.starts_with('#') {
                let title = stripped.trim().to_string();
                if !title.is_empty() {
                    return Some(title);
                }
            }
    }
    None
}

/// Extract first paragraph text (excluding markdown headers) for description
fn extract_first_paragraph(body: &str) -> String {
    let mut paragraph = String::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with("```") {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.is_empty() {
            if !paragraph.is_empty() {
                break;
            }
        } else {
            if !paragraph.is_empty() {
                paragraph.push(' ');
            }
            paragraph.push_str(trimmed);
            if paragraph.len() > 160 {
                break;
            }
        }
    }
    if paragraph.len() > 160 {
        format!("{}...", &paragraph[..157])
    } else {
        paragraph
    }
}

/// Scan all skills across project folders, allowed folders, and global locations
pub fn scan_project_skills(
    root_path: Option<&str>,
    allowed_folders: &[String],
) -> Vec<PromptSkill> {
    let mut candidate_bases = Vec::new();

    // 1. Project root_path
    if let Some(r) = root_path {
        let p = PathBuf::from(r);
        if p.exists() {
            candidate_bases.push(p);
        }
    }

    // 2. Allowed folders
    for f in allowed_folders {
        let p = PathBuf::from(f);
        if p.exists() && !candidate_bases.contains(&p) {
            candidate_bases.push(p);
        }
    }

    // 3. Current working directory
    if let Ok(cwd) = std::env::current_dir()
        && !candidate_bases.contains(&cwd) {
            candidate_bases.push(cwd);
        }

    // Find agent directories across all candidates
    let mut agent_dirs = Vec::new();
    for base in &candidate_bases {
        for ad in find_agent_dirs_in_path(base) {
            if !agent_dirs.contains(&ad) {
                agent_dirs.push(ad);
            }
        }
    }

    // Also include global agent skill directories
    for gd in get_global_agent_skill_dirs() {
        if !agent_dirs.contains(&gd) {
            agent_dirs.push(gd);
        }
    }

    // Scan skills and deduplicate by slug (earlier project-specific candidates take precedence)
    let mut skill_map: HashMap<String, PromptSkill> = HashMap::new();
    for ad in agent_dirs {
        let found = scan_skills_in_agent_dir(&ad);
        for s in found {
            skill_map.entry(s.slug.clone()).or_insert(s);
        }
    }

    let mut result: Vec<PromptSkill> = skill_map.into_values().collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

/// Save or update a skill markdown file inside project's `.skills/<slug>.md`
pub fn save_skill_to_dir(
    base_dir: &Path,
    name: &str,
    slug: &str,
    description: &str,
    system_prompt: &str,
    tags: &[String],
) -> Result<PromptSkill> {
    let clean_slug = slugify(if slug.trim().is_empty() { name } else { slug });
    let target_dir = base_dir.join(".skills");
    fs::create_dir_all(&target_dir).with_context(|| format!("Failed to create skill directory {:?}", target_dir))?;

    let file_path = target_dir.join(format!("{}.md", clean_slug));
    let tags_formatted = serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string());

    let content = format!(
        "---\nname: \"{}\"\nslug: \"{}\"\ndescription: \"{}\"\ntags: {}\n---\n\n{}\n",
        name.replace('"', "\\\""),
        clean_slug,
        description.replace('"', "\\\""),
        tags_formatted,
        system_prompt.trim()
    );

    fs::write(&file_path, content).with_context(|| format!("Failed to write skill file {:?}", file_path))?;

    Ok(PromptSkill {
        id: format!("skill_{}", clean_slug),
        name: name.to_string(),
        slug: clean_slug,
        description: description.to_string(),
        system_prompt: system_prompt.to_string(),
        tags: tags_formatted,
        file_path: Some(file_path.to_string_lossy().to_string()),
        is_active: 1,
        created_at: 0,
        updated_at: 0,
    })
}

/// Delete a skill markdown file by slug or filename
pub fn delete_skill_from_dir(base_dir: &Path, slug_or_name: &str) -> Result<bool> {
    let clean_slug = slugify(slug_or_name);
    let candidate_dirs = find_agent_dirs_in_path(base_dir);

    for ad in candidate_dirs {
        let skills_dir = ad.join("skills");
        let dirs_to_check = if skills_dir.is_dir() {
            vec![skills_dir, ad]
        } else {
            vec![ad]
        };

        for d in dirs_to_check {
            // Check direct file <slug>.md
            let direct = d.join(format!("{}.md", clean_slug));
            if direct.is_file() {
                let _ = fs::remove_file(direct);
                return Ok(true);
            }
            // Check subdirectory <slug>/SKILL.md
            let sub = d.join(&clean_slug).join("SKILL.md");
            if sub.is_file() {
                let _ = fs::remove_file(sub);
                return Ok(true);
            }
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_agent_dir_name() {
        assert!(is_skill_dir_name(".skills"));
        assert!(is_skill_dir_name(".SKILLS"));
        assert!(is_skill_dir_name(".skill"));
        assert!(is_skill_dir_name(".SKILL"));
        assert!(is_skill_dir_name(".Skills"));
        assert!(is_skill_dir_name(".Skill"));
        // Must NOT match .agents, .agent, or non-dot directories
        assert!(!is_skill_dir_name(".agents"));
        assert!(!is_skill_dir_name(".AGENTS"));
        assert!(!is_skill_dir_name(".agent"));
        assert!(!is_skill_dir_name(".AGENT"));
        assert!(!is_skill_dir_name("skills"));
        assert!(!is_skill_dir_name("skill"));
        assert!(!is_skill_dir_name("agents"));
        assert!(!is_skill_dir_name("agent"));
        assert!(!is_skill_dir_name("node_modules"));
        assert!(!is_skill_dir_name("src"));
    }

    #[test]
    fn test_parse_frontmatter_and_content() {
        let md = r#"---
name: Senior Rust Reviewer
slug: senior-rust-reviewer
description: Expert review of Rust code
tags: ["rust", "reviewer"]
---

# Instructions
Always write idiomatic code.
"#;
        let (fm, body) = parse_frontmatter(md);
        assert_eq!(fm.get("name").unwrap(), "Senior Rust Reviewer");
        assert_eq!(fm.get("slug").unwrap(), "senior-rust-reviewer");
        assert!(body.contains("Always write idiomatic code."));
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Senior Rust Reviewer"), "senior-rust-reviewer");
        assert_eq!(slugify("SolidJS_Expert"), "solidjs-expert");
    }

    #[test]
    fn test_strict_md_filtering() {
        let temp_dir = std::env::temp_dir().join(format!("skills_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = fs::create_dir_all(&temp_dir);
        let md_file = temp_dir.join("test_skill.md");
        let txt_file = temp_dir.join("notes.txt");
        let json_file = temp_dir.join("config.json");
        let _ = fs::write(&md_file, "# Test\nSample content");
        let _ = fs::write(&txt_file, "text file");
        let _ = fs::write(&json_file, "{}");

        let mut collected = Vec::new();
        walk_md_files(&temp_dir, &mut |p| {
            collected.push(p.to_path_buf());
        });

        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0], md_file);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
