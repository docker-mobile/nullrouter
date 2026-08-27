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

#[actix_rt::test]
async fn package_metadata_uses_nullrouter_api_names() {
    // Given: the API crate is built as a Rust microservice.
    let package_name = env!("CARGO_PKG_NAME");
    let api_binary = option_env!("CARGO_BIN_EXE_nullrouter-api");

    // When: Cargo exposes package and binary metadata to integration tests.

    // Then: both public names follow the nullrouter-* service naming convention.
    assert_eq!(package_name, "nullrouter-api");
    assert!(api_binary.is_some());
}

async fn get_json(uri: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::get().uri(uri).to_request();

    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body = to_bytes(res.into_body()).await?;
    let json = serde_json::from_slice(&body)?;

    Ok((status, json))
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

fn nested_field<'a>(json: &'a Value, first: &str, second: &str) -> TestResult<&'a Value> {
    field(field(json, first)?, second)
}

#[actix_rt::test]
async fn health_returns_ok_when_called() -> TestResult {
    let (status, json) = get_json("/api/health").await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, serde_json::json!({ "ok": true }));
    Ok(())
}

#[actix_rt::test]
async fn auth_status_is_not_owned_by_api_service() -> TestResult {
    let (status, json) = get_json("/api/auth/status").await?;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json, serde_json::json!({ "error": "Route not found" }));
    Ok(())
}

#[actix_rt::test]
async fn models_are_openai_compatible_when_requested() -> TestResult {
    let (status, json) = get_json("/v1/models").await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(field(&json, "object")?, "list");
    let models = field(&json, "data")?
        .as_array()
        .ok_or_else(|| test_error("models data is an array"))?;
    let first_model = models
        .first()
        .ok_or_else(|| test_error("static model list is not empty"))?;
    assert_eq!(field(first_model, "object")?, "model");
    assert!(
        field(first_model, "id")?
            .as_str()
            .is_some_and(|id| id.contains('/'))
    );
    assert!(field(first_model, "owned_by")?.is_string());
    Ok(())
}

#[actix_rt::test]
async fn chat_rejects_missing_model_when_body_is_empty_object() -> TestResult {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::post()
        .uri("/v1/chat/completions")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload("{}")
        .to_request();

    let res = test::call_service(&app, req).await;

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let json: Value = test::read_body_json(res).await;
    assert_eq!(
        nested_field(&json, "error", "type")?,
        "invalid_request_error"
    );
    assert_eq!(
        nested_field(&json, "error", "message")?,
        "Missing required field: model"
    );
    Ok(())
}

#[actix_rt::test]
async fn chat_rejects_invalid_json_with_structured_error() -> TestResult {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::post()
        .uri("/v1/chat/completions")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload("{")
        .to_request();

    let res = test::call_service(&app, req).await;

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let json: Value = test::read_body_json(res).await;
    assert_eq!(
        nested_field(&json, "error", "type")?,
        "invalid_request_error"
    );
    assert_eq!(
        nested_field(&json, "error", "message")?,
        "Invalid JSON body"
    );
    Ok(())
}

#[actix_rt::test]
async fn valid_chat_request_returns_explicit_provider_execution_stub() -> TestResult {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::post()
        .uri("/v1/chat/completions")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(r#"{"model":"openai/gpt-5","messages":[],"stream":false}"#)
        .to_request();

    let res = test::call_service(&app, req).await;

    assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
    let json: Value = test::read_body_json(res).await;
    assert_eq!(nested_field(&json, "error", "type")?, "not_implemented");
    assert_eq!(
        nested_field(&json, "error", "code")?,
        "provider_execution_unimplemented"
    );
    assert_eq!(nested_field(&json, "error", "model")?, "openai/gpt-5");
    assert_eq!(nested_field(&json, "error", "stream")?, false);
    Ok(())
}

#[actix_rt::test]
async fn streaming_chat_request_returns_sse_error_frame() -> TestResult {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::post()
        .uri("/v1/chat/completions")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(r#"{"model":"openai/gpt-5","messages":[],"stream":true}"#)
        .to_request();

    let res = test::call_service(&app, req).await;

    assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        res.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.starts_with("text/event-stream")),
        Some(true)
    );
    let body = to_bytes(res.into_body()).await?;
    let text = std::str::from_utf8(&body)?;
    assert!(text.contains("data: {\"error\""));
    assert!(text.contains("\"provider_execution_unimplemented\""));
    assert!(text.ends_with("data: [DONE]\n\n"));
    Ok(())
}

#[actix_rt::test]
async fn streaming_responses_and_messages_share_sse_entrypoint() -> TestResult {
    for uri in ["/v1/responses", "/v1/messages"] {
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(app_config()))
                .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
                .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
                .configure(configure),
        )
        .await;
        let req = test::TestRequest::post()
            .uri(uri)
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .set_payload(r#"{"model":"openai/gpt-5","messages":[],"stream":true}"#)
            .to_request();

        let res = test::call_service(&app, req).await;

        assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED, "{uri}");
        assert_eq!(
            res.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.starts_with("text/event-stream")),
            Some(true),
            "{uri}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn requested_bootstrap_routes_return_expected_shapes() -> TestResult {
    let routes = [
        ("/api/status", "ok"),
        ("/api/models", "models"),
        // `requireLogin` was removed deliberately: login is now always
        // required, so there is no setting to report. `tunnelDashboardAccess`
        // stands in as a stable settings field.
        ("/api/settings", "tunnelDashboardAccess"),
        ("/api/keys", "keys"),
        ("/api/providers/client", "connections"),
    ];

    for (uri, required_field) in routes {
        let (status, json) = get_json(uri).await?;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            json.get(required_field).is_some(),
            "{uri} response should include {required_field}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn health_options_returns_no_content_with_cors_headers() -> TestResult {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::OPTIONS)
        .uri("/api/health")
        .to_request();

    let res = test::call_service(&app, req).await;

    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        res.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&header::HeaderValue::from_static("*"))
    );
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
