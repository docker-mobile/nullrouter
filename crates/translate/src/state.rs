//! Streaming translation state.
//!
//! Upstream carries a mutable `state` object through every chunk
//! (`open-sse/translator/index.js` `initState`). This is its Rust equivalent,
//! plus an injectable clock so translated ids and `created` stamps are
//! deterministic under test.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::concerns::Usage;

/// Source of wall-clock time for id/`created` generation.
#[derive(Debug, Clone, Copy)]
pub enum Clock {
    /// Real time.
    System,
    /// Fixed epoch milliseconds, for tests and reproducible framing.
    Fixed(u64),
}

impl Clock {
    /// Milliseconds since the Unix epoch.
    pub fn now_millis(self) -> u64 {
        match self {
            Self::System => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| {
                    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
                }),
            Self::Fixed(millis) => millis,
        }
    }

    /// Seconds since the Unix epoch (OpenAI `created`).
    pub fn now_seconds(self) -> u64 {
        self.now_millis() / 1000
    }
}

/// An in-flight tool call on the Claude -> OpenAI path, keyed by Claude block
/// index.
#[derive(Debug, Clone, Default)]
pub struct OpenAiToolCall {
    pub index: u64,
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// An in-flight tool call on the OpenAI -> Claude path, keyed by OpenAI
/// `tool_calls[].index`.
#[derive(Debug, Clone, Default)]
pub struct ClaudeToolCall {
    pub id: String,
    pub name: String,
    pub block_index: u64,
}

/// Mutable state threaded through a streaming translation.
#[derive(Debug, Clone)]
pub struct StreamState {
    pub clock: Clock,

    // ── shared ──
    pub message_id: Option<String>,
    pub model: Option<String>,
    pub finish_reason: Option<String>,
    pub finish_reason_sent: bool,
    pub usage: Option<Usage>,
    /// Renamed tool names to restore on the way back (Claude OAuth cloaking).
    pub tool_name_map: BTreeMap<String, String>,

    // ── Claude -> OpenAI ──
    pub tool_call_index: u64,
    pub text_block_started: bool,
    pub thinking_block_started: bool,
    pub in_thinking_block: bool,
    pub current_block_index: Option<u64>,
    /// Claude built-in server tool block to skip (web search).
    pub server_tool_block_index: Option<u64>,
    pub openai_tool_calls: BTreeMap<u64, OpenAiToolCall>,
    /// Claude `message_start` usage, kept so `message_delta` (output-only)
    /// cannot reset the cache counts to zero.
    pub claude_input_tokens: u64,
    pub claude_output_tokens: u64,
    pub claude_cache_read_tokens: u64,
    pub claude_cache_creation_tokens: u64,

    // ── OpenAI -> Claude ──
    pub message_start_sent: bool,
    pub next_block_index: u64,
    pub text_block_index: u64,
    pub text_block_closed: bool,
    pub thinking_block_index: u64,
    pub claude_tool_calls: BTreeMap<u64, ClaudeToolCall>,
    /// Buffered tool arguments, flushed and sanitized at finish.
    pub tool_arg_buffers: BTreeMap<u64, String>,

    // ── Gemini -> OpenAI ──
    pub function_index: u64,
    pub gemini_tool_call_count: u64,

    // ── OpenAI -> Responses API ──
    // The Responses API requires a monotonic sequence number and explicit
    // open/close bookkeeping per output item, so more state is tracked here
    // than for the chunk-shaped formats.
    /// Monotonic `sequence_number` stamped on every event.
    pub responses_seq: u64,
    /// `resp_*` id, derived from the first upstream chunk id.
    pub response_id: Option<String>,
    /// `created_at`, fixed at the first event.
    pub responses_created: Option<u64>,
    /// Whether `response.created` has been emitted.
    pub responses_started: bool,
    /// Whether `response.completed` has been emitted.
    pub responses_completed: bool,
    /// Output indices with an open message item.
    pub message_items_added: BTreeSet<u64>,
    /// Output indices whose message item has been closed.
    pub message_items_done: BTreeSet<u64>,
    /// Accumulated text per output index, replayed in the `.done` event.
    pub message_text: BTreeMap<u64, String>,
    pub reasoning_item_added: bool,
    pub reasoning_item_done: bool,
    pub reasoning_buffer: String,
    /// Tool-call indices with an open function item.
    pub function_items_added: BTreeSet<u64>,
    pub function_items_done: BTreeSet<u64>,
    pub function_arguments: BTreeMap<u64, String>,
    pub function_names: BTreeMap<u64, String>,
    pub function_call_ids: BTreeMap<u64, String>,
    /// Tools declared `custom`, which report raw input instead of arguments.
    pub custom_tool_names: BTreeSet<String>,
}

impl StreamState {
    /// Fresh state for a stream.
    pub const fn new(clock: Clock) -> Self {
        Self {
            clock,
            message_id: None,
            model: None,
            finish_reason: None,
            finish_reason_sent: false,
            usage: None,
            tool_name_map: BTreeMap::new(),
            tool_call_index: 0,
            text_block_started: false,
            thinking_block_started: false,
            in_thinking_block: false,
            current_block_index: None,
            server_tool_block_index: None,
            openai_tool_calls: BTreeMap::new(),
            claude_input_tokens: 0,
            claude_output_tokens: 0,
            claude_cache_read_tokens: 0,
            claude_cache_creation_tokens: 0,
            message_start_sent: false,
            next_block_index: 0,
            text_block_index: 0,
            text_block_closed: false,
            thinking_block_index: 0,
            claude_tool_calls: BTreeMap::new(),
            tool_arg_buffers: BTreeMap::new(),
            function_index: 0,
            gemini_tool_call_count: 0,
            responses_seq: 0,
            response_id: None,
            responses_created: None,
            responses_started: false,
            responses_completed: false,
            message_items_added: BTreeSet::new(),
            message_items_done: BTreeSet::new(),
            message_text: BTreeMap::new(),
            reasoning_item_added: false,
            reasoning_item_done: false,
            reasoning_buffer: String::new(),
            function_items_added: BTreeSet::new(),
            function_items_done: BTreeSet::new(),
            function_arguments: BTreeMap::new(),
            function_names: BTreeMap::new(),
            function_call_ids: BTreeMap::new(),
            custom_tool_names: BTreeSet::new(),
        }
    }

    /// Model name, falling back to upstream's `"unknown"` sentinel.
    pub fn model_or_fallback(&self) -> &str {
        self.model
            .as_deref()
            .unwrap_or(crate::schema::MODEL_FALLBACK)
    }

    /// Restore an original tool name through the cloaking map.
    pub fn original_tool_name(&self, name: &str) -> String {
        self.tool_name_map
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_owned())
    }
}

impl Default for StreamState {
    fn default() -> Self {
        Self::new(Clock::System)
    }
}

#[cfg(test)]
mod tests {
    use super::{Clock, StreamState};

    #[test]
    fn fixed_clock_is_deterministic() {
        let clock = Clock::Fixed(1_700_000_123_456);
        assert_eq!(clock.now_millis(), 1_700_000_123_456);
        assert_eq!(clock.now_seconds(), 1_700_000_123);
    }

    #[test]
    fn system_clock_advances_past_epoch() {
        assert!(Clock::System.now_millis() > 1_600_000_000_000);
    }

    #[test]
    fn fresh_state_has_no_model_and_falls_back() {
        let state = StreamState::new(Clock::Fixed(0));
        assert_eq!(state.model_or_fallback(), "unknown");
        assert!(!state.message_start_sent);
        assert_eq!(state.next_block_index, 0);
    }

    #[test]
    fn tool_name_map_restores_original_names() {
        let mut state = StreamState::new(Clock::Fixed(0));
        state
            .tool_name_map
            .insert("proxy_Read".to_owned(), "Read".to_owned());
        assert_eq!(state.original_tool_name("proxy_Read"), "Read");
        // Unmapped names pass through unchanged.
        assert_eq!(state.original_tool_name("Write"), "Write");
    }
}
