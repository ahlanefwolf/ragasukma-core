use super::{LanguageDetector, ProjectLanguageInfo};
use std::path::Path;

pub struct SwiftDetector;

impl LanguageDetector for SwiftDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        if root.join("Package.swift").exists() {
            Some(ProjectLanguageInfo {
                language: "Swift",
                primary_verifier: "swift test".to_string(),
                secondary_verifiers: vec!["swift build".to_string()],
            })
        } else {
            None
        }
    }
}
