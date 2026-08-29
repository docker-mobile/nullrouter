//! Providers that need request shaping the registry cannot describe.
//!
//! `gemini-cli` and `commandcode` were on the unported list. Neither needs its own
//! executor: what they need is an envelope, a per-request header, and a URL whose
//! method depends on whether the request streams.
//!
//! Both target a fixed HTTPS host that cannot be pointed at a loopback socket, so
//! these assert on the request the executor builds rather than on a delivered
//! response — the envelope, headers, and URL suffix *are* the substance of the
//! difference, and the shared dispatch below them is covered by `execution.rs`.

mod mock_upstream;

use mock_upstream::{MockResponse, MockUpstream};
use nullrouter_execute::credentials::{Credentials, build_url};
use nullrouter_execute::{ExecuteRequest, Executor, prepare};
use serde_json::{Value, json};

fn gemini_cli_credentials(project: &str) -> Credentials {
    let mut credentials = Credentials {
        access_token: Some("ya29.token".to_owned()),
        connection_id: "conn_gc".to_owned(),
        connection_name: "gemini cli".to_owned(),
        ..Credentials::default()
    };
    credentials
        .provider_specific_data
        .insert("projectId".to_owned(), json!(project));
    credentials
}

fn gemini_body() -> Value {
    json!({
        "model": "gemini-2.5-pro",
        "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
    })
}

#[test]
fn gemini_cli_wraps_its_body_and_names_its_method_in_the_url() {
    let credentials = gemini_cli_credentials("proj-1");
    let body = gemini_body();

    let streaming = prepare(&ExecuteRequest {
        provider: "gemini-cli",
        body: &body,
        stream: true,
        credentials: &credentials,
    });

    // The Gemini payload is wrapped in the Cloud Code Assist envelope. Sending it
    // bare is rejected: this endpoint takes `{ project, model, request }`.
    assert_eq!(streaming.body.get("project"), Some(&json!("proj-1")));
    assert_eq!(streaming.body.get("model"), Some(&json!("gemini-2.5-pro")));
    assert_eq!(streaming.body.get("request"), Some(&body));

    // The method is selected in the URL, and the streaming one needs `alt=sse` or
    // it answers with one JSON blob instead of a stream.
    let base = build_url("gemini-cli", &credentials, 0).expect("a base url");
    assert_eq!(base, "https://cloudcode-pa.googleapis.com/v1internal");
    assert_eq!(
        format!("{base}{}", streaming.url_suffix),
        "https://cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse"
    );

    let blocking = prepare(&ExecuteRequest {
        provider: "gemini-cli",
        body: &body,
        stream: false,
        credentials: &credentials,
    });
    assert_eq!(
        format!("{base}{}", blocking.url_suffix),
        "https://cloudcode-pa.googleapis.com/v1internal:generateContent"
    );
}

#[test]
fn gemini_cli_sends_the_cli_identity_and_its_oauth_token() {
    let credentials = gemini_cli_credentials("proj-1");
    let body = gemini_body();
    let prepared = prepare(&ExecuteRequest {
        provider: "gemini-cli",
        body: &body,
        stream: true,
        credentials: &credentials,
    });

    // Quota is keyed off the CLI's own user agent, so the generic reqwest one is
    // not interchangeable, and the model has to appear in it.
    let agent = prepared
        .headers
        .get("User-Agent")
        .expect("a user agent")
        .clone();
    assert!(agent.starts_with("GeminiCLI/"), "got {agent}");
    assert!(agent.contains("gemini-2.5-pro"), "got {agent}");
    assert!(prepared.headers.contains_key("X-Goog-Api-Client"));
    // An OAuth connection sends a bearer token, not an API key header.
    assert_eq!(
        prepared.headers.get("Authorization").map(String::as_str),
        Some("Bearer ya29.token")
    );
    assert_eq!(
        prepared.headers.get("Accept").map(String::as_str),
        Some("text/event-stream")
    );
}

#[test]
fn a_retried_gemini_cli_body_is_not_wrapped_twice() {
    // `prepare` runs per dispatch. If it wrapped an already-wrapped body, a retry
    // would send `{ request: { request: … } }` and the provider would reject it.
    let credentials = gemini_cli_credentials("proj-1");
    let once = prepare(&ExecuteRequest {
        provider: "gemini-cli",
        body: &gemini_body(),
        stream: true,
        credentials: &credentials,
    });
    let twice = prepare(&ExecuteRequest {
        provider: "gemini-cli",
        body: &once.body,
        stream: true,
        credentials: &credentials,
    });
    assert_eq!(once.body, twice.body);
    assert!(
        twice.body.pointer("/request/request").is_none(),
        "double-wrapped: {}",
        twice.body
    );
}

#[test]
fn commandcode_carries_a_fresh_session_id_and_its_api_key() {
    let credentials = Credentials {
        api_key: Some("user_abc123".to_owned()),
        connection_id: "conn_cc".to_owned(),
        connection_name: "commandcode".to_owned(),
        ..Credentials::default()
    };
    let body = json!({
        "model": "sonnet",
        "stream": true,
        "messages": [{ "role": "user", "content": "hi" }],
    });
    let request = ExecuteRequest {
        provider: "commandcode",
        body: &body,
        stream: true,
        credentials: &credentials,
    };

    let first = prepare(&request);
    let second = prepare(&request);
    let read = |prepared: &nullrouter_execute::PreparedRequest| {
        prepared
            .headers
            .get("x-session-id")
            .expect("a session id")
            .clone()
    };
    // A reused id makes two requests indistinguishable in the provider's own logs,
    // which is what the header exists to prevent.
    assert_ne!(read(&first), read(&second));

    // The registry's own headers survive alongside it.
    assert!(first.headers.contains_key("x-command-code-version"));
    assert_eq!(
        first.headers.get("x-cli-environment").map(String::as_str),
        Some("cli")
    );
    // The CommandCode key is a bearer token, not an `x-api-key`.
    assert_eq!(
        first.headers.get("Authorization").map(String::as_str),
        Some("Bearer user_abc123")
    );
    // And no envelope: only Cloud Code Assist wants one.
    assert_eq!(first.body, body);
    assert!(first.url_suffix.is_empty());
}

#[tokio::test]
async fn a_provider_with_no_hooks_sends_its_body_unwrapped() {
    // The generic path must be undisturbed by the hooks above, checked end to end
    // against a real socket.
    let server = MockUpstream::start(vec![MockResponse::json(
        200,
        r#"{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","content":"hi"}}]}"#,
    )])
    .await;
    let mut credentials = Credentials {
        api_key: Some("sk-test".to_owned()),
        ..Credentials::default()
    };
    credentials.provider_specific_data.insert(
        "baseUrl".to_owned(),
        json!(format!("http://{}", server.addr)),
    );

    let body = json!({ "model": "gpt-5", "messages": [{ "role": "user", "content": "hi" }] });
    Executor::new()
        .execute(ExecuteRequest {
            provider: "openai-compatible-plain",
            body: &body,
            stream: false,
            credentials: &credentials,
        })
        .await
        .expect("dispatch");

    let requests = server.requests();
    let sent = requests.first().expect("upstream was called");
    let parsed: Value = serde_json::from_str(&sent.body).expect("json body");
    assert!(parsed.get("request").is_none(), "{parsed}");
    assert!(parsed.get("project").is_none(), "{parsed}");
    assert_eq!(parsed.get("model"), Some(&json!("gpt-5")));
    assert_eq!(sent.headers.get("x-session-id"), None);
}
