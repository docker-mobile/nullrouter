#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::Value;

use nullrouter_runtime::{Runtime, app_config, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A closed loopback port: credential lookup fails deterministically as
/// "state unavailable", so these route-shape tests need no state service.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

struct RuntimeResponse {
    status: StatusCode,
    content_type: String,
    body: String,
}

async fn request(method: Method, uri: &str, body: &str) -> TestResult<RuntimeResponse> {
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
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok(RuntimeResponse {
        status,
        content_type,
        body,
    })
}

fn parse_json(body: &str) -> TestResult<Value> {
    Ok(serde_json::from_str(body)?)
}

fn error_message(json: &Value) -> TestResult<&str> {
    json.pointer("/error/message")
        .and_then(Value::as_str)
        .ok_or_else(|| test_error("missing error.message"))
}

#[actix_rt::test]
async fn api_v1_aliases_return_structured_bad_requests_when_json_is_malformed() -> TestResult {
    // Given: aliased runtime POST routes parse JSON at the boundary.
    let routes = [
        "/api/v1/chat/completions",
        "/api/v1/api/chat",
        "/api/v1/embeddings",
        "/api/v1beta/models/gemini/gemini-2.5-pro:generateContent",
    ];

    // When: malformed JSON is posted to each alias.
    for route in routes {
        let response = request(Method::POST, route, "{").await?;
        let json = parse_json(&response.body)?;

        // Then: every alias returns structured JSON 400 rather than HTML.
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{route}");
        assert!(
            response.content_type.starts_with("application/json"),
            "{route}"
        );
        assert_eq!(error_message(&json)?, "Invalid JSON body", "{route}");
    }
    Ok(())
}

#[actix_rt::test]
async fn api_v1_alias_options_preflight_returns_cors_no_content() -> TestResult {
    // Given: browser callers preflight aliased runtime endpoints.
    let routes = [
        "/api/v1/chat/completions",
        "/api/v1/api/chat",
        "/api/v1beta/models",
    ];

    // When: OPTIONS requests are sent.
    for route in routes {
        let response = request(Method::OPTIONS, route, "").await?;

        // Then: the runtime alias responds with CORS no-content.
        assert_eq!(response.status, StatusCode::NO_CONTENT, "{route}");
        assert_eq!(response.body, "", "{route}");
    }
    Ok(())
}

#[actix_rt::test]
async fn api_v1_unknown_aliases_return_structured_json_not_html() -> TestResult {
    // Given: clients can probe unknown paths under the public /api/v1 prefix.

    // When: an unknown alias path is requested.
    let response = request(Method::GET, "/api/v1/not-a-real-route", "").await?;
    let json = parse_json(&response.body)?;

    // Then: the API surface returns a JSON 404 envelope instead of framework HTML.
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert!(response.content_type.starts_with("application/json"));
    assert_eq!(error_message(&json)?, "Runtime route not found");
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
