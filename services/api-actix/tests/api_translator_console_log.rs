#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::Value;

use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A closed loopback port: usage reads fall back to the zeroed shape,
/// so these parity tests need no state service.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

#[derive(Debug)]
struct ApiResponse {
    status: StatusCode,
    content_type: String,
    headers: actix_web::http::header::HeaderMap,
    body: String,
    json: Option<Value>,
}

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

async fn request(method: Method, uri: &str, body: &str) -> TestResult<ApiResponse> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
        .to_request();

    let res = test::call_service(&app, req).await;
    let status = res.status();
    let headers = res.headers().clone();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body_bytes = to_bytes(res.into_body())
        .await
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let body = std::str::from_utf8(&body_bytes)?.to_owned();
    let json = if body_bytes.is_empty() {
        None
    } else {
        Some(serde_json::from_slice(&body_bytes)?)
    };

    Ok(ApiResponse {
        status,
        content_type,
        headers,
        body,
        json,
    })
}

fn assert_structured_json(response: &ApiResponse) {
    assert!(
        response.content_type.starts_with("application/json"),
        "content-type was {}",
        response.content_type
    );
    assert!(!response.body.contains("<html"), "body was HTML");
    assert!(!response.body.contains("<!DOCTYPE"), "body was HTML");
}

fn json(response: &ApiResponse) -> TestResult<&Value> {
    response
        .json
        .as_ref()
        .ok_or_else(|| test_error("missing JSON body"))
}

#[actix_rt::test]
async fn translator_console_logs_get_returns_successful_empty_log_buffer() -> TestResult {
    // Given: nullrouter-api starts without a translator console-log buffer.

    // When: a browser requests the upstream-compatible log buffer.
    let response = request(Method::GET, "/api/translator/console-logs", "").await?;

    // Then: the route returns the upstream success envelope with an empty log array.
    assert_eq!(response.status, StatusCode::OK);
    assert_structured_json(&response);
    assert_eq!(
        json(&response)?,
        &serde_json::json!({ "success": true, "logs": [] })
    );
    Ok(())
}

#[actix_rt::test]
async fn translator_console_logs_delete_returns_success_without_body_requirements() -> TestResult {
    // Given: deleting console logs is an idempotent upstream dashboard action.

    // When: the dashboard clears the console-log buffer without a request body.
    let response = request(Method::DELETE, "/api/translator/console-logs", "").await?;

    // Then: the route reports success as structured JSON.
    assert_eq!(response.status, StatusCode::OK);
    assert_structured_json(&response);
    assert_eq!(json(&response)?, &serde_json::json!({ "success": true }));
    Ok(())
}

#[actix_rt::test]
async fn translator_console_logs_options_returns_no_content_with_cors_headers() -> TestResult {
    // Given: browser clients preflight the console-log endpoint before clearing logs.

    // When: OPTIONS hits the console-log route.
    let response = request(Method::OPTIONS, "/api/translator/console-logs", "").await?;

    // Then: the preflight is accepted with shared CORS headers and no JSON body.
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(response.body.is_empty());
    assert_eq!(
        response
            .headers
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|value| value.to_str().ok()),
        Some("*")
    );
    assert_eq!(
        response
            .headers
            .get(header::ACCESS_CONTROL_ALLOW_METHODS)
            .and_then(|value| value.to_str().ok()),
        Some("GET, POST, PUT, PATCH, DELETE, OPTIONS")
    );
    Ok(())
}

#[actix_rt::test]
async fn translator_console_logs_rejects_unsupported_methods_as_structured_json() -> TestResult {
    // Given: the route only supports the upstream GET, DELETE, and OPTIONS methods.

    // When: unsupported methods hit the console-log endpoint.
    for method in [Method::POST, Method::PUT, Method::from_bytes(b"BREW")?] {
        let response = request(method.clone(), "/api/translator/console-logs", "{}").await?;

        // Then: the route rejects the method with JSON instead of dashboard HTML.
        assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED, "{method}");
        assert_structured_json(&response);
        assert_eq!(
            json(&response)?,
            &serde_json::json!({ "error": "Method not allowed" }),
            "{method}"
        );
    }
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
