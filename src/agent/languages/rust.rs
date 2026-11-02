use std::path::Path;
use super::{LanguageDetector, ProjectLanguageInfo};

pub struct RustDetector;

impl LanguageDetector for RustDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        if root.join("Cargo.toml").exists() {
            Some(ProjectLanguageInfo {
                language: "Rust",
                primary_verifier: "cargo test".to_string(),
                secondary_verifiers: vec![
                    "cargo check".to_string(),
                    "cargo clippy".to_string(),
                    "cargo build".to_string(),
                ],
            })
        } else {
            None
        }
    }
}
