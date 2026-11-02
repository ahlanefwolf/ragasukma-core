use std::fs;
use std::path::PathBuf;

pub fn get_assets_dir() -> PathBuf {
    // 1. In dev mode (Cargo.toml exists), prioritize local assets/ in project root
    if PathBuf::from("Cargo.toml").exists() {
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
        && let Some(exe_dir) = exe_path.parent() {
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
        && let Some(exe_dir) = exe_path.parent() {
            candidates.push(exe_dir.join("assets").join(filename));
            candidates.push(exe_dir.join(filename));
            candidates.push(exe_dir.join("../Resources/assets").join(filename));
            candidates.push(exe_dir.join("../Resources").join(filename));
        }

    // 6. Dev mode candidates
    if PathBuf::from("Cargo.toml").exists() {
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
        && let Some(exe_dir) = exe_path.parent() {
            candidates.push(exe_dir.join("assets").join(filename));
            candidates.push(exe_dir.join(filename));
            candidates.push(exe_dir.join("../Resources/assets").join(filename));
            candidates.push(exe_dir.join("../Resources").join(filename));
        }

    // 6. Dev mode candidates
    if PathBuf::from("Cargo.toml").exists() {
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
