use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

static GLOBAL_SNAPSHOT: OnceLock<WorkspaceSnapshot> = OnceLock::new();

/// Manages in-memory snapshots of files before they are edited or written.
/// Provides safe rollback capability for autonomous execution cycles.
#[derive(Clone, Default)]
pub struct WorkspaceSnapshot {
    backups: Arc<Mutex<HashMap<PathBuf, Option<Vec<u8>>>>>,
}

impl WorkspaceSnapshot {
    pub fn new() -> Self {
        Self {
            backups: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn global() -> &'static WorkspaceSnapshot {
        GLOBAL_SNAPSHOT.get_or_init(Self::new)
    }

    /// Snapshot a file before modifying it.
    /// If the file does not exist, `None` is stored so rollback can delete created files.
    pub fn backup_file(&self, path: &Path) -> Result<()> {
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut backups = self
            .backups
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
        if let std::collections::hash_map::Entry::Vacant(e) = backups.entry(abs_path) {
            let content = if e.key().exists() {
                Some(std::fs::read(e.key())?)
            } else {
                None
            };
            e.insert(content);
        }
        Ok(())
    }

    /// Rollback a specific file to its original state before any modifications.
    pub fn rollback_file(&self, path: &Path) -> Result<bool> {
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut backups = self
            .backups
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
        if let Some(original) = backups.remove(&abs_path) {
            match original {
                Some(bytes) => {
                    if let Some(parent) = abs_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&abs_path, bytes)?;
                }
                None => {
                    if abs_path.exists() {
                        let _ = std::fs::remove_file(&abs_path);
                    }
                }
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Rollback all files modified during this session.
    pub fn rollback_all(&self) -> Result<Vec<PathBuf>> {
        let mut backups = self
            .backups
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
        let mut restored = Vec::new();
        for (path, original) in backups.drain() {
            match original {
                Some(bytes) => {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(&path, bytes);
                }
                None => {
                    if path.exists() {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
            restored.push(path);
        }
        Ok(restored)
    }

    /// Returns list of all tracked files currently in snapshot.
    pub fn tracked_files(&self) -> Vec<PathBuf> {
        self.backups
            .lock()
            .map(|b| b.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Clear all snapshots without restoring.
    pub fn clear(&self) {
        if let Ok(mut backups) = self.backups.lock() {
            backups.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_releases_every_tracked_backup() {
        let snap = WorkspaceSnapshot::new();
        let dir = std::env::temp_dir().join(format!("snap-clear-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("a.txt");
        std::fs::write(&file, b"original").unwrap();

        snap.backup_file(&file).unwrap();
        assert_eq!(snap.tracked_files().len(), 1);

        snap.clear();
        assert!(snap.tracked_files().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
