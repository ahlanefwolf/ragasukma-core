//! Goal store repository.
//!
//! Split by concern (Refactor #3, Stage B). All public items keep their
//! original names and are re-exported here, so `goals::*` is unchanged.

pub mod conclusion;
pub mod evidence;
pub mod lifecycle;
pub mod verification;

pub use conclusion::*;
pub use evidence::*;
pub use lifecycle::*;
pub use verification::*;

// Re-exported for `goals_tests.rs`, which uses `use super::*;`. Historically
// that file lived beside `goals.rs` (super = repository::agent); now it is a
// child of this module, so the helpers it relied on must be visible here.
#[cfg(test)]
pub(crate) use super::generate_id;

#[cfg(test)]
#[path = "../goals_tests.rs"]
mod transaction_tests;
