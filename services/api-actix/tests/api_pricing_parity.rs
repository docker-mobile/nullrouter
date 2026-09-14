#![allow(clippy::future_not_send)]

use actix_web::{
    App,
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
    let json = test::read_body_json(res).await;
    Ok((status, json))
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

#[actix_rt::test]
async fn pricing_routes_match_the_stateful_json_contract() -> TestResult {
    // Given: pricing starts with the upstream provider defaults for this Actix app instance.
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(TunnelManager::new()))
            .configure(configure),
    )
    .await;
    let initial_req = test::TestRequest::get().uri("/api/pricing").to_request();
    let initial_res = test::call_service(&app, initial_req).await;

    // When: pricing is read, patched, read again, and then selectively reset.
    assert_eq!(initial_res.status(), StatusCode::OK);
    let initial: Value = test::read_body_json(initial_res).await;
    assert_eq!(
        field(field(&initial, "gh")?, "gpt-5.3-codex")?,
        &serde_json::json!({
            "input": 1.75,
            "output": 14.0,
            "cached": 0.175,
            "reasoning": 14.0,
            "cache_creation": 1.75
        })
    );

    let patch_req = test::TestRequest::patch()
        .uri("/api/pricing")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(
            r#"{"openai":{"gpt-5":{"input":3,"output":12}},"gh":{"custom-codex":{"input":9}}}"#,
        )
        .to_request();
    let patch_res = test::call_service(&app, patch_req).await;
    assert_eq!(patch_res.status(), StatusCode::OK);
    let patched: Value = test::read_body_json(patch_res).await;
    assert_eq!(field(field(&patched, "openai")?, "gpt-5")?["input"], 3);
    assert_eq!(field(field(&patched, "gh")?, "custom-codex")?["input"], 9);
    assert!(field(field(&patched, "gh")?, "gpt-5.3-codex").is_err());

    let merged_req = test::TestRequest::get().uri("/api/pricing").to_request();
    let merged_res = test::call_service(&app, merged_req).await;
    assert_eq!(merged_res.status(), StatusCode::OK);
    let merged: Value = test::read_body_json(merged_res).await;
    assert_eq!(field(field(&merged, "openai")?, "gpt-5")?["output"], 12);
    assert_eq!(field(field(&merged, "gh")?, "custom-codex")?["input"], 9);
    assert_eq!(
        field(field(&merged, "gh")?, "gpt-5.3-codex")?["output"],
        14.0
    );

    let reset_model_req = test::TestRequest::delete()
        .uri("/api/pricing?provider=gh&model=custom-codex")
        .to_request();
    let reset_model_res = test::call_service(&app, reset_model_req).await;
    assert_eq!(reset_model_res.status(), StatusCode::OK);
    let reset_model: Value = test::read_body_json(reset_model_res).await;
    assert!(field(field(&reset_model, "gh")?, "custom-codex").is_err());
    assert_eq!(field(field(&reset_model, "openai")?, "gpt-5")?["input"], 3);

    let reset_all_req = test::TestRequest::delete().uri("/api/pricing").to_request();
    let reset_all_res = test::call_service(&app, reset_all_req).await;
    assert_eq!(reset_all_res.status(), StatusCode::OK);
    let reset_all: Value = test::read_body_json(reset_all_res).await;
    assert_eq!(
        field(field(&reset_all, "gh")?, "gpt-5.3-codex")?["cached"],
        0.175
    );
    assert!(field(&reset_all, "openai").is_err());
    Ok(())
}

#[actix_rt::test]
async fn pricing_patch_rejects_invalid_shapes_with_structured_errors() -> TestResult {
    // Given: browser clients can submit malformed pricing structures.
    let cases = [
        ("null", "Invalid pricing data format"),
        (r#"{"openai":42}"#, "Invalid pricing for provider: openai"),
        (
            r#"{"openai":{"gpt-5":null}}"#,
            "Invalid pricing for model: openai/gpt-5",
        ),
        (
            r#"{"openai":{"gpt-5":{"prompt":1}}}"#,
            "Invalid pricing field: prompt for openai/gpt-5",
        ),
        (
            r#"{"openai":{"gpt-5":{"input":-1}}}"#,
            "Invalid pricing value for input in openai/gpt-5: must be non-negative number",
        ),
    ];

    // When: each body is PATCHed to /api/pricing.
    for (body, expected_error) in cases {
        let (status, json) = request_json(Method::PATCH, "/api/pricing", body).await?;

        // Then: each boundary failure returns the upstream-compatible JSON error.
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(field(&json, "error")?, expected_error, "{body}");
    }
    Ok(())
}

#[actix_rt::test]
async fn pricing_options_returns_no_content_with_cors_headers() -> TestResult {
    // Given: dashboard clients may preflight the pricing route.
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
        .method(Method::OPTIONS)
        .uri("/api/pricing")
        .to_request();

    // When: OPTIONS /api/pricing is requested.
    let res = test::call_service(&app, req).await;

    // Then: it returns the shared no-content CORS response.
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
