pub fn parse_project_folders(root_path: Option<&str>) -> Vec<String> {
    let Some(raw) = root_path else {
        return Vec::new();
    };
    if let Ok(arr) = serde_json::from_str::<Vec<String>>(raw) {
        return arr;
    }
    let trimmed = raw.trim();
    if !trimmed.is_empty() {
        vec![trimmed.to_string()]
    } else {
        Vec::new()
    }
}

/// Delegated project verification detection for backward compatibility
pub fn detect_project_verification_commands(allowed_folders: &[String]) -> Option<String> {
    crate::agent::languages::detect_primary_verification_command(allowed_folders)
}

/// Delegated verification command check for backward compatibility
pub fn is_verification_command(cmd: &str) -> bool {
    crate::agent::languages::is_verification_command(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_project_folders() {
        assert_eq!(parse_project_folders(None), Vec::<String>::new());
        assert_eq!(
            parse_project_folders(Some("[\"/path/a\", \"/path/b\"]")),
            vec!["/path/a".to_string(), "/path/b".to_string()]
        );
        assert_eq!(
            parse_project_folders(Some("/single/path")),
            vec!["/single/path".to_string()]
        );
    }
}
