//! Streaming response translators (provider format -> client format).
//!
//! Each translator is an incremental state machine: it consumes one upstream
//! chunk plus the mutable [`crate::state::StreamState`] and returns zero or
//! more chunks in the target format.

pub mod claude_to_openai;
pub mod commandcode_to_openai;
pub mod gemini_to_openai;
pub mod grok_web_to_openai;
pub mod ollama_to_openai;
pub mod openai_to_claude;
pub mod openai_to_responses;
