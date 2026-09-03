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

// ── perplexity-web ────────────────────────────────────────────────────────────
//
// Second of the six. Its request is stranger than grok's: the whole conversation is encoded as a JSON
// document inside one string field, because perplexity has no message array and no system prompt.

fn perplexity_credentials(token: Option<&str>, cookie: Option<&str>) -> Credentials {
    Credentials {
        access_token: token.map(str::to_owned),
        api_key: cookie.map(str::to_owned),
        connection_id: "conn_pplx".to_owned(),
        connection_name: "perplexity web".to_owned(),
        ..Credentials::default()
    }
}

fn perplexity_body(model: &str) -> Value {
    json!({
        "model": model,
        "messages": [
            { "role": "system", "content": "Be terse." },
            { "role": "user", "content": "When did Rust 1.0 ship?" },
        ],
    })
}

#[test]
fn perplexity_web_encodes_the_conversation_into_one_query_field() {
    let prepared = prepare(&ExecuteRequest {
        provider: "perplexity-web",
        body: &perplexity_body("pplx-sonnet"),
        stream: true,
        credentials: &perplexity_credentials(Some("tok"), None),
    });

    // Not a chat-completions body: perplexity takes `query_str` plus a params block.
    assert!(prepared.body.get("messages").is_none());
    let query = prepared
        .body
        .get("query_str")
        .and_then(Value::as_str)
        .expect("a query string");
    // The query is itself a JSON document, which is where the system prompt goes — perplexity has no
    // field for one, so a bare question would silently drop it.
    let document: Value = serde_json::from_str(query).expect("the query is a JSON document");
    assert_eq!(
        document.pointer("/instructions/0"),
        Some(&json!("Be terse."))
    );
    assert_eq!(
        document.get("query"),
        Some(&json!("When did Rust 1.0 ship?"))
    );

    // The mode and preference are the model mapping, and the params carry the same query again.
    assert_eq!(
        prepared.body.pointer("/params/mode"),
        Some(&json!("copilot"))
    );
    assert_eq!(
        prepared.body.pointer("/params/model_preference"),
        Some(&json!("claude46sonnet"))
    );
    assert_eq!(
        prepared.body.pointer("/params/query_str"),
        Some(&json!(query))
    );
    // Routed traffic stays out of the account's saved threads.
    assert_eq!(
        prepared.body.pointer("/params/is_incognito"),
        Some(&json!(true))
    );
}

#[test]
fn a_thinking_request_reaches_perplexity_as_its_reasoning_preference() {
    let mut body = perplexity_body("pplx-opus");
    if let Some(object) = body.as_object_mut() {
        object.insert("reasoning_effort".to_owned(), json!("high"));
    }
    let prepared = prepare(&ExecuteRequest {
        provider: "perplexity-web",
        body: &body,
        stream: true,
        credentials: &perplexity_credentials(Some("tok"), None),
    });

    // The mode stays `copilot`; the preference is what carries the request.
    assert_eq!(
        prepared.body.pointer("/params/model_preference"),
        Some(&json!("claude46opusthinking"))
    );
}

#[test]
fn perplexity_web_prefers_a_bearer_token_and_falls_back_to_the_cookie() {
    // Both shapes exist because a user may only be able to copy one of them out of a browser.
    let with_token = prepare(&ExecuteRequest {
        provider: "perplexity-web",
        body: &perplexity_body("pplx-gpt"),
        stream: true,
        credentials: &perplexity_credentials(Some("access-tok"), Some("cookie-val")),
    });
    assert_eq!(
        with_token.headers.get("Authorization").map(String::as_str),
        Some("Bearer access-tok")
    );
    assert!(
        !with_token.headers.contains_key("Cookie"),
        "the token wins; sending both would be two credentials"
    );

    let cookie_only = prepare(&ExecuteRequest {
        provider: "perplexity-web",
        body: &perplexity_body("pplx-gpt"),
        stream: true,
        credentials: &perplexity_credentials(None, Some("cookie-val")),
    });
    assert_eq!(
        cookie_only.headers.get("Cookie").map(String::as_str),
        Some("__Secure-next-auth.session-token=cookie-val")
    );
    assert!(!cookie_only.headers.contains_key("Authorization"));
}

#[test]
fn perplexity_web_sends_the_front_end_headers_and_its_api_version() {
    let prepared = prepare(&ExecuteRequest {
        provider: "perplexity-web",
        body: &perplexity_body("pplx-auto"),
        stream: true,
        credentials: &perplexity_credentials(Some("tok"), None),
    });

    for (header, value) in [
        ("Origin", "https://www.perplexity.ai"),
        ("Referer", "https://www.perplexity.ai/"),
        ("X-App-ApiClient", "default"),
    ] {
        assert_eq!(
            prepared.headers.get(header).map(String::as_str),
            Some(value),
            "{header} is part of looking like the front end"
        );
    }
    // The API version is sent as a header and inside the params; a mismatch between them is how a
    // request gets refused for being inconsistent.
    let version = prepared
        .headers
        .get("X-App-ApiVersion")
        .map(String::as_str)
        .expect("a version header");
    assert_eq!(
        prepared.body.pointer("/params/version"),
        Some(&json!(version))
    );
}

#[test]
fn perplexity_web_posts_to_the_sse_endpoint() {
    let credentials = perplexity_credentials(Some("tok"), None);
    assert_eq!(
        build_url("perplexity-web", &credentials, 0).expect("a base url"),
        "https://www.perplexity.ai/rest/sse/perplexity_ask"
    );
}

#[test]
fn declared_tools_reach_perplexity_as_a_hint_rather_than_a_capability() {
    // Perplexity cannot call a tool. Dropping the declaration silently would leave a model describing
    // a call it has no way to make.
    let mut body = perplexity_body("pplx-sonar");
    if let Some(object) = body.as_object_mut() {
        object.insert(
            "tools".to_owned(),
            json!([{ "function": { "name": "get_weather", "description": "Current weather" } }]),
        );
    }
    let prepared = prepare(&ExecuteRequest {
        provider: "perplexity-web",
        body: &body,
        stream: true,
        credentials: &perplexity_credentials(Some("tok"), None),
    });

    let query = prepared
        .body
        .get("query_str")
        .and_then(Value::as_str)
        .expect("a query");
    let document: Value = serde_json::from_str(query).expect("a JSON document");
    let instructions = document
        .get("instructions")
        .and_then(Value::as_array)
        .expect("instructions")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(instructions.contains("cannot invoke"), "{instructions}");
    assert!(instructions.contains("get_weather"), "{instructions}");
}

#[actix_rt::test]
async fn a_perplexity_thread_is_remembered_and_continued_on_the_next_request() {
    // The loop that makes a multi-turn perplexity conversation work at all, end to end: the answer
    // stream carries a thread id, the executor stores it against this exchange, and the next request —
    // whose `messages` now include the answer — sends only the new question plus that id.
    //
    // Without it every turn re-sends the whole conversation as a fresh query document, so perplexity
    // re-reads context it was already holding and the thread it kept is abandoned.
    use nullrouter_execute::{StreamSummary, pipe_stream};
    use nullrouter_providers::Format;
    use nullrouter_translate::state::{Clock, StreamState};

    let first_body = json!({
        "model": "pplx-sonnet",
        "messages": [{ "role": "user", "content": "When did Rust 1.0 ship?" }],
    });

    // A first turn: no thread is known, so the whole document goes out.
    let opening = prepare(&ExecuteRequest {
        provider: "perplexity-web",
        body: &first_body,
        stream: true,
        credentials: &perplexity_credentials(Some("tok"), None),
    });
    assert_eq!(
        opening.body.pointer("/params/last_backend_uuid"),
        Some(&json!(null)),
        "a first turn has no thread to continue"
    );

    // Perplexity answers, naming the thread it opened.
    let events = concat!(
        "data: {\"backend_uuid\":\"thread-abc\",\"blocks\":[{\"intended_usage\":\"ask_text_markdown\",",
        "\"markdown_block\":{\"progress\":\"DONE\",\"chunks\":[\"May 2015.\"]}}]}\n\n",
    );
    let upstream = MockUpstream::start(vec![MockResponse::sse(events)]).await;
    let response = reqwest::Client::new()
        .post(upstream.url())
        .send()
        .await
        .expect("the stub answers");

    let mut state = StreamState::new(Clock::Fixed(1_700_000_000_000));
    let summary: StreamSummary = pipe_stream(
        response,
        Format::PerplexityWeb,
        Format::OpenAi,
        &mut state,
        |_frame: String| Ok(()),
    )
    .await;

    // The thread id and the finished answer both reach the caller.
    assert_eq!(summary.upstream_thread.as_deref(), Some("thread-abc"));
    assert_eq!(summary.upstream_answer.as_deref(), Some("May 2015."));
    assert_eq!(summary.text, "May 2015.");

    nullrouter_execute::bespoke::remember_thread(
        "perplexity-web",
        &first_body,
        summary.upstream_thread.as_deref().expect("a thread id"),
        summary.upstream_answer.as_deref().expect("an answer"),
    );

    // The second turn carries what a client would actually send back: the question, the answer, and the
    // new question.
    let second_body = json!({
        "model": "pplx-sonnet",
        "messages": [
            { "role": "user", "content": "When did Rust 1.0 ship?" },
            { "role": "assistant", "content": "May 2015." },
            { "role": "user", "content": "And 2.0?" },
        ],
    });
    let continued = prepare(&ExecuteRequest {
        provider: "perplexity-web",
        body: &second_body,
        stream: true,
        credentials: &perplexity_credentials(Some("tok"), None),
    });

    assert_eq!(
        continued.body.pointer("/params/last_backend_uuid"),
        Some(&json!("thread-abc")),
        "the remembered thread must be continued rather than restarted"
    );
    // And the query is just the new question, because perplexity still holds the rest.
    assert_eq!(continued.body.get("query_str"), Some(&json!("And 2.0?")));
}

// ── codex ─────────────────────────────────────────────────────────────────────
//
// Third of the six, and the one whose story text was misleading: `is_executor_supported("codex")` was
// already true, because the registry resolves it to `openai-responses`. So it was never refused with a
// 501 — it dispatched with an unshaped body, which the backend rejects. That is a worse failure than a
// clean refusal, and these tests pin the shaping that fixes it.

fn codex_credentials(account: Option<&str>) -> Credentials {
    let mut credentials = Credentials {
        access_token: Some("chatgpt-access-token".to_owned()),
        connection_id: "conn_codex_1".to_owned(),
        connection_name: "codex".to_owned(),
        ..Credentials::default()
    };
    if let Some(account) = account {
        credentials
            .provider_specific_data
            .insert("chatgptAccountId".to_owned(), json!(account));
    }
    credentials
}

#[test]
fn codex_bodies_are_shaped_for_a_backend_that_refuses_the_raw_request() {
    let body = json!({
        "model": "gpt-5.3-codex-high",
        "input": [
            { "type": "message", "role": "system", "content": [{ "type": "input_text", "text": "rules" }] },
            { "id": "rs_server_generated", "role": "assistant", "content": [{ "type": "output_text", "text": "prior" }] },
        ],
        "temperature": 0.7,
        "max_output_tokens": 8192,
        "store": true,
        "stream": false,
    });

    let prepared = prepare(&ExecuteRequest {
        provider: "codex",
        body: &body,
        stream: true,
        credentials: &codex_credentials(Some("acct-9")),
    });

    // The four requirements that make an unshaped request fail outright.
    assert_eq!(prepared.body.get("store"), Some(&json!(false)));
    assert_eq!(prepared.body.get("stream"), Some(&json!(true)));
    assert_eq!(
        prepared.body.pointer("/input/0/role"),
        Some(&json!("developer"))
    );
    assert!(
        prepared.body.pointer("/input/1/id").is_none(),
        "a server-generated id is unresolvable with store:false: {}",
        prepared.body
    );

    // The effort suffix is read off the model and the model is sent without it.
    assert_eq!(prepared.body.get("model"), Some(&json!("gpt-5.3-codex")));
    assert_eq!(
        prepared.body.pointer("/reasoning/effort"),
        Some(&json!("high"))
    );

    // Unknown fields are gone: these are `routing_unsupported` upstream.
    let object = prepared.body.as_object().expect("an object");
    assert!(!object.contains_key("temperature"));
    assert!(!object.contains_key("max_output_tokens"));
}

#[test]
fn codex_binds_a_request_to_its_own_chatgpt_account() {
    // With more than one Codex connection configured, a request without this header can land on the
    // wrong account and fail as `token_invalid` — which reads as an expired token rather than as a
    // mis-routed request.
    let prepared = prepare(&ExecuteRequest {
        provider: "codex",
        body: &json!({ "model": "gpt-5.3-codex" }),
        stream: true,
        credentials: &codex_credentials(Some("acct-9")),
    });
    assert_eq!(
        prepared
            .headers
            .get("ChatGPT-Account-ID")
            .map(String::as_str),
        Some("acct-9")
    );
    // The registry already supplies the CLI identity; the session header is added per connection.
    assert_eq!(
        prepared.headers.get("originator").map(String::as_str),
        Some("codex_cli_rs")
    );
    assert!(
        prepared
            .headers
            .get("session_id")
            .is_some_and(|session| !session.is_empty())
    );

    // No account configured: the header is omitted rather than sent blank.
    let anonymous = prepare(&ExecuteRequest {
        provider: "codex",
        body: &json!({ "model": "gpt-5.3-codex" }),
        stream: true,
        credentials: &codex_credentials(None),
    });
    assert!(!anonymous.headers.contains_key("ChatGPT-Account-ID"));
}

#[test]
fn codexs_session_and_cache_key_are_stable_across_requests_on_one_connection() {
    // Codex keys its prompt cache by these. A value that changes per request still succeeds, so this
    // would never surface as an error — it would just re-bill the whole conversation every turn.
    let credentials = codex_credentials(Some("acct-9"));
    let request = || {
        prepare(&ExecuteRequest {
            provider: "codex",
            body: &json!({ "model": "gpt-5.3-codex" }),
            stream: true,
            credentials: &credentials,
        })
    };
    let first = request();
    let second = request();

    assert_eq!(
        first.headers.get("session_id"),
        second.headers.get("session_id"),
        "the session must survive a turn or the prompt cache is discarded"
    );
    assert_eq!(
        first.body.get("prompt_cache_key"),
        second.body.get("prompt_cache_key")
    );
}

#[test]
fn a_client_supplied_codex_session_is_used_as_the_cache_key() {
    // A client managing its own conversation ids knows better than this router which requests belong
    // together.
    let prepared = prepare(&ExecuteRequest {
        provider: "codex",
        body: &json!({ "model": "gpt-5.3-codex", "session_id": "client-conversation-3" }),
        stream: true,
        credentials: &codex_credentials(None),
    });
    assert_eq!(
        prepared.body.get("prompt_cache_key"),
        Some(&json!("client-conversation-3"))
    );
    // And `session_id` itself is not a Codex body field.
    assert!(prepared.body.get("session_id").is_none());
}

#[test]
fn another_responses_provider_is_left_unshaped() {
    // The shaping is keyed off the provider id, not the format. Every other `openai-responses` provider
    // must reach its endpoint with the body its client sent.
    let credentials = Credentials {
        api_key: Some("sk-test".to_owned()),
        connection_id: "conn_other".to_owned(),
        ..Credentials::default()
    };
    let body = json!({ "model": "gpt-4.1", "input": "hi", "temperature": 0.5, "store": true });
    let prepared = prepare(&ExecuteRequest {
        provider: "openai-compatible-responses-abc",
        body: &body,
        stream: true,
        credentials: &credentials,
    });

    assert_eq!(
        prepared.body, body,
        "a non-codex provider must not be reshaped"
    );
}

fn antigravity_credentials() -> Credentials {
    Credentials {
        access_token: Some("ya29.ag-token".to_owned()),
        connection_id: "conn_ag".to_owned(),
        connection_name: "antigravity".to_owned(),
        ..Credentials::default()
    }
}

#[test]
fn antigravity_wraps_its_body_in_the_ide_envelope_and_names_its_internal_method() {
    let credentials = antigravity_credentials();
    let body = json!({
        "model": "gemini-3-pro",
        "request": { "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }] },
    });
    let prepared = prepare(&ExecuteRequest {
        provider: "antigravity",
        body: &body,
        stream: true,
        credentials: &credentials,
    });

    assert_eq!(prepared.body.get("userAgent"), Some(&json!("antigravity")));
    assert_eq!(prepared.body.get("requestType"), Some(&json!("agent")));
    // No project on the connection, so one is derived from it rather than left null — Google rejects a
    // request with no project.
    let project = prepared
        .body
        .get("project")
        .and_then(Value::as_str)
        .expect("a project id");
    assert!(!project.is_empty(), "got {project:?}");

    // The request id has the IDE's five-field shape. Antigravity reads it as a conversation identity.
    let request_id = prepared
        .body
        .get("requestId")
        .and_then(Value::as_str)
        .expect("a request id");
    let fields: Vec<&str> = request_id.split('/').collect();
    assert_eq!(fields.len(), 5, "got {request_id}");
    assert_eq!(fields.first(), Some(&"agent"));

    // The method lives under `/v1internal`, which the registry's base URL does not already include.
    let base = build_url("antigravity", &credentials, 0).expect("a base url");
    assert_eq!(base, "https://daily-cloudcode-pa.googleapis.com");
    assert_eq!(
        format!("{base}{}", prepared.url_suffix),
        "https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse"
    );
}

#[test]
fn antigravity_sends_the_ide_user_agent_and_its_oauth_token() {
    let prepared = prepare(&ExecuteRequest {
        provider: "antigravity",
        body: &json!({ "model": "gemini-3-pro", "request": { "contents": [] } }),
        stream: true,
        credentials: &antigravity_credentials(),
    });

    let agent = prepared
        .headers
        .get("User-Agent")
        .map(String::as_str)
        .expect("a user agent");
    // The IDE build string, not the generic reqwest one. Unlike gemini-cli's, no model appears in it.
    assert!(agent.starts_with("antigravity/ide/"), "got {agent}");
    assert!(!agent.contains("gemini-3-pro"), "got {agent}");
    assert_eq!(
        prepared.headers.get("Authorization").map(String::as_str),
        Some("Bearer ya29.ag-token")
    );
    assert_eq!(
        prepared.headers.get("Accept").map(String::as_str),
        Some("text/event-stream")
    );
}

#[test]
fn an_antigravity_image_request_cannot_stream_even_when_asked_to() {
    // The streaming method refuses an image request outright, so the flag is overridden by the model.
    let prepared = prepare(&ExecuteRequest {
        provider: "antigravity",
        body: &json!({
            "model": "gemini-3-pro-image-16x9",
            "request": { "contents": [{ "role": "user", "parts": [{ "text": "a cat" }] }] },
        }),
        stream: true,
        credentials: &antigravity_credentials(),
    });

    assert_eq!(prepared.url_suffix, "/v1internal:generateContent");
    assert_eq!(
        prepared.headers.get("Accept").map(String::as_str),
        Some("application/json")
    );
    assert_eq!(prepared.body.get("requestType"), Some(&json!("image_gen")));
    // The dimension suffix is this router's own convention and is not a model Antigravity knows.
    assert_eq!(
        prepared.body.get("model"),
        Some(&json!("gemini-3-pro-image"))
    );
    assert_eq!(
        prepared
            .body
            .pointer("/request/generationConfig/imageConfig/aspectRatio"),
        Some(&json!("16:9"))
    );
}

#[test]
fn one_antigravity_connection_keeps_one_conversation_identity() {
    // Both uuids in the request id derive from the session, and the session derives from the connection.
    // A fresh identity per request would make Antigravity see each turn as a new agent run.
    let credentials = antigravity_credentials();
    let request = || {
        prepare(&ExecuteRequest {
            provider: "antigravity",
            body: &json!({ "model": "gemini-3-pro", "request": { "contents": [] } }),
            stream: true,
            credentials: &credentials,
        })
    };
    let ids = |prepared: &nullrouter_execute::PreparedRequest| {
        prepared
            .body
            .get("requestId")
            .and_then(Value::as_str)
            .map(|id| id.split('/').map(str::to_owned).collect::<Vec<_>>())
            .expect("a request id")
    };
    let (first, second) = (request(), request());
    let (a, b) = (ids(&first), ids(&second));
    assert_eq!(
        a.get(1),
        b.get(1),
        "the conversation id must survive a turn"
    );
    assert_eq!(a.get(3), b.get(3), "the trajectory id must survive a turn");
    // And the project is stable for the same reason: it appears in Google's logs against this account.
    assert_eq!(first.body.get("project"), second.body.get("project"));
}

#[test]
fn a_gemini_cli_request_is_not_given_the_antigravity_envelope() {
    // Both formats wrap a Gemini payload, and both reach a `cloudcode-pa` host. Only Antigravity adds
    // the IDE fields, and sending them to plain Cloud Code Assist is a rejection.
    let prepared = prepare(&ExecuteRequest {
        provider: "gemini-cli",
        body: &gemini_body(),
        stream: true,
        credentials: &gemini_cli_credentials("proj-1"),
    });
    assert!(prepared.body.get("requestId").is_none());
    assert!(prepared.body.get("userAgent").is_none());
    assert!(prepared.body.get("requestType").is_none());
}

fn cursor_credentials() -> Credentials {
    let mut credentials = Credentials {
        access_token: Some("user_01ABC::cursor-token".to_owned()),
        connection_id: "conn_cursor".to_owned(),
        connection_name: "cursor".to_owned(),
        ..Credentials::default()
    };
    credentials
        .provider_specific_data
        .insert("machineId".to_owned(), json!("m".repeat(64)));
    credentials
}

#[test]
fn cursor_sends_protobuf_bytes_rather_than_json() {
    // Cursor is the only provider here that is not JSON on the wire. A serialised JSON body reaches its
    // Connect-RPC endpoint as a parse failure.
    let prepared = prepare(&ExecuteRequest {
        provider: "cursor",
        body: &json!({
            "model": "claude-4.5-sonnet",
            "messages": [{ "role": "user", "content": "hi" }],
        }),
        stream: true,
        credentials: &cursor_credentials(),
    });

    let bytes = prepared.binary_body.as_deref().expect("a binary body");
    // A Connect frame: an uncompressed flag byte then a big-endian length that matches the remainder.
    assert_eq!(bytes.first(), Some(&0x00));
    let length = bytes
        .get(1..5)
        .and_then(|slice| <[u8; 4]>::try_from(slice).ok())
        .map(u32::from_be_bytes)
        .expect("a length");
    assert_eq!(
        usize::try_from(length).expect("fits"),
        bytes.len().saturating_sub(5),
        "the frame length must match its payload"
    );
    // `payload()` is what dispatch sends, and it must be the bytes rather than the JSON.
    assert_eq!(prepared.payload().expect("a payload"), bytes);
    // The readable body is kept for logging, so a request is still auditable.
    assert_eq!(
        prepared.body.pointer("/messages/0/content"),
        Some(&json!("hi"))
    );
}

#[test]
fn cursor_posts_to_the_rpc_method_named_in_the_registry() {
    let credentials = cursor_credentials();
    let prepared = prepare(&ExecuteRequest {
        provider: "cursor",
        body: &json!({ "model": "m", "messages": [] }),
        stream: true,
        credentials: &credentials,
    });
    let base = build_url("cursor", &credentials, 0).expect("a base url");
    assert_eq!(base, "https://api2.cursor.sh");
    // An RPC path replaces the base's path rather than appending to a version, so the suffix stays empty.
    assert_eq!(prepared.url_suffix, "");
    assert_eq!(
        format!(
            "{base}{}",
            prepared.chat_path.as_deref().expect("a chat path")
        ),
        "https://api2.cursor.sh/aiserver.v1.ChatService/StreamUnifiedChatWithTools"
    );
}

#[test]
fn cursor_carries_its_checksum_and_the_connect_content_type() {
    let prepared = prepare(&ExecuteRequest {
        provider: "cursor",
        body: &json!({ "model": "m", "messages": [] }),
        stream: true,
        credentials: &cursor_credentials(),
    });
    let read = |name: &str| {
        prepared
            .headers
            .get(name)
            .map(String::as_str)
            .unwrap_or_default()
    };

    // The generic JSON content type would be rejected; the protobuf one has to win.
    assert_eq!(read("content-type"), "application/connect+proto");
    assert!(
        !prepared.headers.contains_key("Content-Type"),
        "the generic JSON header must be removed, not merely shadowed: {:?}",
        prepared.headers
    );
    assert_eq!(read("connect-protocol-version"), "1");
    // The prefix is stripped from the token, or every derived value disagrees with it.
    assert_eq!(read("authorization"), "Bearer cursor-token");
    // The checksum ends with the machine id in the clear — it is obfuscation, not a signature.
    let checksum = read("x-cursor-checksum");
    assert!(checksum.ends_with(&"m".repeat(64)), "got {checksum}");
    // Ghost mode defaults on: with it off Cursor may retain the conversation.
    assert_eq!(read("x-ghost-mode"), "true");
    // The timezone is a fingerprint, so it is fixed rather than read from the host clock.
    assert_eq!(read("x-cursor-timezone"), "UTC");
    assert_eq!(read("x-session-id").len(), 36, "a v5 uuid of the token");
}

#[test]
fn a_json_provider_gets_no_binary_body_and_no_rpc_path() {
    let prepared = prepare(&ExecuteRequest {
        provider: "openai",
        body: &json!({ "model": "gpt-5", "messages": [] }),
        stream: true,
        credentials: &Credentials {
            api_key: Some("sk-test".to_owned()),
            ..Credentials::default()
        },
    });
    assert!(prepared.binary_body.is_none());
    assert!(prepared.chat_path.is_none());
    // And its payload is still the serialised JSON.
    let payload = prepared.payload().expect("a payload");
    assert_eq!(
        serde_json::from_slice::<Value>(&payload).expect("json"),
        prepared.body
    );
}

/// A Connect-RPC frame: a flag byte, a big-endian length, then the payload.
///
/// Built by hand rather than with the encoder under test — a fixture produced by the same code it is
/// checking would pass even if both were wrong.
fn connect_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0x00];
    // Saturating rather than fallible: a fixture longer than `u32::MAX` cannot occur here, and an
    // unwrap in a helper outside a `#[test]` fn is denied by the workspace lints.
    let length = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// A protobuf length-delimited field, by hand.
fn len_field(field: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![(field << 3) | 2];
    // Every fixture here is well under 128 bytes, so the length is a single-byte varint. The assertion
    // states that rather than encoding a multi-byte one the fixtures never need.
    let length = u8::try_from(payload.len()).unwrap_or(u8::MAX);
    assert!(
        length < 0x80 && usize::from(length) == payload.len(),
        "the fixture must fit a one-byte varint"
    );
    out.push(length);
    out.extend_from_slice(payload);
    out
}

/// `StreamUnifiedChatResponse.text` wrapped in `StreamUnifiedChatResponseWithTools.response`.
fn cursor_text_frame(text: &str) -> Vec<u8> {
    connect_frame(&len_field(2, &len_field(1, text.as_bytes())))
}

/// `AgentServerMessage.interaction_update.text_delta.text`, the newer endpoint's shape.
///
/// Field 1 at the top, where `ChatService` puts a tool call — which is why the two schemas cannot be
/// told apart by inspection and the decoder has to try one and fall through.
fn cursor_agent_text_frame(text: &str) -> Vec<u8> {
    connect_frame(&len_field(1, &len_field(1, &len_field(1, text.as_bytes()))))
}

/// `AgentServerMessage.exec_request.request_context`, the mid-stream ask for IDE context.
fn cursor_agent_context_ask() -> Vec<u8> {
    connect_frame(&len_field(2, &len_field(10, &[])))
}

#[tokio::test]
async fn cursor_agentservice_frames_also_reach_the_client_as_openai_chunks() {
    use nullrouter_execute::pipe_binary_stream;
    use nullrouter_providers::Format;
    use nullrouter_translate::state::{Clock, StreamState};

    // The regression this pins: `ChatService`'s `response` and `AgentService`'s `exec_request` are both
    // field 2, so a decoder that reads the AgentService schema first turns every ChatService delta into
    // an "unsupported IDE tool" refusal. Both fixtures therefore have to pass through one decoder.
    let mut body = cursor_agent_context_ask();
    body.extend_from_slice(&cursor_agent_text_frame("Cursor "));
    body.extend_from_slice(&cursor_agent_text_frame("agent answered."));

    let upstream =
        MockUpstream::start(vec![MockResponse::bytes("application/connect+proto", body)]).await;
    let response = reqwest::Client::new()
        .post(upstream.url())
        .send()
        .await
        .expect("the stub answers");

    let mut state = StreamState::new(Clock::Fixed(1_700_000_000_000));
    let summary = pipe_binary_stream(
        response,
        Format::Cursor,
        Format::OpenAi,
        "claude-4.5-sonnet",
        &mut state,
        |_frame: String| Ok(()),
    )
    .await;

    // The context ask contributes no text and is not an error: this executor answers it with an empty
    // context on a duplex stream, and cannot answer it at all on a request/response one.
    assert_eq!(summary.text, "Cursor agent answered.");
    assert!(summary.error.is_none(), "{:?}", summary.error);
}

#[tokio::test]
async fn cursor_protobuf_frames_reach_the_client_as_openai_chunks() {
    use nullrouter_execute::pipe_binary_stream;
    use nullrouter_providers::Format;
    use nullrouter_translate::state::{Clock, StreamState};

    // Three frames carrying deltas, then a trailer. Cursor streams deltas rather than whole answers, so
    // each frame's text is new and none of it is a repeat.
    let mut body = Vec::new();
    body.extend_from_slice(&cursor_text_frame("Rust 1.0 "));
    body.extend_from_slice(&cursor_text_frame("shipped in "));
    body.extend_from_slice(&cursor_text_frame("May 2015."));
    // A trailer frame, which must contribute nothing.
    let mut trailer = vec![0x02];
    let grpc_status = b"grpc-status: 0\r\n";
    trailer.extend_from_slice(
        &u32::try_from(grpc_status.len())
            .expect("fits")
            .to_be_bytes(),
    );
    trailer.extend_from_slice(grpc_status);
    body.extend_from_slice(&trailer);

    let upstream =
        MockUpstream::start(vec![MockResponse::bytes("application/connect+proto", body)]).await;
    let response = reqwest::Client::new()
        .post(upstream.url())
        .send()
        .await
        .expect("the stub answers");

    let frames = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let collected = std::sync::Arc::clone(&frames);
    let mut state = StreamState::new(Clock::Fixed(1_700_000_000_000));
    let summary = pipe_binary_stream(
        response,
        Format::Cursor,
        Format::OpenAi,
        "claude-4.5-sonnet",
        &mut state,
        move |frame: String| {
            collected.lock().expect("the lock").push(frame);
            Ok(())
        },
    )
    .await;

    // The three deltas arrive concatenated, which is what a client reassembles.
    assert_eq!(summary.text, "Rust 1.0 shipped in May 2015.");
    assert!(summary.error.is_none());

    let sent = frames.lock().expect("the lock").clone();
    // Three content frames, a terminal chunk, and `[DONE]`.
    assert_eq!(sent.len(), 5, "{sent:?}");
    // The first delta announces the role; the rest do not repeat it.
    assert!(
        sent.first()
            .is_some_and(|frame| frame.contains("\"role\":\"assistant\"")),
        "{sent:?}"
    );
    assert!(
        sent.get(1).is_some_and(|frame| !frame.contains("\"role\"")),
        "{sent:?}"
    );
    assert!(
        sent.get(3)
            .is_some_and(|frame| frame.contains("\"finish_reason\":\"stop\"")),
        "{sent:?}"
    );
    assert_eq!(sent.last().map(String::as_str), Some("data: [DONE]\n\n"));
}

#[tokio::test]
async fn a_cursor_error_frame_before_any_content_is_reported_rather_than_swallowed() {
    use nullrouter_execute::pipe_binary_stream;
    use nullrouter_providers::Format;
    use nullrouter_translate::state::{Clock, StreamState};

    // Cursor reports a rejection as a JSON frame inside a protobuf stream, after the headers have already
    // said 200. A stream that simply stops looks to a client like a truncated answer.
    let error = br#"{"error":{"code":"resource_exhausted","message":"quota","details":[{"debug":{"details":{"title":"You have reached your usage limit"}}}]}}"#;
    let upstream = MockUpstream::start(vec![MockResponse::bytes(
        "application/connect+proto",
        connect_frame(error),
    )])
    .await;
    let response = reqwest::Client::new()
        .post(upstream.url())
        .send()
        .await
        .expect("the stub answers");

    let mut state = StreamState::new(Clock::Fixed(1_700_000_000_000));
    let summary = pipe_binary_stream(
        response,
        Format::Cursor,
        Format::OpenAi,
        "claude-4.5-sonnet",
        &mut state,
        |_frame: String| Ok(()),
    )
    .await;

    // The useful sentence is in `details[0].debug.details.title`, not `error.message`.
    assert_eq!(
        summary.error.as_deref(),
        Some("You have reached your usage limit")
    );
    assert!(summary.text.is_empty());
}

#[tokio::test]
async fn a_cursor_error_after_content_keeps_the_answer_already_streamed() {
    use nullrouter_execute::pipe_binary_stream;
    use nullrouter_providers::Format;
    use nullrouter_translate::state::{Clock, StreamState};

    // The text already delivered is real. Turning a truncated answer into an error would discard it.
    let mut body = cursor_text_frame("Here is what I found.");
    body.extend_from_slice(&connect_frame(br#"{"error":{"message":"quota"}}"#));

    let upstream =
        MockUpstream::start(vec![MockResponse::bytes("application/connect+proto", body)]).await;
    let response = reqwest::Client::new()
        .post(upstream.url())
        .send()
        .await
        .expect("the stub answers");

    let mut state = StreamState::new(Clock::Fixed(1_700_000_000_000));
    let summary = pipe_binary_stream(
        response,
        Format::Cursor,
        Format::OpenAi,
        "claude-4.5-sonnet",
        &mut state,
        |_frame: String| Ok(()),
    )
    .await;

    assert_eq!(summary.text, "Here is what I found.");
    assert!(
        summary.error.is_none(),
        "an error after content must not discard the answer"
    );
}

#[tokio::test]
async fn cursor_frames_split_across_reads_are_reassembled() {
    use nullrouter_execute::pipe_binary_stream;
    use nullrouter_providers::Format;
    use nullrouter_translate::state::{Clock, StreamState};

    // A frame does not arrive whole. Splitting on read boundaries — which is what treating the body as
    // lines would do — would lose every frame that straddles one.
    let body = cursor_text_frame("a frame that spans two reads");
    let split = body.len() / 2;
    let upstream = MockUpstream::start(vec![MockResponse::bytes(
        "application/connect+proto",
        body.clone(),
    )])
    .await;
    let response = reqwest::Client::new()
        .post(upstream.url())
        .send()
        .await
        .expect("the stub answers");
    assert!(split > 5, "the split must fall inside the payload");

    let mut state = StreamState::new(Clock::Fixed(1_700_000_000_000));
    let summary = pipe_binary_stream(
        response,
        Format::Cursor,
        Format::OpenAi,
        "claude-4.5-sonnet",
        &mut state,
        |_frame: String| Ok(()),
    )
    .await;
    assert_eq!(summary.text, "a frame that spans two reads");
}

#[tokio::test]
async fn a_non_streaming_cursor_request_collapses_to_one_json_response() {
    use nullrouter_execute::collapse_stream_to_json;
    use nullrouter_providers::Format;
    use nullrouter_translate::state::{Clock, StreamState};

    // A client that asked for no stream still gets one from Cursor, so the frames are collapsed into a
    // single chat-completion. The frames are protobuf, so the line-oriented path cannot read them.
    let mut body = cursor_text_frame("Rust 1.0 ");
    body.extend_from_slice(&cursor_text_frame("shipped in May 2015."));

    let upstream =
        MockUpstream::start(vec![MockResponse::bytes("application/connect+proto", body)]).await;
    let response = reqwest::Client::new()
        .post(upstream.url())
        .send()
        .await
        .expect("the stub answers");

    let mut state = StreamState::new(Clock::Fixed(1_700_000_000_000));
    let collapsed =
        collapse_stream_to_json(response, Format::Cursor, "claude-4.5-sonnet", &mut state).await;

    assert_eq!(collapsed.get("object"), Some(&json!("chat.completion")));
    assert_eq!(
        collapsed.pointer("/choices/0/message/content"),
        Some(&json!("Rust 1.0 shipped in May 2015."))
    );
    assert_eq!(
        collapsed.pointer("/choices/0/finish_reason"),
        Some(&json!("stop"))
    );
}

#[tokio::test]
async fn cursor_chunks_are_carried_on_to_a_claude_client() {
    use nullrouter_execute::pipe_binary_stream;
    use nullrouter_providers::Format;
    use nullrouter_translate::state::{Clock, StreamState};

    // The decoder produces OpenAI chunks, and the second translation step carries them to whatever the
    // client asked for. Nothing in the cursor decoder knows about Claude.
    let upstream = MockUpstream::start(vec![MockResponse::bytes(
        "application/connect+proto",
        cursor_text_frame("hello"),
    )])
    .await;
    let response = reqwest::Client::new()
        .post(upstream.url())
        .send()
        .await
        .expect("the stub answers");

    let frames = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let collected = std::sync::Arc::clone(&frames);
    let mut state = StreamState::new(Clock::Fixed(1_700_000_000_000));
    let _summary = pipe_binary_stream(
        response,
        Format::Cursor,
        Format::Claude,
        "claude-4.5-sonnet",
        &mut state,
        move |frame: String| {
            collected.lock().expect("the lock").push(frame);
            Ok(())
        },
    )
    .await;

    let sent = frames.lock().expect("the lock").join("");
    // Claude's own event names, not OpenAI's chunk shape.
    assert!(sent.contains("content_block_delta"), "{sent}");
    assert!(sent.contains("hello"), "{sent}");
    assert!(!sent.contains("chat.completion.chunk"), "{sent}");
}

fn kiro_credentials(auth_method: Option<&str>) -> Credentials {
    let mut credentials = Credentials {
        access_token: Some("kiro-token".to_owned()),
        connection_id: "conn_kiro".to_owned(),
        connection_name: "kiro".to_owned(),
        ..Credentials::default()
    };
    if let Some(method) = auth_method {
        credentials
            .provider_specific_data
            .insert("authMethod".to_owned(), json!(method));
    }
    credentials
}

#[test]
fn kiro_sends_a_conversation_state_document_rather_than_a_chat_body() {
    // CodeWhisperer takes a `conversationState`, which shares nothing with a chat-completions body.
    let prepared = prepare(&ExecuteRequest {
        provider: "kiro",
        body: &json!({
            "model": "claude-sonnet-4",
            "messages": [
                { "role": "user", "content": "first" },
                { "role": "assistant", "content": "reply" },
                { "role": "user", "content": "second" },
            ],
        }),
        stream: true,
        credentials: &kiro_credentials(None),
    });

    assert_eq!(
        prepared.body.pointer("/conversationState/chatTriggerType"),
        Some(&json!("MANUAL"))
    );
    // The last user turn is the current message and is not repeated in the history.
    let current = prepared
        .body
        .pointer("/conversationState/currentMessage/userInputMessage/content")
        .and_then(Value::as_str)
        .expect("a current message");
    assert!(current.contains("second"), "{current}");
    let history = prepared
        .body
        .pointer("/conversationState/history")
        .and_then(Value::as_array)
        .expect("a history");
    assert_eq!(history.len(), 2, "{history:?}");
    // The body is JSON, so no binary body is built even though the response is binary.
    assert!(prepared.binary_body.is_none());
    // An OAuth connection keeps the shared default profile ARN, which its token accepts.
    assert!(prepared.body.get("profileArn").is_some());
}

#[test]
fn kiro_asks_for_an_event_stream_and_marks_an_api_key_as_one() {
    let plain = prepare(&ExecuteRequest {
        provider: "kiro",
        body: &json!({ "model": "m", "messages": [] }),
        stream: true,
        credentials: &kiro_credentials(None),
    });
    let read = |prepared: &nullrouter_execute::PreparedRequest, name: &str| {
        prepared
            .headers
            .get(name)
            .map(String::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    // The registry's own Accept header: the response is `vnd.amazon.eventstream`, not SSE.
    assert_eq!(read(&plain, "Accept"), "application/vnd.amazon.eventstream");
    assert_eq!(read(&plain, "Authorization"), "Bearer kiro-token");
    // A Kiro OIDC token carries no TokenType.
    assert!(!plain.headers.contains_key("TokenType"));
    assert!(
        !read(&plain, "Amz-Sdk-Invocation-Id").is_empty(),
        "the AWS SDK invocation id must be present"
    );

    // An API key needs the marker, or CodeWhisperer reads it as an OIDC token and refuses it.
    let mut api_key = kiro_credentials(Some("api_key"));
    api_key.api_key = Some("kiro-api-key".to_owned());
    let keyed = prepare(&ExecuteRequest {
        provider: "kiro",
        body: &json!({ "model": "m", "messages": [] }),
        stream: true,
        credentials: &api_key,
    });
    assert_eq!(read(&keyed, "TokenType"), "API_KEY");
    assert_eq!(read(&keyed, "Authorization"), "Bearer kiro-api-key");
    // And an account-bound credential is never sent the shared default ARN.
    assert!(keyed.body.get("profileArn").is_none());
}

#[test]
fn an_api_key_connection_reaches_the_q_endpoint_first() {
    // `codewhisperer.*` authenticates the key and then rejects the same valid body with a terminal 400, so
    // ordering decides whether the working endpoint is ever tried.
    let mut api_key = kiro_credentials(Some("api_key"));
    api_key.api_key = Some("k".to_owned());
    let first = build_url("kiro", &api_key, 0).expect("a first url");
    assert!(first.contains("://q."), "{first}");

    // A Kiro OIDC connection keeps the registry's order, whose first entry is the kiro.dev gateway.
    let oauth = build_url("kiro", &kiro_credentials(None), 0).expect("a first url");
    assert!(oauth.contains("kiro.dev"), "{oauth}");
}

#[test]
fn an_aws_credential_from_another_region_is_sent_to_that_region() {
    // An AWS token is only valid where it was minted, and the registry's URLs are hardcoded to us-east-1.
    let mut frankfurt = kiro_credentials(Some("idc"));
    frankfurt
        .provider_specific_data
        .insert("region".to_owned(), json!("eu-central-1"));
    let url = build_url("kiro", &frankfurt, 0).expect("a url");
    assert!(url.contains("eu-central-1"), "{url}");
    assert!(url.ends_with("/generateAssistantResponse"), "{url}");
}

/// An AWS event-stream frame, built by hand so the fixture is independent of the parser.
fn kiro_frame(event: &str, payload: Option<&str>) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFF_u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _bit in 0..8 {
                crc = (crc >> 1) ^ if crc & 1 == 1 { 0xEDB8_8320 } else { 0 };
            }
        }
        crc ^ 0xFFFF_FFFF
    }
    fn text_header(block: &mut Vec<u8>, name: &str, value: &str) {
        block.push(u8::try_from(name.len()).unwrap_or(0));
        block.extend_from_slice(name.as_bytes());
        block.push(7);
        block.extend_from_slice(&u16::try_from(value.len()).unwrap_or(0).to_be_bytes());
        block.extend_from_slice(value.as_bytes());
    }

    let mut headers = Vec::new();
    text_header(&mut headers, ":event-type", event);
    text_header(&mut headers, ":message-type", "event");
    let body = payload.unwrap_or("");
    let total = 16 + headers.len() + body.len();

    let mut frame = Vec::with_capacity(total);
    frame.extend_from_slice(&u32::try_from(total).unwrap_or(0).to_be_bytes());
    frame.extend_from_slice(&u32::try_from(headers.len()).unwrap_or(0).to_be_bytes());
    let prelude = crc32(frame.get(..8).unwrap_or_default());
    frame.extend_from_slice(&prelude.to_be_bytes());
    frame.extend_from_slice(&headers);
    frame.extend_from_slice(body.as_bytes());
    let message = crc32(&frame);
    frame.extend_from_slice(&message.to_be_bytes());
    frame
}

#[tokio::test]
async fn kiro_event_stream_frames_reach_the_client_as_openai_chunks() {
    use nullrouter_execute::pipe_binary_stream;
    use nullrouter_providers::Format;
    use nullrouter_translate::state::{Clock, StreamState};

    let mut body = kiro_frame("assistantResponseEvent", Some(r#"{"content":"Rust 1.0 "}"#));
    body.extend_from_slice(&kiro_frame(
        "assistantResponseEvent",
        Some(r#"{"content":"shipped in May 2015."}"#),
    ));
    // An accounting event, which must contribute nothing.
    body.extend_from_slice(&kiro_frame("meteringEvent", Some(r#"{"credits":1}"#)));
    body.extend_from_slice(&kiro_frame("messageStopEvent", None));

    let upstream = MockUpstream::start(vec![MockResponse::bytes(
        "application/vnd.amazon.eventstream",
        body,
    )])
    .await;
    let response = reqwest::Client::new()
        .post(upstream.url())
        .send()
        .await
        .expect("the stub answers");

    let frames = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let collected = std::sync::Arc::clone(&frames);
    let mut state = StreamState::new(Clock::Fixed(1_700_000_000_000));
    let summary = pipe_binary_stream(
        response,
        Format::Kiro,
        Format::OpenAi,
        "claude-sonnet-4",
        &mut state,
        move |frame: String| {
            collected.lock().expect("the lock").push(frame);
            Ok(())
        },
    )
    .await;

    assert_eq!(summary.text, "Rust 1.0 shipped in May 2015.");
    assert!(summary.error.is_none());

    let sent = frames.lock().expect("the lock").clone();
    // Two content frames, a terminal chunk, and `[DONE]`. The metering event added nothing.
    assert_eq!(sent.len(), 4, "{sent:?}");
    assert!(
        sent.first()
            .is_some_and(|frame| frame.contains("\"role\":\"assistant\"")),
        "{sent:?}"
    );
    // Kiro sends no finish reason of its own, so it is derived — `stop`, with no tool used.
    assert!(
        sent.get(2)
            .is_some_and(|frame| frame.contains("\"finish_reason\":\"stop\"")),
        "{sent:?}"
    );
    assert_eq!(sent.last().map(String::as_str), Some("data: [DONE]\n\n"));
}

#[tokio::test]
async fn kiro_tool_fragments_are_relayed_for_the_client_to_reassemble() {
    use nullrouter_execute::pipe_binary_stream;
    use nullrouter_providers::Format;
    use nullrouter_translate::state::{Clock, StreamState};

    // The tool input is one JSON document split across events. A fragment cannot be parsed alone.
    let mut body = kiro_frame(
        "toolUseEvent",
        Some(r#"{"toolUseId":"t1","name":"read_file","input":"{\"path\":","stop":false}"#),
    );
    body.extend_from_slice(&kiro_frame(
        "toolUseEvent",
        Some(r#"{"toolUseId":"t1","name":"read_file","input":"\"a.txt\"}","stop":true}"#),
    ));
    body.extend_from_slice(&kiro_frame("messageStopEvent", None));

    let upstream = MockUpstream::start(vec![MockResponse::bytes(
        "application/vnd.amazon.eventstream",
        body,
    )])
    .await;
    let response = reqwest::Client::new()
        .post(upstream.url())
        .send()
        .await
        .expect("the stub answers");

    let frames = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let collected = std::sync::Arc::clone(&frames);
    let mut state = StreamState::new(Clock::Fixed(1_700_000_000_000));
    let _summary = pipe_binary_stream(
        response,
        Format::Kiro,
        Format::OpenAi,
        "claude-sonnet-4",
        &mut state,
        move |frame: String| {
            collected.lock().expect("the lock").push(frame);
            Ok(())
        },
    )
    .await;

    let sent = frames.lock().expect("the lock").join("");
    // Both fragments are relayed against index 0, so a client can concatenate them into one document.
    assert!(sent.contains(r#"\"path\":"#), "{sent}");
    assert!(sent.contains(r#"\"a.txt\"}"#), "{sent}");
    // A tool was used, so the derived finish reason says so.
    assert!(sent.contains("\"finish_reason\":\"tool_calls\""), "{sent}");
}

#[tokio::test]
async fn a_kiro_exception_frame_is_reported_rather_than_ending_in_silence() {
    use nullrouter_execute::pipe_binary_stream;
    use nullrouter_providers::Format;
    use nullrouter_translate::state::{Clock, StreamState};

    // Kiro reports a throttle in-band, after the headers already said 200 — the one failure that cannot
    // become a status code.
    fn exception_frame() -> Vec<u8> {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = 0xFFFF_FFFF_u32;
            for byte in bytes {
                crc ^= u32::from(*byte);
                for _bit in 0..8 {
                    crc = (crc >> 1) ^ if crc & 1 == 1 { 0xEDB8_8320 } else { 0 };
                }
            }
            crc ^ 0xFFFF_FFFF
        }
        let mut headers = Vec::new();
        for (name, value) in [
            (":message-type", "exception"),
            (":exception-type", "ThrottlingException"),
        ] {
            headers.push(u8::try_from(name.len()).unwrap_or(0));
            headers.extend_from_slice(name.as_bytes());
            headers.push(7);
            headers.extend_from_slice(&u16::try_from(value.len()).unwrap_or(0).to_be_bytes());
            headers.extend_from_slice(value.as_bytes());
        }
        let body = r#"{"message":"Too many requests"}"#;
        let total = 16 + headers.len() + body.len();
        let mut frame = Vec::new();
        frame.extend_from_slice(&u32::try_from(total).unwrap_or(0).to_be_bytes());
        frame.extend_from_slice(&u32::try_from(headers.len()).unwrap_or(0).to_be_bytes());
        frame.extend_from_slice(&crc32(frame.get(..8).unwrap_or_default()).to_be_bytes());
        frame.extend_from_slice(&headers);
        frame.extend_from_slice(body.as_bytes());
        let message = crc32(&frame);
        frame.extend_from_slice(&message.to_be_bytes());
        frame
    }

    let upstream = MockUpstream::start(vec![MockResponse::bytes(
        "application/vnd.amazon.eventstream",
        exception_frame(),
    )])
    .await;
    let response = reqwest::Client::new()
        .post(upstream.url())
        .send()
        .await
        .expect("the stub answers");

    let mut state = StreamState::new(Clock::Fixed(1_700_000_000_000));
    let summary = pipe_binary_stream(
        response,
        Format::Kiro,
        Format::OpenAi,
        "claude-sonnet-4",
        &mut state,
        |_frame: String| Ok(()),
    )
    .await;

    assert_eq!(
        summary.error.as_deref(),
        Some("ThrottlingException: Too many requests")
    );
    assert!(summary.text.is_empty());
}

#[tokio::test]
async fn a_corrupt_kiro_frame_stops_the_stream_rather_than_resuming_at_a_guess() {
    use nullrouter_execute::pipe_binary_stream;
    use nullrouter_providers::Format;
    use nullrouter_translate::state::{Clock, StreamState};

    // A CRC failure means the length fields cannot be trusted, so there is no safe offset to resume from.
    // The text already delivered is kept; the stream ends there.
    let mut body = kiro_frame("assistantResponseEvent", Some(r#"{"content":"partial"}"#));
    let mut corrupt = kiro_frame("assistantResponseEvent", Some(r#"{"content":"lost"}"#));
    let last = corrupt.len().saturating_sub(6);
    if let Some(byte) = corrupt.get_mut(last) {
        *byte ^= 0xFF;
    }
    body.extend_from_slice(&corrupt);

    let upstream = MockUpstream::start(vec![MockResponse::bytes(
        "application/vnd.amazon.eventstream",
        body,
    )])
    .await;
    let response = reqwest::Client::new()
        .post(upstream.url())
        .send()
        .await
        .expect("the stub answers");

    let mut state = StreamState::new(Clock::Fixed(1_700_000_000_000));
    let summary = pipe_binary_stream(
        response,
        Format::Kiro,
        Format::OpenAi,
        "claude-sonnet-4",
        &mut state,
        |_frame: String| Ok(()),
    )
    .await;

    assert_eq!(summary.text, "partial", "the delivered text must survive");
    // Content had already been produced, so the corruption truncates rather than replaces the answer.
    assert!(summary.error.is_none());
}

#[test]
fn kiro_advances_endpoints_on_an_auth_failure_but_not_on_a_bad_body() {
    use nullrouter_execute::bespoke::advances_on_status;

    // Kiro's three endpoints are alternate auth surfaces, not replicas: a 401/403/404 from one says the
    // credential belongs to another, which the next may accept.
    for status in [401_u16, 403, 404] {
        assert!(
            advances_on_status("kiro", status),
            "{status} should advance to the next kiro endpoint"
        );
    }
    // A 400 is about the body. Advancing on it would send the same rejected body to every surface and burn
    // the one that might have worked — which is exactly the api-key trap that made endpoint order matter.
    assert!(!advances_on_status("kiro", 400));
    // And no other provider advances on these: its endpoints are replicas, so a 403 is a real failure.
    for status in [401_u16, 403, 404, 400] {
        assert!(
            !advances_on_status("openai", status),
            "openai must not advance on {status}"
        );
    }
}
