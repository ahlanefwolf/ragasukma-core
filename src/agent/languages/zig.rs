use std::path::Path;
use super::{LanguageDetector, ProjectLanguageInfo};

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
