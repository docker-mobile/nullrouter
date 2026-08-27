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

struct TestResponse {
    status: StatusCode,
    content_type: Option<String>,
    cors_origin: Option<String>,
    body: Vec<u8>,
}

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

async fn request(method: Method, uri: &str, body: &str) -> TestResult<TestResponse> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
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
    let content_type = header_value(&response, header::CONTENT_TYPE);
    let cors_origin = header_value(&response, header::ACCESS_CONTROL_ALLOW_ORIGIN);
    let body = to_bytes(response.into_body()).await?.to_vec();

    Ok(TestResponse {
        status,
        content_type,
        cors_origin,
        body,
    })
}

fn header_value<B>(
    response: &actix_web::dev::ServiceResponse<B>,
    name: header::HeaderName,
) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn json(response: &TestResponse) -> TestResult<Value> {
    Ok(serde_json::from_slice(&response.body)?)
}

fn assert_json_not_found(response: &TestResponse, uri: &str) -> TestResult {
    assert_eq!(response.status, StatusCode::NOT_FOUND, "{uri}");
    assert!(
        response
            .content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("application/json")),
        "{uri}: expected JSON content type, got {:?}",
        response.content_type
    );
    assert_eq!(response.cors_origin.as_deref(), Some("*"), "{uri}");

    let body = std::str::from_utf8(&response.body)?;
    let lowercase_body = body.to_ascii_lowercase();
    assert!(!lowercase_body.contains("<html"), "{uri}: {body}");
    assert!(!lowercase_body.contains("<!doctype"), "{uri}: {body}");
    assert_eq!(
        json(response)?,
        serde_json::json!({ "error": "Route not found" })
    );
    Ok(())
}

#[actix_rt::test]
async fn api_health_and_models_remain_owned_by_api_service() -> TestResult {
    // Given: health and dashboard models are established nullrouter-api routes.

    // When: clients call both routes through the API service router.
    let health = request(Method::GET, "/api/health", "").await?;
    let models = request(Method::GET, "/api/models", "").await?;

    // Then: both existing contracts remain available as JSON.
    assert_eq!(health.status, StatusCode::OK);
    assert_eq!(json(&health)?, serde_json::json!({ "ok": true }));
    assert_eq!(models.status, StatusCode::OK);
    assert!(json(&models)?.get("models").is_some_and(Value::is_array));
    Ok(())
}

#[actix_rt::test]
async fn auth_routes_are_absent_and_return_json_not_found() -> TestResult {
    // Given: nullrouter-auth exclusively owns every /api/auth route.
    let requests = [
        (Method::GET, "/api/auth", ""),
        (Method::GET, "/api/auth/status", ""),
        (Method::POST, "/api/auth/login", r#"{"password":"bad"}"#),
        (Method::POST, "/api/auth/login", "{"),
        (Method::OPTIONS, "/api/auth/login", ""),
        (Method::GET, "/api/auth/oidc/start", ""),
    ];

    // When: direct API-port requests probe former cosmetic auth routes.
    for (method, uri, body) in requests {
        let response = request(method, uri, body).await?;

        // Then: the shared API fallback responds without parsing or serving auth behavior.
        assert_json_not_found(&response, uri)?;
    }
    Ok(())
}

#[actix_rt::test]
async fn unknown_routes_use_structured_json_fallback() -> TestResult {
    // Given: the requested path is not owned by nullrouter-api.

    // When: a client requests the unknown path directly from the API router.
    let response = request(Method::GET, "/api/not-owned-by-nullrouter-api", "").await?;

    // Then: the service returns its shared JSON fallback rather than framework text or HTML.
    assert_json_not_found(&response, "/api/not-owned-by-nullrouter-api")
}
