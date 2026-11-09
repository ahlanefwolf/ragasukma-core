//! Deprecated shim: the agent repository now lives at [`crate::repository::agent`].
//!
//! This module is kept only so existing `crate::agent::repository::*` paths keep
//! compiling. New code should import from `crate::repository::agent::*` instead.

pub use crate::repository::agent::*;
