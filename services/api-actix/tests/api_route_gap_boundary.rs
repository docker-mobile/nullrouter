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

async fn request_json(method: Method, uri: &str, body: &str) -> TestResult<(StatusCode, Value)> {
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
    let body = to_bytes(res.into_body()).await?;
    let json = serde_json::from_slice(&body)?;
    Ok((status, json))
}

async fn request_empty(
    method: Method,
    uri: &str,
) -> TestResult<(StatusCode, Vec<(String, String)>)> {
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
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let headers = res
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), value.to_owned()))
        })
        .collect();
    Ok((status, headers))
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

fn has_cors_header(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(name, value)| name == "access-control-allow-origin" && value == "*")
}

#[actix_rt::test]
async fn gap_routes_reject_malformed_or_missing_inputs_as_json() -> TestResult {
    // Given: browser clients can send malformed or incomplete JSON to the new gap routes.

    // When: representative mutating routes receive malformed or missing inputs.
    let malformed_routes = [
        "/api/locale",
        "/api/oauth/codex/import-token",
        "/api/providers/openai/test",
        "/api/providers/openai/test-models",
        "/api/providers/test-batch",
        "/api/proxy-pools/pool/test",
        "/api/proxy-pools/vercel-deploy",
        "/api/models/test",
    ];
    for uri in malformed_routes {
        let (status, json) = request_json(Method::POST, uri, "{").await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(field(&json, "error")?, "Invalid JSON body", "{uri}");
    }

    let (locale_missing_status, locale_missing) =
        request_json(Method::POST, "/api/locale", "{}").await?;
    let (batch_missing_status, batch_missing) =
        request_json(Method::POST, "/api/providers/test-batch", "{}").await?;
    let (request_details_status, request_details) =
        request_json(Method::GET, "/api/usage/request-details?page=0", "").await?;

    // Then: validation failures stay structured and use upstream-compatible status codes.
    assert_eq!(locale_missing_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&locale_missing, "error")?, "Invalid locale");
    assert_eq!(batch_missing_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&batch_missing, "error")?, "mode is required");
    assert_eq!(request_details_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&request_details, "error")?, "Page must be >= 1");
    Ok(())
}

#[actix_rt::test]
async fn gap_routes_support_cors_options_for_browser_calls() -> TestResult {
    // Given: dashboard browser calls preflight API routes before mutating them.

    // When: OPTIONS requests hit the new route families.
    for uri in [
        "/api/locale",
        "/api/oauth/codex/import-token",
        "/api/providers/openai/test",
        "/api/proxy-pools/pool/test",
        "/api/models/test",
        "/api/usage/connection_1/codex-reset-credits",
    ] {
        let (status, headers) = request_empty(Method::OPTIONS, uri).await?;

        // Then: the route answers no-content with the shared CORS headers.
        assert_eq!(status, StatusCode::NO_CONTENT, "{uri}");
        assert!(has_cors_header(&headers), "{uri}");
    }
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
