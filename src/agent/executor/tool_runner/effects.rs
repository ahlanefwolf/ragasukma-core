//! Post-execution side effects: verification persistence, evidence caching,
//! streak tracking and message records.

pub(super) fn extract_modified_paths_from_command(cmd: &str) -> Vec<String> {
    let mut paths = Vec::new();

    // 1. Python open('path', 'w'/'a') or open("path", "w"/"a")
    if cmd.contains("open(") {
        for part in cmd.split("open(") {
            let trimmed = part.trim_start();
            if (trimmed.starts_with('\'') || trimmed.starts_with('"'))
                && let Some(quote) = trimmed.chars().next()
            {
                let rest = &trimmed[1..];
                if let Some((p, post)) = rest.split_once(quote)
                    && (post.contains("'w'")
                        || post.contains("\"w\"")
                        || post.contains("'a'")
                        || post.contains("\"a\"")
                        || post.contains("'wb'")
                        || post.contains("\"wb\""))
                {
                    let clean = p.trim_start_matches("./").trim();
                    if !clean.is_empty() && !paths.contains(&clean.to_string()) {
                        paths.push(clean.to_string());
                    }
                }
            }
        }
    }

    // 2. touch file.txt
    if cmd.starts_with("touch ") {
        for arg in cmd.strip_prefix("touch ").unwrap_or("").split_whitespace() {
            let clean = arg.trim_start_matches("./").trim();
            if !clean.starts_with('-') && !clean.is_empty() && !paths.contains(&clean.to_string()) {
                paths.push(clean.to_string());
            }
        }
    }

    paths
}
