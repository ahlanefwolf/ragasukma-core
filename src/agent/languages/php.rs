use std::path::Path;
use super::{LanguageDetector, ProjectLanguageInfo};

pub struct PhpDetector;

impl LanguageDetector for PhpDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        let has_composer = root.join("composer.json").exists();
        let has_artisan = root.join("artisan").exists();

        if !has_composer && !has_artisan {
            return None;
        }

        let (primary, secondaries) = if has_artisan {
            (
                "php artisan test".to_string(),
                vec!["./vendor/bin/phpunit".to_string(), "composer test".to_string()],
            )
        } else if root.join("vendor/bin/phpunit").exists() {
            (
                "./vendor/bin/phpunit".to_string(),
                vec!["composer test".to_string()],
            )
        } else {
            (
                "composer test".to_string(),
                vec!["./vendor/bin/pest".to_string(), "./vendor/bin/phpunit".to_string()],
            )
        };

        Some(ProjectLanguageInfo {
            language: "PHP",
            primary_verifier: primary,
            secondary_verifiers: secondaries,
        })
    }
}
