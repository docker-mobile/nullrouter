//! `/v1/videos/*` route wiring and refusals.
//!
//! The upstream call itself is covered by `nullrouter-execute`'s `raw_dispatch`
//! tests, which can point at a loopback socket. What can only be checked here is
//! the routing: that `generations` is an action and not a job id, that a provider
//! without video support is refused rather than silently rerouted to the one that
//! has it, and that a malformed body is rejected before any account is selected.

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

/// A closed loopback port: credential selection fails deterministically, so these
/// route-shape assertions need no state service.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

struct Reply {
    status: StatusCode,
    body: String,
    json: Option<Value>,
}

async fn call(method: Method, uri: &str, content_type: &str, body: &str) -> TestResult<Reply> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(
                UNREACHABLE_STATE_ADDR,
            )))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, content_type))
        .set_payload(body.to_owned())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    let json = serde_json::from_str::<Value>(&body).ok();
    Ok(Reply { status, body, json })
}

async fn post(uri: &str, body: &str) -> TestResult<Reply> {
    call(Method::POST, uri, "application/json", body).await
}

fn error_message(reply: &Reply) -> String {
    reply
        .json
        .as_ref()
        .and_then(|json| {
            json.pointer("/error/message")
                .or_else(|| json.get("error"))
                .and_then(Value::as_str)
        })
        .unwrap_or(&reply.body)
        .to_owned()
}

#[actix_rt::test]
async fn the_three_creation_actions_are_routed_and_reach_credential_selection() -> TestResult {
    for action in ["generations", "edits", "extensions"] {
        let reply = post(
            &format!("/v1/videos/{action}"),
            r#"{"model":"xai/grok-imagine-video","prompt":"a cat"}"#,
        )
        .await?;

        // The route exists (not 404) and got as far as needing an account, which
        // with no state service is a 503.
        assert_eq!(
            reply.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{action}: {}",
            reply.body
        );
        let message = error_message(&reply);
        assert!(
            message.contains("state service"),
            "{action} should fail at selection, got {message}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn a_poll_is_routed_separately_from_a_creation() -> TestResult {
    // A GET on a job id is a poll. It must not be read as an action.
    let reply = call(Method::GET, "/v1/videos/vid_abc123", "application/json", "").await?;
    assert_ne!(reply.status, StatusCode::NOT_FOUND, "{}", reply.body);
    assert_eq!(
        reply.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        reply.body
    );

    // And a GET on an action name is not a creation: POST is the only creation
    // method, so a GET there is method-not-allowed rather than a job poll of a
    // job called "generations".
    let wrong_method = call(
        Method::GET,
        "/v1/videos/generations",
        "application/json",
        "",
    )
    .await?;
    assert_eq!(
        wrong_method.status,
        StatusCode::METHOD_NOT_ALLOWED,
        "{}",
        wrong_method.body
    );
    Ok(())
}

#[actix_rt::test]
async fn a_provider_without_video_support_is_refused_before_any_account_is_used() -> TestResult {
    let reply = post(
        "/v1/videos/generations",
        r#"{"model":"openai/sora-2","prompt":"a cat"}"#,
    )
    .await?;

    // 400, not a reroute to xAI: billing an account the client did not name would
    // be worse than refusing.
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.body);
    let message = error_message(&reply);
    assert!(message.contains("openai"), "got {message}");
    assert!(message.contains("video"), "got {message}");
    Ok(())
}

#[actix_rt::test]
async fn a_malformed_json_body_is_rejected_before_credential_selection() -> TestResult {
    let reply = post("/v1/videos/generations", "{not json").await?;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.body);
    assert!(
        error_message(&reply).contains("Invalid JSON"),
        "got {}",
        reply.body
    );
    Ok(())
}

#[actix_rt::test]
async fn a_multipart_body_is_not_parsed_and_routes_to_the_default_provider() -> TestResult {
    // Parsing this would mint a new boundary, so it is never parsed — which means a
    // multipart request cannot name a provider and uses the default one.
    let boundary = "----nullrouterBoundaryTest";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nrain\r\n--{boundary}--\r\n"
    );
    let reply = call(
        Method::POST,
        "/v1/videos/edits",
        &format!("multipart/form-data; boundary={boundary}"),
        &body,
    )
    .await?;

    // Not a 400: an unparsed body is expected here, not malformed input.
    assert_eq!(
        reply.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        reply.body
    );
    assert!(
        error_message(&reply).contains("state service"),
        "got {}",
        reply.body
    );
    Ok(())
}

#[actix_rt::test]
async fn the_video_routes_answer_cors_preflight() -> TestResult {
    for uri in [
        "/v1/videos/generations",
        "/v1/videos/edits",
        "/v1/videos/extensions",
        "/v1/videos/vid_1",
    ] {
        let reply = call(Method::OPTIONS, uri, "application/json", "").await?;
        assert_eq!(reply.status, StatusCode::NO_CONTENT, "{uri}");
    }
    Ok(())
}

#[actix_rt::test]
async fn the_api_v1_alias_serves_the_video_routes_too() -> TestResult {
    // Every other v1 family is reachable under both prefixes; video is no exception.
    let reply = post(
        "/api/v1/videos/generations",
        r#"{"model":"xai/grok-imagine-video","prompt":"a cat"}"#,
    )
    .await?;
    assert_ne!(reply.status, StatusCode::NOT_FOUND, "{}", reply.body);
    assert_eq!(
        reply.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        reply.body
    );
    Ok(())
}
