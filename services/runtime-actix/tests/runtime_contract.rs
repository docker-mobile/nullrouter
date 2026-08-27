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

async fn request_json(method: Method, uri: &str, body: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(
                UNREACHABLE_STATE_ADDR,
            )))
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
    Ok((status, serde_json::from_slice(&body)?))
}

/// A raw response, for routes whose framing is not JSON.
struct RawResponse {
    status: StatusCode,
    content_type: String,
    body: String,
}

async fn request(method: Method, uri: &str, body: &str) -> TestResult<RawResponse> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(
                UNREACHABLE_STATE_ADDR,
            )))
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
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok(RawResponse {
        status,
        content_type,
        body,
    })
}

async fn get_json(uri: &str) -> TestResult<(StatusCode, Value)> {
    request_json(Method::GET, uri, "").await
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

#[actix_rt::test]
async fn health_reports_runtime_service_when_requested() -> TestResult {
    // Given: the runtime service is configured with default app state.

    // When: the health endpoint is requested.
    let (status, json) = get_json("/health").await?;

    // Then: the service returns the runtime-specific JSON health contract.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(field(&json, "ok")?, true);
    assert_eq!(field(&json, "service")?, "nullrouter-runtime");
    Ok(())
}

#[actix_rt::test]
async fn model_routes_return_default_metadata_when_no_provider_state_exists() -> TestResult {
    // Given: no provider catalog state is configured in this runtime slice.

    // When: OpenAI, typed-kind, info, and Gemini model endpoints are requested.
    let (root_status, root) = get_json("/v1").await?;
    let (models_status, models) = get_json("/v1/models").await?;
    let (kind_status, kind) = get_json("/v1/models/image").await?;
    let (info_status, info) = get_json("/v1/models/info?id=openai/gpt-5").await?;
    let (info_missing_status, info_missing) = get_json("/v1/models/info").await?;
    let (beta_status, beta) = get_json("/v1beta/models").await?;
    let (beta_post_status, beta_post) = request_json(Method::POST, "/v1beta/models", "{}").await?;

    // Then: each route returns structured JSON rather than a route miss.
    assert_eq!(root_status, StatusCode::OK);
    assert_eq!(models_status, StatusCode::OK);
    assert_eq!(field(&root, "object")?, "list");
    assert_eq!(field(&models, "object")?, "list");
    assert!(
        field(&models, "data")?
            .as_array()
            .is_some_and(|models| !models.is_empty())
    );
    assert_eq!(kind_status, StatusCode::OK);
    assert_eq!(field(&kind, "object")?, "list");
    assert_eq!(info_status, StatusCode::OK);
    assert_eq!(field(&info, "id")?, "openai/gpt-5");
    assert_eq!(field(&info, "endpoint")?, "/v1/chat/completions");
    assert_eq!(info_missing_status, StatusCode::BAD_REQUEST);
    assert_eq!(
        field(field(&info_missing, "error")?, "message")?,
        "Missing required query param: id"
    );
    assert_eq!(beta_status, StatusCode::OK);
    assert!(
        field(&beta, "models")?
            .as_array()
            .is_some_and(|models| !models.is_empty())
    );
    assert_eq!(beta_post_status, StatusCode::OK);
    assert!(field(&beta_post, "models")?.is_array());
    Ok(())
}

#[actix_rt::test]
async fn v1beta_dynamic_routes_return_structured_generation_errors() -> TestResult {
    // Given: a Gemini-style generation request references a model but no provider can execute it.
    let body = r#"{"contents":[{"parts":[{"text":"hello"}]}]}"#;

    // When: non-stream and stream Gemini action routes are requested.
    let (json_status, json) = request_json(
        Method::POST,
        "/v1beta/models/gemini/gemini-2.5-pro:generateContent",
        body,
    )
    .await?;
    // `:streamGenerateContent` is a streaming route, so its error arrives as
    // SSE frames rather than a bare JSON body.
    let stream = request(
        Method::POST,
        "/v1beta/models/gemini/gemini-2.5-pro:streamGenerateContent",
        body,
    )
    .await?;

    // Then: both answer with structured error envelopes instead of an HTML 404.
    assert_eq!(json_status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        !field(field(&json, "error")?, "message")?
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "error envelope must carry a message"
    );

    assert_eq!(stream.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        stream.content_type.starts_with("text/event-stream"),
        "streaming Gemini errors must be SSE, got {}",
        stream.content_type
    );
    assert!(stream.body.contains("\"error\""), "{}", stream.body);
    assert!(stream.body.contains("data: [DONE]"), "{}", stream.body);
    Ok(())
}

#[actix_rt::test]
async fn provider_endpoints_return_explicit_not_configured_or_local_defaults() -> TestResult {
    // Given: valid requests for provider-backed runtime endpoints.
    let endpoints = [
        (
            "/v1/embeddings",
            r#"{"model":"openai/text-embedding-3-small","input":"hello"}"#,
        ),
        (
            "/v1/images/generations",
            r#"{"model":"openai/dall-e-3","prompt":"hello"}"#,
        ),
        (
            "/v1/audio/speech",
            r#"{"model":"openai/tts-1","input":"hello"}"#,
        ),
        (
            "/v1/audio/transcriptions",
            r#"{"model":"openai/whisper-1","file":"ignored"}"#,
        ),
        ("/v1/search", r#"{"provider":"tavily","query":"hello"}"#),
        (
            "/v1/web/fetch",
            r#"{"provider":"firecrawl","url":"https://example.com"}"#,
        ),
        ("/v1/responses/compact", r#"{"model":"openai/gpt-5"}"#),
    ];

    // When: each endpoint is invoked.
    for (uri, body) in endpoints {
        let (status, json) = request_json(Method::POST, uri, body).await?;

        // Then: provider-backed endpoints are explicit about execution not being wired.
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}");
        assert!(
            !field(field(&json, "error")?, "message")?
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "{uri} must carry an error message"
        );
    }

    let (count_status, count) = request_json(
        Method::POST,
        "/v1/messages/count_tokens",
        r#"{"messages":[{"content":"hello world"}]}"#,
    )
    .await?;
    let (voices_status, voices) = get_json("/v1/audio/voices").await?;

    assert_eq!(count_status, StatusCode::OK);
    assert_eq!(field(&count, "input_tokens")?, 3);
    assert_eq!(voices_status, StatusCode::OK);
    assert_eq!(field(&voices, "object")?, "list");
    assert_eq!(field(&voices, "data")?, &serde_json::json!([]));
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
