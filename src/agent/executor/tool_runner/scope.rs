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
/// - every verifier detectable in any allowed folder (workspace scope), and
/// - every verifier detectable for the files modified in this turn.
///
/// The modified-file set is always included so that a file in a folder we cannot
/// probe still demands its own verification. Duplicates are removed while
/// preserving first-seen order.
pub fn required_verifiers(allowed_folders: &[String], modified_files: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    for folder in allowed_folders {
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

/// Returns the canonical verifier string that the given command satisfies, if
/// any. Used to record *which* verifier passed, so coverage accounting does not
/// rely on substring guesses at report time.
pub fn matched_verifier(cmd: &str) -> Option<String> {
    let cmd_lower = cmd.to_lowercase();
    let all = crate::agent::languages::known_verifier_commands();
    all.iter()
        .copied()
        .find(|v| cmd_lower.contains(&v.to_lowercase()))
        .map(str::to_string)
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
