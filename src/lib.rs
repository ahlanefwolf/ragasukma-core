//! # Ragasukma Core
//!
//! Core autonomous agent, tools execution pipeline, and multi-provider LLM engine.

#[macro_export]
macro_rules! term_log {
    ($($arg:tt)*) => {
        println!($($arg)*);
    };
}

#[macro_export]
macro_rules! term_err {
    ($($arg:tt)*) => {
        eprintln!($($arg)*);
    };
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod agent;
pub mod assets;
pub mod config;
pub mod llm;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_version() {
        assert_eq!(VERSION, "0.2.0");
    }
}