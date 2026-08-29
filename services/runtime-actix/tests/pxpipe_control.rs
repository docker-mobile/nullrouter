//! `/internal/pxpipe/*`: the control surface for the worker that holds the transform.
//!
//! The worker itself is covered in `nullrouter-pxpipe`, against a real `node`. What
//! only this level can check is that the routes exist, that each failure keeps a
//! status code a caller can act on, and — the one that matters most — that the
//! request path is unaffected when the saver is off or unavailable.

#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_runtime::{Runtime, app_config, configure};
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A closed loopback port: state reads fail deterministically, which is also the
/// "state is down" case these assertions care about.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

struct Reply {
    status: StatusCode,
    json: Option<Value>,
}

/// Call one route against a runtime whose token-saver directory is `data_dir`.
///
/// The directory is explicit so a test never installs into, or writes events to,
/// whatever `DATA_DIR` or `$HOME` the suite happens to run under.
async fn call(method: Method, uri: &str, data_dir: &std::path::Path) -> TestResult<Reply> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr_and_pxpipe_dir(
                UNREACHABLE_STATE_ADDR,
                data_dir,
            )))
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
    Ok(Reply {
        status,
        json: serde_json::from_str::<Value>(&body).ok(),
    })
}

fn field<'a>(reply: &'a Reply, pointer: &str) -> Option<&'a Value> {
    reply.json.as_ref()?.pointer(pointer)
}

#[actix_web::test]
async fn status_reports_the_install_state_without_inventing_a_version() -> TestResult {
    let dir = tempfile::tempdir()?;
    let reply = call(Method::GET, "/internal/pxpipe/status", dir.path()).await?;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(field(&reply, "/installed"), Some(&Value::Bool(false)));
    assert_eq!(field(&reply, "/running"), Some(&Value::Bool(false)));
    // Absent rather than null or empty: there is no version to report.
    assert_eq!(field(&reply, "/version"), None);
    // Named for what this port does, not for upstream's in-process arrangement.
    assert_eq!(
        field(&reply, "/mode").and_then(Value::as_str),
        Some("worker")
    );
    // Whether the host can install and run at all is reported, because those are the
    // two things a user would otherwise have to guess at.
    assert!(field(&reply, "/npmAvailable").is_some());
    assert!(field(&reply, "/nodeAvailable").is_some());
    Ok(())
}

#[actix_web::test]
async fn health_answers_on_get_as_well_as_post() -> TestResult {
    let dir = tempfile::tempdir()?;
    for method in [Method::GET, Method::POST] {
        let reply = call(method.clone(), "/internal/pxpipe/health", dir.path()).await?;
        assert_eq!(reply.status, StatusCode::OK, "{method} was refused");
        assert_eq!(field(&reply, "/healthy"), Some(&Value::Bool(false)));
        assert_eq!(
            field(&reply, "/error").and_then(Value::as_str),
            Some("pxpipe not installed")
        );
    }
    Ok(())
}

#[actix_web::test]
async fn health_stops_at_the_first_failing_step() -> TestResult {
    let dir = tempfile::tempdir()?;
    let reply = call(Method::POST, "/internal/pxpipe/health", dir.path()).await?;
    let checks = field(&reply, "/checks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // One step, not three. "Cannot load" and "cannot transform" would both be true
    // with nothing installed, and neither is the thing to fix.
    assert_eq!(checks.len(), 1);
    assert_eq!(
        checks.first().and_then(|step| step.pointer("/id")),
        Some(&Value::String("installed".to_owned()))
    );
    Ok(())
}

#[actix_web::test]
async fn starting_without_an_install_is_a_conflict_not_a_server_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let reply = call(Method::POST, "/internal/pxpipe/start", dir.path()).await?;
    // 409 with a code, because the caller can fix this by installing. A 500 would say
    // the router is broken, which it is not.
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(field(&reply, "/success"), Some(&Value::Bool(false)));
    let code = field(&reply, "/code").and_then(Value::as_str).unwrap_or("");
    // State is unreachable here, so `pxpipeAutoInstall` reads false and the refusal is
    // about the setting rather than about npm. Either code is a correct answer for
    // "nothing is installed"; what must not happen is a 500 or a claimed success.
    assert!(
        matches!(code, "NOT_INSTALLED" | "NPM_MISSING" | "INSTALL_FAILED"),
        "unexpected code {code}"
    );
    Ok(())
}

#[actix_web::test]
async fn stopping_a_saver_that_was_never_started_is_not_an_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let reply = call(Method::POST, "/internal/pxpipe/stop", dir.path()).await?;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(field(&reply, "/stopped"), Some(&Value::Bool(false)));
    // The status is merged in, so one call tells a dashboard both what happened and
    // where things now stand.
    assert_eq!(field(&reply, "/running"), Some(&Value::Bool(false)));
    Ok(())
}

#[actix_web::test]
async fn restarting_without_an_install_reports_why_rather_than_succeeding() -> TestResult {
    let dir = tempfile::tempdir()?;
    let reply = call(Method::POST, "/internal/pxpipe/restart", dir.path()).await?;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(
        field(&reply, "/code").and_then(Value::as_str),
        Some("NOT_INSTALLED")
    );
    Ok(())
}

#[actix_web::test]
async fn the_control_routes_answer_options() -> TestResult {
    let dir = tempfile::tempdir()?;
    for action in ["status", "health", "start", "stop", "restart"] {
        let reply = call(
            Method::OPTIONS,
            &format!("/internal/pxpipe/{action}"),
            dir.path(),
        )
        .await?;
        assert_eq!(
            reply.status,
            StatusCode::NO_CONTENT,
            "{action} did not answer OPTIONS"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn a_chat_request_is_unaffected_when_the_saver_cannot_run() -> TestResult {
    // The property that matters most: with state unreachable the saver reads as
    // disabled, and a request must still be routed exactly as it would be otherwise.
    // A token saver that can change the outcome of a request when it is unavailable
    // is worse than one that does nothing.
    let dir = tempfile::tempdir()?;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr_and_pxpipe_dir(
                UNREACHABLE_STATE_ADDR,
                dir.path(),
            )))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::post()
        .uri("/v1/messages")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(
            serde_json::json!({
                "model": "anthropic/claude-fable-5",
                "max_tokens": 16,
                "messages": [{ "role": "user", "content": "hello" }],
            })
            .to_string(),
        )
        .to_request();
    let res = test::call_service(&app, req).await;
    // The state service is unreachable, so this fails at credential selection — the
    // same 503 it would return with no token saver in the build at all. What is
    // asserted is that it is *that* failure and not a transform one.
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    assert!(
        !body.to_lowercase().contains("pxpipe"),
        "the token saver leaked into an unrelated failure: {body}"
    );

    // And nothing was recorded: the saver never ran, so it has no events to its name.
    let events = dir.path().join("pxpipe").join("events.jsonl");
    assert!(
        !events.exists(),
        "a disabled saver wrote an event log at {}",
        events.display()
    );
    Ok(())
}
