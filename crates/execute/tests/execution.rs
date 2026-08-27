//! End-to-end execution tests: real sockets, real HTTP, real translation.

mod mock_upstream;

use std::collections::BTreeMap;

use mock_upstream::{MockResponse, MockUpstream};
use nullrouter_execute::credentials::Credentials;
use nullrouter_execute::{ExecuteRequest, Executor, collapse_stream_to_json, pipe_stream};
use nullrouter_providers::Format;
use nullrouter_translate::StreamState;
use nullrouter_translate::state::Clock;
use serde_json::{Value, json};

/// Credentials pointing an `openai-compatible-*` provider at a mock server.
fn credentials_for(base_url: &str) -> Credentials {
    let mut credentials = Credentials {
        api_key: Some("sk-test".to_owned()),
        connection_id: "conn_1".to_owned(),
        connection_name: "test".to_owned(),
        ..Credentials::default()
    };
    credentials
        .provider_specific_data
        .insert("baseUrl".to_owned(), json!(base_url));
    credentials
}

/// `openai-compatible-*` targets `{baseUrl}/chat/completions`.
fn base_url_for(server: &MockUpstream) -> String {
    format!("http://{}", server.addr)
}

const fn state() -> StreamState {
    StreamState::new(Clock::Fixed(1_700_000_123_456))
}

#[tokio::test]
async fn non_streaming_request_reaches_upstream_with_auth_and_body() {
    let server = MockUpstream::start(vec![MockResponse::json(
        200,
        r#"{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","content":"hi"}}]}"#,
    )])
    .await;
    let credentials = credentials_for(&base_url_for(&server));
    let body = json!({ "model": "gpt-5", "messages": [{ "role": "user", "content": "hello" }] });

    let outcome = Executor::new()
        .execute(ExecuteRequest {
            provider: "openai-compatible-test",
            body: &body,
            stream: false,
            credentials: &credentials,
        })
        .await
        .expect("request succeeds");

    assert!(outcome.is_success());
    assert!(
        outcome.url.ends_with("/chat/completions"),
        "got {}",
        outcome.url
    );

    let requests = server.requests();
    let request = requests.first().expect("one request recorded");
    assert_eq!(request.path, "/chat/completions");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer sk-test")
    );
    assert_eq!(
        request.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    // Non-streaming requests must not ask for an event stream. reqwest still
    // sends its own default `accept: */*`, so this asserts the value.
    assert_ne!(
        request.headers.get("accept").map(String::as_str),
        Some("text/event-stream")
    );

    let sent: Value = serde_json::from_str(&request.body).expect("body is JSON");
    assert_eq!(sent.pointer("/messages/0/content"), Some(&json!("hello")));
}

#[tokio::test]
async fn streaming_request_sets_accept_header() {
    let server = MockUpstream::start(vec![MockResponse::sse("data: [DONE]\n\n")]).await;
    let credentials = credentials_for(&base_url_for(&server));
    let body = json!({ "model": "gpt-5", "stream": true });

    let outcome = Executor::new()
        .execute(ExecuteRequest {
            provider: "openai-compatible-test",
            body: &body,
            stream: true,
            credentials: &credentials,
        })
        .await
        .expect("request succeeds");
    assert!(outcome.is_event_stream());

    let requests = server.requests();
    assert_eq!(
        requests
            .first()
            .and_then(|request| request.headers.get("accept"))
            .map(String::as_str),
        Some("text/event-stream")
    );
}

#[tokio::test]
async fn openai_stream_passes_through_to_an_openai_client() {
    let upstream = concat!(
        "data: {\"id\":\"chatcmpl-1\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let server = MockUpstream::start(vec![MockResponse::sse(upstream)]).await;
    let credentials = credentials_for(&base_url_for(&server));
    let body = json!({ "model": "gpt-5", "stream": true });

    let outcome = Executor::new()
        .execute(ExecuteRequest {
            provider: "openai-compatible-test",
            body: &body,
            stream: true,
            credentials: &credentials,
        })
        .await
        .expect("request succeeds");

    let mut frames = Vec::new();
    let mut state = state();
    let summary = pipe_stream(
        outcome.response,
        Format::OpenAi,
        Format::OpenAi,
        &mut state,
        |frame| {
            frames.push(frame);
            Ok(())
        },
    )
    .await;

    // Four chunks plus our terminator.
    assert_eq!(frames.len(), 5);
    assert_eq!(frames.last().map(String::as_str), Some("data: [DONE]\n\n"));
    assert_eq!(summary.text, "Hello");
    assert_eq!(summary.finish_reason.as_deref(), Some("stop"));
}

#[tokio::test]
async fn claude_upstream_is_translated_for_an_openai_client() {
    let upstream = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_abcdef12\",\"model\":\"claude-sonnet-4.5\",\"usage\":{\"input_tokens\":9}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi there\"}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let server = MockUpstream::start(vec![MockResponse::sse(upstream)]).await;
    let credentials = credentials_for(&base_url_for(&server));
    let body = json!({ "model": "claude-sonnet-4.5", "stream": true });

    let outcome = Executor::new()
        .execute(ExecuteRequest {
            provider: "openai-compatible-test",
            body: &body,
            stream: true,
            credentials: &credentials,
        })
        .await
        .expect("request succeeds");

    let mut frames = Vec::new();
    let mut state = state();
    let summary = pipe_stream(
        outcome.response,
        Format::Claude,
        Format::OpenAi,
        &mut state,
        |frame| {
            frames.push(frame);
            Ok(())
        },
    )
    .await;

    assert_eq!(summary.text, "Hi there");
    // Every non-terminal frame is a decodable OpenAI chunk.
    let chunks: Vec<Value> = frames
        .iter()
        .filter(|frame| !frame.contains("[DONE]"))
        .filter_map(|frame| {
            let payload = frame.strip_prefix("data: ")?.trim_end();
            serde_json::from_str(payload).ok()
        })
        .collect();
    assert!(!chunks.is_empty());
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.get("object") == Some(&json!("chat.completion.chunk"))),
        "all frames must be OpenAI chunks"
    );
    assert_eq!(
        chunks
            .last()
            .and_then(|chunk| chunk.pointer("/choices/0/finish_reason")),
        Some(&json!("stop"))
    );
    // Upstream's own `event:` lines must not leak to an OpenAI client.
    assert!(frames.iter().all(|frame| !frame.starts_with("event:")));
}

#[tokio::test]
async fn openai_upstream_is_translated_for_a_claude_client() {
    let upstream = concat!(
        "data: {\"id\":\"chatcmpl-abcdef12\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Yo\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-abcdef12\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let server = MockUpstream::start(vec![MockResponse::sse(upstream)]).await;
    let credentials = credentials_for(&base_url_for(&server));
    let body = json!({ "model": "gpt-5", "stream": true });

    let outcome = Executor::new()
        .execute(ExecuteRequest {
            provider: "openai-compatible-test",
            body: &body,
            stream: true,
            credentials: &credentials,
        })
        .await
        .expect("request succeeds");

    let mut frames = Vec::new();
    let mut state = state();
    pipe_stream(
        outcome.response,
        Format::OpenAi,
        Format::Claude,
        &mut state,
        |frame| {
            frames.push(frame);
            Ok(())
        },
    )
    .await;

    let joined = frames.concat();
    // Claude clients need named events in the documented order.
    assert!(joined.contains("event: message_start"), "{joined}");
    assert!(joined.contains("event: content_block_start"), "{joined}");
    assert!(joined.contains("event: content_block_delta"), "{joined}");
    assert!(joined.contains("event: message_delta"), "{joined}");
    assert!(joined.contains("event: message_stop"), "{joined}");
    assert!(joined.ends_with("data: [DONE]\n\n"));

    let start_at = joined.find("event: message_start").unwrap_or(usize::MAX);
    let stop_at = joined.find("event: message_stop").unwrap_or(0);
    assert!(
        start_at < stop_at,
        "message_start must precede message_stop"
    );
}

#[tokio::test]
async fn split_sse_frames_across_tcp_reads_are_reassembled() {
    // A frame deliberately larger than one read, plus CRLF line endings.
    let long_text = "x".repeat(9000);
    let upstream = format!(
        "data: {{\"id\":\"chatcmpl-1\",\"model\":\"gpt-5\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{long_text}\"}}}}]}}\r\n\r\ndata: [DONE]\r\n\r\n"
    );
    let server = MockUpstream::start(vec![MockResponse::sse(&upstream)]).await;
    let credentials = credentials_for(&base_url_for(&server));
    let body = json!({ "model": "gpt-5", "stream": true });

    let outcome = Executor::new()
        .execute(ExecuteRequest {
            provider: "openai-compatible-test",
            body: &body,
            stream: true,
            credentials: &credentials,
        })
        .await
        .expect("request succeeds");

    let mut state = state();
    let summary = pipe_stream(
        outcome.response,
        Format::OpenAi,
        Format::OpenAi,
        &mut state,
        |_| Ok(()),
    )
    .await;
    assert_eq!(
        summary.text.len(),
        9000,
        "large frame must survive reassembly"
    );
}

#[tokio::test]
async fn client_disconnect_stops_consuming_upstream() {
    let upstream = concat!(
        "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"a\"}}]}\n\n",
        "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"b\"}}]}\n\n",
        "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"c\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    let server = MockUpstream::start(vec![MockResponse::sse(upstream)]).await;
    let credentials = credentials_for(&base_url_for(&server));
    let body = json!({ "model": "gpt-5", "stream": true });

    let outcome = Executor::new()
        .execute(ExecuteRequest {
            provider: "openai-compatible-test",
            body: &body,
            stream: true,
            credentials: &credentials,
        })
        .await
        .expect("request succeeds");

    let mut delivered = 0_usize;
    let mut state = state();
    pipe_stream(
        outcome.response,
        Format::OpenAi,
        Format::OpenAi,
        &mut state,
        |_| {
            delivered += 1;
            // Simulate the client going away after the first frame.
            if delivered >= 1 { Err(()) } else { Ok(()) }
        },
    )
    .await;

    // No terminator is written once the client is gone.
    assert_eq!(delivered, 1);
}

#[tokio::test]
async fn forced_stream_collapses_into_a_single_json_body() {
    let upstream = concat!(
        "data: {\"id\":\"chatcmpl-abcdef12\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-abcdef12\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-abcdef12\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
        "data: [DONE]\n\n",
    );
    let server = MockUpstream::start(vec![MockResponse::sse(upstream)]).await;
    let credentials = credentials_for(&base_url_for(&server));
    let body = json!({ "model": "gpt-5" });

    let outcome = Executor::new()
        .execute(ExecuteRequest {
            provider: "openai-compatible-test",
            body: &body,
            stream: false,
            credentials: &credentials,
        })
        .await
        .expect("request succeeds");

    let mut state = state();
    let collapsed =
        collapse_stream_to_json(outcome.response, Format::OpenAi, "gpt-5", &mut state).await;

    assert_eq!(collapsed.get("object"), Some(&json!("chat.completion")));
    assert_eq!(
        collapsed.pointer("/choices/0/message/content"),
        Some(&json!("Hello"))
    );
    assert_eq!(
        collapsed.pointer("/choices/0/message/role"),
        Some(&json!("assistant"))
    );
    assert_eq!(
        collapsed.pointer("/choices/0/finish_reason"),
        Some(&json!("stop"))
    );
    assert_eq!(collapsed.pointer("/usage/total_tokens"), Some(&json!(5)));
    assert_eq!(collapsed.get("model"), Some(&json!("gpt-5")));
}

#[tokio::test]
async fn collapsed_stream_reassembles_tool_call_arguments() {
    let upstream = concat!(
        "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"Read\",\"arguments\":\"{\\\"p\\\"\"}}]}}]}\n\n",
        "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":1}\"}}]}}]}\n\n",
        "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let server = MockUpstream::start(vec![MockResponse::sse(upstream)]).await;
    let credentials = credentials_for(&base_url_for(&server));
    let body = json!({ "model": "gpt-5" });

    let outcome = Executor::new()
        .execute(ExecuteRequest {
            provider: "openai-compatible-test",
            body: &body,
            stream: false,
            credentials: &credentials,
        })
        .await
        .expect("request succeeds");

    let mut state = state();
    let collapsed =
        collapse_stream_to_json(outcome.response, Format::OpenAi, "gpt-5", &mut state).await;

    let arguments = collapsed
        .pointer("/choices/0/message/tool_calls/0/function/arguments")
        .and_then(Value::as_str)
        .expect("tool call arguments");
    let parsed: Value = serde_json::from_str(arguments).expect("arguments reassemble into JSON");
    assert_eq!(parsed.get("p"), Some(&json!(1)));
    assert_eq!(
        collapsed.pointer("/choices/0/finish_reason"),
        Some(&json!("tool_calls"))
    );
}

#[tokio::test]
async fn upstream_error_status_is_surfaced_not_retried_for_4xx() {
    let server = MockUpstream::start(vec![MockResponse::json(
        400,
        r#"{"error":{"message":"model not found"}}"#,
    )])
    .await;
    let credentials = credentials_for(&base_url_for(&server));
    let body = json!({ "model": "nope" });

    let outcome = Executor::new()
        .execute(ExecuteRequest {
            provider: "openai-compatible-test",
            body: &body,
            stream: false,
            credentials: &credentials,
        })
        .await
        .expect("dispatch itself succeeds");

    assert_eq!(outcome.status().as_u16(), 400);
    assert!(!outcome.is_success());
    // 400 is not in the retry policy: exactly one attempt.
    assert_eq!(server.request_count(), 1);

    let text = outcome.response.text().await.unwrap_or_default();
    let parsed = nullrouter_execute::errors::parse_upstream_error(400, &text);
    assert_eq!(parsed.message, "model not found");
}

/// Takes ~9s on purpose: a network failure maps onto the 502 retry policy
/// (3 attempts, 3s apart), so this also proves retries actually fire.
#[tokio::test]
async fn transport_failure_reports_a_bad_gateway_class_error() {
    // Nothing is listening on this port.
    let mut credentials = Credentials {
        api_key: Some("sk-test".to_owned()),
        ..Credentials::default()
    };
    credentials
        .provider_specific_data
        .insert("baseUrl".to_owned(), json!("http://127.0.0.1:1"));
    let body = json!({ "model": "gpt-5" });

    let error = Executor::new()
        .execute(ExecuteRequest {
            provider: "openai-compatible-test",
            body: &body,
            stream: false,
            credentials: &credentials,
        })
        .await
        .expect_err("connection must fail");
    assert_eq!(error.client_status(), 502);
}

#[tokio::test]
async fn anthropic_compatible_targets_the_messages_endpoint_with_x_api_key() {
    let server = MockUpstream::start(vec![MockResponse::json(200, "{}")]).await;
    let mut credentials = Credentials {
        api_key: Some("sk-ant-test".to_owned()),
        ..Credentials::default()
    };
    credentials
        .provider_specific_data
        .insert("baseUrl".to_owned(), json!(base_url_for(&server)));
    let body = json!({ "model": "claude-sonnet-4.5" });

    let outcome = Executor::new()
        .execute(ExecuteRequest {
            provider: "anthropic-compatible-test",
            body: &body,
            stream: false,
            credentials: &credentials,
        })
        .await
        .expect("request succeeds");
    assert!(outcome.url.ends_with("/messages"), "got {}", outcome.url);

    let requests = server.requests();
    let headers: &BTreeMap<String, String> = &requests.first().expect("recorded").headers;
    assert_eq!(
        headers.get("x-api-key").map(String::as_str),
        Some("sk-ant-test")
    );
    assert_eq!(
        headers.get("anthropic-version").map(String::as_str),
        Some("2023-06-01")
    );
}
