use std::path::PathBuf;

pub fn get_assets_dir() -> PathBuf {
    crate::config::get_app_config_dir().join("assets")
}

pub fn find_file_in_candidates(filename: &str) -> PathBuf {
    let mut candidates = vec![
        get_assets_dir().join(filename),
        crate::config::get_app_data_dir().join("assets").join(filename),
        crate::config::get_app_data_dir().join(filename),
        crate::config::get_app_config_dir().join("assets").join(filename),
        crate::config::get_app_config_dir().join(filename),
        crate::config::get_app_cache_dir().join(filename),
    ];

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            candidates.push(exe_dir.join("assets").join(filename));
            candidates.push(exe_dir.join(filename));
            candidates.push(exe_dir.join("../Resources/assets").join(filename));
            candidates.push(exe_dir.join("../Resources").join(filename));
        }
    }

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
        get_assets_dir().join(filename),
        crate::config::get_app_data_dir().join("assets").join(filename),
        crate::config::get_app_data_dir().join(filename),
        crate::config::get_app_config_dir().join("assets").join(filename),
        crate::config::get_app_config_dir().join(filename),
        crate::config::get_app_cache_dir().join(filename),
    ];

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            candidates.push(exe_dir.join("assets").join(filename));
            candidates.push(exe_dir.join(filename));
            candidates.push(exe_dir.join("../Resources/assets").join(filename));
            candidates.push(exe_dir.join("../Resources").join(filename));
        }
    }

    if PathBuf::from("Cargo.toml").exists() {
        candidates.push(PathBuf::from(format!("assets/{}", filename)));
        candidates.push(PathBuf::from(filename));
        candidates.push(PathBuf::from(format!("../assets/{}", filename)));
        candidates.push(PathBuf::from(format!("../{}", filename)));
    }

    candidates
}
