//! Decides *which* verifications a passing command actually proves.
//!
//! A workspace can hold several independent components (e.g. a `ragasukma-core`
//! library and a `ragasukma-tui` binary). Detecting verifiers only for the files
//! touched in this session means a green `cargo test` in *one* component can
//! retire the verification of *another*, untouched component. That prematurely
//! concludes the goal and locks the turn down with a `VERIFICATION PASSED`
//! directive while downstream code is still broken.
//!
//! [`required_verifiers`] widens the demanded set to every verifier detectable
//! across the allowed folders, so a pass only concludes a goal once no component
//! is left unverified. When nothing can be detected we fall back to the
//! modified-file detection, preserving the previous behaviour for single-crate
//! projects.

/// Returns the full set of verification commands a conclusion must cover.
///
/// The result is the union of:
/// - verifiers detectable in any allowed folder containing modified files (workspace scope), and
/// - every verifier detectable for the files modified in this turn.
///
/// When no files were modified or folder mapping is ambiguous, all allowed folders are checked.
/// Duplicates are removed while preserving first-seen order.
pub fn required_verifiers(allowed_folders: &[String], modified_files: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    let target_folders: Vec<&String> = if modified_files.is_empty() {
        allowed_folders.iter().collect()
    } else {
        let matching: Vec<&String> = allowed_folders
            .iter()
            .filter(|folder| folder_has_modified_files(folder, modified_files))
            .collect();
        if matching.is_empty() {
            allowed_folders.iter().collect()
        } else {
            matching
        }
    };

    for folder in target_folders {
        let folder_ref = std::slice::from_ref(folder);
        if let Some(v) = crate::agent::languages::detect_primary_verification_command(folder_ref) {
            push_unique(&mut out, v);
        }
    }

    for v in crate::agent::languages::detect_verifiers_for_modified_files(
        allowed_folders,
        modified_files,
    ) {
        push_unique(&mut out, v);
    }

    out
}

fn folder_has_modified_files(folder: &str, modified_files: &[String]) -> bool {
    let folder_path = std::path::Path::new(folder);
    let folder_name = folder_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    modified_files.iter().any(|f| {
        let clean = f.trim_start_matches("./").trim_start_matches('@');
        f.starts_with(folder)
            || (!folder_name.is_empty()
                && (clean.starts_with(folder_name)
                    || clean.contains(&format!("/{}/", folder_name))))
    })
}

/// Checks whether an executed command satisfies a required verifier string.
/// Supports compound commands, subshells, and chained pipelines.
pub fn is_verifier_satisfied(required_verifier: &str, executed_cmd: &str) -> bool {
    let req = required_verifier.to_lowercase();
    let exec = executed_cmd.to_lowercase();

    if exec.contains(&req) {
        return true;
    }

    // Chained commands e.g. "cargo test && (cd web && npm test)"
    let parts: Vec<&str> = req
        .split("&&")
        .map(|p| p.trim().trim_matches(|c| c == '(' || c == ')').trim())
        .filter(|p| !p.is_empty())
        .collect();

    if !parts.is_empty()
        && parts.iter().all(|part| {
            if exec.contains(part) {
                return true;
            }
            // If part is like "cd <subdir>", check if exec has "cd ...<subdir>"
            if let Some(dir) = part.strip_prefix("cd ") {
                let trimmed_dir = dir.trim();
                return exec.contains(&format!("cd {trimmed_dir}"))
                    || exec.contains(&format!("/{trimmed_dir}"))
                    || exec.contains(&format!("cwd: {trimmed_dir}"));
            }
            false
        })
    {
        return true;
    }

    false
}

/// Returns all canonical verifier strings that the given command satisfies.
pub fn matched_verifiers(cmd: &str) -> Vec<String> {
    let cmd_lower = cmd.to_lowercase();
    let all = crate::agent::languages::known_verifier_commands();
    all.iter()
        .copied()
        .filter(|v| cmd_lower.contains(&v.to_lowercase()))
        .map(str::to_string)
        .collect()
}

/// Returns the first canonical verifier string that the given command satisfies, if
/// any. Kept for backward compatibility.
#[allow(dead_code)]
pub fn matched_verifier(cmd: &str) -> Option<String> {
    matched_verifiers(cmd).into_iter().next()
}

fn push_unique(out: &mut Vec<String>, value: String) {
    if !out.iter().any(|existing| existing.eq_ignore_ascii_case(&value)) {
        out.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_unique_is_case_insensitive() {
        let mut v = Vec::new();
        push_unique(&mut v, "cargo test".to_string());
        push_unique(&mut v, "Cargo Test".to_string());
        assert_eq!(v, vec!["cargo test".to_string()]);
    }

    #[test]
    fn matched_verifier_identifies_the_canonical_command() {
        assert_eq!(
            matched_verifier("cargo test --all-targets").as_deref(),
            Some("cargo test")
        );
        assert!(matched_verifier("ls -la").is_none());
    }

    #[test]
    fn matched_verifier_is_case_insensitive() {
        assert_eq!(matched_verifier("CARGO TEST").as_deref(), Some("cargo test"));
    }

    #[test]
    fn is_verifier_satisfied_handles_compound_and_subshells() {
        let req = "cargo test && (cd web && npm test)";
        let exec = "cargo test && (cd web && npm test)";
        assert!(is_verifier_satisfied(req, exec));

        let exec_wrapped = "cargo test && (cd /path/to/web && npm test)";
        assert!(is_verifier_satisfied("cd web && npm test", exec_wrapped));

        assert!(is_verifier_satisfied("cargo test", "cargo test --bin ragasukma"));
        assert!(!is_verifier_satisfied("cargo test", "npm test"));
    }

    #[test]
    fn matched_verifiers_finds_all_matching_canonical_commands() {
        let cmd = "cargo test && (cd web && npm test)";
        let matched = matched_verifiers(cmd);
        assert!(matched.contains(&"cargo test".to_string()));
        assert!(matched.contains(&"npm test".to_string()));
    }

    #[test]
    fn empty_inputs_demand_nothing() {        assert!(required_verifiers(&[], &[]).is_empty());
    }

    #[test]
    fn dedupes_across_folders_and_files() {
        // Two identical folders must not double up the demanded verifiers.
        let folders = vec!["/nonexistent/a".to_string(), "/nonexistent/a".to_string()];
        let files = vec!["/nonexistent/a/lib.rs".to_string()];
        let verifiers = required_verifiers(&folders, &files);
        let mut seen = std::collections::HashSet::new();
        for v in &verifiers {
            assert!(seen.insert(v.to_lowercase()), "duplicate verifier: {v}");
        }
    }
}
