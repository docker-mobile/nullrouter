//! Asking a user-added provider which models it serves.
//!
//! A compatible connection points at a host its owner chose, so the registry cannot know
//! its models. Before probing, `/v1/models` reported **nothing** for such a connection:
//! `models_for_key` finds no row for the node id, and an owner who has not typed a model
//! list by hand got an empty picker in every client that reads the route. This was found
//! against a real imported config, where the route returned zero rows for two working
//! connections.
//!
//! What only this level can check:
//!
//! * a probe's ids reach `/v1/models`, carrying the connection's own prefix;
//! * a *failed* probe leaves the configured list alone instead of emptying it — the
//!   asymmetry the module is built around;
//! * the result is cached, so a route editors poll does not probe per request;
//! * a configured list is never overridden by a probe.

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
    http::{Method, StatusCode},
    test, web,
};
use nullrouter_runtime::{Runtime, app_config, configure};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The provider id a compatible connection carries, and the prefix its owner chose.
const NODE_ID: &str = "openai-compatible-chat-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const PREFIX: &str = "myllm";

#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
}

/// A loopback server that records requests and answers per path suffix.
#[derive(Debug)]
struct Recorder {
    addr: std::net::SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Recorder {
    async fn start(routes: Vec<(&'static str, (u16, String))>) -> Self {
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

    fn count(&self, suffix: &str) -> usize {
        self.seen
            .lock()
            .map(|seen| seen.iter().filter(|e| e.path.contains(suffix)).count())
            .unwrap_or_default()
    }

    fn saw_header(&self, suffix: &str, name: &str, value: &str) -> bool {
        self.seen
            .lock()
            .map(|seen| {
                seen.iter()
                    .filter(|entry| entry.path.contains(suffix))
                    .any(|entry| {
                        entry
                            .headers
                            .iter()
                            .any(|(key, got)| key.eq_ignore_ascii_case(name) && got.trim() == value)
                    })
            })
            .unwrap_or_default()
    }
}

async fn serve(
    mut stream: TcpStream,
    routes: Vec<(&'static str, (u16, String))>,
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
    let headers: Vec<(String, String)> = raw
        .lines()
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_owned(), value.trim().to_owned()))
        })
        .collect();
    if let Ok(mut sink) = seen.lock() {
        sink.push(Seen {
            path: path.clone(),
            headers,
        });
    }

    let (status, reply) = routes
        .iter()
        .find(|(suffix, _)| path.contains(suffix))
        .map_or_else(
            || (404, r#"{"error":"unrouted"}"#.to_owned()),
            |(_, reply)| reply.clone(),
        );
    let response = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len(),
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

/// A state service reporting one compatible connection, with `enabled_models` as given.
async fn state_service(provider_base: &str, enabled_models: Vec<&str>) -> Recorder {
    let routing = json!({
        "combos": [],
        "connections": [{
            "provider": NODE_ID,
            "prefix": PREFIX,
            "enabledModels": enabled_models,
        }],
        "settings": {},
    });
    let targets = json!({
        "targets": [{
            "connectionId": "conn_probe",
            "provider": NODE_ID,
            "credentials": {
                "connectionId": "conn_probe",
                "connectionName": "myllm",
                "apiKey": "sk-probe",
                "providerSpecificData": { "baseUrl": provider_base, "prefix": PREFIX },
            },
        }],
    });
    Recorder::start(vec![
        (
            "/internal/v1/keys/gate",
            (
                200,
                r#"{"requireApiKey":false,"valid":false,"active":false}"#.to_owned(),
            ),
        ),
        ("/internal/v1/routing-context", (200, routing.to_string())),
        ("/internal/v1/probe-targets", (200, targets.to_string())),
    ])
    .await
}

async fn models(state_addr: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(state_addr)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::GET)
        .uri("/v1/models")
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok((status, serde_json::from_str(&body)?))
}

/// One app instance, so the probe cache is shared across calls the way it is in a
/// running router. `models()` builds a fresh Runtime each time and cannot show caching.
async fn models_twice(state_addr: &str) -> TestResult<(Value, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(state_addr)))
            .configure(configure),
    )
    .await;
    let mut bodies = Vec::new();
    for _ in 0..2 {
        let req = test::TestRequest::default()
            .method(Method::GET)
            .uri("/v1/models")
            .to_request();
        let res = test::call_service(&app, req).await;
        let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
        bodies.push(serde_json::from_str::<Value>(&body)?);
    }
    Ok((bodies[0].clone(), bodies[1].clone()))
}

fn ids(body: &Value) -> Vec<String> {
    body["data"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[actix_web::test]
async fn a_probe_populates_models_under_the_connections_own_prefix() -> TestResult {
    let provider = Recorder::start(vec![(
        "/models",
        (
            200,
            json!({ "object": "list", "data": [{"id": "some-model"}, {"id": "other-model"}] })
                .to_string(),
        ),
    )])
    .await;
    let state = state_service(&provider.base_url(), vec![]).await;

    let (status, body) = models(&state.addr_string()).await?;
    assert_eq!(status, StatusCode::OK);

    let listed = ids(&body);
    assert!(
        listed.contains(&format!("{PREFIX}/some-model")),
        "probed models missing from /v1/models: {listed:?}"
    );
    assert!(
        listed.contains(&format!("{PREFIX}/other-model")),
        "second probed model missing: {listed:?}"
    );
    // The uuid must not appear: clients would have to type it, which is the bug this and
    // the prefix-routing fix exist to remove.
    assert!(
        !listed.iter().any(|id| id.contains(NODE_ID)),
        "the node uuid leaked into the model list: {listed:?}"
    );
    Ok(())
}

#[actix_web::test]
async fn a_failed_probe_leaves_the_route_working() -> TestResult {
    // 401 is the common failure: the owner typed the wrong key.
    let provider = Recorder::start(vec![(
        "/models",
        (401, r#"{"error":{"message":"bad key"}}"#.to_owned()),
    )])
    .await;
    let state = state_service(&provider.base_url(), vec![]).await;

    let (status, body) = models(&state.addr_string()).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "a rejected probe must not fail the route: {body}"
    );
    assert!(
        body["data"].is_array(),
        "the route should still answer a list: {body}"
    );
    Ok(())
}

#[actix_web::test]
async fn a_configured_model_list_is_not_replaced_by_a_probe() -> TestResult {
    // The provider would answer with different models. The owner's own list wins, and no
    // probe should even be attempted for this connection.
    let provider = Recorder::start(vec![(
        "/models",
        (
            200,
            json!({ "data": [{"id": "provider-said-this"}] }).to_string(),
        ),
    )])
    .await;
    let state = state_service(&provider.base_url(), vec!["owner-chose-this"]).await;

    let (_, body) = models(&state.addr_string()).await?;
    let listed = ids(&body);

    assert!(
        listed.contains(&format!("{PREFIX}/owner-chose-this")),
        "the configured model is missing: {listed:?}"
    );
    assert!(
        !listed.iter().any(|id| id.contains("provider-said-this")),
        "a probe overrode the owner's configured list: {listed:?}"
    );
    assert_eq!(
        provider.count("/models"),
        0,
        "a connection with a configured list should not be probed at all"
    );
    Ok(())
}

#[actix_web::test]
async fn a_key_pool_probes_once_and_follows_the_first_connection() -> TestResult {
    // A compatible node can hold several connections as a key pool. Only the first
    // contributes rows to the model list, so only the first decides whether to probe, and
    // one probe answers for the whole pool. Probing per connection would make N provider
    // calls to ask one host the same question.
    let provider = Recorder::start(vec![(
        "/models",
        (200, json!({ "data": [{"id": "some-model"}] }).to_string()),
    )])
    .await;
    let base = provider.base_url();

    let routing = json!({
        "combos": [],
        "connections": [
            { "provider": NODE_ID, "prefix": PREFIX, "enabledModels": [] },
            { "provider": NODE_ID, "prefix": PREFIX, "enabledModels": [] },
            { "provider": NODE_ID, "prefix": PREFIX, "enabledModels": [] },
        ],
        "settings": {},
    });
    let credentials = |id: &str| {
        json!({
            "connectionId": id,
            "connectionName": "myllm",
            "apiKey": "sk-probe",
            "providerSpecificData": { "baseUrl": base, "prefix": PREFIX },
        })
    };
    let targets = json!({
        "targets": [
            { "connectionId": "conn_a", "provider": NODE_ID, "credentials": credentials("conn_a") },
            { "connectionId": "conn_b", "provider": NODE_ID, "credentials": credentials("conn_b") },
            { "connectionId": "conn_c", "provider": NODE_ID, "credentials": credentials("conn_c") },
        ],
    });
    let state = Recorder::start(vec![
        (
            "/internal/v1/keys/gate",
            (
                200,
                r#"{"requireApiKey":false,"valid":false,"active":false}"#.to_owned(),
            ),
        ),
        ("/internal/v1/routing-context", (200, routing.to_string())),
        ("/internal/v1/probe-targets", (200, targets.to_string())),
    ])
    .await;

    let (_, body) = models(&state.addr_string()).await?;
    assert!(
        ids(&body).contains(&format!("{PREFIX}/some-model")),
        "the pool's models are missing: {:?}",
        ids(&body)
    );
    assert_eq!(
        provider.count("/models"),
        1,
        "expected one probe for a three-connection pool, got {}",
        provider.count("/models")
    );
    Ok(())
}

#[actix_web::test]
async fn a_pool_whose_first_connection_has_a_list_is_not_probed() -> TestResult {
    // The first connection carries the owner's list, so the route shows that and no probe
    // is warranted — even though later connections in the pool have none.
    let provider = Recorder::start(vec![(
        "/models",
        (
            200,
            json!({ "data": [{"id": "provider-said-this"}] }).to_string(),
        ),
    )])
    .await;
    let base = provider.base_url();

    let routing = json!({
        "combos": [],
        "connections": [
            { "provider": NODE_ID, "prefix": PREFIX, "enabledModels": ["owner-chose-this"] },
            { "provider": NODE_ID, "prefix": PREFIX, "enabledModels": [] },
        ],
        "settings": {},
    });
    let targets = json!({
        "targets": [{
            "connectionId": "conn_a",
            "provider": NODE_ID,
            "credentials": {
                "connectionId": "conn_a",
                "connectionName": "myllm",
                "apiKey": "sk-probe",
                "providerSpecificData": { "baseUrl": base, "prefix": PREFIX },
            },
        }],
    });
    let state = Recorder::start(vec![
        (
            "/internal/v1/keys/gate",
            (
                200,
                r#"{"requireApiKey":false,"valid":false,"active":false}"#.to_owned(),
            ),
        ),
        ("/internal/v1/routing-context", (200, routing.to_string())),
        ("/internal/v1/probe-targets", (200, targets.to_string())),
    ])
    .await;

    let (_, body) = models(&state.addr_string()).await?;
    assert!(
        ids(&body).contains(&format!("{PREFIX}/owner-chose-this")),
        "{:?}",
        ids(&body)
    );
    assert_eq!(
        provider.count("/models"),
        0,
        "probed on behalf of a connection whose models the route ignores"
    );
    Ok(())
}

#[actix_web::test]
async fn a_probe_result_is_cached_across_requests() -> TestResult {
    let provider = Recorder::start(vec![(
        "/models",
        (200, json!({ "data": [{"id": "some-model"}] }).to_string()),
    )])
    .await;
    let state = state_service(&provider.base_url(), vec![]).await;

    let (first, second) = models_twice(&state.addr_string()).await?;

    // Both answers carry the models; only the first cost a provider call. `/v1/models` is
    // polled by editors on startup and sometimes per completion, so one probe per request
    // would put a provider round trip on a route expected to be cheap.
    assert!(ids(&first).contains(&format!("{PREFIX}/some-model")));
    assert!(
        ids(&second).contains(&format!("{PREFIX}/some-model")),
        "the cached answer lost the models: {:?}",
        ids(&second)
    );
    assert_eq!(
        provider.count("/models"),
        1,
        "expected one probe for two requests, got {}",
        provider.count("/models")
    );
    Ok(())
}

#[actix_web::test]
async fn a_probe_marked_request_is_answered_without_probing() -> TestResult {
    // The loop guard. A compatible node's base URL can point at another router, or at
    // this one; if both probe on /v1/models they probe each other on every call, forever.
    // A request carrying the marker is answered from configuration alone.
    let provider = Recorder::start(vec![(
        "/models",
        (200, json!({ "data": [{"id": "some-model"}] }).to_string()),
    )])
    .await;
    let state = state_service(&provider.base_url(), vec![]).await;

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(
                &state.addr_string(),
            )))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::GET)
        .uri("/v1/models")
        .insert_header((nullrouter_execute::probe::INTERNAL_PROBE_HEADER, "1"))
        .to_request();
    let res = test::call_service(&app, req).await;

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        provider.count("/models"),
        0,
        "a marked request probed anyway; two routers pointed at each other would not terminate"
    );
    Ok(())
}

#[actix_web::test]
async fn an_unmarked_request_still_probes() -> TestResult {
    // The other half of the guard: it must not suppress probing for ordinary clients.
    let provider = Recorder::start(vec![(
        "/models",
        (200, json!({ "data": [{"id": "some-model"}] }).to_string()),
    )])
    .await;
    let state = state_service(&provider.base_url(), vec![]).await;

    let (_, body) = models(&state.addr_string()).await?;
    assert!(
        ids(&body).contains(&format!("{PREFIX}/some-model")),
        "an ordinary request should still probe: {:?}",
        ids(&body)
    );
    assert_eq!(provider.count("/models"), 1);
    Ok(())
}

#[actix_web::test]
async fn a_probe_carries_the_marker_so_the_far_side_can_break_the_loop() -> TestResult {
    // Asserted on the wire rather than trusted: the guard only works if both halves are
    // present, and the sending half is invisible from this router's own behaviour.
    let provider = Recorder::start(vec![(
        "/models",
        (200, json!({ "data": [{"id": "m"}] }).to_string()),
    )])
    .await;
    let state = state_service(&provider.base_url(), vec![]).await;
    models(&state.addr_string()).await?;

    assert!(
        provider.saw_header(
            "/models",
            nullrouter_execute::probe::INTERNAL_PROBE_HEADER,
            "1"
        ),
        "the outgoing probe did not carry the loop-guard header"
    );
    Ok(())
}

#[actix_web::test]
async fn an_unreachable_provider_does_not_hang_the_route() -> TestResult {
    // Port 1 on loopback is closed, so the probe fails to connect rather than timing out.
    // A route that waited the full probe timeout here would stall every editor's startup.
    let state = state_service("http://127.0.0.1:1", vec![]).await;

    let started = std::time::Instant::now();
    let (status, _) = models(&state.addr_string()).await?;
    assert_eq!(status, StatusCode::OK);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the route took {:?} on an unreachable provider",
        started.elapsed()
    );
    Ok(())
}
