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

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

struct ApiResponse {
    status: StatusCode,
    body: String,
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
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok(ApiResponse { status, body })
}

async fn request_json(method: Method, uri: &str, body: &str) -> TestResult<(StatusCode, Value)> {
    let response = request(method, uri, body).await?;
    Ok((response.status, serde_json::from_str(&response.body)?))
}

async fn get_json(uri: &str) -> TestResult<(StatusCode, Value)> {
    request_json(Method::GET, uri, "").await
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

#[actix_rt::test]
async fn cowork_mcp_routes_return_deterministic_json_defaults() -> TestResult {
    // Given: nullrouter-api does not perform live external MCP discovery in tests.

    // When: the upstream Cowork MCP helper routes are requested.
    let (registry_status, registry) = get_json("/api/cli-tools/cowork-mcp-registry").await?;
    let (tools_status, tools) = request_json(
        Method::POST,
        "/api/cli-tools/cowork-mcp-tools",
        r#"{"url":"https://example.invalid/mcp"}"#,
    )
    .await?;

    // Then: both routes return deterministic JSON defaults rather than generic CLI 404s.
    assert_eq!(registry_status, StatusCode::OK);
    assert_eq!(field(&registry, "servers")?, &serde_json::json!([]));
    assert_eq!(field(&registry, "total")?, 0);
    assert_eq!(tools_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&tools, "tools")?, &serde_json::json!([]));
    assert_eq!(field(&tools, "requiresAuth")?, false);
    assert_eq!(field(&tools, "unsupported")?, true);
    Ok(())
}

#[actix_rt::test]
async fn cowork_mcp_routes_validate_json_and_options_boundaries() -> TestResult {
    // Given: Cowork MCP tool probing accepts only JSON with a non-empty url.

    // When: malformed, missing, and preflight requests are sent.
    let malformed = request_json(Method::POST, "/api/cli-tools/cowork-mcp-tools", "{").await?;
    let missing = request_json(
        Method::POST,
        "/api/cli-tools/cowork-mcp-tools",
        r#"{"url":""}"#,
    )
    .await?;
    let tools_options = request(Method::OPTIONS, "/api/cli-tools/cowork-mcp-tools", "").await?;
    let registry_options =
        request(Method::OPTIONS, "/api/cli-tools/cowork-mcp-registry", "").await?;

    // Then: boundaries are explicit structured JSON or CORS no-content.
    assert_eq!(malformed.0, StatusCode::BAD_REQUEST);
    assert_eq!(field(&malformed.1, "error")?, "Invalid JSON body");
    assert_eq!(missing.0, StatusCode::BAD_REQUEST);
    assert_eq!(field(&missing.1, "error")?, "url required");
    assert_eq!(tools_options.status, StatusCode::NO_CONTENT);
    assert_eq!(registry_options.status, StatusCode::NO_CONTENT);
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
