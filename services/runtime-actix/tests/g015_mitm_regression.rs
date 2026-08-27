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
    let request = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
        .to_request();
    let response = test::call_service(&app, request).await;
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = String::from_utf8(to_bytes(response.into_body()).await?.to_vec())?;

    Ok(RuntimeResponse {
        status,
        content_type,
        body,
    })
}

fn parse_json(body: &str) -> TestResult<Value> {
    Ok(serde_json::from_str(body)?)
}

fn assert_not_html(route: &str, response: &RuntimeResponse) {
    let lower_body = response.body.to_ascii_lowercase();
    assert!(!response.content_type.starts_with("text/html"), "{route}");
    assert!(!lower_body.contains("<!doctype"), "{route}");
    assert!(!lower_body.contains("<html"), "{route}");
}

#[actix_rt::test]
async fn g015_mitm_paths_are_not_runtime_owned() -> TestResult {
    // Given: MITM routes are owned by the API upstream, not the runtime service.
    let routes = [
        "/api/cli-tools/antigravity-mitm",
        "/api/cli-tools/antigravity-mitm/alias",
    ];

    // When: the runtime service receives direct and alias MITM paths.
    for route in routes {
        let response = request(Method::GET, route, "").await?;

        // Then: both paths are 404s rather than HTML fallbacks.
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{route}");
        assert_not_html(route, &response);
    }
    Ok(())
}

#[actix_rt::test]
async fn g015_models_route_remains_openai_json() -> TestResult {
    // Given: callers use the runtime's OpenAI-compatible model discovery route.

    // When: the model list is requested.
    let response = request(Method::GET, "/v1/models", "").await?;
    let json = parse_json(&response.body)?;

    // Then: the route remains a successful structured JSON response.
    assert_eq!(response.status, StatusCode::OK);
    assert!(response.content_type.starts_with("application/json"));
    assert_eq!(json.get("object"), Some(&Value::String("list".to_owned())));
    assert!(
        json.get("data")
            .and_then(Value::as_array)
            .is_some_and(|data| { !data.is_empty() })
    );
    assert_not_html("/v1/models", &response);
    Ok(())
}

#[actix_rt::test]
async fn g015_malformed_chat_body_keeps_structured_error_contract() -> TestResult {
    // Given: the chat route receives malformed JSON at the runtime boundary.

    // When: an invalid JSON body is posted.
    let response = request(Method::POST, "/v1/chat/completions", "{").await?;
    let json = parse_json(&response.body)?;

    // Then: malformed input remains a JSON 400 response, not framework HTML.
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(response.content_type.starts_with("application/json"));
    assert_eq!(
        json.pointer("/error/message"),
        Some(&Value::String("Invalid JSON body".to_owned()))
    );
    assert_not_html("/v1/chat/completions", &response);
    Ok(())
}
