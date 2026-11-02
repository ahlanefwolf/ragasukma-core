use std::path::Path;
use super::{LanguageDetector, ProjectLanguageInfo};

pub struct JavaDetector;

impl LanguageDetector for JavaDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        let has_maven = root.join("pom.xml").exists();
        let has_gradle = root.join("build.gradle").exists();
        let has_gradlew = root.join("gradlew").exists();

        if !has_maven && !has_gradle {
            return None;
        }

        let (primary, secondaries) = if has_gradlew {
            (
                "./gradlew test".to_string(),
                vec!["./gradlew check".to_string(), "./gradlew build".to_string()],
            )
        } else if has_gradle {
            (
                "gradle test".to_string(),
                vec!["gradle check".to_string(), "gradle build".to_string()],
            )
        } else {
            (
                "mvn test".to_string(),
                vec!["mvn compile".to_string(), "mvn verify".to_string()],
            )
        };

        Some(ProjectLanguageInfo {
            language: "Java",
            primary_verifier: primary,
            secondary_verifiers: secondaries,
        })
    }
}
