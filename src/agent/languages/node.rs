use std::fs;
use std::path::Path;
use super::{LanguageDetector, ProjectLanguageInfo};

pub struct NodeDetector;

impl LanguageDetector for NodeDetector {
    fn detect(&self, root: &Path) -> Option<ProjectLanguageInfo> {
        let pkg_json = root.join("package.json");
        if !pkg_json.exists() {
            if root.join("deno.json").exists() || root.join("deno.jsonc").exists() {
                return Some(ProjectLanguageInfo {
                    language: "Deno / TypeScript",
                    primary_verifier: "deno test".to_string(),
                    secondary_verifiers: vec!["deno check".to_string(), "deno lint".to_string()],
                });
            }
            if root.join("bun.lockb").exists() || root.join("bun.lock").exists() {
                return Some(ProjectLanguageInfo {
                    language: "Bun",
                    primary_verifier: "bun test".to_string(),
                    secondary_verifiers: vec!["bun run build".to_string()],
                });
            }
            return None;
        }

        let is_pnpm = root.join("pnpm-lock.yaml").exists();
        let is_yarn = root.join("yarn.lock").exists();
        let is_bun = root.join("bun.lockb").exists() || root.join("bun.lock").exists();
        let pkg_runner = if is_pnpm {
            "pnpm"
        } else if is_yarn {
            "yarn"
        } else if is_bun {
            "bun"
        } else {
            "npm"
        };

        let mut primary = format!("{} test", pkg_runner);
        let mut secondaries = Vec::new();

        if let Ok(content) = fs::read_to_string(&pkg_json)
            && let Ok(val) = serde_json::from_str::<serde_json::Value>(&content)
                && let Some(scripts) = val.get("scripts").and_then(|s| s.as_object()) {
                    let has_test_script = scripts.contains_key("test");
                    let test_is_dummy = scripts
                        .get("test")
                        .and_then(|t| t.as_str())
                        .map(|t| t.contains("no test specified") || t.contains("exit 1"))
                        .unwrap_or(false);

                    if !has_test_script || test_is_dummy {
                        // If test script is absent or dummy, prioritize check / build scripts
                        if scripts.contains_key("check") && scripts.contains_key("build") {
                            primary = format!("{} run check && {} run build", pkg_runner, pkg_runner);
                        } else if scripts.contains_key("check") {
                            primary = format!("{} run check", pkg_runner);
                        } else if scripts.contains_key("typecheck") {
                            primary = format!("{} run typecheck", pkg_runner);
                        } else if root.join("tsconfig.json").exists() && scripts.contains_key("build") {
                            primary = format!("npx tsc --noEmit && {} run build", pkg_runner);
                        } else if root.join("tsconfig.json").exists() {
                            primary = "npx tsc --noEmit".to_string();
                        } else if scripts.contains_key("build") {
                            primary = format!("{} run build", pkg_runner);
                        }
                    }

                    if scripts.contains_key("typecheck") && !primary.contains("typecheck") {
                        secondaries.push(format!("{} run typecheck", pkg_runner));
                    }
                    if scripts.contains_key("check") {
                        secondaries.push(format!("{} run check", pkg_runner));
                    }
                    if scripts.contains_key("build") && !primary.contains("build") {
                        secondaries.push(format!("{} run build", pkg_runner));
                    }
                    if scripts.contains_key("lint") {
                        secondaries.push(format!("{} run lint", pkg_runner));
                    }
                }

        if root.join("tsconfig.json").exists() && !primary.contains("tsc") && !secondaries.iter().any(|s| s.contains("tsc")) {
            secondaries.push("npx tsc --noEmit".to_string());
        }

        Some(ProjectLanguageInfo {
            language: "Node.js / TypeScript",
            primary_verifier: primary,
            secondary_verifiers: secondaries,
        })
    }
}
