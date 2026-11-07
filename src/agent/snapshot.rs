use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

static GLOBAL_SNAPSHOT: OnceLock<WorkspaceSnapshot> = OnceLock::new();

/// Manages in-memory snapshots of files before they are edited or written.
/// Provides safe rollback capability for autonomous execution cycles.
///
/// The snapshot store is global and lives as long as the process. Nothing used
/// to call `clear`, so every file the agent ever touched stayed resident in
/// memory for the lifetime of the app. A turn boundary must release it.
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
    ///
    /// A poisoned lock must not be reported as "nothing is tracked": callers use
    /// this to decide whether a rollback is needed, so silently returning an
    /// empty list would let un-restored files go unnoticed.
    pub fn tracked_files(&self) -> Result<Vec<PathBuf>> {
        let backups = self
            .backups
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
        Ok(backups.keys().cloned().collect())
    }

    /// Clear all snapshots without restoring.
    ///
    /// Clearing is not rolling back: the file on disk stays as it is.
    pub fn clear(&self) -> Result<()> {
        let mut backups = self
            .backups
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
        backups.clear();
        Ok(())
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
        assert_eq!(
            snap.tracked_files().unwrap().len(),
            1,
            "the file must be tracked after backup"
        );

        snap.clear().unwrap();
        assert!(snap.tracked_files().unwrap().is_empty());

        // Clearing is not rolling back: the file on disk stays as it is.
        assert_eq!(std::fs::read(&file).unwrap(), b"original");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The revert tool is only worth exposing if it actually restores content and
    /// deletes files the session created.
    #[test]
    fn rollback_restores_edited_content_and_removes_created_files() {
        let snap = WorkspaceSnapshot::new();
        let dir = std::env::temp_dir().join(format!("snap-rb-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let edited = dir.join("edited.txt");
        let created = dir.join("created.txt");
        std::fs::write(&edited, b"before").unwrap();

        snap.backup_file(&edited).unwrap();
        snap.backup_file(&created).unwrap(); // does not exist yet

        std::fs::write(&edited, b"after").unwrap();
        std::fs::write(&created, b"new file").unwrap();

        let restored = snap.rollback_all().unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(
            std::fs::read(&edited).unwrap(),
            b"before",
            "edited file must be restored"
        );
        assert!(
            !created.exists(),
            "a file created during the session must be removed"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Poisons the lock, then proves `clear`/`tracked_files` report the failure
    /// instead of pretending the store is empty.
    #[test]
    fn poisoned_lock_is_reported_not_swallowed() {
        let snap = WorkspaceSnapshot::new();
        let weak = std::sync::Arc::downgrade(&snap.backups);
        let handle = {
            let snap = snap.clone();
            std::thread::spawn(move || {
                let _guard = snap.backups.lock().unwrap();
                panic!("intentional panic to poison the snapshot lock");
            })
        };
        let _ = handle.join();
        // Keep the Arc alive so the Mutex stays poisoned rather than dropped.
        drop(weak);

        assert!(snap.tracked_files().is_err(), "poisoned lock must surface as an error");
        assert!(snap.clear().is_err(), "poisoned lock must surface as an error");
    }
}
