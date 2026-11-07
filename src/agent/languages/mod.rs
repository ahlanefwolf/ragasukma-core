pub mod c_cpp;
mod command_rules;

pub mod csharp;
pub mod dart;
pub mod elixir;
pub mod golang;
pub mod java;
pub mod kotlin;
pub mod node;
pub mod php;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod swift;
pub mod zig;

use std::path::Path;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectLanguageInfo {
    pub language: &'static str,
    pub primary_verifier: String,
    pub secondary_verifiers: Vec<String>,
}

pub trait LanguageDetector: Send + Sync {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo>;
}

/// Registry of all language detectors
fn get_detectors() -> Vec<Box<dyn LanguageDetector>> {
    vec![
        Box::new(rust::RustDetector),
        Box::new(node::NodeDetector),
        Box::new(python::PythonDetector),
        Box::new(golang::GoDetector),
        Box::new(dart::DartDetector),
        Box::new(csharp::CSharpDetector),
        Box::new(java::JavaDetector),
        Box::new(kotlin::KotlinDetector),
        Box::new(c_cpp::CCppDetector),
        Box::new(php::PhpDetector),
        Box::new(ruby::RubyDetector),
        Box::new(elixir::ElixirDetector),
        Box::new(swift::SwiftDetector),
        Box::new(zig::ZigDetector),
    ]
}

/// Scans workspace roots and identifies all project languages present,
/// including root manifests and immediate 1-level subprojects (e.g. backend, frontend, server, client, web).
pub fn detect_all_languages(allowed_folders: &[String]) -> Vec<ProjectLanguageInfo> {
    let detectors = get_detectors();
    let mut results = Vec::new();

    for folder in allowed_folders {
        let root = Path::new(folder);
        if !root.exists() || !root.is_dir() {
            continue;
        }

        // 1. First, check root directory directly
        for detector in &detectors {
            if let Some(info) = detector.detect(root)
                && !results
                    .iter()
                    .any(|r: &ProjectLanguageInfo| r.primary_verifier == info.primary_verifier)
            {
                results.push(info);
            }
        }

        // 2. Scan immediate 1-level subdirectories (e.g. backend/, frontend/, client/, server/, web/, etc.)
        // This is essential for monorepos, fullstack apps, and multi-component workspaces
        if let Ok(entries) = std::fs::read_dir(root) {
            let mut subdirs = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !name.starts_with('.')
                        && name != "node_modules"
                        && name != "target"
                        && name != "dist"
                        && name != "build"
                        && name != "vendor"
                        && name != "venv"
                        && name != ".venv"
                        && name != "cache"
                        && name != "tmp"
                    {
                        subdirs.push((name, path));
                    }
                }
            }
            subdirs.sort_by(|a, b| a.0.cmp(&b.0));

            for (sub_name, sub_path) in subdirs {
                for detector in &detectors {
                    if let Some(mut info) = detector.detect(&sub_path) {
                        info.primary_verifier =
                            format!("cd {} && {}", sub_name, info.primary_verifier);
                        info.secondary_verifiers = info
                            .secondary_verifiers
                            .into_iter()
                            .map(|v| format!("cd {} && {}", sub_name, v))
                            .collect();

                        if !results.iter().any(|r: &ProjectLanguageInfo| {
                            r.primary_verifier == info.primary_verifier
                        }) {
                            results.push(info);
                        }
                    }
                }
            }
        }
    }

    results
}

/// Extracts the primary binary/executable name from a shell command string.
/// E.g. "cargo check" -> "cargo", "cd sub && npm test" -> "npm", "(cd dir && pytest)" -> "pytest"
pub fn extract_binary_from_command(cmd: &str) -> Option<String> {
    let clean = cmd.trim().trim_start_matches('(').trim_end_matches(')');
    let actual_cmd = if let Some((_, after)) = clean.split_once(" && ") {
        after.trim()
    } else {
        clean
    };
    let first_token = actual_cmd.split_whitespace().next()?;
    Some(first_token.to_string())
}

/// Checks if an executable binary is installed and reachable in the environment's PATH,
/// or exists as a local script/binary in the workspace.
pub fn is_executable_available(bin_name: &str, workspace_roots: &[String]) -> bool {
    let trimmed = bin_name.trim();
    if trimmed.is_empty() {
        return false;
    }

    // 1. If it's a relative path like ./gradlew or bin/test
    if trimmed.starts_with("./")
        || trimmed.starts_with("../")
        || trimmed.contains('/')
        || trimmed.contains('\\')
    {
        for root in workspace_roots {
            let p = Path::new(root).join(trimmed);
            if p.exists() {
                return true;
            }
        }
    }

    // 2. Check system PATH
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            let direct = path.join(trimmed);
            if direct.is_file() {
                return true;
            }
            #[cfg(windows)]
            {
                for ext in &[".exe", ".cmd", ".bat", ".ps1"] {
                    let with_ext = path.join(format!("{}{}", trimmed, ext));
                    if with_ext.is_file() {
                        return true;
                    }
                }
            }
        }
    }

    false
}

/// Returns the primary verification command for the detected workspace.
/// For multi-component workspaces, chains each component verifier in a subshell.
/// Probes host environment availability so uninstalled tools are not recommended.
pub fn detect_primary_verification_command(allowed_folders: &[String]) -> Option<String> {
    let languages = detect_all_languages(allowed_folders);
    let verified_languages: Vec<ProjectLanguageInfo> = languages
        .into_iter()
        .filter(|lang| {
            if let Some(bin) = extract_binary_from_command(&lang.primary_verifier) {
                is_executable_available(&bin, allowed_folders)
            } else {
                true
            }
        })
        .collect();

    if verified_languages.is_empty() {
        None
    } else if verified_languages.len() == 1 {
        Some(verified_languages[0].primary_verifier.clone())
    } else {
        // Multi-component: chain them together in subshells so cwd changes do not interfere
        let chained = verified_languages
            .into_iter()
            .map(|l| {
                if l.primary_verifier.starts_with("cd ") {
                    format!("({})", l.primary_verifier)
                } else {
                    l.primary_verifier
                }
            })
            .collect::<Vec<_>>()
            .join(" && ");
        Some(chained)
    }
}

/// Returns the verification commands relevant to the modified files, or all detected primary verifiers.
/// Filters out commands whose executables are not installed on the system.
pub fn detect_verifiers_for_modified_files(
    allowed_folders: &[String],
    modified_files: &[String],
) -> Vec<String> {
    let languages = detect_all_languages(allowed_folders);
    if languages.is_empty() {
        return Vec::new();
    }

    let mut candidate_verifiers: Vec<String> = if modified_files.is_empty() {
        languages.into_iter().map(|l| l.primary_verifier).collect()
    } else {
        let mut relevant = Vec::new();
        for lang in &languages {
            if let Some(rest) = lang.primary_verifier.strip_prefix("cd ") {
                if let Some((sub_name, _)) = rest.split_once(" && ") {
                    let sub_trimmed = sub_name.trim();
                    let is_modified = modified_files.iter().any(|f| {
                        let clean = f.trim_start_matches("./");
                        clean.starts_with(sub_trimmed)
                            || clean.contains(&format!("/{}/", sub_trimmed))
                            || clean.contains(&format!("{}/", sub_trimmed))
                    });
                    if is_modified && !relevant.contains(&lang.primary_verifier) {
                        relevant.push(lang.primary_verifier.clone());
                    }
                }
            } else {
                // Root project verifier
                if !relevant.contains(&lang.primary_verifier) {
                    relevant.push(lang.primary_verifier.clone());
                }
            }
        }
        if relevant.is_empty() {
            languages.into_iter().map(|l| l.primary_verifier).collect()
        } else {
            relevant
        }
    };

    // Filter by probe: only keep verifiers whose executables are actually available in the environment
    candidate_verifiers.retain(|cmd| {
        if let Some(bin) = extract_binary_from_command(cmd) {
            is_executable_available(&bin, allowed_folders)
        } else {
            true
        }
    });

    candidate_verifiers
}
/// Checks if an executed shell command is a verification / test / typecheck / build command across any language.
///
/// The rule tables live in [`command_rules`]; the logic here is only the ordering:
/// 1. split chained commands, 2. drop package management, 3. accept known verifiers,
/// 4. fall back to language-agnostic signals.
///
/// Noise filtering is deliberately *not* a blanket early `continue`. Informational and
/// filesystem commands are only discarded **after** the verifier checks have failed, so
/// a verifier whose name shares a prefix with an informational command can never be
/// silently shadowed.
pub fn is_verification_command(cmd: &str) -> bool {
    use command_rules::*;

    let lower = cmd.trim().to_lowercase();

    for sub in split_chained_commands(&lower) {
        let sub_trimmed = trim_sub_command(sub);

        if sub_trimmed.is_empty() {
            continue;
        }

        // Known verifiers win over every noise table, so no informational entry can
        // ever shadow a real verifier — the failure mode a blanket skip invites.
        if matches_prefix(sub_trimmed, KNOWN_VERIFICATION_PREFIXES) {
            return true;
        }

        // Package installation/management never proves the code is correct, even when a
        // keyword such as `build` shows up among the arguments.
        if matches_prefix(sub_trimmed, PKG_MGMT_PREFIXES) {
            continue;
        }

        // A bare filesystem probe reports a fact about the filesystem, never about
        // behaviour: `Test-Path src` succeeding says nothing about whether the code
        // works. This has to be decided *before* the argument scan, because `Path` is
        // otherwise indistinguishable from a verification keyword in tail position.
        if is_probe_only(sub_trimmed) {
            continue;
        }

        // Universal language-agnostic detection:
        // Any command invoking a test runner, linter, typechecker, or compiler script
        let mut parts = sub_trimmed.split_whitespace();
        let Some(first) = parts.next() else {
            continue;
        };
        let base_cmd = first.rsplit(['/', '\\']).next().unwrap_or(first);

        if BUILD_TOOL_BINARIES.contains(&base_cmd) {
            return true;
        }

        // Script executions e.g. "./test.sh", "bash run_tests.sh", "sh check.sh", "python3 tests/test.py"
        if SCRIPT_VERIFY_KEYWORDS
            .iter()
            .any(|kw| base_cmd.contains(kw))
        {
            return true;
        }

        // Arguments containing verification keywords
        if parts.any(|arg| {
            VERIFY_ARGS.contains(&arg) || VERIFY_ARG_SUBSTRINGS.iter().any(|kw| arg.contains(kw))
        }) {
            return true;
        }

        // No verification signal found. Informational output and filesystem bookkeeping
        // are dropped *here* rather than up front, so they only ever suppress themselves:
        // `mkdir -p dist && npm run build` is still judged on the build, and a verifier
        // sharing a prefix with `ls` / `dir` is never swallowed by the probe table.
        if matches_prefix(sub_trimmed, PURE_INFO_PREFIXES)
            || matches_prefix(sub_trimmed, FS_MUTATION_PREFIXES)
            || matches_prefix(sub_trimmed, FS_PROBE_PREFIXES)
        {
            continue;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_rust_detection() {
        let temp = std::env::temp_dir().join("test_agent_rust_proj");
        let _ = fs::create_dir_all(&temp);
        fs::write(temp.join("Cargo.toml"), "[package]\nname = \"foo\"").unwrap();

        let detected = detect_all_languages(&[temp.to_string_lossy().to_string()]);
        assert_eq!(detected.len(), 1);
        assert_eq!(detected[0].language, "Rust");
        assert_eq!(detected[0].primary_verifier, "cargo test");

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_is_verification_command() {
        assert!(is_verification_command("cargo test --lib"));
        assert!(is_verification_command("npm test"));
        assert!(is_verification_command("npx tsc --noEmit"));
        assert!(is_verification_command("pytest tests/"));
        assert!(is_verification_command(
            "python3 -m unittest discover -s tests"
        ));
        assert!(is_verification_command(
            "cd simulations/python_scenario && python3 -m unittest discover -s tests"
        ));
        assert!(is_verification_command("./test.sh"));
        assert!(is_verification_command("make check"));
        assert!(is_verification_command("bazel test //..."));
        assert!(is_verification_command("just test"));
        assert!(!is_verification_command("git status"));
        assert!(!is_verification_command("git diff"));
        assert!(!is_verification_command("ls -la"));
        assert!(!is_verification_command("npm install axios"));
        assert!(!is_verification_command("npm i"));
        assert!(!is_verification_command("cargo add serde"));
        assert!(!is_verification_command("cargo tree"));
        assert!(!is_verification_command("yarn add react"));
        assert!(!is_verification_command("pip install requests"));
        assert!(!is_verification_command("poetry add fastapi"));
        assert!(is_verification_command("node --test tests/*.test.js"));
        assert!(is_verification_command("go test ./..."));
        assert!(is_verification_command("flutter test"));
        assert!(is_verification_command("dotnet test"));
        assert!(is_verification_command(
            "(cd backend && cargo test) && (cd frontend && npm run build)"
        ));
        assert!(!is_verification_command("git status"));
        assert!(!is_verification_command("ls -la"));

        let failing_json = r#"{"command":"npx tsc --noEmit","cwd":"/home/headway/MyWork/project/portofolio","error_banner":"❌ COMMAND FAILED with exit code 2.","exit_code":2,"status":"FAILED","stderr":"","stdout":"src/App.tsx(28,5): error TS2741","success":false}"#;
        assert!(crate::agent::diagnostics::is_command_output_error(
            failing_json
        ));

        let success_json = r#"{"command":"npx tsc --noEmit","cwd":"/home/headway/MyWork/project/portofolio","exit_code":0,"stderr":"","stdout":"","success":true}"#;
        assert!(!crate::agent::diagnostics::is_command_output_error(
            success_json
        ));
    }

    /// Filesystem mutations are not verification, however they are spelled.
    ///
    /// A real session recorded `New-Item ... ; Test-Path ...` as a PASSED verification
    /// because the classifier only knew a blocklist of informational commands, and
    /// `New-Item` was not on it. An inventory of forbidden commands will always be
    /// incomplete, so the failure mode to guard against is "unrecognised command is
    /// treated as evidence of correctness".
    #[test]
    fn filesystem_mutations_never_count_as_verification() {
        for cmd in [
            "New-Item -ItemType Directory -Force -Path src\\components | Out-Null; Test-Path src\\components",
            "New-Item -ItemType Directory -Force -Path src",
            "mkdir -p src/components",
            "touch src/index.css",
            "cp a.rs b.rs",
            "mv a.rs b.rs",
            "rm -rf dist",
            "Set-Content -Path a.txt -Value 'x'",
        ] {
            assert!(
                !is_verification_command(cmd),
                "`{cmd}` only changes the filesystem; it proves nothing about correctness"
            );
        }
    }

    /// The classes of command that *are* verification must keep working, so the guard
    /// above cannot be satisfied by simply returning `false` everywhere.
    #[test]
    fn genuine_verification_is_still_recognised() {
        for cmd in [
            "npm run build",
            "cargo test",
            "pytest -q",
            "go test ./...",
            "mkdir -p out && npm run build",
        ] {
            assert!(
                is_verification_command(cmd),
                "`{cmd}` must count as verification"
            );
        }
    }

    /// A generic interpreter invocation is not verification: `python -m http.server`
    /// starts a server and proves nothing about the code.
    #[test]
    fn generic_module_invocations_are_not_verification() {
        for cmd in [
            "python -m http.server 8000",
            "python3 -m venv .venv",
            "py -m pip install requests",
            "uv run python app.py",
            "poetry run python app.py",
        ] {
            assert!(
                !is_verification_command(cmd),
                "`{cmd}` runs an interpreter but verifies nothing"
            );
        }
    }

    /// Informational prefixes must never shadow a real verifier.
    ///
    /// `FS_PROBE_PREFIXES` and `PURE_INFO_PREFIXES` contain short entries such as `ls`
    /// and `dir`. If those were consulted before the verifier tables, any command
    /// starting with those letters would be skipped. This pins the ordering: the
    /// verifier checks always run first.
    #[test]
    fn informational_prefixes_do_not_shadow_verifiers() {
        for cmd in [
            "ls-tests && cargo test",
            "dir-check; go test ./...",
            "curl -s localhost:3000 | grep ok && pytest",
        ] {
            assert!(
                is_verification_command(cmd),
                "`{cmd}` contains a real verifier and must not be filtered as noise"
            );
        }
    }

    #[test]
    fn test_multi_subproject_detection() {
        let temp = std::env::temp_dir().join("test_agent_fullstack_proj");
        let backend = temp.join("backend");
        let frontend = temp.join("frontend");
        let _ = fs::create_dir_all(&backend);
        let _ = fs::create_dir_all(&frontend);

        fs::write(backend.join("Cargo.toml"), "[package]\nname = \"backend\"").unwrap();
        fs::write(
            frontend.join("package.json"),
            r#"{"name": "frontend", "scripts": {"check": "tsc --noEmit", "build": "vite build"}}"#,
        )
        .unwrap();

        let allowed = vec![temp.to_string_lossy().to_string()];
        let detected = detect_all_languages(&allowed);
        assert_eq!(detected.len(), 2);
        assert_eq!(detected[0].primary_verifier, "cd backend && cargo test");
        assert_eq!(
            detected[1].primary_verifier,
            "cd frontend && npm run check && npm run build"
        );

        let primary_chained = detect_primary_verification_command(&allowed);
        if is_executable_available("npm", &allowed) {
            assert_eq!(
                primary_chained,
                Some(
                    "(cd backend && cargo test) && (cd frontend && npm run check && npm run build)"
                        .to_string()
                )
            );
        } else {
            assert_eq!(
                primary_chained,
                Some("cd backend && cargo test".to_string())
            );
        }

        // Test file scoping
        let mod_frontend = vec!["frontend/src/App.tsx".to_string()];
        let scoped_front = detect_verifiers_for_modified_files(&allowed, &mod_frontend);
        if is_executable_available("npm", &allowed) {
            assert_eq!(
                scoped_front,
                vec!["cd frontend && npm run check && npm run build".to_string()]
            );
        } else {
            assert!(scoped_front.is_empty());
        }

        let mod_both = vec![
            "backend/src/main.rs".to_string(),
            "frontend/src/App.tsx".to_string(),
        ];
        let scoped_both = detect_verifiers_for_modified_files(&allowed, &mod_both);
        if is_executable_available("npm", &allowed) {
            assert_eq!(
                scoped_both,
                vec![
                    "cd backend && cargo test".to_string(),
                    "cd frontend && npm run check && npm run build".to_string()
                ]
            );
        } else {
            assert_eq!(scoped_both, vec!["cd backend && cargo test".to_string()]);
        }

        let _ = fs::remove_dir_all(&temp);
    }
}
