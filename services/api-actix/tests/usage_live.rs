//! The usage API must serve real data, degrade honestly, and actually apply
//! every filter it declares.
//!
//! These endpoints previously returned hardcoded zeros while real usage sat in
//! `nullrouter-state`. Two failure modes are worse than an empty dashboard and
//! are what this file pins:
//!
//! 1. Fabricated numbers presented as readings.
//! 2. A declared query parameter that is silently ignored — a filter that does
//!    not filter is a lie about the result set.
//!
//! State is stubbed on loopback so the assertions are about *this* service's
//! behaviour, including what it does when state is unreachable.

#![allow(
    clippy::future_not_send,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration test: the workspace allow-*-in-tests settings only reach #[cfg(test)] modules"
)]

use std::sync::{Arc, Mutex};

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A closed loopback port: every state read fails, exercising degradation.
const UNREACHABLE: &str = "127.0.0.1:1";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

/// A stub state service that answers usage reads and records the paths it saw.
struct StateStub {
    addr: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl StateStub {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("addr").to_string();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let sink = Arc::clone(&recorded);
                tokio::spawn(async move {
                    let _ = serve(stream, sink).await;
                });
            }
        });

        Self { addr, seen }
    }

    /// Full request targets (path plus query) the stub received.
    fn seen(&self) -> Vec<String> {
        self.seen
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default()
    }
}

/// Records exercising every filter dimension the API exposes.
fn stub_records() -> Value {
    json!([
        {
            "id": "req_1", "timestamp": 3_000_u64, "provider": "openai", "model": "gpt-5",
            "connectionId": "conn_a", "endpoint": "/v1/chat/completions", "status": "success",
            "statusCode": 200, "promptTokens": 10, "completionTokens": 4, "cachedTokens": 0,
            "totalTokens": 14, "latencyMs": 120,
        },
        {
            "id": "req_2", "timestamp": 2_000_u64, "provider": "anthropic",
            "model": "claude-sonnet-4.5", "connectionId": "conn_b",
            "endpoint": "/v1/messages", "status": "error", "statusCode": 429,
            "promptTokens": 3, "completionTokens": 0, "cachedTokens": 0, "totalTokens": 3,
            "latencyMs": 40, "error": "Rate limit exceeded",
        },
        {
            "id": "req_3", "timestamp": 1_000_u64, "provider": "openai", "model": "gpt-4o",
            "connectionId": "conn_a", "endpoint": "/v1/chat/completions", "status": "success",
            "statusCode": 200, "promptTokens": 7, "completionTokens": 2, "cachedTokens": 1,
            "totalTokens": 9, "latencyMs": 90,
        },
    ])
}

fn stub_stats() -> Value {
    json!({
        "totalRequests": 3,
        "totalPromptTokens": 20,
        "totalCompletionTokens": 6,
        "totalCachedTokens": 1,
        "totalCost": 0,
        "byProvider": {
            "openai": { "requests": 2, "promptTokens": 17, "completionTokens": 6,
                        "cachedTokens": 1, "totalTokens": 23, "errors": 0, "cost": 0 },
            "anthropic": { "requests": 1, "promptTokens": 3, "completionTokens": 0,
                           "cachedTokens": 0, "totalTokens": 3, "errors": 1, "cost": 0 },
        },
        "byModel": {},
        "byAccount": {},
        "byApiKey": {},
        "byEndpoint": {},
        "last10Minutes": [{ "timestamp": 0_u64, "requests": 3, "tokens": 26 }],
    })
}

/// Apply the query filters the way state does, so "the filter reached state" and
/// "the filter changed the result" are both observable from this test.
fn filter_records(query: &str) -> Value {
    let param = |name: &str| {
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_owned())
    };

    let mut records: Vec<Value> = stub_records().as_array().cloned().unwrap_or_default();
    for (name, field) in [
        ("provider", "provider"),
        ("model", "model"),
        ("connectionId", "connectionId"),
        ("status", "status"),
    ] {
        if let Some(wanted) = param(name) {
            records.retain(|record| {
                record.get(field).and_then(Value::as_str) == Some(wanted.as_str())
            });
        }
    }

    let total = records.len();
    let page = param("page")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);
    let page_size = param("pageSize")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20);
    let start = page.saturating_sub(1).saturating_mul(page_size);
    let window: Vec<Value> = records.into_iter().skip(start).take(page_size).collect();

    let total_pages = if page_size == 0 {
        0
    } else {
        total.div_ceil(page_size)
    };
    json!({
        "records": window,
        "page": page,
        "pageSize": page_size,
        "totalItems": total,
        "totalPages": total_pages,
    })
}

async fn serve(mut stream: TcpStream, seen: Arc<Mutex<Vec<String>>>) -> std::io::Result<()> {
    let mut chunk = [0_u8; 8192];
    let read = stream.read(&mut chunk).await?;
    let request = String::from_utf8_lossy(chunk.get(..read).unwrap_or_default()).into_owned();
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();

    if let Ok(mut sink) = seen.lock() {
        sink.push(target.clone());
    }

    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let body = if path.contains("/usage/stats") || path.contains("/usage/aggregate") {
        stub_stats().to_string()
    } else if path.contains("/usage/records") {
        json!({ "records": stub_records() }).to_string()
    } else if path.contains("/usage/details") {
        filter_records(query).to_string()
    } else if path.contains("/usage/providers") {
        json!({ "providers": [
            { "provider": "openai", "requests": 2, "totalTokens": 23, "errors": 0 },
            { "provider": "anthropic", "requests": 1, "totalTokens": 3, "errors": 1 },
        ] })
        .to_string()
    } else if path.contains("/usage/connection/") {
        json!({ "connectionId": "conn_a", "requests": 2, "totalTokens": 23 }).to_string()
    } else if path.contains("/usage/live") {
        json!({ "activeRequests": 0, "pending": {} }).to_string()
    } else {
        json!({ "ok": true }).to_string()
    };

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

async fn get(state_addr: &str, uri: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(state_addr)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::GET)
        .uri(uri)
        .insert_header((header::ACCEPT, "application/json"))
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let bytes = to_bytes(res.into_body()).await?;
    // Every usage endpoint must answer JSON, even when degrading.
    Ok((status, serde_json::from_slice(&bytes)?))
}

// ── real data ────────────────────────────────────────────────────────────────

#[actix_web::test]
async fn stats_serve_real_totals_from_state() -> TestResult {
    let state = StateStub::start().await;

    let (status, body) = get(&state.addr, "/api/usage/stats?period=7d").await?;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    // The point of the whole endpoint: these are state's numbers, not zeros.
    assert_eq!(body.get("totalRequests"), Some(&json!(3)));
    assert_eq!(body.get("totalPromptTokens"), Some(&json!(20)));
    assert_eq!(body.get("totalCompletionTokens"), Some(&json!(6)));
    assert_eq!(body.pointer("/byProvider/openai/requests"), Some(&json!(2)));
    assert_eq!(
        body.pointer("/byProvider/anthropic/errors"),
        Some(&json!(1))
    );
    Ok(())
}

#[actix_web::test]
async fn stats_response_always_carries_every_documented_key() -> TestResult {
    let state = StateStub::start().await;

    let (_, body) = get(&state.addr, "/api/usage/stats?period=7d").await?;

    // The dashboard indexes these directly; a missing key is a render failure,
    // so the shape is completed even when state omits a section.
    for key in [
        "totalRequests",
        "totalPromptTokens",
        "totalCompletionTokens",
        "totalCachedTokens",
        "totalCost",
        "byProvider",
        "byModel",
        "byAccount",
        "byApiKey",
        "byEndpoint",
        "last10Minutes",
    ] {
        assert!(body.get(key).is_some(), "stats missing `{key}`: {body}");
    }
    Ok(())
}

#[actix_web::test]
async fn logs_and_providers_serve_real_records() -> TestResult {
    let state = StateStub::start().await;

    let (log_status, logs) = get(&state.addr, "/api/usage/logs").await?;
    assert_eq!(log_status, StatusCode::OK);
    let rows = logs.as_array().expect("logs is an array");
    assert_eq!(rows.len(), 3, "expected state's three records: {logs}");
    assert!(
        rows.iter()
            .any(|row| row.get("provider") == Some(&json!("anthropic"))),
        "real provider names must survive: {logs}"
    );

    let (provider_status, providers) = get(&state.addr, "/api/usage/providers").await?;
    assert_eq!(provider_status, StatusCode::OK);
    assert!(
        providers.to_string().contains("openai"),
        "provider aggregates must be real: {providers}"
    );
    Ok(())
}

#[actix_web::test]
async fn connection_usage_reads_the_requested_connection() -> TestResult {
    let state = StateStub::start().await;

    let (status, body) = get(&state.addr, "/api/usage/conn_a").await?;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        state
            .seen()
            .iter()
            .any(|target| target.contains("/usage/connection/conn_a")),
        "the connection id must reach state, saw {:?}",
        state.seen()
    );
    Ok(())
}

// ── declared filters must actually filter ───────────────────────────────────

#[actix_web::test]
async fn request_details_forwards_every_declared_filter() -> TestResult {
    let state = StateStub::start().await;

    let (status, _) = get(
        &state.addr,
        "/api/usage/request-details?page=1&pageSize=10&provider=openai&model=gpt-5\
         &connectionId=conn_a&status=success&startDate=2026-01-01&endDate=2026-12-31",
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let seen = state.seen();
    let target = seen
        .iter()
        .find(|target| target.contains("/usage/details"))
        .expect("details request reached state");

    // A declared-but-dropped parameter is the bug this guards.
    for expected in [
        "page=1",
        "pageSize=10",
        "provider=openai",
        "model=gpt-5",
        "connectionId=conn_a",
        "status=success",
        "startDate=2026-01-01",
        "endDate=2026-12-31",
    ] {
        assert!(
            target.contains(expected),
            "filter `{expected}` was dropped from {target}"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn provider_filter_narrows_the_result_set() -> TestResult {
    let state = StateStub::start().await;

    let (_, unfiltered) = get(&state.addr, "/api/usage/request-details").await?;
    let (_, filtered) = get(&state.addr, "/api/usage/request-details?provider=anthropic").await?;

    let count = |body: &Value| {
        body.get("details")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    };
    assert_eq!(count(&unfiltered), 3, "unfiltered: {unfiltered}");
    // Forwarding is necessary but not sufficient — the result must change.
    assert_eq!(count(&filtered), 1, "filtered: {filtered}");
    assert_eq!(
        filtered.pointer("/details/0/provider"),
        Some(&json!("anthropic"))
    );
    Ok(())
}

#[actix_web::test]
async fn status_filter_separates_successes_from_errors() -> TestResult {
    let state = StateStub::start().await;

    let (_, errors) = get(&state.addr, "/api/usage/request-details?status=error").await?;
    let rows = errors
        .get("details")
        .and_then(Value::as_array)
        .expect("details");

    assert_eq!(rows.len(), 1, "only the failed request: {errors}");
    assert_eq!(
        rows.first().and_then(|row| row.get("statusCode")),
        Some(&json!(429))
    );
    Ok(())
}

#[actix_web::test]
async fn pagination_windows_the_result_set() -> TestResult {
    let state = StateStub::start().await;

    let (_, first) = get(&state.addr, "/api/usage/request-details?page=1&pageSize=2").await?;
    let (_, second) = get(&state.addr, "/api/usage/request-details?page=2&pageSize=2").await?;

    let ids = |body: &Value| {
        body.get("details")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row.get("id").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };

    assert_eq!(ids(&first).len(), 2, "first page: {first}");
    assert_eq!(ids(&second).len(), 1, "second page: {second}");
    // Pages must not overlap, or a user sees duplicates while scrolling.
    assert!(
        ids(&first).iter().all(|id| !ids(&second).contains(id)),
        "pages overlap: {:?} vs {:?}",
        ids(&first),
        ids(&second)
    );
    // And the reported total is the full set, not the page — otherwise page
    // controls cannot know how many pages exist.
    assert_eq!(first.pointer("/pagination/totalItems"), Some(&json!(3)));
    assert_eq!(first.pointer("/pagination/totalPages"), Some(&json!(2)));
    assert_eq!(first.pointer("/pagination/hasNext"), Some(&json!(true)));
    assert_eq!(second.pointer("/pagination/hasPrev"), Some(&json!(true)));
    Ok(())
}

// ── validation ───────────────────────────────────────────────────────────────

#[actix_web::test]
async fn invalid_period_is_rejected_rather_than_silently_defaulted() -> TestResult {
    let state = StateStub::start().await;

    for uri in [
        "/api/usage/stats?period=nonsense",
        "/api/usage/chart?period=nonsense",
    ] {
        let (status, body) = get(&state.addr, uri).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} -> {body}");
    }

    // `all` is valid for stats but not for chart, and that distinction must hold.
    let (stats_all, _) = get(&state.addr, "/api/usage/stats?period=all").await?;
    assert_eq!(stats_all, StatusCode::OK);
    let (chart_all, _) = get(&state.addr, "/api/usage/chart?period=all").await?;
    assert_eq!(chart_all, StatusCode::BAD_REQUEST);
    Ok(())
}

#[actix_web::test]
async fn out_of_range_pagination_is_rejected() -> TestResult {
    let state = StateStub::start().await;

    for uri in [
        "/api/usage/request-details?page=0",
        "/api/usage/request-details?pageSize=0",
        "/api/usage/request-details?pageSize=101",
    ] {
        let (status, body) = get(&state.addr, uri).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} -> {body}");
    }
    Ok(())
}

// ── honest degradation ───────────────────────────────────────────────────────

#[actix_web::test]
async fn unreachable_state_degrades_to_zeroes_not_invented_numbers() -> TestResult {
    // With state down there is no data. The endpoint must still answer the
    // documented JSON shape so the dashboard renders, with zeroes that are
    // truthfully zero rather than plausible-looking traffic.
    let (status, body) = get(UNREACHABLE, "/api/usage/stats?period=7d").await?;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body.get("totalRequests"), Some(&json!(0)));
    assert_eq!(body.get("totalPromptTokens"), Some(&json!(0)));
    assert_eq!(
        body.get("byProvider"),
        Some(&json!({})),
        "no provider may be invented while state is down: {body}"
    );
    Ok(())
}

#[actix_web::test]
async fn unreachable_state_yields_empty_collections_for_list_endpoints() -> TestResult {
    for uri in ["/api/usage/logs", "/api/usage/request-logs"] {
        let (status, body) = get(UNREACHABLE, uri).await?;
        assert_eq!(status, StatusCode::OK, "{uri} -> {body}");
        assert_eq!(
            body.as_array().map(Vec::len),
            Some(0),
            "{uri} must be empty, not fabricated: {body}"
        );
    }

    let (status, details) = get(UNREACHABLE, "/api/usage/request-details").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        details
            .get("details")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "details must be empty: {details}"
    );
    Ok(())
}

#[actix_web::test]
async fn every_usage_route_answers_json_under_both_conditions() -> TestResult {
    let state = StateStub::start().await;

    // A route that 500s or returns HTML breaks the dashboard regardless of
    // whether state is up, so both paths are checked for every route.
    for uri in [
        "/api/usage/stats?period=24h",
        "/api/usage/history",
        "/api/usage/chart?period=7d",
        "/api/usage/logs",
        "/api/usage/request-logs",
        "/api/usage/providers",
        "/api/usage/request-details",
        "/api/usage/conn_a",
    ] {
        let (up, up_body) = get(&state.addr, uri).await?;
        assert!(
            up.is_success(),
            "{uri} failed with state up: {up} {up_body}"
        );

        let (down, down_body) = get(UNREACHABLE, uri).await?;
        assert!(
            down.is_success()
                || down == StatusCode::SERVICE_UNAVAILABLE
                || down == StatusCode::NOT_FOUND,
            "{uri} failed unacceptably with state down: {down} {down_body}"
        );
        assert!(
            down_body.is_object() || down_body.is_array(),
            "{uri} must answer JSON even while degrading: {down_body}"
        );
    }
    Ok(())
}
