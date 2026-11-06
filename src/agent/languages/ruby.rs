use super::{LanguageDetector, ProjectLanguageInfo};
use std::path::Path;

pub struct RubyDetector;

impl LanguageDetector for RubyDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        let has_gemfile = root.join("Gemfile").exists();
        let has_rakefile = root.join("Rakefile").exists();

        if !has_gemfile && !has_rakefile {
            return None;
        }

        let primary = if root.join(".rspec").exists() || root.join("spec").is_dir() {
            "bundle exec rspec".to_string()
        } else {
            "bundle exec rake test".to_string()
        };

        Some(ProjectLanguageInfo {
            language: "Ruby",
            primary_verifier: primary,
            secondary_verifiers: vec!["rake test".to_string(), "bundle exec rubocop".to_string()],
        })
    }
}
