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
    body: String,
    json: Value,
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
    let body_bytes = to_bytes(res.into_body()).await?;
    let body = std::str::from_utf8(&body_bytes)?.to_owned();
    let json = serde_json::from_slice(&body_bytes)?;

    Ok(JsonResponse {
        status,
        content_type,
        body,
        json,
    })
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

fn assert_structured_json(response: &JsonResponse) {
    assert!(
        response.content_type.starts_with("application/json"),
        "content-type was {}",
        response.content_type
    );
    assert!(!response.body.contains("<html"), "body was HTML");
    assert!(!response.body.contains("<!DOCTYPE"), "body was HTML");
}

fn has_cors_header(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(name, value)| name == "access-control-allow-origin" && value == "*")
}

#[actix_rt::test]
async fn translator_load_rejects_missing_and_invalid_file_as_structured_json() -> TestResult {
    // Given: the upstream load route accepts only a specific translator log filename allow-list.

    // When: the file query is missing or outside the allow-list.
    for uri in [
        "/api/translator/load",
        "/api/translator/load?file=../secret.json",
    ] {
        let response = request_json(Method::GET, uri, "").await?;

        // Then: the route returns a structured 400 JSON error, not HTML.
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{uri}");
        assert_structured_json(&response);
        assert_eq!(
            field(&response.json, "error")?,
            "Valid file parameter required",
            "{uri}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn translator_save_rejects_invalid_requests_as_structured_json() -> TestResult {
    // Given: save requires an allow-listed file plus content.
    let cases = [
        (
            r#"{"file":"1_req_client.json"}"#,
            "File and content required",
        ),
        (
            r#"{"file":"../secret.json","content":"{}"}"#,
            "File and content required",
        ),
        ("{", "Invalid JSON body"),
    ];

    for (body, expected_error) in cases {
        // When: a malformed or incomplete save request arrives.
        let response = request_json(Method::POST, "/api/translator/save", body).await?;

        // Then: it returns structured 400 JSON with no framework HTML fallback.
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{body}");
        assert_structured_json(&response);
        assert_eq!(field(&response.json, "error")?, expected_error, "{body}");
    }
    Ok(())
}

#[actix_rt::test]
async fn translator_translate_rejects_invalid_requests_as_structured_json() -> TestResult {
    // Given: translate requires JSON with a body object and a step of 1, 2, 3 — or 5, this
    // port's own response step, which upstream leaves to hand-pasting.
    //
    // Every case here is rejected at the boundary, before any proxying, so these stay 400s
    // rather than becoming 503s about a runtime the request never reached.
    let cases = [
        (r#"{"body":{}}"#, "Step and body required"),
        (r#"{"step":1}"#, "Step and body required"),
        (
            r#"{"step":4,"body":{}}"#,
            "Invalid step (1-3, or 5 for a response)",
        ),
        (
            r#"{"step":6,"body":{}}"#,
            "Invalid step (1-3, or 5 for a response)",
        ),
        ("{", "Invalid JSON body"),
    ];

    for (body, expected_error) in cases {
        // When: an invalid translate request arrives.
        let response = request_json(Method::POST, "/api/translator/translate", body).await?;

        // Then: it returns structured 400 JSON with no framework HTML fallback.
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{body}");
        assert_structured_json(&response);
        assert_eq!(field(&response.json, "error")?, expected_error, "{body}");
    }
    Ok(())
}

#[actix_rt::test]
async fn translator_send_rejects_invalid_requests_as_structured_json() -> TestResult {
    // Given: send requires provider, model, and translated body.
    let cases = [
        (
            r#"{"model":"gpt-5","body":{}}"#,
            "provider, model, and body required",
        ),
        (
            r#"{"provider":"openai","body":{}}"#,
            "provider, model, and body required",
        ),
        (
            r#"{"provider":"openai","model":"gpt-5"}"#,
            "provider, model, and body required",
        ),
        ("{", "Invalid JSON body"),
    ];

    for (body, expected_error) in cases {
        // When: an invalid send request arrives.
        let response = request_json(Method::POST, "/api/translator/send", body).await?;

        // Then: it returns structured 400 JSON with no framework HTML fallback.
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{body}");
        assert_structured_json(&response);
        assert_eq!(field(&response.json, "error")?, expected_error, "{body}");
    }
    Ok(())
}

#[actix_rt::test]
async fn translator_mutating_routes_support_cors_options() -> TestResult {
    // Given: dashboard browser calls preflight mutating translator routes.

    // When: OPTIONS requests hit those endpoints.
    for uri in [
        "/api/translator/save",
        "/api/translator/translate",
        "/api/translator/send",
    ] {
        let (status, headers) = request_empty(Method::OPTIONS, uri).await?;

        // Then: the route answers no-content with shared CORS headers.
        assert_eq!(status, StatusCode::NO_CONTENT, "{uri}");
        assert!(has_cors_header(&headers), "{uri}");
    }
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
