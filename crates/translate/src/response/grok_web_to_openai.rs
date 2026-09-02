//! grok.com NDJSON responses -> OpenAI `chat.completion.chunk` stream.
//!
//! Ports the response half of `open-sse/executors/grok-web.js`.
//!
//! grok.com streams newline-delimited JSON; the framing is handled upstream of this module by
//! `Encoding::Ndjson`. What is handled here is a shape with three quirks worth naming, because each one
//! silently corrupts a reply if read naively:
//!
//! * **Every event is wrapped twice.** The payload lives at `result.response`, and an event without
//!   that path is a keep-alive or a metadata frame carrying nothing for the client.
//! * **`token` and `modelResponse` are alternatives, not a sequence.** Incremental deltas arrive as
//!   `token`; `modelResponse.message` is the *whole* answer, sent at the end. Appending both would
//!   duplicate the entire reply, so a `modelResponse` supersedes what the tokens accumulated.
//! * **Reasoning is not a separate field.** On a thinking mode, the text that arrives before the final
//!   `modelResponse` is the model's reasoning. Whether a given stream is a thinking one is not in the
//!   response at all — it comes from the requested mode, so the caller records it in [`StreamState`].
//!
//! grok.com reports no token counts, so usage is left absent rather than estimated. Upstream divides
//! character length by four and reports that as both prompt and completion tokens, which is not a
//! measurement — a caller cannot tell it apart from a real count, and it would be billed against.

use serde_json::{Value, json};

use crate::concerns::{ChunkMeta, build_chunk};
use crate::state::StreamState;

fn chunk_meta(state: &StreamState) -> ChunkMeta {
    ChunkMeta {
        id: state
            .message_id
            .clone()
            .unwrap_or_else(|| format!("chatcmpl-grok-{}", state.clock.now_millis())),
        created: state
            .grok_created
            .unwrap_or_else(|| state.clock.now_seconds()),
        model: state.model.clone().unwrap_or_default(),
    }
}

/// Translate one grok.com NDJSON event into zero or more OpenAI chunks.
pub fn translate(raw: &Value, state: &mut StreamState) -> Vec<Value> {
    if !raw.is_object() {
        return Vec::new();
    }
    // Identity is fixed on the first event so every chunk of one response shares it.
    if state.message_id.is_none() {
        state.message_id = Some(format!("chatcmpl-grok-{}", state.clock.now_millis()));
        state.grok_created = Some(state.clock.now_seconds());
    }

    // An error arrives in place of a response and ends the turn. It is surfaced as content rather than
    // dropped: a stream that stops with no explanation looks to a client like a truncated answer.
    if let Some(error) = raw.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| error.get("code").map(|code| format!("Grok error: {code}")))
            .unwrap_or_else(|| "Grok error".to_owned());
        state.finish_reason = Some("stop".to_owned());
        return vec![build_chunk(
            &chunk_meta(state),
            json!({ "content": format!("[Error: {message}]") }),
            Some("stop"),
        )];
    }

    let Some(response) = raw.pointer("/result/response") else {
        return Vec::new();
    };

    // The model hash identifies which build answered. Recorded so the terminal chunk can report it as
    // `system_fingerprint`, which is where an OpenAI client looks for exactly that.
    if let Some(hash) = response
        .pointer("/llmInfo/modelHash")
        .and_then(Value::as_str)
        .filter(|hash| !hash.is_empty())
        && state.grok_fingerprint.is_none()
    {
        state.grok_fingerprint = Some(hash.to_owned());
    }

    // The final whole-message event. It supersedes the accumulated tokens rather than appending to
    // them, because it repeats the entire answer.
    if let Some(model_response) = response.get("modelResponse") {
        if let Some(hash) = model_response
            .pointer("/metadata/llm_info/modelHash")
            .and_then(Value::as_str)
            .filter(|hash| !hash.is_empty())
        {
            state.grok_fingerprint = Some(hash.to_owned());
        }
        let message = model_response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if message.is_empty() {
            return Vec::new();
        }
        // On a thinking mode the tokens streamed so far were reasoning, not answer. The whole message
        // is the answer, so it is emitted in full and the earlier text is left as reasoning.
        if state.grok_thinking {
            state.grok_saw_model_response = true;
            return vec![build_chunk(
                &chunk_meta(state),
                json!({ "content": message }),
                None,
            )];
        }
        // On a non-thinking mode the tokens already carried this text. Emitting it again would send the
        // answer twice, so only the tail beyond what was already sent is emitted.
        let already = state.grok_emitted.len();
        let remainder = message.get(already..).unwrap_or_default();
        state.grok_saw_model_response = true;
        if remainder.is_empty() {
            return Vec::new();
        }
        state.grok_emitted.push_str(remainder);
        return vec![build_chunk(
            &chunk_meta(state),
            json!({ "content": remainder }),
            None,
        )];
    }

    // An incremental token.
    let Some(token) = response.get("token").and_then(Value::as_str) else {
        return Vec::new();
    };
    if token.is_empty() {
        return Vec::new();
    }
    // Before the final message on a thinking mode, tokens are the model reasoning aloud.
    let field = if state.grok_thinking && !state.grok_saw_model_response {
        "reasoning_content"
    } else {
        "content"
    };
    if field == "content" {
        state.grok_emitted.push_str(token);
    }
    vec![build_chunk(
        &chunk_meta(state),
        json!({ field: token }),
        None,
    )]
}

/// The terminal chunk, carrying the finish reason and the build that answered.
///
/// grok.com's stream simply ends; it sends no `finish_reason` of its own, and a client waiting for one
/// would hang. Called by the executor once the NDJSON body is exhausted.
pub fn finish(state: &mut StreamState) -> Value {
    state.finish_reason = Some("stop".to_owned());
    let mut chunk = build_chunk(&chunk_meta(state), json!({}), Some("stop"));
    if let Some(fingerprint) = state.grok_fingerprint.as_ref()
        && let Some(object) = chunk.as_object_mut()
    {
        object.insert("system_fingerprint".to_owned(), json!(fingerprint));
    }
    chunk
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{finish, translate};
    use crate::state::{Clock, StreamState};

    fn state(thinking: bool) -> StreamState {
        let mut state = StreamState::new(Clock::Fixed(1_700_000_123_456));
        state.grok_thinking = thinking;
        state.model = Some("grok-4".to_owned());
        state
    }

    fn token(text: &str) -> Value {
        json!({ "result": { "response": { "token": text } } })
    }

    #[test]
    fn tokens_become_content_deltas_sharing_one_id() {
        let mut state = state(false);
        let first = translate(&token("he"), &mut state);
        let second = translate(&token("llo"), &mut state);

        let first = first.first().expect("first chunk");
        let second = second.first().expect("second chunk");
        assert_eq!(
            first.pointer("/choices/0/delta/content"),
            Some(&json!("he"))
        );
        assert_eq!(
            second.pointer("/choices/0/delta/content"),
            Some(&json!("llo"))
        );
        // Both chunks belong to one response.
        assert_eq!(first.get("id"), second.get("id"));
        assert_eq!(first.get("model"), Some(&json!("grok-4")));
    }

    #[test]
    fn an_event_without_the_nested_response_carries_nothing() {
        // grok.com sends metadata and keep-alive frames. Reading them as content would inject noise
        // into the answer.
        let mut state = state(false);
        assert!(translate(&json!({ "result": {} }), &mut state).is_empty());
        assert!(translate(&json!({ "unrelated": true }), &mut state).is_empty());
        assert!(translate(&token(""), &mut state).is_empty());
    }

    #[test]
    fn a_final_message_does_not_repeat_what_the_tokens_already_sent() {
        // This is the quirk that would duplicate an entire reply: `modelResponse.message` is the whole
        // answer, sent after the tokens that already spelled it out.
        let mut state = state(false);
        translate(&token("hello "), &mut state);
        translate(&token("world"), &mut state);
        let final_chunks = translate(
            &json!({ "result": { "response": { "modelResponse": { "message": "hello world" } } } }),
            &mut state,
        );

        assert!(
            final_chunks.is_empty(),
            "the tokens already carried this text: {final_chunks:?}"
        );
    }

    #[test]
    fn a_final_message_longer_than_the_tokens_emits_only_the_tail() {
        // A stream that ends early still has to deliver the rest, without resending the start.
        let mut state = state(false);
        translate(&token("hello"), &mut state);
        let out = translate(
            &json!({ "result": { "response": { "modelResponse": { "message": "hello world" } } } }),
            &mut state,
        );

        assert_eq!(
            out.first()
                .and_then(|chunk| chunk.pointer("/choices/0/delta/content")),
            Some(&json!(" world"))
        );
    }

    #[test]
    fn on_a_thinking_mode_early_tokens_are_reasoning_and_the_answer_is_the_final_message() {
        // The response never says which stream is a thinking one; the requested mode does. Reading the
        // reasoning as answer text would put the model's scratch work in front of the reply.
        let mut state = state(true);
        let thought = translate(&token("let me consider"), &mut state);
        assert_eq!(
            thought
                .first()
                .and_then(|chunk| chunk.pointer("/choices/0/delta/reasoning_content")),
            Some(&json!("let me consider"))
        );
        assert!(
            thought
                .first()
                .and_then(|chunk| chunk.pointer("/choices/0/delta/content"))
                .is_none()
        );

        let answer = translate(
            &json!({ "result": { "response": { "modelResponse": { "message": "42" } } } }),
            &mut state,
        );
        assert_eq!(
            answer
                .first()
                .and_then(|chunk| chunk.pointer("/choices/0/delta/content")),
            Some(&json!("42"))
        );

        // Tokens after the answer are answer text, not more reasoning.
        let trailing = translate(&token("!"), &mut state);
        assert_eq!(
            trailing
                .first()
                .and_then(|chunk| chunk.pointer("/choices/0/delta/content")),
            Some(&json!("!"))
        );
    }

    #[test]
    fn an_error_event_is_surfaced_as_content_and_stops_the_turn() {
        // A stream that simply stops looks like a truncated answer. Saying what happened is the
        // difference between a visible failure and a silent one.
        let mut state = state(false);
        let out = translate(
            &json!({ "error": { "message": "rate limited", "code": 429 } }),
            &mut state,
        );
        let chunk = out.first().expect("an error chunk");
        assert_eq!(
            chunk.pointer("/choices/0/delta/content"),
            Some(&json!("[Error: rate limited]"))
        );
        assert_eq!(
            chunk.pointer("/choices/0/finish_reason"),
            Some(&json!("stop"))
        );
    }

    #[test]
    fn an_error_without_a_message_still_names_its_code() {
        let mut state = state(false);
        let out = translate(&json!({ "error": { "code": 500 } }), &mut state);
        assert_eq!(
            out.first()
                .and_then(|chunk| chunk.pointer("/choices/0/delta/content")),
            Some(&json!("[Error: Grok error: 500]"))
        );
    }

    #[test]
    fn the_terminal_chunk_reports_stop_and_the_build_that_answered() {
        // grok.com sends no finish reason of its own, and a client waiting for one hangs.
        let mut state = state(false);
        translate(
            &json!({ "result": { "response": { "llmInfo": { "modelHash": "abc123" }, "token": "hi" } } }),
            &mut state,
        );
        let terminal = finish(&mut state);

        assert_eq!(
            terminal.pointer("/choices/0/finish_reason"),
            Some(&json!("stop"))
        );
        assert_eq!(terminal.get("system_fingerprint"), Some(&json!("abc123")));
        assert_eq!(state.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn no_usage_is_reported_because_grok_sends_none() {
        // Upstream reports length/4 as both prompt and completion tokens. That is not a measurement,
        // and a caller cannot tell it from a real count — so nothing is claimed.
        let mut state = state(false);
        translate(&token("some answer text"), &mut state);
        finish(&mut state);
        assert!(
            state.usage.is_none(),
            "an invented token count is worse than none"
        );
    }
}
