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

// ── grok-web ──────────────────────────────────────────────────────────────────
//
// The first of the six formats upstream implements as a full executor subclass rather than as request
// shaping. It stays on the shared path here, because what actually differs is expressible as hooks:
// the body is replaced rather than wrapped, the credential is a cookie rather than a header token, and
// the response is NDJSON. Its fixed host cannot be pointed at loopback, so these assert on the request
// the executor builds — which is the whole of the difference.

fn grok_credentials() -> Credentials {
    Credentials {
        // Users paste the `sso` cookie into the field a panel labels "API key".
        api_key: Some("sso=session-value".to_owned()),
        connection_id: "conn_grok".to_owned(),
        connection_name: "grok web".to_owned(),
        ..Credentials::default()
    }
}

#[test]
fn grok_web_replaces_the_body_with_grok_coms_own_payload() {
    let body = json!({
        "model": "grok-4-thinking",
        "messages": [
            { "role": "system", "content": "Be brief." },
            { "role": "user", "content": "Why is the sky blue?" },
        ],
    });

    let prepared = prepare(&ExecuteRequest {
        provider: "grok-web",
        body: &body,
        stream: true,
        credentials: &grok_credentials(),
    });

    // Not a chat-completions body at all: grok.com takes one `message` string plus a mode.
    assert!(
        prepared.body.get("messages").is_none(),
        "the OpenAI array must not survive: {}",
        prepared.body
    );
    assert_eq!(prepared.body.get("modelName"), Some(&json!("grok-4")));
    assert_eq!(
        prepared.body.get("modelMode"),
        Some(&json!("MODEL_MODE_GROK_4_THINKING"))
    );
    // Role prefixes on every turn but the final user one, which is the prompt itself.
    assert_eq!(
        prepared.body.get("message"),
        Some(&json!("system: Be brief.\n\nWhy is the sky blue?"))
    );
    // Keeps routed traffic out of the account's saved history.
    assert_eq!(prepared.body.get("temporary"), Some(&json!(true)));
}

#[test]
fn grok_web_authenticates_by_cookie_and_carries_no_bearer_token() {
    let prepared = prepare(&ExecuteRequest {
        provider: "grok-web",
        body: &json!({ "model": "grok-4", "messages": [{ "role": "user", "content": "hi" }] }),
        stream: true,
        credentials: &grok_credentials(),
    });

    // The pasted `sso=` prefix must not be doubled.
    assert_eq!(
        prepared.headers.get("Cookie").map(String::as_str),
        Some("sso=session-value")
    );
    // A bearer token alongside the cookie is how this endpoint rejects a request for carrying two
    // conflicting credentials, so the generic auth header must be gone.
    assert!(
        !prepared.headers.contains_key("Authorization"),
        "{:?}",
        prepared.headers
    );
    assert!(!prepared.headers.contains_key("x-api-key"));
    // The credential must never appear anywhere else.
    let rendered = format!("{:?}{}", prepared.headers, prepared.body);
    assert_eq!(
        rendered.matches("session-value").count(),
        1,
        "the cookie belongs in exactly one place: {rendered}"
    );
}

#[test]
fn grok_web_sends_the_browser_headers_the_endpoint_requires() {
    let prepared = prepare(&ExecuteRequest {
        provider: "grok-web",
        body: &json!({ "model": "grok-4", "messages": [{ "role": "user", "content": "hi" }] }),
        stream: true,
        credentials: &grok_credentials(),
    });

    // This is the web app's endpoint, not an API. It refuses a request that does not look like its own
    // client, so these are load-bearing rather than cosmetic.
    for (header, value) in [
        ("Origin", "https://grok.com"),
        ("Referer", "https://grok.com/"),
        ("Sec-Fetch-Site", "same-origin"),
    ] {
        assert_eq!(
            prepared.headers.get(header).map(String::as_str),
            Some(value),
            "{header} is required by the endpoint"
        );
    }
    assert!(
        prepared
            .headers
            .get("User-Agent")
            .is_some_and(|agent| agent.contains("Chrome")),
        "a browser User-Agent is required: {:?}",
        prepared.headers.get("User-Agent")
    );

    // `Accept-Encoding` is deliberately not claimed: reqwest negotiates what it can actually decode,
    // and advertising zstd support this client lacks yields a body it cannot read.
    assert!(!prepared.headers.contains_key("Accept-Encoding"));
}

#[test]
fn grok_webs_per_request_ids_differ_between_calls() {
    let request = || {
        prepare(&ExecuteRequest {
            provider: "grok-web",
            body: &json!({ "model": "grok-4", "messages": [{ "role": "user", "content": "hi" }] }),
            stream: true,
            credentials: &grok_credentials(),
        })
    };
    let first = request();
    let second = request();

    // A fixed id would make every routed request look like one retried call in xAI's own tracing.
    assert_ne!(
        first.headers.get("x-xai-request-id"),
        second.headers.get("x-xai-request-id")
    );
    assert_ne!(
        first.headers.get("traceparent"),
        second.headers.get("traceparent")
    );
    // Still the right shape: version, 32-hex trace, 16-hex span, flags.
    let traceparent = first
        .headers
        .get("traceparent")
        .expect("a traceparent")
        .clone();
    let parts: Vec<&str> = traceparent.split('-').collect();
    assert_eq!(parts.len(), 4, "{traceparent}");
    assert_eq!(parts.first().copied(), Some("00"));
    assert_eq!(
        parts.get(1).map(|trace| trace.len()),
        Some(32),
        "{traceparent}"
    );
    assert_eq!(
        parts.get(2).map(|span| span.len()),
        Some(16),
        "{traceparent}"
    );
}

#[test]
fn grok_web_posts_to_the_conversation_endpoint() {
    let credentials = grok_credentials();
    let url = build_url("grok-web", &credentials, 0).expect("a base url");
    assert_eq!(
        url, "https://grok.com/rest/app-chat/conversations/new",
        "the registry already carries grok.com's endpoint"
    );
    // No URL suffix: unlike gemini-cli, grok.com does not select a method in the path.
    let prepared = prepare(&ExecuteRequest {
        provider: "grok-web",
        body: &json!({ "model": "grok-4", "messages": [{ "role": "user", "content": "hi" }] }),
        stream: true,
        credentials: &credentials,
    });
    assert_eq!(prepared.url_suffix, "");
}

#[test]
fn an_unmapped_grok_model_still_dispatches() {
    // xAI ships model names faster than a table learns them. Refusing an unknown one would turn a
    // working account into a dead one until this port is edited.
    let prepared = prepare(&ExecuteRequest {
        provider: "grok-web",
        body: &json!({
            "model": "grok-9-unreleased",
            "messages": [{ "role": "user", "content": "hi" }],
        }),
        stream: true,
        credentials: &grok_credentials(),
    });
    assert_eq!(
        prepared.body.get("modelName"),
        Some(&json!("grok-4-1-thinking-1129")),
        "an unknown name falls back to the default mode"
    );
}
