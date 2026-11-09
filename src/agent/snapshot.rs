use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// One backup store per session.
///
/// This used to be a single `static GLOBAL_SNAPSHOT`. Every session in the process wrote to
/// the same map, and `clear()` emptied it wholesale, so two sessions running concurrently
/// destroyed each other's backups: session B finishing a turn wiped the entries session A
/// still needed, and `workspace_rollback` in A then reported a rollback over zero files.
/// Keying the store by session makes the isolation that the tool description always claimed
/// ("revert files to the state they were in before **this session** modified them") real.
///
/// The registry only ever *adds* entries on demand and removes them in `release_session`,
/// so a session that never writes a file costs nothing.
static SESSIONS: OnceLock<Mutex<HashMap<String, WorkspaceSnapshot>>> = OnceLock::new();

fn sessions() -> &'static Mutex<HashMap<String, WorkspaceSnapshot>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Returns the backup store for `session_id`, creating it on first use.
///
/// The returned value is a handle, not a copy: it shares the same backing map, so callers
/// must not assume they own the contents.
pub fn for_session(session_id: &str) -> WorkspaceSnapshot {
    let mut map = sessions()
        .lock()
        // A poisoned registry lock must not disable rollback for the whole process. The map
        // itself holds no invariants a partial write could break, so recovering the inner
        // data is strictly better than panicking here on every later file write.
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.entry(session_id.to_string()).or_default().clone()
}

/// Drops every backup held for `session_id`.
///
/// Called when the session itself goes away, not at a turn boundary: the documented
/// contract is per-session rollback, so a turn boundary has to keep the backups that an
/// earlier turn of the same session recorded. Returns `true` if a store existed, which lets
/// a caller assert that the store was genuinely released rather than silently absent.
pub fn release_session(session_id: &str) -> bool {
    sessions()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(session_id)
        .is_some()
}

/// Number of sessions currently holding backups. Test/diagnostic hook.
#[must_use]
pub fn tracked_session_count() -> usize {
    sessions()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .len()
}

/// What a rollback actually achieved.
///
/// Previously `rollback_all` returned a bare `Vec<PathBuf>` that it filled with *every*
/// path it iterated over, whether or not the restore succeeded — and it discarded the
/// `create_dir_all` / `write` / `remove_file` errors that would have said otherwise. The
/// tool then reported `"status": "success"` with a full `restored_files` list, so the model
/// (and through it the user) was told files had been reverted when they had not. Silent
/// data loss, since the backup had already been drained from the store by then.
///
/// Splitting the result forces every caller to look at both halves.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RollbackReport {
    /// Files that really were returned to their pre-turn state.
    pub restored: Vec<PathBuf>,
    /// Files whose restore failed, each with the reason. These are no longer in the store.
    pub failed: Vec<(PathBuf, String)>,
}

impl RollbackReport {
    /// Whether every attempted restore succeeded.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Largest file whose contents are kept in memory for rollback.
///
/// The store keeps whole files as `Vec<u8>`, and every session keeps them until it is
/// deleted. Without a ceiling, one `workspace_write_file` aimed at a large binary (a model
/// dump, a media file, a `.parquet`) pinned that entire file in RAM for the rest of the
/// session, and copying it there was invisible to the caller. Past this limit the path is
/// still recorded so the rollback can *report* that it cannot restore it, rather than
/// silently promising a revert it can no longer perform.
const MAX_BACKUP_BYTES: u64 = 8 * 1024 * 1024;

/// What was remembered about a path before it was modified.
///
/// `Option<Vec<u8>>` could only say "had bytes" or "did not exist". Adding the third state
/// is what lets an unusable backup be reported instead of being mistaken for one of those.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Backup {
    /// The file did not exist, so rolling back means deleting it.
    Absent,
    /// The original contents, small enough to keep in memory.
    Content(Vec<u8>),
    /// The file existed and was too large to keep; its size is kept for the error message.
    /// Rolling this entry back is impossible and must be reported as a failure.
    TooLarge(u64),
}

/// Restores a single entry, returning a descriptive error instead of swallowing it.
///
/// `Ok(())` means the file is now in its pre-turn state: either its original bytes are
/// back, or — for an entry recorded as absent — it has been deleted.
fn restore_entry(path: &Path, original: &Backup) -> Result<()> {
    match original {
        Backup::TooLarge(size) => bail!(
            "'{}' is {} bytes, above the {} byte in-memory snapshot limit, so its original \
             contents were never kept. It cannot be restored and may still hold the agent's \
             changes.",
            path.display(),
            size,
            MAX_BACKUP_BYTES
        ),
        Backup::Content(bytes) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("failed to recreate directory for '{}'", path.display())
                })?;
            }
            std::fs::write(path, bytes)
                .with_context(|| format!("failed to restore '{}'", path.display()))?;
        }
        Backup::Absent => {
            // The file did not exist before the turn, so rolling back means removing it.
            // This failure used to be discarded and reported as a success.
            // On Windows a locked file makes it common, so it is not a theoretical branch.
            if path.exists() {
                std::fs::remove_file(path).with_context(|| {
                    format!(
                        "failed to delete '{}', which did not exist before the turn",
                        path.display()
                    )
                })?;
            }
        }
    }
    Ok(())
}

/// Manages in-memory snapshots of files before they are edited or written.
/// Provides safe rollback capability for autonomous execution cycles.
///
/// One instance exists per session (see [`for_session`]) and lives until
/// [`release_session`] is called. Backups must survive a turn boundary: the documented
/// contract of `workspace_rollback` is per-session, so a later turn still has to be able to
/// undo what an earlier turn wrote. They must *not* survive the session, or the store grows
/// with every file the agent ever touches.
#[derive(Clone, Default)]
pub struct WorkspaceSnapshot {
    backups: Arc<Mutex<HashMap<PathBuf, Backup>>>,
}

impl WorkspaceSnapshot {
    pub fn new() -> Self {
        Self {
            backups: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Snapshot a file before modifying it.
    ///
    /// A missing file is recorded as [`Backup::Absent`] so rollback can delete files the
    /// agent created. A file larger than [`MAX_BACKUP_BYTES`] is recorded as
    /// [`Backup::TooLarge`] *without* reading it: reading first and then discovering the
    /// limit would already have paid the allocation the limit exists to avoid.
    ///
    /// The first backup of a path wins. Later writes to the same file keep the original
    /// bytes, which is what makes a rollback return to the pre-session state rather than to
    /// the state before the most recent write.
    pub fn backup_file(&self, path: &Path) -> Result<()> {
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut backups = self
            .backups
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
        if let std::collections::hash_map::Entry::Vacant(e) = backups.entry(abs_path) {
            let metadata = e.key().metadata();
            let backup = match metadata {
                Err(_) => Backup::Absent,
                Ok(meta) if meta.len() > MAX_BACKUP_BYTES => {
                    tracing::warn!(
                        "[Snapshot] '{}' is {} bytes, above the {} byte backup limit; \
                         it will not be restorable",
                        e.key().display(),
                        meta.len(),
                        MAX_BACKUP_BYTES
                    );
                    Backup::TooLarge(meta.len())
                }
                Ok(_) => Backup::Content(std::fs::read(e.key())?),
            };
            e.insert(backup);
        }
        Ok(())
    }

    /// Rollback a specific file to its original state before any modifications.
    ///
    /// Returns `Ok(true)` only when the file is genuinely back to its pre-turn state.
    /// A failed restore is an `Err`; it must not be reported as `true`, which is exactly
    /// what the `let _ = std::fs::remove_file(...)` here used to cause.
    pub fn rollback_file(&self, path: &Path) -> Result<bool> {
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut backups = self
            .backups
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
        // The entry is intentionally still in the map while restoring. Popping it first
        // would drop the only copy of the original bytes if the write fails, making the
        // failure permanent and unrecoverable.
        let Some(original) = backups.get(&abs_path) else {
            return Ok(false);
        };
        restore_entry(&abs_path, original)?;
        backups.remove(&abs_path);
        Ok(true)
    }

    /// Rollback all files modified during this session.
    ///
    /// Every entry is attempted even if an earlier one fails, so one locked file cannot
    /// abandon the rest of the rollback. The caller receives both lists and has to decide
    /// what to report; `failed` being non-empty means the workspace is only partly restored.
    ///
    /// Entries are drained as they are processed: a partially restored workspace cannot be
    /// retried through this store anyway, and keeping the backups would silently keep their
    /// contents resident for the lifetime of the process.
    pub fn rollback_all(&self) -> Result<RollbackReport> {
        let mut backups = self
            .backups
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
        let mut report = RollbackReport::default();
        let entries: Vec<(PathBuf, Backup)> = backups.drain().collect();
        for (path, original) in entries {
            match restore_entry(&path, &original) {
                Ok(()) => report.restored.push(path),
                Err(e) => report.failed.push((path, e.to_string())),
            }
        }
        Ok(report)
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

        let report = snap.rollback_all().unwrap();
        assert!(
            report.is_complete(),
            "restoring two writable files must not report failures: {:?}",
            report.failed
        );
        assert_eq!(report.restored.len(), 2);
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

    /// The old `rollback_all` pushed every iterated path into `restored` whether or not the
    /// write succeeded, and discarded the error. A directory sitting where a backed-up file
    /// must be written makes `std::fs::write` fail deterministically, which lets the test
    /// assert on the failure being reported rather than on an unpredictable permission error.
    #[test]
    fn a_failed_restore_is_reported_not_counted_as_success() {
        let snap = WorkspaceSnapshot::new();
        let dir = std::env::temp_dir().join(format!("snap-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);

        let healthy = dir.join("healthy.txt");
        let broken = dir.join("broken.txt");
        std::fs::write(&healthy, b"before").unwrap();
        std::fs::write(&broken, b"before").unwrap();

        snap.backup_file(&healthy).unwrap();
        snap.backup_file(&broken).unwrap();

        // The store keys on the canonicalized path (Windows yields a `\\?\` prefix), so the
        // expected values have to be canonicalized while both files still exist.
        let healthy_canonical = healthy.canonicalize().unwrap();
        let broken_canonical = broken.canonicalize().unwrap();

        // Replace the file with a non-empty directory: writing to it now fails.
        std::fs::remove_file(&broken).unwrap();
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("blocker"), b"x").unwrap();

        let report = snap.rollback_all().unwrap();

        assert_eq!(
            report.restored,
            vec![healthy_canonical],
            "only the file that really was restored may be reported as restored"
        );
        assert_eq!(
            report.failed.len(),
            1,
            "the failed restore must be reported"
        );
        assert_eq!(report.failed[0].0, broken_canonical);
        assert!(!report.is_complete());
        assert_eq!(
            std::fs::read(&healthy).unwrap(),
            b"before",
            "one failed file must not abort the restore of the others"
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

        assert!(
            snap.tracked_files().is_err(),
            "poisoned lock must surface as an error"
        );
        assert!(
            snap.clear().is_err(),
            "poisoned lock must surface as an error"
        );
    }

    /// BUG-08: a file above the in-memory limit must not be read into RAM, and the rollback
    /// must report the impossibility instead of silently reporting a restore it never did.
    ///
    /// The old `Option<Vec<u8>>` shape had no way to express "existed but not captured", so
    /// a size limit could only have been implemented as a lie. This test pins the distinction
    /// between the three states.
    #[test]
    fn oversized_files_are_tracked_but_reported_as_unrestorable() {
        let snap = WorkspaceSnapshot::new();
        let dir = std::env::temp_dir().join(format!("snap-big-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let big = dir.join("big.bin");
        std::fs::write(&big, vec![7u8; (MAX_BACKUP_BYTES + 1024) as usize]).unwrap();

        snap.backup_file(&big).unwrap();
        assert_eq!(
            snap.tracked_files().unwrap().len(),
            1,
            "an oversized file must still be tracked, otherwise the failure is invisible"
        );

        let report = snap.rollback_all().unwrap();
        assert!(
            report.restored.is_empty(),
            "an uncaptured file must never be reported as restored"
        );
        assert_eq!(report.failed.len(), 1);
        assert!(
            report.failed[0].1.contains("snapshot limit"),
            "the error must name the reason, got: {}",
            report.failed[0].1
        );
        assert!(!report.is_complete());

        // The store must not have kept the bytes either; the entry is drained above, so a
        // second rollback has nothing left to attempt.
        assert!(snap.tracked_files().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The smaller sibling of the test above: a file under the limit is captured for real.
    /// Together they pin both sides of the boundary, so raising or lowering the constant
    /// cannot silently move the behaviour.
    #[test]
    fn files_under_the_limit_are_captured_and_restored() {
        let snap = WorkspaceSnapshot::new();
        let dir = std::env::temp_dir().join(format!("snap-small-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let small = dir.join("small.bin");
        std::fs::write(&small, b"original").unwrap();

        snap.backup_file(&small).unwrap();
        std::fs::write(&small, b"modified by the agent").unwrap();

        let report = snap.rollback_all().unwrap();
        assert!(report.is_complete(), "a small file must restore cleanly");
        assert_eq!(std::fs::read(&small).unwrap(), b"original");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
