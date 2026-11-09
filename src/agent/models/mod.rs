//! Agent domain model types.
//!
//! Split into focused submodules for readability. Everything is re-exported
//! here so existing `crate::agent::models::Type` paths keep working unchanged.

mod audit;
mod evidence;
mod goal;
mod mcp;
mod message;
mod project;
mod session;
mod skill;

pub use audit::*;
pub use evidence::*;
pub use goal::*;
pub use mcp::*;
pub use message::*;
pub use project::*;
pub use session::*;
pub use skill::*;
