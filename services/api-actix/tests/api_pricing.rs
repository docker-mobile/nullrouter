#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::Value;

use nullrouter_api::{AppConfig, RuntimeClient, StateClient, TunnelManager, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A closed loopback port: usage reads fall back to the zeroed shape,
/// so these parity tests need no state service.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

struct ApiResponse {
    status: StatusCode,
    content_type: String,
    headers: actix_web::http::header::HeaderMap,
    body: Vec<u8>,
}

async fn request(method: Method, uri: &str, body: &str) -> TestResult<ApiResponse> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(TunnelManager::new()))
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
    let body = to_bytes(res.into_body())
        .await
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?
        .to_vec();
    Ok(ApiResponse {
        status,
        content_type,
        headers,
        body,
    })
}

async fn request_json(
    method: Method,
    uri: &str,
    body: &str,
) -> TestResult<(StatusCode, Value, String)> {
    let response = request(method, uri, body).await?;
    let json = serde_json::from_slice(&response.body)?;
    Ok((response.status, json, response.content_type))
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

#[actix_rt::test]
async fn pricing_routes_return_upstream_compatible_default_json() -> TestResult {
    // Given: no custom pricing has been persisted.

    // When: the pricing collection is fetched, patched with an empty object, and reset.
    let (get_status, get_body, get_content_type) =
        request_json(Method::GET, "/api/pricing", "").await?;
    let (patch_status, patch_body, patch_content_type) =
        request_json(Method::PATCH, "/api/pricing", "{}").await?;
    let (delete_status, delete_body, delete_content_type) =
        request_json(Method::DELETE, "/api/pricing", "").await?;

    // Then: GET/DELETE return merged defaults, while PATCH returns user pricing.
    assert_eq!(get_status, StatusCode::OK);
    assert!(get_content_type.starts_with("application/json"));
    assert_eq!(
        field(field(&get_body, "gh")?, "gpt-5.3-codex")?,
        &serde_json::json!({
            "input": 1.75,
            "output": 14.0,
            "cached": 0.175,
            "reasoning": 14.0,
            "cache_creation": 1.75
        })
    );
    assert_eq!(patch_status, StatusCode::OK);
    assert!(patch_content_type.starts_with("application/json"));
    assert!(
        patch_body
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
    );
    assert_eq!(delete_status, StatusCode::OK);
    assert!(delete_content_type.starts_with("application/json"));
    assert_eq!(
        field(field(&delete_body, "gh")?, "gpt-5.3-codex")?["cached"],
        0.175
    );
    Ok(())
}

#[actix_rt::test]
async fn pricing_routes_keep_boundary_errors_json_and_cors_safe() -> TestResult {
    // Given: browser clients may preflight or send bad pricing editor payloads.

    // When: malformed, unsupported, and OPTIONS requests hit /api/pricing.
    let (malformed_status, malformed, malformed_content_type) =
        request_json(Method::PATCH, "/api/pricing", "{").await?;
    let (unsupported_status, unsupported, unsupported_content_type) =
        request_json(Method::PATCH, "/api/pricing", "[]").await?;
    let options = request(Method::OPTIONS, "/api/pricing", "").await?;

    // Then: pricing never falls through to HTML and the preflight is browser-safe.
    assert_eq!(malformed_status, StatusCode::BAD_REQUEST);
    assert!(malformed_content_type.starts_with("application/json"));
    assert_eq!(
        malformed.get("error"),
        Some(&Value::String("Invalid JSON body".to_owned()))
    );
    assert_eq!(unsupported_status, StatusCode::BAD_REQUEST);
    assert!(unsupported_content_type.starts_with("application/json"));
    assert_eq!(
        unsupported.get("error"),
        Some(&Value::String("Invalid pricing data format".to_owned()))
    );
    assert_eq!(options.status, StatusCode::NO_CONTENT);
    assert!(options.body.is_empty());
    assert_eq!(
        options
            .headers
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|value| value.to_str().ok()),
        Some("*")
    );
    assert_eq!(
        options
            .headers
            .get(header::ACCESS_CONTROL_ALLOW_METHODS)
            .and_then(|value| value.to_str().ok()),
        Some("GET, POST, PUT, PATCH, DELETE, OPTIONS")
    );
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
