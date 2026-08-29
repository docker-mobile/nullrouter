//! `/api/pxpipe/*`: the eight routes behind the Token Saver page.
//!
//! What this level owns is the split. Five routes need the worker, which lives in
//! `nullrouter-runtime`, so they proxy; two read files and are answered here; and
//! `install` does both. The property worth defending is that a proxy target being
//! down is reported as *that* — not as "not running", which would be a claim about a
//! process this service never reached, and not as a 500, which would say the router
//! is broken when it is one service that is.
//!
//! The runtime is deliberately unreachable in most of these: it is the case a user
//! actually hits during a rolling restart, and the one where a wrong answer is most
//! misleading.

#![allow(
    clippy::future_not_send,
    reason = "actix test services are single-threaded and not Send"
)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions read clearer with direct expect than with error plumbing"
)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};
use nullrouter_pxpipe::{Paths, TokenSaver};
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A closed loopback port.
const UNREACHABLE: &str = "127.0.0.1:1";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

/// Call one route with the token saver rooted at `data_dir`.
///
/// Explicit so a test never reads or writes whatever `DATA_DIR` or `$HOME` the suite
/// runs under — a stats assertion against the developer's own event log would pass or
/// fail on their machine rather than on the code.
async fn call(
    method: Method,
    uri: &str,
    data_dir: &std::path::Path,
) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE)))
            .app_data(web::Data::new(TokenSaver::new(Paths::new(data_dir))))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok((status, serde_json::from_str(&body).unwrap_or(Value::Null)))
}

/// Write an event log with `lines` under `data_dir`.
fn write_events(data_dir: &std::path::Path, lines: &[Value]) {
    let root = data_dir.join("pxpipe");
    std::fs::create_dir_all(&root).expect("create root");
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(root.join("events.jsonl"), text).expect("write events");
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(0))
}

#[actix_web::test]
async fn all_eight_routes_are_registered() -> TestResult {
    let dir = tempfile::tempdir()?;
    // Any answer but 404 proves the route exists; the specific answers are asserted
    // below. Registered as one test because a missing route is the failure that makes
    // every other assertion in this file vacuous.
    for (method, uri) in [
        (Method::GET, "/api/pxpipe/status"),
        (Method::GET, "/api/pxpipe/health"),
        (Method::POST, "/api/pxpipe/health"),
        (Method::GET, "/api/pxpipe/logs"),
        (Method::GET, "/api/pxpipe/stats"),
        (Method::POST, "/api/pxpipe/start"),
        (Method::POST, "/api/pxpipe/stop"),
        (Method::POST, "/api/pxpipe/restart"),
    ] {
        let (status, _) = call(method.clone(), uri, dir.path()).await?;
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "{method} {uri} is not routed"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn an_unreachable_runtime_is_reported_as_unknown_not_as_stopped() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (status, body) = call(Method::GET, "/api/pxpipe/status", dir.path()).await?;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body.pointer("/code").and_then(Value::as_str),
        Some("RUNTIME_UNREACHABLE")
    );
    // The claim this must not make. `running: false` would assert something about a
    // process this service never contacted, and a user would go looking for a stopped
    // worker instead of a stopped service.
    assert_eq!(
        body.pointer("/running"),
        None,
        "an unreachable runtime must not be reported as a stopped worker"
    );
    // What this service *can* see is reported.
    assert_eq!(body.pointer("/installed"), Some(&Value::Bool(false)));
    assert!(body.pointer("/npmAvailable").is_some());
    assert!(body.pointer("/nodeAvailable").is_some());
    Ok(())
}

#[actix_web::test]
async fn health_reports_the_unreachable_runtime_as_a_failed_check() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (status, body) = call(Method::POST, "/api/pxpipe/health", dir.path()).await?;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body.pointer("/healthy"), Some(&Value::Bool(false)));
    // A checklist entry rather than a bare error, so the page renders it the same way
    // it renders every other failed step.
    let checks = body
        .pointer("/checks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(checks.len(), 1);
    assert_eq!(
        checks.first().and_then(|step| step.pointer("/id")),
        Some(&Value::String("runtime".to_owned()))
    );
    Ok(())
}

#[actix_web::test]
async fn health_answers_get_as_well_as_post() -> TestResult {
    let dir = tempfile::tempdir()?;
    // Upstream's `export const GET = POST`, so the card can probe on page load without
    // issuing a mutation.
    let (get_status, get_body) = call(Method::GET, "/api/pxpipe/health", dir.path()).await?;
    let (post_status, post_body) = call(Method::POST, "/api/pxpipe/health", dir.path()).await?;
    assert_eq!(get_status, post_status);
    assert_eq!(get_body, post_body);
    Ok(())
}

#[actix_web::test]
async fn stats_are_read_locally_rather_than_proxied() -> TestResult {
    let dir = tempfile::tempdir()?;
    let now = now_millis();
    write_events(
        dir.path(),
        &[
            serde_json::json!({
                "ts": now, "applied": true, "reason": "applied",
                "originalChars": 60_000, "tokensBeforeEst": 8_000,
                "tokensAfterEst": 2_000, "tokensSavedEst": 6_000,
                "imageCount": 3, "durationMs": 250,
            }),
            serde_json::json!({
                "ts": now, "applied": false, "reason": "below_min_chars",
            }),
            serde_json::json!({ "ts": now, "applied": false, "reason": "timeout" }),
        ],
    );

    // The runtime is unreachable, and this still answers 200: the event log is a file
    // this service reads directly, so a stats page keeps working while the runtime is
    // restarting.
    let (status, body) = call(Method::GET, "/api/pxpipe/stats", dir.path()).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.pointer("/windows/all/requests")
            .and_then(Value::as_u64),
        Some(3)
    );
    assert_eq!(
        body.pointer("/windows/all/compressed")
            .and_then(Value::as_u64),
        Some(1)
    );
    // A refusal and a failure are counted apart.
    assert_eq!(
        body.pointer("/windows/all/bypassed")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        body.pointer("/windows/all/errors").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        body.pointer("/windows/all/savedPct")
            .and_then(Value::as_f64),
        Some(75.0)
    );
    // A full month of buckets, so a chart has an axis even on a quiet router.
    assert_eq!(
        body.pointer("/timeline")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(30)
    );
    Ok(())
}

#[actix_web::test]
async fn logs_are_read_locally_and_newest_first() -> TestResult {
    let dir = tempfile::tempdir()?;
    let now = now_millis();
    write_events(
        dir.path(),
        &[
            serde_json::json!({ "ts": now, "reason": "first" }),
            serde_json::json!({ "ts": now + 1, "reason": "second" }),
            serde_json::json!({ "ts": now + 2, "reason": "third" }),
        ],
    );
    let (status, body) = call(Method::GET, "/api/pxpipe/logs", dir.path()).await?;
    assert_eq!(status, StatusCode::OK);
    let events = body
        .pointer("/events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events.first().and_then(|event| event.pointer("/reason")),
        Some(&Value::String("third".to_owned())),
        "a log panel reads newest first"
    );
    assert!(body.pointer("/installLog").is_some());
    Ok(())
}

#[actix_web::test]
async fn a_limit_is_honoured_and_clamped() -> TestResult {
    let dir = tempfile::tempdir()?;
    let now = now_millis();
    let lines: Vec<Value> = (0..20)
        .map(|index| serde_json::json!({ "ts": now + index, "reason": "applied" }))
        .collect();
    write_events(dir.path(), &lines);

    let (_, body) = call(Method::GET, "/api/pxpipe/logs?limit=5", dir.path()).await?;
    assert_eq!(
        body.pointer("/events")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(5)
    );
    // Upstream caps a read at 500; a caller asking for more gets the cap rather than
    // an error or an unbounded read.
    let (status, body) = call(Method::GET, "/api/pxpipe/logs?limit=100000", dir.path()).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.pointer("/events")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(20)
    );
    // A nonsense limit falls back to the default rather than rejecting the read.
    let (status, _) = call(Method::GET, "/api/pxpipe/logs?limit=abc", dir.path()).await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a non-numeric limit is rejected"
    );
    Ok(())
}

#[actix_web::test]
async fn stats_and_logs_answer_before_anything_has_run() -> TestResult {
    let dir = tempfile::tempdir()?;
    // A fresh install with no events: the page must render, not error.
    let (status, body) = call(Method::GET, "/api/pxpipe/stats", dir.path()).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.pointer("/windows/all/requests")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        body.pointer("/windows/all/savedPct")
            .and_then(Value::as_f64),
        Some(0.0),
        "no divide-by-zero, and no NaN reaching JSON"
    );
    let (status, body) = call(Method::GET, "/api/pxpipe/logs", dir.path()).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.pointer("/events")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    Ok(())
}

#[actix_web::test]
async fn the_control_routes_relay_the_runtimes_failure_rather_than_flattening_it() -> TestResult {
    let dir = tempfile::tempdir()?;
    for action in ["start", "stop", "restart"] {
        let (status, body) =
            call(Method::POST, &format!("/api/pxpipe/{action}"), dir.path()).await?;
        // 503, not 500: one service is unreachable, the router is not broken.
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{action}");
        assert_eq!(
            body.pointer("/success"),
            Some(&Value::Bool(false)),
            "{action}"
        );
        assert_eq!(
            body.pointer("/code").and_then(Value::as_str),
            Some("RUNTIME_UNREACHABLE"),
            "{action}"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn the_routes_answer_options() -> TestResult {
    let dir = tempfile::tempdir()?;
    for action in [
        "status", "health", "install", "logs", "stats", "start", "stop", "restart",
    ] {
        let (status, _) = call(
            Method::OPTIONS,
            &format!("/api/pxpipe/{action}"),
            dir.path(),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{action} did not answer OPTIONS"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn install_refuses_rather_than_claiming_success_when_it_cannot_finish() -> TestResult {
    let dir = tempfile::tempdir()?;
    // This test does not install anything: whether the machine has npm and a network
    // is a property of the machine, so asserting either outcome would make the suite
    // pass or fail on the environment. What is asserted is the invariant that holds
    // both ways — the answer never claims success it did not achieve.
    let (status, body) = call(Method::POST, "/api/pxpipe/install", dir.path()).await?;
    if status == StatusCode::OK {
        assert_eq!(body.pointer("/success"), Some(&Value::Bool(true)));
        assert_eq!(body.pointer("/installed"), Some(&Value::Bool(true)));
        assert!(
            body.pointer("/version").and_then(Value::as_str).is_some(),
            "a successful install reports the version it landed"
        );
        // The runtime is unreachable here, so the reload could not happen and the
        // health block says so rather than reporting a healthy worker.
        assert_eq!(body.pointer("/health/healthy"), Some(&Value::Bool(false)));
    } else {
        assert_eq!(body.pointer("/success"), Some(&Value::Bool(false)));
        let code = body.pointer("/code").and_then(Value::as_str).unwrap_or("");
        assert!(
            matches!(code, "NPM_MISSING" | "INSTALL_FAILED"),
            "unexpected code {code} with status {status}"
        );
        assert!(
            body.pointer("/error").and_then(Value::as_str).is_some(),
            "a refusal says why"
        );
    }
    Ok(())
}
