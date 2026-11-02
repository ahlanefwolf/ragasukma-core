use std::path::Path;
use super::{LanguageDetector, ProjectLanguageInfo};

pub struct KotlinDetector;

impl LanguageDetector for KotlinDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        let has_kts = root.join("build.gradle.kts").exists();
        let has_gradlew = root.join("gradlew").exists();

        if !has_kts {
            return None;
        }

        let (primary, secondaries) = if has_gradlew {
            (
                "./gradlew test".to_string(),
                vec!["./gradlew check".to_string(), "./gradlew build".to_string()],
            )
        } else {
            (
                "gradle test".to_string(),
                vec!["gradle check".to_string(), "gradle build".to_string()],
            )
        };

        Some(ProjectLanguageInfo {
            language: "Kotlin",
            primary_verifier: primary,
            secondary_verifiers: secondaries,
        })
    }
}
