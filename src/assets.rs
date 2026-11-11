use std::fs;
use std::path::{Path, PathBuf};

/// Environment variable that explicitly turns dev mode on (`1`/`true`/`yes`/`on`)
/// or off (`0`/`false`/`no`/`off`), regardless of build profile.
pub const DEV_MODE_ENV: &str = "RAGASUKMA_DEV";

/// Whether runtime data (DB, accounts, config, music) should live in the
/// current directory's `assets/` folder instead of the per-user data directory.
///
/// Dev mode is only inferred for **debug builds** started from a directory that
/// contains a `Cargo.toml`. Release binaries therefore never write their data
/// into whatever Rust project the user happens to launch them from.
/// Set `RAGASUKMA_DEV` to force the behaviour either way.
pub fn is_dev_mode() -> bool {
    resolve_dev_mode(
        std::env::var(DEV_MODE_ENV).ok().as_deref(),
        cfg!(debug_assertions),
        Path::new("Cargo.toml").exists(),
    )
}

/// Pure decision logic behind [`is_dev_mode`], kept separate so it can be tested
/// without mutating process-wide environment variables or the working directory.
fn resolve_dev_mode(env_value: Option<&str>, debug_build: bool, cargo_toml_in_cwd: bool) -> bool {
    match env_value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("1" | "true" | "yes" | "on") => true,
        Some("0" | "false" | "no" | "off") => false,
        // Unset, empty, or unrecognised values fall back to auto-detection.
        _ => debug_build && cargo_toml_in_cwd,
    }
}

pub fn get_assets_dir() -> PathBuf {
    // 1. In dev mode (debug build run from a crate root, or RAGASUKMA_DEV=1),
    //    prioritize local assets/ in the project root
    if is_dev_mode() {
        let local = PathBuf::from("assets");
        let _ = fs::create_dir_all(&local);
        return local;
    }

    // 2. Standard persistent user data directory:
    //    Linux: ~/.local/share/ragasukma/assets
    //    macOS: ~/Library/Application Support/ragasukma/assets
    //    Windows: %LOCALAPPDATA%\ragasukma\assets
    let data_dir = crate::config::get_app_data_dir().join("assets");
    if data_dir.exists() {
        return data_dir;
    }

    // 3. Backward compatibility: legacy config assets directory (~/.config/ragasukma/assets)
    let legacy_cfg = crate::config::get_app_config_dir().join("assets");
    if legacy_cfg.exists() {
        return legacy_cfg;
    }

    // 4. Bundled / portable distribution: only if exe_dir/assets contains real bundled assets
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        let beside = exe_dir.join("assets");
        if beside.is_dir()
            && (beside.join("char.vrm").exists()
                || beside.join("icon.png").exists()
                || beside.join("i18n").exists())
        {
            return beside;
        }
        let mac_resources = exe_dir.join("../Resources/assets");
        if mac_resources.exists() {
            return mac_resources;
        }
    }

    let _ = fs::create_dir_all(&data_dir);
    data_dir
}

pub fn find_file_in_candidates(filename: &str) -> PathBuf {
    let mut candidates = vec![
        // 1. Active assets directory (user data dir or legacy config dir or dev assets)
        get_assets_dir().join(filename),
        // 2. Standard user data directory candidates
        crate::config::get_app_data_dir()
            .join("assets")
            .join(filename),
        crate::config::get_app_data_dir().join(filename),
        // 3. Backward-compatible config directory candidates
        crate::config::get_app_config_dir()
            .join("assets")
            .join(filename),
        crate::config::get_app_config_dir().join(filename),
        // 4. Cache directory candidate
        crate::config::get_app_cache_dir().join(filename),
    ];

    // 5. Bundled / portable beside exe
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        candidates.push(exe_dir.join("assets").join(filename));
        candidates.push(exe_dir.join(filename));
        candidates.push(exe_dir.join("../Resources/assets").join(filename));
        candidates.push(exe_dir.join("../Resources").join(filename));
    }

    // 6. Dev mode candidates
    if is_dev_mode() {
        candidates.push(PathBuf::from(format!("assets/{}", filename)));
        candidates.push(PathBuf::from(filename));
        candidates.push(PathBuf::from(format!("../assets/{}", filename)));
        candidates.push(PathBuf::from(format!("../{}", filename)));
    }

    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("LOCALAPPDATA").or_else(|_| std::env::var("APPDATA")) {
        let appdata_path = PathBuf::from(appdata);
        let p = appdata_path.join("ragasukma").join("assets").join(filename);
        if p.exists() {
            return p;
        }
        let p_root = appdata_path.join("ragasukma").join(filename);
        if p_root.exists() {
            return p_root;
        }
    }

    for c in &candidates {
        if c.exists() {
            return c.clone();
        }
    }

    get_assets_dir().join(filename)
}

pub fn find_all_file_candidates(filename: &str) -> Vec<PathBuf> {
    let mut candidates = vec![
        // 1. Active assets directory
        get_assets_dir().join(filename),
        // 2. Standard user data directory candidates
        crate::config::get_app_data_dir()
            .join("assets")
            .join(filename),
        crate::config::get_app_data_dir().join(filename),
        // 3. Backward-compatible config directory candidates
        crate::config::get_app_config_dir()
            .join("assets")
            .join(filename),
        crate::config::get_app_config_dir().join(filename),
        // 4. Cache directory candidate
        crate::config::get_app_cache_dir().join(filename),
    ];

    // 5. Bundled / portable beside exe
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        candidates.push(exe_dir.join("assets").join(filename));
        candidates.push(exe_dir.join(filename));
        candidates.push(exe_dir.join("../Resources/assets").join(filename));
        candidates.push(exe_dir.join("../Resources").join(filename));
    }

    // 6. Dev mode candidates
    if is_dev_mode() {
        candidates.push(PathBuf::from(format!("assets/{}", filename)));
        candidates.push(PathBuf::from(filename));
        candidates.push(PathBuf::from(format!("../assets/{}", filename)));
        candidates.push(PathBuf::from(format!("../{}", filename)));
    }

    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("LOCALAPPDATA").or_else(|_| std::env::var("APPDATA")) {
        let appdata_path = PathBuf::from(appdata);
        candidates.push(appdata_path.join("ragasukma").join("assets").join(filename));
        candidates.push(appdata_path.join("ragasukma").join(filename));
    }

    let mut existing = Vec::new();
    for c in candidates {
        if c.exists() && !existing.contains(&c) {
            existing.push(c);
        }
    }
    existing
}

#[cfg(test)]
mod tests {
    use super::resolve_dev_mode;

    #[test]
    fn auto_detect_requires_debug_build_and_cargo_toml() {
        assert!(resolve_dev_mode(None, true, true));
        assert!(!resolve_dev_mode(None, true, false));
        // Release binary launched from some other Rust project must not use ./assets
        assert!(!resolve_dev_mode(None, false, true));
        assert!(!resolve_dev_mode(None, false, false));
    }

    #[test]
    fn env_override_forces_dev_mode_on() {
        for v in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(resolve_dev_mode(Some(v), false, false), "value {v:?}");
        }
    }

    #[test]
    fn env_override_forces_dev_mode_off() {
        for v in ["0", "false", "No", "off"] {
            assert!(!resolve_dev_mode(Some(v), true, true), "value {v:?}");
        }
    }

    #[test]
    fn unrecognised_env_value_falls_back_to_auto_detect() {
        assert!(resolve_dev_mode(Some(""), true, true));
        assert!(resolve_dev_mode(Some("maybe"), true, true));
        assert!(!resolve_dev_mode(Some("maybe"), false, true));
    }
}
