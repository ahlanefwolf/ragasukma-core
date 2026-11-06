use super::{LanguageDetector, ProjectLanguageInfo};
use std::path::Path;

pub struct CCppDetector;

impl LanguageDetector for CCppDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        let has_cmake = root.join("CMakeLists.txt").exists();
        let has_makefile = root.join("Makefile").exists();
        let has_meson = root.join("meson.build").exists();

        if !has_cmake && !has_makefile && !has_meson {
            return None;
        }

        let (primary, secondaries) = if has_cmake {
            (
                "ctest --output-on-failure".to_string(),
                vec![
                    "cmake --build build".to_string(),
                    "cmake -B build".to_string(),
                ],
            )
        } else if has_meson {
            (
                "meson test -C build".to_string(),
                vec!["ninja -C build".to_string()],
            )
        } else {
            (
                "make test".to_string(),
                vec!["make check".to_string(), "make all".to_string()],
            )
        };

        Some(ProjectLanguageInfo {
            language: "C / C++",
            primary_verifier: primary,
            secondary_verifiers: secondaries,
        })
    }
}
