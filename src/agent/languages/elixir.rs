use std::path::Path;
use super::{LanguageDetector, ProjectLanguageInfo};

pub struct ElixirDetector;

impl LanguageDetector for ElixirDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        if root.join("mix.exs").exists() {
            Some(ProjectLanguageInfo {
                language: "Elixir",
                primary_verifier: "mix test".to_string(),
                secondary_verifiers: vec!["mix compile --warnings-as-errors".to_string()],
            })
        } else {
            None
        }
    }
}
