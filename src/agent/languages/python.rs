use std::path::Path;
use super::{LanguageDetector, ProjectLanguageInfo};

pub struct PythonDetector;

impl LanguageDetector for PythonDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        let is_python = root.join("pyproject.toml").exists()
            || root.join("pytest.ini").exists()
            || root.join("requirements.txt").exists()
            || root.join("setup.py").exists()
            || root.join("Pipfile").exists()
            || root.join("manage.py").exists();

        if !is_python {
            return None;
        }

        let is_poetry = root.join("poetry.lock").exists();
        let is_pipenv = root.join("Pipfile").exists();
        let is_uv = root.join("uv.lock").exists();

        let primary = if is_uv {
            "uv run pytest".to_string()
        } else if is_poetry {
            "poetry run pytest".to_string()
        } else if is_pipenv {
            "pipenv run pytest".to_string()
        } else {
            "pytest".to_string()
        };

        let secondaries = vec![
            "python -m pytest".to_string(),
            "python -m unittest".to_string(),
            "mypy .".to_string(),
            "ruff check .".to_string(),
        ];

        Some(ProjectLanguageInfo {
            language: "Python",
            primary_verifier: primary,
            secondary_verifiers: secondaries,
        })
    }
}
