use super::{LanguageDetector, ProjectLanguageInfo};
use std::path::Path;

pub struct ZigDetector;

impl LanguageDetector for ZigDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        if root.join("build.zig").exists() {
            Some(ProjectLanguageInfo {
                language: "Zig",
                primary_verifier: "zig build test".to_string(),
                secondary_verifiers: vec!["zig build".to_string()],
            })
        } else {
            None
        }
    }
}
