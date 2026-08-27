//! OpenAI Responses API translation.
//!
//! The Responses API is a lifecycle-event protocol, not a reshaped chunk stream.
//! A client reads event *names* and relies on every opened item being closed and
//! on `sequence_number` increasing monotonically. Emitting chat chunks here — as
//! a naive pivot through OpenAI would — produces a stream no client can parse.

use nullrouter_providers::Format;
use nullrouter_translate::schema::DEFAULT_MAX_TOKENS;
use nullrouter_translate::state::Clock;
use nullrouter_translate::{StreamState, finalize_response, request, response, translate_request};
use serde_json::{Value, json};

const fn state() -> StreamState {
    StreamState::new(Clock::Fixed(1_700_000_123_456))
}

/// Event names in emission order.
fn names(events: &[response::openai_to_responses::ResponseEvent]) -> Vec<String> {
    events.iter().map(|event| event.event.clone()).collect()
}

fn chunk(delta: &Value) -> Value {
    json!({
        "id": "chatcmpl-abc123",
        "model": "gpt-5",
        "choices": [{ "index": 0, "delta": delta.clone() }],
    })
}

fn finish_chunk() -> Value {
    json!({
        "id": "chatcmpl-abc123",
        "model": "gpt-5",
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
    })
}

// ── request side ──

#[test]
fn input_string_becomes_a_single_user_message() {
    let body = json!({ "input": "hello" });
    let result = request::responses_to_openai::translate("gpt-5", &body, false);

    assert_eq!(result.pointer("/messages/0/role"), Some(&json!("user")));
    assert_eq!(
        result.pointer("/messages/0/content/0/text"),
        Some(&json!("hello"))
    );
    // Responses-only fields must not leak to a Chat provider.
    assert!(result.get("input").is_none());
}

#[test]
fn empty_input_gets_a_placeholder_message() {
    // An empty messages[] is rejected by every provider.
    for body in [json!({ "input": [] }), json!({ "input": "   " })] {
        let result = request::responses_to_openai::translate("gpt-5", &body, false);
        let messages = result
            .get("messages")
            .and_then(Value::as_array)
            .expect("messages");
        assert_eq!(messages.len(), 1, "body {body} must yield one message");
        assert_eq!(
            result.pointer("/messages/0/content/0/text"),
            Some(&json!("..."))
        );
    }
}

#[test]
fn instructions_become_a_leading_system_message() {
    let body = json!({
        "instructions": "be terse",
        "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hi" }] }],
    });
    let result = request::responses_to_openai::translate("gpt-5", &body, false);

    assert_eq!(result.pointer("/messages/0/role"), Some(&json!("system")));
    assert_eq!(
        result.pointer("/messages/0/content"),
        Some(&json!("be terse"))
    );
    assert_eq!(result.pointer("/messages/1/role"), Some(&json!("user")));
    assert!(result.get("instructions").is_none());
}

#[test]
fn function_calls_collapse_into_one_assistant_turn() {
    let body = json!({
        "input": [
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "go" }] },
            { "type": "function_call", "call_id": "call_1", "name": "Read", "arguments": "{\"p\":1}" },
            { "type": "function_call", "call_id": "call_2", "name": "Write", "arguments": "{}" },
            { "type": "function_call_output", "call_id": "call_1", "output": "ok" },
        ],
    });
    let result = request::responses_to_openai::translate("gpt-5", &body, false);
    let messages = result
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages");

    // user, assistant(2 tool_calls), tool
    assert_eq!(messages.len(), 3, "got {messages:?}");
    assert_eq!(
        messages.get(1).and_then(|m| m.get("role")),
        Some(&json!("assistant"))
    );
    assert_eq!(
        messages
            .get(1)
            .and_then(|m| m.get("tool_calls"))
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        messages.get(2).and_then(|m| m.get("role")),
        Some(&json!("tool"))
    );
    assert_eq!(
        messages.get(2).and_then(|m| m.get("tool_call_id")),
        Some(&json!("call_1"))
    );
}

#[test]
fn nameless_tool_calls_are_dropped() {
    // Upstream rejects these, so forwarding one would fail the whole request.
    let body = json!({
        "input": [
            { "type": "function_call", "call_id": "call_1", "name": "", "arguments": "{}" },
            { "type": "function_call", "call_id": "call_2", "name": "Read", "arguments": "{}" },
        ],
    });
    let result = request::responses_to_openai::translate("gpt-5", &body, false);
    let calls = result
        .pointer("/messages/0/tool_calls")
        .and_then(Value::as_array)
        .expect("tool_calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls.first().and_then(|c| c.pointer("/function/name")),
        Some(&json!("Read"))
    );
}

#[test]
fn over_long_call_ids_are_clamped() {
    let long_id = "c".repeat(120);
    let body = json!({
        "input": [{ "type": "function_call", "call_id": long_id, "name": "Read", "arguments": "{}" }],
    });
    let result = request::responses_to_openai::translate("gpt-5", &body, false);
    let id = result
        .pointer("/messages/0/tool_calls/0/id")
        .and_then(Value::as_str)
        .expect("id");
    assert_eq!(id.len(), 64, "Responses API caps call_id at 64 chars");
}

#[test]
fn reasoning_items_attach_to_the_next_assistant_turn() {
    let body = json!({
        "input": [
            { "type": "reasoning", "summary": [{ "type": "summary_text", "text": "thinking" }],
              "encrypted_content": "blob" },
            { "type": "function_call", "call_id": "call_1", "name": "Read", "arguments": "{}" },
        ],
    });
    let result = request::responses_to_openai::translate("gpt-5", &body, false);
    assert_eq!(
        result.pointer("/messages/0/reasoning_content"),
        Some(&json!("thinking"))
    );
    // The encrypted blob is continuity state for store:false multi-turn.
    assert_eq!(
        result.pointer("/messages/0/encrypted_content"),
        Some(&json!("blob"))
    );
}

#[test]
fn responses_only_fields_are_mapped_or_stripped() {
    let body = json!({
        "input": "hi",
        "max_output_tokens": 4096,
        "reasoning": { "effort": "high" },
        "include": ["x"],
        "store": false,
        "prompt_cache_key": "k",
        "client_metadata": { "a": 1 },
    });
    let result = request::responses_to_openai::translate("gpt-5", &body, false);

    // max_output_tokens is the Responses spelling of max_tokens.
    assert_eq!(result.get("max_tokens"), Some(&json!(4096)));
    assert!(result.get("max_output_tokens").is_none());
    assert_eq!(result.get("reasoning_effort"), Some(&json!("high")));
    for stripped in [
        "reasoning",
        "include",
        "store",
        "prompt_cache_key",
        "client_metadata",
    ] {
        assert!(
            result.get(stripped).is_none(),
            "{stripped} must be stripped"
        );
    }
}

#[test]
fn hosted_tools_without_names_are_filtered_out() {
    let body = json!({
        "input": "hi",
        "tools": [
            { "type": "request_user_input" },
            { "type": "function", "name": "Read", "parameters": { "type": "object" } },
        ],
    });
    let result = request::responses_to_openai::translate("gpt-5", &body, false);
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .expect("tools");
    assert_eq!(
        tools.len(),
        1,
        "nameless hosted tools cannot be represented"
    );
    assert_eq!(
        tools.first().and_then(|t| t.pointer("/function/name")),
        Some(&json!("Read"))
    );
    // An object schema must always carry properties.
    assert!(
        tools
            .first()
            .and_then(|t| t.pointer("/function/parameters/properties"))
            .is_some()
    );
}

#[test]
fn custom_tools_become_functions_with_a_raw_input_argument() {
    let body = json!({
        "input": "hi",
        "tools": [{
            "type": "custom",
            "name": "shell",
            "description": "run it",
            "format": { "syntax": "bash" },
        }],
    });
    let result = request::responses_to_openai::translate("gpt-5", &body, false);
    assert_eq!(
        result.pointer("/tools/0/function/parameters/required"),
        Some(&json!(["input"]))
    );
    let description = result
        .pointer("/tools/0/function/description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(description.contains("run it"), "got {description}");
    assert!(
        description.contains("bash"),
        "format hint retained: {description}"
    );
}

#[test]
fn dispatch_routes_responses_source_through_its_own_translator() {
    // Regression guard: pivoting a Responses body through the chat translator
    // would leave `input` in place and produce no `messages`.
    let body = json!({ "input": "hi", "instructions": "be terse" });
    let result = translate_request(
        Format::OpenAiResponses,
        Format::OpenAi,
        "gpt-5",
        &body,
        false,
        DEFAULT_MAX_TOKENS,
    );
    assert!(
        result.body.get("messages").is_some(),
        "must produce messages"
    );
    assert!(result.body.get("input").is_none(), "must consume input");
}

#[test]
fn responses_input_reaches_a_claude_provider() {
    // Cross-format: Responses client -> Claude provider.
    let body = json!({
        "instructions": "be terse",
        "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hi" }] }],
    });
    let result = translate_request(
        Format::OpenAiResponses,
        Format::Claude,
        "claude-sonnet-4.5",
        &body,
        true,
        DEFAULT_MAX_TOKENS,
    );
    // instructions -> system message -> Claude system block
    assert_eq!(
        result.body.pointer("/system/1/text"),
        Some(&json!("be terse"))
    );
    assert_eq!(
        result.body.pointer("/messages/0/role"),
        Some(&json!("user"))
    );
}

// ── response side ──

#[test]
fn stream_opens_with_created_and_in_progress() {
    let mut state = state();
    let events = response::openai_to_responses::translate(
        Some(&chunk(&json!({ "role": "assistant" }))),
        &mut state,
    );
    assert_eq!(
        names(&events),
        vec!["response.created", "response.in_progress"]
    );
    // The response id derives from the upstream chunk id.
    assert_eq!(
        events.first().and_then(|e| e.data.pointer("/response/id")),
        Some(&json!("resp_chatcmpl-abc123"))
    );
}

#[test]
fn text_emits_item_part_and_delta_then_closes_on_finish() {
    let mut state = state();
    let mut events = response::openai_to_responses::translate(
        Some(&chunk(&json!({ "content": "he" }))),
        &mut state,
    );
    events.extend(response::openai_to_responses::translate(
        Some(&chunk(&json!({ "content": "llo" }))),
        &mut state,
    ));
    events.extend(response::openai_to_responses::translate(
        Some(&finish_chunk()),
        &mut state,
    ));

    let sequence = names(&events);
    assert_eq!(
        sequence,
        vec![
            "response.created",
            "response.in_progress",
            "response.output_item.added",
            "response.content_part.added",
            "response.output_text.delta",
            "response.output_text.delta",
            "response.output_text.done",
            "response.content_part.done",
            "response.output_item.done",
            "response.completed",
        ]
    );

    // The `.done` event replays the full text, not just the last delta.
    let done = events
        .iter()
        .find(|event| event.event == "response.output_text.done")
        .expect("done event");
    assert_eq!(done.data.get("text"), Some(&json!("hello")));
}

#[test]
fn sequence_numbers_are_monotonic_from_one() {
    let mut state = state();
    let mut events = response::openai_to_responses::translate(
        Some(&chunk(&json!({ "content": "a" }))),
        &mut state,
    );
    events.extend(response::openai_to_responses::translate(
        Some(&chunk(&json!({ "content": "b" }))),
        &mut state,
    ));
    events.extend(response::openai_to_responses::translate(
        Some(&finish_chunk()),
        &mut state,
    ));

    let numbers: Vec<u64> = events
        .iter()
        .filter_map(|event| event.data.get("sequence_number").and_then(Value::as_u64))
        .collect();
    assert_eq!(
        numbers.len(),
        events.len(),
        "every event carries a sequence number"
    );
    assert_eq!(numbers.first(), Some(&1));
    assert!(
        numbers
            .windows(2)
            .all(|pair| pair.get(1) == pair.first().map(|first| first + 1).as_ref()),
        "sequence must increase by one: {numbers:?}"
    );
}

#[test]
fn reasoning_emits_summary_events_and_closes_before_text() {
    let mut state = state();
    let mut events = response::openai_to_responses::translate(
        Some(&chunk(&json!({ "reasoning_content": "pondering" }))),
        &mut state,
    );
    events.extend(response::openai_to_responses::translate(
        Some(&chunk(&json!({ "content": "answer" }))),
        &mut state,
    ));
    events.extend(response::openai_to_responses::translate(
        Some(&finish_chunk()),
        &mut state,
    ));

    let sequence = names(&events);
    let reasoning_done = sequence
        .iter()
        .position(|name| name == "response.reasoning_summary_part.done");
    let text_added = sequence
        .iter()
        .position(|name| name == "response.content_part.added");
    assert!(
        reasoning_done < text_added,
        "reasoning must close before text opens: {sequence:?}"
    );
    assert!(sequence.contains(&"response.reasoning_summary_text.delta".to_owned()));
}

#[test]
fn tool_calls_emit_function_events_and_close() {
    let mut state = state();
    let mut events = response::openai_to_responses::translate(
        Some(&chunk(&json!({
            "tool_calls": [{
                "index": 0, "id": "call_1", "type": "function",
                "function": { "name": "Read", "arguments": "{\"p\"" },
            }],
        }))),
        &mut state,
    );
    events.extend(response::openai_to_responses::translate(
        Some(&chunk(&json!({
            "tool_calls": [{ "index": 0, "function": { "arguments": ":1}" } }],
        }))),
        &mut state,
    ));
    events.extend(response::openai_to_responses::translate(
        Some(&finish_chunk()),
        &mut state,
    ));

    let sequence = names(&events);
    assert!(sequence.contains(&"response.function_call_arguments.delta".to_owned()));
    assert!(sequence.contains(&"response.function_call_arguments.done".to_owned()));

    // Arguments are reassembled across deltas.
    let done = events
        .iter()
        .find(|event| event.event == "response.function_call_arguments.done")
        .expect("args done");
    assert_eq!(done.data.get("arguments"), Some(&json!("{\"p\":1}")));

    // The final item carries the call id and name.
    let item_done = events
        .iter()
        .rev()
        .find(|event| event.event == "response.output_item.done")
        .expect("item done");
    assert_eq!(
        item_done.data.pointer("/item/call_id"),
        Some(&json!("call_1"))
    );
    assert_eq!(item_done.data.pointer("/item/name"), Some(&json!("Read")));
    assert_eq!(
        item_done.data.pointer("/item/type"),
        Some(&json!("function_call"))
    );
}

#[allow(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "`.added`/`.done` are SSE event-name suffixes, not file extensions; the match must stay exact"
)]
#[test]
fn every_opened_item_is_closed_and_completed_exactly_once() {
    let mut state = state();
    let mut events = response::openai_to_responses::translate(
        Some(&chunk(&json!({ "content": "x" }))),
        &mut state,
    );
    events.extend(response::openai_to_responses::translate(
        Some(&finish_chunk()),
        &mut state,
    ));
    // A second flush must not duplicate the completion.
    events.extend(response::openai_to_responses::flush(&mut state));

    let sequence = names(&events);
    let added = sequence
        .iter()
        .filter(|name| name.ends_with(".added"))
        .count();
    let done = sequence
        .iter()
        .filter(|name| name.ends_with(".done"))
        .count();
    assert!(done >= added, "every opened item must close: {sequence:?}");
    assert_eq!(
        sequence
            .iter()
            .filter(|name| *name == "response.completed")
            .count(),
        1,
        "completed must be emitted exactly once: {sequence:?}"
    );
}

#[test]
fn flush_closes_a_stream_that_ended_without_a_finish_reason() {
    // A provider that drops the connection mid-stream must still yield a
    // terminated Responses stream, or the client hangs.
    let mut state = state();
    let _ = response::openai_to_responses::translate(
        Some(&chunk(&json!({ "content": "partial" }))),
        &mut state,
    );
    let flushed = response::openai_to_responses::flush(&mut state);
    let sequence = names(&flushed);

    assert!(
        sequence.contains(&"response.output_text.done".to_owned()),
        "{sequence:?}"
    );
    assert_eq!(
        sequence.last().map(String::as_str),
        Some("response.completed")
    );
}

#[test]
fn flush_on_an_unstarted_stream_emits_nothing() {
    let mut state = state();
    assert!(response::openai_to_responses::flush(&mut state).is_empty());
    assert!(finalize_response(Format::OpenAiResponses, &mut state).is_empty());
}

#[test]
fn finalize_is_a_noop_for_chunk_shaped_formats() {
    let mut state = state();
    assert!(finalize_response(Format::OpenAi, &mut state).is_empty());
    assert!(finalize_response(Format::Claude, &mut state).is_empty());
}

#[test]
fn malformed_chunks_never_panic() {
    let mut state = state();
    for bad in [json!({}), json!({ "choices": [] }), json!(null)] {
        let _ = response::openai_to_responses::translate(Some(&bad), &mut state);
    }
}
