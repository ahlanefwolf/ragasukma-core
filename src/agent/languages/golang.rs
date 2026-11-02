use std::path::Path;
use super::{LanguageDetector, ProjectLanguageInfo};

pub struct GoDetector;

impl LanguageDetector for GoDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        if root.join("go.mod").exists() || root.join("go.sum").exists() {
            Some(ProjectLanguageInfo {
                language: "Go",
                primary_verifier: "go test ./...".to_string(),
                secondary_verifiers: vec![
                    "go vet ./...".to_string(),
                    "go build ./...".to_string(),
                    "golangci-lint run".to_string(),
                ],
            })
        } else {
            None
        }
    }
}
