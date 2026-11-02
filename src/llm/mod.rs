pub mod client;
pub mod providers;
pub mod types;
pub mod usage;
pub mod v1_engine;

// Re-export primary active components
pub use client::LlmClient;

pub mod history {
    pub fn log_conversation_turn(
        _user_callsign: &str,
        _assistant_name: &str,
        _personality_role: &str,
        _user_message: &str,
        _assistant_response: &str,
    ) {
    }
}
