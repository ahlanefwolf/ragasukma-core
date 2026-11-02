pub mod client;
pub mod providers;
pub mod types;
pub mod usage;
pub mod v1_engine;

// Re-export primary active components
pub use client::LlmClient;

pub mod history {
    pub fn log_conversation_turn(
        user_callsign: &str,
        assistant_name: &str,
        personality_role: &str,
        user_message: &str,
        assistant_response: &str,
    ) {
        crate::repository::chat_history::log_conversation_turn(
            user_callsign,
            assistant_name,
            personality_role,
            user_message,
            assistant_response,
        );
    }
}
