//! Streaming response-translation parity tests.
//!
//! These assert the exact client-visible frame sequences, since a subtly wrong
//! stream is worse than a failed request: clients hang or render garbage.

use nullrouter_providers::Format;
use nullrouter_translate::state::Clock;
use nullrouter_translate::{StreamState, response, translate_response};
use serde_json::{Value, json};

const FIXED_MILLIS: u64 = 1_700_000_123_456;

const fn state() -> StreamState {
    StreamState::new(Clock::Fixed(FIXED_MILLIS))
}

/// Event/chunk discriminators in order, for sequence assertions.
fn claude_types(frames: &[Value]) -> Vec<String> {
    frames
        .iter()
        .filter_map(|frame| frame.get("type").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

fn claude_stream(state: &mut StreamState, events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .flat_map(|event| response::claude_to_openai::translate(event, state))
        .collect()
}

#[test]
fn claude_stream_becomes_openai_chunks_in_order() {
    let mut state = state();
    let frames = claude_stream(
        &mut state,
        &[
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_abc12345",
                    "model": "claude-sonnet-4.5",
                    "usage": { "input_tokens": 10, "cache_read_input_tokens": 4 },
                },
            }),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text", "text": "" },
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": "Hello" },
            }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn" },
                "usage": { "output_tokens": 7 },
            }),
            json!({ "type": "message_stop" }),
        ],
    );

    // role chunk, content chunk, final chunk with finish_reason.
    assert_eq!(frames.len(), 3);
    assert_eq!(
        frames
            .first()
            .and_then(|f| f.pointer("/choices/0/delta/role")),
        Some(&json!("assistant"))
    );
    assert_eq!(
        frames
            .get(1)
            .and_then(|f| f.pointer("/choices/0/delta/content")),
        Some(&json!("Hello"))
    );

    let last = frames.get(2).expect("final chunk");
    assert_eq!(
        last.pointer("/choices/0/finish_reason"),
        Some(&json!("stop"))
    );
    // Ids are derived from the Claude message id.
    assert_eq!(last.get("id"), Some(&json!("chatcmpl-msg_abc12345")));
    assert_eq!(last.get("model"), Some(&json!("claude-sonnet-4.5")));
    // prompt_tokens folds input + cache; cache detail is preserved.
    assert_eq!(last.pointer("/usage/prompt_tokens"), Some(&json!(14)));
    assert_eq!(last.pointer("/usage/completion_tokens"), Some(&json!(7)));
    assert_eq!(last.pointer("/usage/total_tokens"), Some(&json!(21)));
    assert_eq!(
        last.pointer("/usage/prompt_tokens_details/cached_tokens"),
        Some(&json!(4))
    );
}

#[test]
fn claude_message_stop_does_not_duplicate_the_finish_chunk() {
    let mut state = state();
    let frames = claude_stream(
        &mut state,
        &[
            json!({ "type": "message_start", "message": { "id": "msg_abc12345" } }),
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn" },
                "usage": { "output_tokens": 1 },
            }),
            json!({ "type": "message_stop" }),
        ],
    );
    // Exactly one chunk carries finish_reason.
    let finishes = frames
        .iter()
        .filter(|frame| {
            frame
                .pointer("/choices/0/finish_reason")
                .is_some_and(|reason| !reason.is_null())
        })
        .count();
    assert_eq!(finishes, 1);
}

#[test]
fn claude_message_stop_alone_still_terminates_the_stream() {
    let mut state = state();
    let frames = claude_stream(
        &mut state,
        &[
            json!({ "type": "message_start", "message": { "id": "msg_abc12345" } }),
            json!({ "type": "message_stop" }),
        ],
    );
    // Without a message_delta, message_stop must synthesize the finish chunk.
    assert_eq!(
        frames
            .last()
            .and_then(|f| f.pointer("/choices/0/finish_reason")),
        Some(&json!("stop"))
    );
}

#[test]
fn claude_thinking_blocks_are_wrapped_in_think_tags() {
    let mut state = state();
    let frames = claude_stream(
        &mut state,
        &[
            json!({ "type": "message_start", "message": { "id": "msg_abc12345" } }),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "thinking", "thinking": "" },
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "thinking_delta", "thinking": "pondering" },
            }),
            json!({ "type": "content_block_stop", "index": 0 }),
        ],
    );

    let contents: Vec<&str> = frames
        .iter()
        .filter_map(|frame| {
            frame
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
        })
        .collect();
    assert_eq!(contents, vec!["<think>", "</think>"]);
    // Thinking text itself surfaces as reasoning_content, not content.
    assert_eq!(
        frames
            .get(2)
            .and_then(|f| f.pointer("/choices/0/delta/reasoning_content")),
        Some(&json!("pondering"))
    );
}

#[test]
fn claude_tool_calls_stream_incremental_arguments() {
    let mut state = state();
    let frames = claude_stream(
        &mut state,
        &[
            json!({ "type": "message_start", "message": { "id": "msg_abc12345" } }),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "tool_use", "id": "toolu_1", "name": "Read" },
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "input_json_delta", "partial_json": "{\"file" },
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "input_json_delta", "partial_json": "_path\":\"/a\"}" },
            }),
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": "tool_use" },
                "usage": { "output_tokens": 3 },
            }),
        ],
    );

    assert_eq!(
        frames
            .get(1)
            .and_then(|f| f.pointer("/choices/0/delta/tool_calls/0/function/name")),
        Some(&json!("Read"))
    );
    assert_eq!(
        frames
            .get(1)
            .and_then(|f| f.pointer("/choices/0/delta/tool_calls/0/id")),
        Some(&json!("toolu_1"))
    );
    // Argument fragments are forwarded as they arrive.
    let fragments: Vec<&str> = frames
        .iter()
        .filter_map(|frame| {
            frame
                .pointer("/choices/0/delta/tool_calls/0/function/arguments")
                .and_then(Value::as_str)
        })
        .filter(|fragment| !fragment.is_empty())
        .collect();
    assert_eq!(fragments, vec!["{\"file", "_path\":\"/a\"}"]);
    assert_eq!(
        frames
            .last()
            .and_then(|f| f.pointer("/choices/0/finish_reason")),
        Some(&json!("tool_calls"))
    );
}

#[test]
fn claude_server_tool_blocks_are_skipped_entirely() {
    let mut state = state();
    let frames = claude_stream(
        &mut state,
        &[
            json!({ "type": "message_start", "message": { "id": "msg_abc12345" } }),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "server_tool_use", "id": "srv_1", "name": "web_search" },
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "input_json_delta", "partial_json": "{\"q\":\"x\"}" },
            }),
            json!({ "type": "content_block_stop", "index": 0 }),
        ],
    );
    // Only the initial role chunk: Claude runs built-in tools itself.
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames
            .first()
            .and_then(|f| f.pointer("/choices/0/delta/role")),
        Some(&json!("assistant"))
    );
}

#[test]
fn openai_stream_becomes_claude_events_in_order() {
    let mut state = state();
    let frames: Vec<Value> = [
        json!({
            "id": "chatcmpl-abcdef123",
            "model": "gpt-5",
            "choices": [{ "index": 0, "delta": { "role": "assistant" } }],
        }),
        json!({
            "id": "chatcmpl-abcdef123",
            "model": "gpt-5",
            "choices": [{ "index": 0, "delta": { "content": "Hi" } }],
        }),
        json!({
            "id": "chatcmpl-abcdef123",
            "model": "gpt-5",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 5 },
        }),
    ]
    .iter()
    .flat_map(|chunk| response::openai_to_claude::translate(chunk, &mut state))
    .collect();

    assert_eq!(
        claude_types(&frames),
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ]
    );
    assert_eq!(
        frames.first().and_then(|f| f.pointer("/message/id")),
        Some(&json!("abcdef123"))
    );
    assert_eq!(
        frames.first().and_then(|f| f.pointer("/message/model")),
        Some(&json!("gpt-5"))
    );
    // stop -> end_turn, with Claude-native token names.
    let delta = frames.get(4).expect("message_delta");
    assert_eq!(
        delta.pointer("/delta/stop_reason"),
        Some(&json!("end_turn"))
    );
    assert_eq!(delta.pointer("/usage/input_tokens"), Some(&json!(12)));
    assert_eq!(delta.pointer("/usage/output_tokens"), Some(&json!(5)));
}

#[test]
fn openai_to_claude_emits_message_start_even_without_a_role_delta() {
    let mut state = state();
    let frames = response::openai_to_claude::translate(
        &json!({
            "id": "chatcmpl-abcdef123",
            "model": "gpt-5",
            "choices": [{ "index": 0, "delta": { "content": "x" } }],
        }),
        &mut state,
    );
    assert_eq!(
        claude_types(&frames).first().map(String::as_str),
        Some("message_start")
    );
}

#[test]
fn openai_to_claude_subtracts_cache_tokens_from_input() {
    let mut state = state();
    let frames = response::openai_to_claude::translate(
        &json!({
            "id": "chatcmpl-abcdef123",
            "model": "gpt-5",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 10,
                "prompt_tokens_details": { "cached_tokens": 30, "cache_creation_tokens": 5 },
            },
        }),
        &mut state,
    );
    let delta = frames
        .iter()
        .find(|frame| frame.get("type").and_then(Value::as_str) == Some("message_delta"))
        .expect("message_delta");
    // Claude input_tokens excludes cache; OpenAI prompt_tokens includes it.
    assert_eq!(delta.pointer("/usage/input_tokens"), Some(&json!(65)));
    assert_eq!(
        delta.pointer("/usage/cache_read_input_tokens"),
        Some(&json!(30))
    );
    assert_eq!(
        delta.pointer("/usage/cache_creation_input_tokens"),
        Some(&json!(5))
    );
}

#[test]
fn openai_to_claude_buffers_tool_args_and_sanitizes_read_limits() {
    let mut state = state();
    let frames: Vec<Value> = [
        json!({
            "id": "chatcmpl-abcdef123",
            "model": "gpt-5",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "Read", "arguments": "" },
                    }],
                },
            }],
        }),
        json!({
            "id": "chatcmpl-abcdef123",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "{\"file_path\":\"/a\",\"limit\":\"9999\"" },
                    }],
                },
            }],
        }),
        json!({
            "id": "chatcmpl-abcdef123",
            "choices": [{
                "index": 0,
                "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": ",\"offset\":\"-5\"}" } }] },
            }],
        }),
        json!({
            "id": "chatcmpl-abcdef123",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }],
        }),
    ]
    .iter()
    .flat_map(|chunk| response::openai_to_claude::translate(chunk, &mut state))
    .collect();

    // Arguments are emitted once, at finish, not streamed piecemeal.
    let json_deltas: Vec<&Value> = frames
        .iter()
        .filter(|frame| {
            frame.pointer("/delta/type").and_then(Value::as_str) == Some("input_json_delta")
        })
        .collect();
    assert_eq!(json_deltas.len(), 1);

    let payload = json_deltas
        .first()
        .and_then(|frame| frame.pointer("/delta/partial_json"))
        .and_then(Value::as_str)
        .expect("sanitized payload");
    let parsed: Value = serde_json::from_str(payload).expect("valid JSON after sanitization");
    // Numeric strings coerced; limit clamped to 2000; negative offset floored to 0.
    assert_eq!(parsed.get("limit"), Some(&json!(2000)));
    assert_eq!(parsed.get("offset"), Some(&json!(0)));
    assert_eq!(parsed.get("file_path"), Some(&json!("/a")));

    assert_eq!(
        frames.last().and_then(|f| f.get("type")),
        Some(&json!("message_stop"))
    );
}

#[test]
fn openai_to_claude_drops_pages_for_non_pdf_reads() {
    let mut state = state();
    let frames: Vec<Value> = [
        json!({
            "id": "chatcmpl-abcdef123",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "Read",
                            "arguments": "{\"file_path\":\"/a.txt\",\"pages\":\"1-2\"}",
                        },
                    }],
                },
            }],
        }),
        json!({
            "id": "chatcmpl-abcdef123",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }],
        }),
    ]
    .iter()
    .flat_map(|chunk| response::openai_to_claude::translate(chunk, &mut state))
    .collect();

    let payload = frames
        .iter()
        .find(|frame| {
            frame.pointer("/delta/type").and_then(Value::as_str) == Some("input_json_delta")
        })
        .and_then(|frame| frame.pointer("/delta/partial_json"))
        .and_then(Value::as_str)
        .expect("payload");
    let parsed: Value = serde_json::from_str(payload).expect("valid JSON");
    assert!(
        parsed.get("pages").is_none(),
        "pages is invalid for a .txt path"
    );
}

#[test]
fn openai_to_claude_opens_one_block_per_tool_index() {
    let mut state = state();
    // Some vendors repeat the id with a null name on every argument chunk.
    let frames: Vec<Value> = [
        json!({
            "id": "chatcmpl-abcdef123",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "Read", "arguments": "{}" },
                    }],
                },
            }],
        }),
        json!({
            "id": "chatcmpl-abcdef123",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": { "name": null, "arguments": "" },
                    }],
                },
            }],
        }),
    ]
    .iter()
    .flat_map(|chunk| response::openai_to_claude::translate(chunk, &mut state))
    .collect();

    let opens = frames
        .iter()
        .filter(|frame| {
            frame.get("type").and_then(Value::as_str) == Some("content_block_start")
                && frame.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use")
        })
        .count();
    assert_eq!(opens, 1);
}

#[test]
fn openai_to_claude_closes_text_before_opening_a_tool_block() {
    let mut state = state();
    let frames: Vec<Value> = [
        json!({
            "id": "chatcmpl-abcdef123",
            "choices": [{ "index": 0, "delta": { "content": "thinking..." } }],
        }),
        json!({
            "id": "chatcmpl-abcdef123",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "Read", "arguments": "{}" },
                    }],
                },
            }],
        }),
    ]
    .iter()
    .flat_map(|chunk| response::openai_to_claude::translate(chunk, &mut state))
    .collect();

    let types = claude_types(&frames);
    let stop_at = types.iter().position(|kind| kind == "content_block_stop");
    let tool_open_at = frames.iter().position(|frame| {
        frame.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use")
    });
    assert!(
        stop_at < tool_open_at,
        "text block must close before the tool block opens: {types:?}"
    );
    // Block indices must not collide.
    assert_eq!(
        frames
            .iter()
            .find(|f| f.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use"))
            .and_then(|f| f.get("index")),
        Some(&json!(1))
    );
}

#[test]
fn gemini_stream_becomes_openai_chunks() {
    let mut state = state();
    let frames: Vec<Value> = [
        json!({
            "responseId": "resp_1",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [{ "content": { "parts": [{ "text": "Hello" }] } }],
        }),
        json!({
            "candidates": [{ "content": { "parts": [] }, "finishReason": "STOP" }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 4,
                "totalTokenCount": 14,
            },
        }),
    ]
    .iter()
    .flat_map(|chunk| response::gemini_to_openai::translate(chunk, &mut state))
    .collect();

    assert_eq!(
        frames
            .first()
            .and_then(|f| f.pointer("/choices/0/delta/role")),
        Some(&json!("assistant"))
    );
    assert_eq!(
        frames
            .get(1)
            .and_then(|f| f.pointer("/choices/0/delta/content")),
        Some(&json!("Hello"))
    );
    assert_eq!(
        frames.first().and_then(|f| f.get("id")),
        Some(&json!("chatcmpl-resp_1"))
    );

    let last = frames.last().expect("final chunk");
    assert_eq!(
        last.pointer("/choices/0/finish_reason"),
        Some(&json!("stop"))
    );
    assert_eq!(last.pointer("/usage/prompt_tokens"), Some(&json!(10)));
    assert_eq!(last.pointer("/usage/total_tokens"), Some(&json!(14)));
}

#[test]
fn gemini_thought_parts_become_reasoning_not_content() {
    let mut state = state();
    let frames = response::gemini_to_openai::translate(
        &json!({
            "responseId": "resp_1",
            "candidates": [{
                "content": {
                    "parts": [
                        { "text": "internal", "thought": true },
                        { "text": "visible" },
                    ],
                },
            }],
        }),
        &mut state,
    );
    assert_eq!(
        frames
            .get(1)
            .and_then(|f| f.pointer("/choices/0/delta/reasoning_content")),
        Some(&json!("internal"))
    );
    assert_eq!(
        frames
            .get(2)
            .and_then(|f| f.pointer("/choices/0/delta/content")),
        Some(&json!("visible"))
    );
}

#[test]
fn gemini_function_calls_force_tool_calls_finish_reason() {
    let mut state = state();
    let frames: Vec<Value> = [
        json!({
            "responseId": "resp_1",
            "candidates": [{
                "content": { "parts": [{ "functionCall": { "name": "Read", "args": { "p": 1 } } }] },
            }],
        }),
        json!({ "candidates": [{ "content": { "parts": [] }, "finishReason": "STOP" }] }),
    ]
    .iter()
    .flat_map(|chunk| response::gemini_to_openai::translate(chunk, &mut state))
    .collect();

    assert_eq!(
        frames
            .get(1)
            .and_then(|f| f.pointer("/choices/0/delta/tool_calls/0/function/name")),
        Some(&json!("Read"))
    );
    // Gemini reports STOP even after emitting tool calls; it must be corrected.
    assert_eq!(
        frames
            .last()
            .and_then(|f| f.pointer("/choices/0/finish_reason")),
        Some(&json!("tool_calls"))
    );
}

#[test]
fn gemini_inline_image_data_becomes_a_data_uri() {
    let mut state = state();
    let frames = response::gemini_to_openai::translate(
        &json!({
            "responseId": "resp_1",
            "candidates": [{
                "content": {
                    "parts": [{ "inlineData": { "mimeType": "image/png", "data": "QUJD" } }],
                },
            }],
        }),
        &mut state,
    );
    assert_eq!(
        frames
            .get(1)
            .and_then(|f| f.pointer("/choices/0/delta/images/0/image_url/url")),
        Some(&json!("data:image/png;base64,QUJD"))
    );
}

#[test]
fn antigravity_response_wrapper_is_unwrapped() {
    let mut state = state();
    let frames = response::gemini_to_openai::translate(
        &json!({
            "response": {
                "responseId": "resp_1",
                "candidates": [{ "content": { "parts": [{ "text": "hi" }] } }],
            },
        }),
        &mut state,
    );
    assert_eq!(
        frames
            .get(1)
            .and_then(|f| f.pointer("/choices/0/delta/content")),
        Some(&json!("hi"))
    );
}

#[test]
fn dispatch_pivots_gemini_upstream_to_a_claude_client() {
    let mut state = state();
    let frames = translate_response(
        Format::Gemini,
        Format::Claude,
        &json!({
            "responseId": "resp_1",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [{ "content": { "parts": [{ "text": "hi" }] } }],
        }),
        &mut state,
    );
    // Gemini -> OpenAI -> Claude, emerging as Claude events.
    let types = claude_types(&frames);
    assert!(types.contains(&"message_start".to_owned()), "got {types:?}");
    assert!(
        types.contains(&"content_block_delta".to_owned()),
        "got {types:?}"
    );
}

#[test]
fn dispatch_passes_matching_formats_through_untouched() {
    let mut state = state();
    let chunk = json!({ "type": "content_block_delta", "delta": { "text": "x" } });
    let frames = translate_response(Format::Claude, Format::Claude, &chunk, &mut state);
    assert_eq!(frames, vec![chunk]);
}

#[test]
fn malformed_chunks_never_panic_and_yield_nothing() {
    let mut state = state();
    for chunk in [
        json!({}),
        json!({ "choices": [] }),
        json!({ "type": "unknown_event" }),
        json!(null),
        json!({ "candidates": [] }),
    ] {
        // Each translator must tolerate junk without panicking.
        let _ = response::claude_to_openai::translate(&chunk, &mut state);
        let _ = response::gemini_to_openai::translate(&chunk, &mut state);
        let _ = response::openai_to_claude::translate(&chunk, &mut state);
    }
}
