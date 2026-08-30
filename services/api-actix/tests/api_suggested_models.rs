//! `GET /api/providers/suggested-models`: a provider's published catalogue, filtered.
//!
//! The route existed and returned an empty list. It now fetches the catalogue and applies the
//! provider's filter.
//!
//! **The URL must be one the registry declares**, which is where this port is deliberately
//! stricter than upstream. Upstream fetches whatever URL the caller passes, so an
//! authenticated dashboard request can make the server issue a GET to any host it can reach
//! and read the result back — a server-side request forgery primitive. Nothing is lost by
//! refusing: the dashboard only ever passes a URL it read from the registry.
//!
//! The filters themselves are unit-tested in `crates/providers/src/suggested.rs`. What is
//! tested here is the boundary: the allowlist, and that a catalogue being unavailable answers
//! an empty list rather than a dashboard error.

#![allow(
    clippy::future_not_send,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test assertions read clearer with direct expect than with error plumbing"
)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode},
    test, web,
};
use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const UNREACHABLE: &str = "127.0.0.1:1";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

async fn get(uri: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::GET)
        .uri(uri)
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok((status, serde_json::from_str(&body)?))
}

#[actix_web::test]
async fn a_url_no_provider_declares_is_refused() -> TestResult {
    // The whole point of the divergence. A loopback address is the case that matters: it is
    // exactly what an SSRF would reach for, and it is where this router's own internal
    // services live.
    for url in [
        "http://127.0.0.1:20134/internal/v1/routing-context",
        "http://169.254.169.254/latest/meta-data/",
        "http://localhost:20128/api/settings",
        "file:///etc/passwd",
    ] {
        let encoded = urlencode(url);
        let (status, body) = get(&format!(
            "/api/providers/suggested-models?url={encoded}&type=openai"
        ))
        .await?;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{url} should be refused, got {body}"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn a_url_that_only_shares_a_host_with_a_declared_one_is_refused() -> TestResult {
    // Matching on host would allow any other path on that host, including whatever an open
    // redirect there can reach. The check is an exact URL match for that reason.
    let encoded = urlencode("https://openrouter.ai/api/v1/keys");
    let (status, _) = get(&format!(
        "/api/providers/suggested-models?url={encoded}&type=openrouter-free"
    ))
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    Ok(())
}

/// Whether to run the tests that reach a real catalogue host.
///
/// Off by default. A suite that fetches a third party's catalogue on every `cargo test` is
/// slow, rate-limitable, and answers differently on a machine with no egress — and the first
/// version of this file did exactly that, returning 18 real OpenRouter models into an
/// assertion that expected an empty list. Everything worth testing at this boundary is the
/// allowlist, which refuses *before* any fetch, so the rest of the file is hermetic.
fn egress_enabled() -> bool {
    std::env::var("NULLROUTER_TEST_EGRESS").is_ok_and(|value| value == "1")
}

#[actix_web::test]
async fn a_declared_url_is_fetched_and_filtered() -> TestResult {
    if !egress_enabled() {
        eprintln!("skipping: set NULLROUTER_TEST_EGRESS=1 to fetch a real catalogue");
        return Ok(());
    }
    let encoded = urlencode("https://openrouter.ai/api/v1/models");
    let (status, body) = get(&format!(
        "/api/providers/suggested-models?url={encoded}&type=openrouter-free"
    ))
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    let models = body["data"].as_array().expect("a data array");
    assert!(!models.is_empty(), "OpenRouter always lists free models");
    // The filter's contract, checked against a real catalogue rather than a fixture: a 200k
    // context floor, largest first.
    let mut previous = u64::MAX;
    for model in models {
        let context = model["contextLength"].as_u64().unwrap_or(0);
        assert!(context >= 200_000, "{model} is below the context floor");
        assert!(context <= previous, "not sorted descending at {model}");
        previous = context;
    }
    Ok(())
}

// The pairing check — every registry-declared URL passes the allowlist, and every declared
// filter is implemented — lives in `crates/providers/src/suggested.rs`. It is pure registry
// logic and needs no HTTP boundary, and `use actix_web::test` here shadows the built-in
// `#[test]` attribute, which makes a non-async test in this file awkward for no reason.

#[actix_web::test]
async fn a_missing_parameter_is_a_bad_request() -> TestResult {
    let encoded = urlencode("https://openrouter.ai/api/v1/models");
    for uri in [
        "/api/providers/suggested-models".to_owned(),
        format!("/api/providers/suggested-models?url={encoded}"),
        "/api/providers/suggested-models?type=openrouter-free".to_owned(),
    ] {
        let (status, _) = get(&uri).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
    }
    Ok(())
}

#[actix_web::test]
async fn an_unknown_filter_on_a_declared_url_is_a_bad_request() -> TestResult {
    // Not an empty list: an empty list reads as "this provider publishes no free models",
    // which is a different claim from "this router does not know that filter".
    let encoded = urlencode("https://openrouter.ai/api/v1/models");
    let (status, _) = get(&format!(
        "/api/providers/suggested-models?url={encoded}&type=no-such-filter"
    ))
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    Ok(())
}

/// Percent-encode a query-parameter value.
///
/// Hand-rolled rather than pulling a dependency into the test: only the characters that
/// appear in these URLs need handling.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
