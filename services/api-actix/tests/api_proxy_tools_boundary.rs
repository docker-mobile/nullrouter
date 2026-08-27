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
struct JsonResponse {
    status: StatusCode,
    content_type: String,
    json: Value,
}

#[derive(Debug)]
struct EmptyResponse {
    status: StatusCode,
    headers: Vec<(String, String)>,
}

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

async fn request_json(method: Method, uri: &str, body: &str) -> TestResult<JsonResponse> {
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
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = to_bytes(res.into_body()).await?;
    let json = serde_json::from_slice(&body)?;

    Ok(JsonResponse {
        status,
        content_type,
        json,
    })
}

async fn request_empty(method: Method, uri: &str) -> TestResult<EmptyResponse> {
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

    Ok(EmptyResponse { status, headers })
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> TestResult<&'a str> {
    headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| test_error(format!("missing header {name}")))
}

fn assert_json_content_type(response: &JsonResponse) {
    assert!(
        response.content_type.starts_with("application/json"),
        "content-type was {}",
        response.content_type
    );
}

#[actix_rt::test]
async fn proxy_pool_tool_routes_reject_malformed_json_as_json_errors() -> TestResult {
    // Given: browser callers can submit malformed JSON to proxy helper routes.
    let routes = [
        "/api/proxy-pools/pool-1/test",
        "/api/proxy-pools/vercel-deploy",
        "/api/proxy-pools/cloudflare-deploy",
        "/api/proxy-pools/deno-deploy",
    ];

    for uri in routes {
        // When: a malformed JSON body reaches the route boundary.
        let response = request_json(Method::POST, uri, "{").await?;

        // Then: the route returns a structured 400 JSON error.
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{uri}");
        assert_json_content_type(&response);
        assert_eq!(
            field(&response.json, "error")?,
            "Invalid JSON body",
            "{uri}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn proxy_pool_deploy_routes_require_upstream_credentials() -> TestResult {
    // Given: deploy requests are missing one required upstream credential field.
    let cases = [
        (
            "/api/proxy-pools/vercel-deploy",
            "{}",
            "Vercel API token is required",
        ),
        (
            "/api/proxy-pools/cloudflare-deploy",
            r#"{"apiToken":"token"}"#,
            "Cloudflare Account ID and API Token are required",
        ),
        (
            "/api/proxy-pools/cloudflare-deploy",
            r#"{"accountId":"account"}"#,
            "Cloudflare Account ID and API Token are required",
        ),
        (
            "/api/proxy-pools/deno-deploy",
            r#"{"denoToken":"token"}"#,
            "Organization domain is required",
        ),
        (
            "/api/proxy-pools/deno-deploy",
            r#"{"orgDomain":"example.com"}"#,
            "Deno Deploy API token is required",
        ),
    ];

    for (uri, body, error) in cases {
        // When: a required credential field is absent.
        let response = request_json(Method::POST, uri, body).await?;

        // Then: validation fails before the unsupported deploy response.
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{uri}");
        assert_json_content_type(&response);
        assert_eq!(field(&response.json, "error")?, error, "{uri}");
    }
    Ok(())
}

#[actix_rt::test]
async fn proxy_pool_tool_routes_answer_cors_preflight() -> TestResult {
    // Given: dashboard clients preflight proxy helper routes before POSTing.
    let routes = [
        "/api/proxy-pools/pool-1/test",
        "/api/proxy-pools/vercel-deploy",
        "/api/proxy-pools/cloudflare-deploy",
        "/api/proxy-pools/deno-deploy",
    ];

    for uri in routes {
        // When: OPTIONS is requested.
        let response = request_empty(Method::OPTIONS, uri).await?;

        // Then: the route returns the shared no-content CORS response.
        assert_eq!(response.status, StatusCode::NO_CONTENT, "{uri}");
        assert_eq!(
            header_value(
                &response.headers,
                header::ACCESS_CONTROL_ALLOW_ORIGIN.as_str(),
            )?,
            "*",
            "{uri}"
        );
        assert_eq!(
            header_value(
                &response.headers,
                header::ACCESS_CONTROL_ALLOW_METHODS.as_str(),
            )?,
            "GET, POST, PUT, PATCH, DELETE, OPTIONS",
            "{uri}"
        );
        assert_eq!(
            header_value(
                &response.headers,
                header::ACCESS_CONTROL_ALLOW_HEADERS.as_str(),
            )?,
            "*",
            "{uri}"
        );
    }
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
