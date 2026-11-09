//! Antigravity v1 API adapter.
//!
//! Split (Refactor #3, Stage A) from a single 1190-line file into concern
//! modules. All items remain reachable at their historical paths
//! (`crate::llm::providers::antigravity::v1_adapter::X`) via the re-exports
//! below.

pub mod model_config;
pub mod payload;
pub mod signature;
pub mod stream;

pub use model_config::*;
pub use payload::*;
pub use signature::*;
pub use stream::*;

#[cfg(test)]
#[path = "../v1_adapter_tests.rs"]
mod tests;
