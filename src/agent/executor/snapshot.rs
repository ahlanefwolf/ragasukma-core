//! Re-export of the canonical workspace snapshot store.
//!
//! This module used to hold a byte-for-byte duplicate of `crate::agent::snapshot`.
//! Two copies meant the same bug (a poisoned lock being swallowed by `clear` /
//! `tracked_files`) had to be fixed twice and could silently regress in one of
//! them. The implementation now lives in a single place.

pub use crate::agent::snapshot::WorkspaceSnapshot;
