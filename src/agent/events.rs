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
pub fn unregister_session_task(session_id: &str) {
    let mut map = get_running_tasks_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.remove(session_id);

    let mut cmap = get_cancellation_flags_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    cmap.remove(session_id);
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

pub fn get_session_sender(session_id: &str) -> broadcast::Sender<AgentSessionEvent> {
    let mut map = get_broadcasters_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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
