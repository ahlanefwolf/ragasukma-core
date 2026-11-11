use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::broadcast;

use super::models::AgentSessionEvent;

static SESSION_BROADCASTERS: OnceLock<
    Mutex<HashMap<String, broadcast::Sender<AgentSessionEvent>>>,
> = OnceLock::new();

static RUNNING_SESSION_TASKS: OnceLock<Mutex<HashMap<String, tokio::task::AbortHandle>>> =
    OnceLock::new();

static SESSION_CANCELLATION_FLAGS: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> =
    OnceLock::new();

fn get_broadcasters_map() -> &'static Mutex<HashMap<String, broadcast::Sender<AgentSessionEvent>>> {
    SESSION_BROADCASTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn get_running_tasks_map() -> &'static Mutex<HashMap<String, tokio::task::AbortHandle>> {
    RUNNING_SESSION_TASKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn get_cancellation_flags_map() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    SESSION_CANCELLATION_FLAGS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Retrieves or creates an atomic cancellation flag for a session.
pub fn get_session_cancellation_flag(session_id: &str) -> Arc<AtomicBool> {
    let mut map = get_cancellation_flags_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.entry(session_id.to_string())
        .or_insert_with(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

/// Checks whether the session has been flagged for cancellation.
pub fn is_session_cancelled(session_id: &str) -> bool {
    let map = get_cancellation_flags_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.get(session_id)
        .map(|f| f.load(Ordering::Relaxed))
        .unwrap_or(false)
}

pub fn remove_session_broadcaster(session_id: &str) {
    let mut map = get_broadcasters_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.remove(session_id);
}

/// Registers an active Tokio background task handle for a session.
/// If a prior task was already running for this session, it is immediately aborted.
pub fn register_session_task(session_id: &str, abort_handle: tokio::task::AbortHandle) {
    {
        let mut cmap = get_cancellation_flags_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let flag = cmap
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)));
        flag.store(false, Ordering::SeqCst);
    }

    let mut map = get_running_tasks_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(old_handle) = map.insert(session_id.to_string(), abort_handle) {
        old_handle.abort();
    }
}

/// Unregisters a completed task from the active running sessions registry.
/// Also prunes the session broadcaster if no active receivers remain.
pub fn unregister_session_task(session_id: &str) {
    let mut map = get_running_tasks_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.remove(session_id);

    let mut cmap = get_cancellation_flags_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    cmap.remove(session_id);

    let mut bmap = get_broadcasters_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(sender) = bmap.get(session_id)
        && sender.receiver_count() == 0
    {
        bmap.remove(session_id);
    }
}

/// Aborts and removes an active running session task immediately.
/// Sets thread-safe cancellation flag so blocking worker threads terminate promptly.
/// Returns true if an active task was found and aborted.
pub fn cancel_session_task(session_id: &str) -> bool {
    {
        let cmap = get_cancellation_flags_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(flag) = cmap.get(session_id) {
            flag.store(true, Ordering::SeqCst);
        }
    }

    let mut map = get_running_tasks_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(handle) = map.remove(session_id) {
        handle.abort();
        true
    } else {
        false
    }
}

/// Checks whether an agent background task is actively running for the given session.
pub fn is_session_task_running(session_id: &str) -> bool {
    let map = get_running_tasks_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.contains_key(session_id)
}

/// Removes every per-session entry owned by this crate.
///
/// The broadcaster, running-task handle, and cancellation flag each live in a
/// process-wide `HashMap` keyed by session id. Those maps are populated lazily on
/// first use, so a long-running host that spins up many sessions would otherwise
/// leak one entry (plus a capacity-2048 channel) per session forever.
///
/// Call this once a session has finished — successfully, cancelled, or errored.
/// Any running task is aborted first so a stale handle cannot keep the session
/// alive after its bookkeeping is gone.
pub fn cleanup_session(session_id: &str) {
    let aborted_task = {
        let mut map = get_running_tasks_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.remove(session_id)
    };
    // Abort outside the map lock so a dropped task cannot re-enter cleanup.
    if let Some(handle) = aborted_task {
        handle.abort();
    }

    get_cancellation_flags_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_id);

    get_broadcasters_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_id);
}

/// Prunes all session broadcasters that have 0 active receivers and no active running task.
pub fn prune_idle_broadcasters() {
    let running = {
        let rmap = get_running_tasks_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        rmap.keys().cloned().collect::<std::collections::HashSet<_>>()
    };

    let mut bmap = get_broadcasters_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    bmap.retain(|session_id, sender| {
        running.contains(session_id) || sender.receiver_count() > 0
    });
}

pub fn get_session_sender(session_id: &str) -> broadcast::Sender<AgentSessionEvent> {
    let mut map = get_broadcasters_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    // Opportunistically prune idle broadcasters if registry grows large (>64 entries)
    if map.len() > 64 {
        let running = get_running_tasks_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.retain(|s_id, sender| {
            running.contains_key(s_id) || sender.receiver_count() > 0
        });
    }

    if let Some(sender) = map.get(session_id) {
        sender.clone()
    } else {
        // High capacity channel to prevent Lagged errors during rapid streaming / large tool calls
        let (sender, _) = broadcast::channel(2048);
        map.insert(session_id.to_string(), sender.clone());
        sender
    }
}

pub fn broadcast_session_event(session_id: &str, event: AgentSessionEvent) {
    let sender = get_session_sender(session_id);
    let _ = sender.send(event);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracked_entry_count() -> usize {
        let tasks = get_running_tasks_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        let flags = get_cancellation_flags_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        let senders = get_broadcasters_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        tasks + flags + senders
    }

    #[test]
    fn cleanup_session_removes_all_three_registries() {
        // Populate every registry for a fresh session id.
        let session = "cleanup_test_session_a";
        let _flag = get_session_cancellation_flag(session);
        let _sender = get_session_sender(session);
        let handle = tokio::runtime::Runtime::new()
            .unwrap()
            .spawn(async {})
            .abort_handle();
        register_session_task(session, handle);

        assert_eq!(
            tracked_entry_count(),
            3,
            "session should own one entry in each of the three registries"
        );

        cleanup_session(session);

        assert_eq!(
            tracked_entry_count(),
            0,
            "cleanup_session must release task handle, cancellation flag, and broadcaster"
        );
    }

    #[test]
    fn cleanup_session_is_idempotent_and_scoped() {
        let kept = "cleanup_test_session_keep";
        let removed = "cleanup_test_session_remove";
        let _ = get_session_sender(kept);
        let _ = get_session_sender(removed);

        cleanup_session(removed);
        cleanup_session(removed); // second call must not panic

        // The untouched session survived; only the cleaned one is gone.
        assert!(
            get_broadcasters_map()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(kept)
        );
        assert!(
            !get_broadcasters_map()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(removed)
        );

        // Leave no residue for other tests sharing the process-global maps.
        cleanup_session(kept);
    }
}
