//! Reaching a compatible provider by the prefix its owner chose.
//!
//! A compatible connection is addressed as `myllm/some-model`, where `myllm` is a
//! user-defined prefix and the connection's real provider id is
//! `openai-compatible-chat-<uuid>`. Upstream resolves the prefix in `getModelInfo`;
//! without it a migrated install is reachable only by that uuid, so every client config
//! that worked against 9Router breaks after an import.
//!
//! Two things need checking at this level and only this level:
//!
//! * the resolved provider id reaches the credential lookup, not the prefix;
//! * a registry provider does **not** consult the connection store. `routing-context` is
//!   an HTTP hop to the state service and `provider/model` is the common path, so a
//!   resolution that ran unconditionally would add a round trip to every request. That is
//!   invisible in a functional test that only asserts the reply, so it is asserted here
//!   against the recorded request list.

#![allow(
    clippy::future_not_send,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test assertions read clearer with direct expect than with error plumbing"
)]

use std::sync::{Arc, Mutex};

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_runtime::{Runtime, app_config, configure};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// One recorded request.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    body: String,
}

/// A loopback server that records every request and answers per path suffix.
#[derive(Debug)]
struct Recorder {
    addr: std::net::SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Recorder {
    async fn start(routes: Vec<(&'static str, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let routes = routes.clone();
                let sink = Arc::clone(&recorded);
                tokio::spawn(async move {
                    let _ = serve(stream, routes, sink).await;
                });
            }
        });
        Self { addr, seen }
    }

    fn addr_string(&self) -> String {
        self.addr.to_string()
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default()
    }

    fn body_for(&self, suffix: &str) -> Option<String> {
        self.seen()
            .into_iter()
            .find(|entry| entry.path.contains(suffix))
            .map(|entry| entry.body)
    }

    fn saw(&self, suffix: &str) -> bool {
        self.seen().iter().any(|entry| entry.path.contains(suffix))
    }

    fn count(&self, suffix: &str) -> usize {
        self.seen()
            .iter()
            .filter(|entry| entry.path.contains(suffix))
            .count()
    }
}

async fn serve(
    mut stream: TcpStream,
    routes: Vec<(&'static str, String)>,
    seen: Arc<Mutex<Vec<Seen>>>,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = vec![0_u8; 16_384];

    let (head_end, content_length) = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break (buffer.len(), 0);
        }
        buffer.extend_from_slice(chunk.get(..read).unwrap_or_default());
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(buffer.get(..position).unwrap_or_default());
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            break (position + 4, length);
        }
    };
    while buffer.len() < head_end + content_length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(chunk.get(..read).unwrap_or_default());
    }

    let raw = String::from_utf8_lossy(&buffer).into_owned();
    let path = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();
    let body = raw.get(head_end..).unwrap_or_default().to_owned();

    if let Ok(mut sink) = seen.lock() {
        sink.push(Seen {
            path: path.clone(),
            body,
        });
    }

    let reply = routes
        .iter()
        .find(|(suffix, _)| path.contains(suffix))
        .map_or_else(
            || r#"{"error":"unrouted"}"#.to_owned(),
            |(_, body)| body.clone(),
        );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len(),
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

/// The provider id a compatible connection really carries.
const NODE_ID: &str = "openai-compatible-chat-11111111-2222-3333-4444-555555555555";

fn openai_reply() -> String {
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "created": 1,
        "model": "some-model",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "done" },
            "finish_reason": "stop",
        }],
        "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 },
    })
    .to_string()
}

/// A state service whose one connection is a compatible node with prefix `myllm`.
async fn state_service(provider_base: &str) -> Recorder {
    let credentials = json!({
        "status": "selected",
        "credentials": {
            "connectionId": "conn_prefix",
            "connectionName": "myllm",
            "apiKey": "sk-prefix",
            "providerSpecificData": { "baseUrl": provider_base, "prefix": "myllm" },
        },
    });
    let routing = json!({
        "combos": [],
        "connections": [{
            "provider": NODE_ID,
            "prefix": "myllm",
            "enabledModels": [],
        }],
        "settings": {},
    });
    Recorder::start(vec![
        ("/internal/v1/credentials/select", credentials.to_string()),
        ("/internal/v1/credentials/clear-error", "{}".to_owned()),
        ("/internal/v1/credentials/unavailable", "{}".to_owned()),
        ("/internal/v1/usage", r#"{"ok":true}"#.to_owned()),
        (
            "/internal/v1/keys/gate",
            r#"{"requireApiKey":false,"valid":false,"active":false}"#.to_owned(),
        ),
        ("/internal/v1/routing-context", routing.to_string()),
    ])
    .await
}

/// A state service that records the lookup and then declines it.
///
/// For the reserved-prefix tests the claim is entirely about *which provider is asked
/// for*, and those use a real registry provider — whose URL comes from the registry, not
/// from the connection, so a fake provider cannot receive the call. Letting the dispatch
/// proceed sends a request to the real `api.openai.com` (it did, on the first run of this
/// file, and came back with a genuine OpenAI 401). Declining the credential lookup keeps
/// the test hermetic: resolution has already happened and been recorded by then, and
/// nothing leaves the machine.
async fn state_service_declining(prefix: &str) -> Recorder {
    let routing = json!({
        "combos": [],
        "connections": [{
            "provider": NODE_ID,
            "prefix": prefix,
            "enabledModels": [],
        }],
        "settings": {},
    });
    Recorder::start(vec![
        (
            "/internal/v1/credentials/select",
            json!({ "status": "no_credentials" }).to_string(),
        ),
        ("/internal/v1/credentials/clear-error", "{}".to_owned()),
        ("/internal/v1/credentials/unavailable", "{}".to_owned()),
        ("/internal/v1/usage", r#"{"ok":true}"#.to_owned()),
        (
            "/internal/v1/keys/gate",
            r#"{"requireApiKey":false,"valid":false,"active":false}"#.to_owned(),
        ),
        ("/internal/v1/routing-context", routing.to_string()),
    ])
    .await
}

async fn post(state_addr: &str, uri: &str, body: &str) -> TestResult<(StatusCode, String)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(state_addr)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::POST)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok((status, body))
}

#[actix_web::test]
async fn a_user_defined_prefix_resolves_to_the_connections_provider_id() -> TestResult {
    let provider = Recorder::start(vec![("/chat/completions", openai_reply())]).await;
    let state = state_service(&provider.base_url()).await;

    let (status, body) = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"myllm/some-model","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "body: {body}");

    // The credential lookup must ask for the node id. Asking for `myllm` is the bug:
    // no connection is stored under that name, so it fails as "no active credentials".
    let selected = state
        .body_for("/internal/v1/credentials/select")
        .expect("credential selection was requested");
    let selected: Value = serde_json::from_str(&selected)?;
    assert_eq!(
        selected["provider"], NODE_ID,
        "credential lookup used the prefix instead of the resolved provider id: {selected}"
    );

    // And the request really reached the provider.
    assert!(
        provider.saw("/chat/completions"),
        "provider was never called: {:?}",
        provider.seen()
    );
    Ok(())
}

#[actix_web::test]
async fn the_model_name_keeps_its_own_prefix_off_the_wire() -> TestResult {
    let provider = Recorder::start(vec![("/chat/completions", openai_reply())]).await;
    let state = state_service(&provider.base_url()).await;

    let (status, _) = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"myllm/some-model","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    // The provider never hears the routing prefix: it is addressing information for the
    // router, and a provider asked for `myllm/some-model` answers 404.
    let sent = provider
        .body_for("/chat/completions")
        .expect("provider was called");
    let sent: Value = serde_json::from_str(&sent)?;
    assert_eq!(
        sent["model"], "some-model",
        "prefix leaked upstream: {sent}"
    );
    Ok(())
}

#[actix_web::test]
async fn a_registry_provider_adds_no_routing_context_fetch() -> TestResult {
    // `routing-context` is a round trip to the state service and `provider/model` is the
    // common path, so prefix resolution must not add one.
    //
    // This used to assert `baseline + 1`: the request path fetched routing context three
    // separate times, and prefix resolution was the third. The note here said pinning the
    // test to that count would make it fail once the duplication was fixed, and that is
    // what happened -- `StateClient` now caches the context for 250ms, so the three reads
    // are one fetch and prefix resolution costs nothing extra.
    //
    // So the assertion is now equality, which is the stronger form of the same property.
    // It still fails if resolution starts making an uncached call of its own.
    let registry_state = state_service_declining("myllm").await;
    post(
        &registry_state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"openai/gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;
    let baseline = registry_state.count("/internal/v1/routing-context");

    let prefix_state = state_service_declining("myllm").await;
    post(
        &prefix_state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"myllm/some-model","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;
    let with_prefix = prefix_state.count("/internal/v1/routing-context");

    assert_eq!(
        with_prefix, baseline,
        "prefix resolution should add no routing-context fetch \
         (registry provider: {baseline}, user prefix: {with_prefix})"
    );
    // And the cache must not have removed the fetch altogether: routing context is still read,
    // once, or this test would pass against a router that never consulted it.
    assert!(
        baseline >= 1,
        "routing context should still be fetched at least once, got {baseline}"
    );

    let selected = registry_state
        .body_for("/internal/v1/credentials/select")
        .expect("credential selection was requested");
    let selected: Value = serde_json::from_str(&selected)?;
    assert_eq!(selected["provider"], "openai");
    Ok(())
}

#[actix_web::test]
async fn a_prefix_cannot_shadow_a_registry_provider() -> TestResult {
    // A connection claiming the prefix `openai`. Upstream guards this with
    // RESERVED_PROVIDER_PREFIXES so a user cannot capture a built-in id — otherwise adding
    // a compatible node called `openai` silently redirects every `openai/*` request.
    let state = state_service_declining("openai").await;

    post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"openai/gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;

    let selected = state
        .body_for("/internal/v1/credentials/select")
        .expect("credential selection was requested");
    let selected: Value = serde_json::from_str(&selected)?;
    assert_eq!(
        selected["provider"], "openai",
        "a user prefix captured a built-in provider id: {selected}"
    );
    Ok(())
}

#[actix_web::test]
async fn an_unknown_prefix_with_no_matching_connection_is_left_alone() -> TestResult {
    let provider = Recorder::start(vec![("/chat/completions", openai_reply())]).await;
    let state = state_service(&provider.base_url()).await;

    // `nosuchnode` is neither a registry provider nor a configured prefix. It must stay
    // as-is and fail as an unknown provider, rather than silently landing on whichever
    // connection happens to be first.
    let (_, body) = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"nosuchnode/some-model","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;

    if let Some(selected) = state.body_for("/internal/v1/credentials/select") {
        let selected: Value = serde_json::from_str(&selected)?;
        assert_eq!(
            selected["provider"], "nosuchnode",
            "an unmatched prefix was rewritten to another connection: {selected}"
        );
    }
    assert!(
        !provider.saw("/chat/completions"),
        "an unmatched prefix reached a provider: {body}"
    );
    Ok(())
}
