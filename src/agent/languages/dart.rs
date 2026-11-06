use super::{LanguageDetector, ProjectLanguageInfo};
use std::fs;
use std::path::Path;

pub struct DartDetector;

impl LanguageDetector for DartDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        let pubspec = root.join("pubspec.yaml");
        if !pubspec.exists() {
            return None;
        }

        let is_flutter = fs::read_to_string(&pubspec)
            .map(|content| content.contains("flutter:") || content.contains("sdk: flutter"))
            .unwrap_or(false);

        let (primary, secondaries) = if is_flutter {
            (
                "flutter test".to_string(),
                vec!["flutter analyze".to_string()],
            )
        } else {
            ("dart test".to_string(), vec!["dart analyze".to_string()])
        };

        Some(ProjectLanguageInfo {
            language: if is_flutter { "Flutter / Dart" } else { "Dart" },
            primary_verifier: primary,
            secondary_verifiers: secondaries,
        })
    }
}
