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

#[actix_rt::test]
async fn translator_load_returns_structured_json_for_allowed_file() -> TestResult {
    // Given: the upstream translator dashboard loads one of the allowed replay log files.

    // When: nullrouter-api receives the load request without local translator persistence.
    let response = request_json(
        Method::GET,
        "/api/translator/load?file=1_req_client.json",
        "",
    )
    .await?;

    // Then: the route returns a structured JSON not-found stub instead of HTML.
    assert_eq!(response.status, StatusCode::OK);
    assert_structured_json(&response);
    assert_eq!(field(&response.json, "success")?, false);
    assert_eq!(field(&response.json, "content")?, &Value::Null);
    assert_eq!(field(&response.json, "error")?, "File not found");
    Ok(())
}

#[actix_rt::test]
async fn translator_save_returns_explicit_unsupported_json_for_valid_input() -> TestResult {
    // Given: the dashboard sends a valid translator replay log save request.

    // When: nullrouter-api handles it without local translator log persistence.
    let response = request_json(
        Method::POST,
        "/api/translator/save",
        r#"{"file":"1_req_client.json","content":"{}"}"#,
    )
    .await?;

    // Then: the response is structured JSON that explicitly declares persistence unsupported.
    assert_eq!(response.status, StatusCode::OK);
    assert_structured_json(&response);
    assert_eq!(field(&response.json, "success")?, false);
    assert_eq!(field(&response.json, "unsupported")?, true);
    assert_eq!(
        field(&response.json, "error")?,
        "Translator log persistence is not supported"
    );
    Ok(())
}

#[actix_rt::test]
async fn translator_translate_accepts_supported_steps_with_default_metadata() -> TestResult {
    // Given: the dashboard can replay the three upstream translator stages.

    for step in [1, 2, 3] {
        // When: a supported translator step is requested.
        let body = serde_json::json!({
            "step": step,
            "provider": "openai",
            "model": "gpt-5",
            "body": { "model": "openai/gpt-5" }
        })
        .to_string();
        let response = request_json(Method::POST, "/api/translator/translate", &body).await?;

        // Then: the route returns success plus deterministic default translation metadata.
        assert_eq!(response.status, StatusCode::OK, "step {step}");
        assert_structured_json(&response);
        assert_eq!(field(&response.json, "success")?, true, "step {step}");
        let result = field(&response.json, "result")?;
        assert_eq!(field(result, "step")?, step, "step {step}");
        assert_eq!(field(result, "provider")?, "openai", "step {step}");
        assert_eq!(field(result, "model")?, "gpt-5", "step {step}");
        assert_eq!(field(result, "sourceFormat")?, "unknown", "step {step}");
        assert_eq!(field(result, "targetFormat")?, "unknown", "step {step}");
        assert_eq!(
            field(result, "body")?,
            &serde_json::json!({}),
            "step {step}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn translator_send_returns_explicit_not_implemented_json_for_valid_input() -> TestResult {
    // Given: the dashboard has provider, model, and translated body ready to send.

    // When: nullrouter-api receives the send request without provider execution support.
    let response = request_json(
        Method::POST,
        "/api/translator/send",
        r#"{"provider":"openai","model":"gpt-5","body":{"messages":[]}}"#,
    )
    .await?;

    // Then: the response is structured JSON that explicitly declares execution unsupported.
    assert_eq!(response.status, StatusCode::NOT_IMPLEMENTED);
    assert_structured_json(&response);
    assert_eq!(field(&response.json, "success")?, false);
    assert_eq!(field(&response.json, "unsupported")?, true);
    assert_eq!(
        field(&response.json, "error")?,
        "Translator execution is not supported by nullrouter-api"
    );
    Ok(())
}

#[actix_rt::test]
async fn translator_console_logs_returns_empty_logs_json() -> TestResult {
    // Given: nullrouter-api does not capture dashboard translator console logs.

    // When: the dashboard requests the console log buffer.
    let response = request_json(Method::GET, "/api/translator/console-logs", "").await?;

    // Then: the route returns the upstream-compatible empty logs JSON object.
    assert_eq!(response.status, StatusCode::OK);
    assert_structured_json(&response);
    assert_eq!(
        response.json,
        serde_json::json!({ "success": true, "logs": [] })
    );
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
