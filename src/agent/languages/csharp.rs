use std::fs;
use std::path::Path;
use super::{LanguageDetector, ProjectLanguageInfo};

pub struct CSharpDetector;

impl LanguageDetector for CSharpDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        let has_sln = fs::read_dir(root).ok().map(|mut entries| {
            entries.any(|e| {
                e.ok()
                    .map(|entry| {
                        let name = entry.file_name().to_string_lossy().to_string();
                        name.ends_with(".sln") || name.ends_with(".csproj") || name.ends_with(".fsproj")
                    })
                    .unwrap_or(false)
            })
        }).unwrap_or(false);

        if has_sln {
            Some(ProjectLanguageInfo {
                language: "C# / .NET",
                primary_verifier: "dotnet test".to_string(),
                secondary_verifiers: vec!["dotnet build".to_string()],
            })
        } else {
            None
        }
    }
}
