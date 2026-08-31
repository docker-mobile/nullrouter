#![allow(
    clippy::future_not_send,
    clippy::expect_used,
    reason = "test helper: failing to bind a loopback socket should abort the test"
)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use nullrouter_runtime::{Runtime, app_config, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// State stub for route-shape tests.
///
/// The dynamic API-key gate is consulted by every public runtime endpoint, even when it reports
/// that keys are not required. A closed port would therefore test only the gate's fail-closed path,
/// not the aliases' contracts. This stub declares the gate public and otherwise supplies the
/// ordinary no-credentials failure used by the provider-backed alias tests.
async fn public_state_stub() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buffer = [0_u8; 8192];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(buffer.get(..read).unwrap_or_default());
                let body = if request.contains("/internal/v1/keys/gate") {
                    serde_json::json!({"requireApiKey": false, "valid": false, "active": false})
                } else if request.contains("/internal/v1/routing-context") {
                    serde_json::json!({"combos": [], "connections": [], "settings": {}})
                } else if request.contains("/internal/v1/credentials/select") {
                    serde_json::json!({"status": "unavailable", "message": "state stub"})
                } else {
                    serde_json::json!({"ok": true})
                }
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    });
    addr
}

struct RuntimeResponse {
    status: StatusCode,
    content_type: String,
    body: String,
}

async fn request(method: Method, uri: &str, body: &str) -> TestResult<RuntimeResponse> {
    let state_addr = public_state_stub().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(&state_addr)))
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
    Ok(RuntimeResponse {
        status,
        content_type,
        body,
    })
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
async fn api_v1_model_aliases_return_same_defaults_as_v1_routes() -> TestResult {
    // Given: public upstream routes mount runtime under /api/v1 and /api/v1beta.

    // When: model metadata aliases are requested.
    let (root_status, root) = get_json("/api/v1").await?;
    let (models_status, models) = get_json("/api/v1/models").await?;
    let (kind_status, kind) = get_json("/api/v1/models/chat").await?;
    let (info_status, info) = get_json("/api/v1/models/info?id=openai/gpt-5").await?;
    let (beta_status, beta) = get_json("/api/v1beta/models").await?;
    let (beta_post_status, beta_post) =
        request_json(Method::POST, "/api/v1beta/models", "{}").await?;

    // Then: aliases return structured runtime JSON rather than a route miss.
    assert_eq!(root_status, StatusCode::OK);
    assert_eq!(models_status, StatusCode::OK);
    assert_eq!(field(&root, "object")?, "list");
    assert_eq!(field(&models, "object")?, "list");
    assert_eq!(kind_status, StatusCode::OK);
    assert_eq!(field(&kind, "object")?, "list");
    assert_eq!(info_status, StatusCode::OK);
    assert_eq!(field(&info, "id")?, "openai/gpt-5");
    assert_eq!(beta_status, StatusCode::OK);
    assert!(field(&beta, "models")?.is_array());
    assert_eq!(beta_post_status, StatusCode::OK);
    assert!(field(&beta_post, "models")?.is_array());
    Ok(())
}

#[actix_rt::test]
async fn api_v1_provider_aliases_return_runtime_defaults() -> TestResult {
    // Given: provider-backed runtime execution is not wired in this local slice.
    let provider_routes = [
        (
            "/api/v1/embeddings",
            r#"{"model":"openai/text-embedding-3-small","input":"hello"}"#,
        ),
        (
            "/api/v1/images/generations",
            r#"{"model":"openai/dall-e-3","prompt":"hello"}"#,
        ),
        (
            "/api/v1/audio/speech",
            r#"{"model":"openai/tts-1","input":"hello"}"#,
        ),
        (
            "/api/v1/audio/transcriptions",
            r#"{"model":"openai/whisper-1","file":"ignored"}"#,
        ),
        ("/api/v1/search", r#"{"provider":"tavily","query":"hello"}"#),
        (
            "/api/v1/web/fetch",
            r#"{"provider":"firecrawl","url":"https://example.com"}"#,
        ),
        ("/api/v1/responses/compact", r#"{"model":"openai/gpt-5"}"#),
        (
            "/api/v1beta/models/gemini/gemini-2.5-pro:generateContent",
            r#"{"contents":[{"parts":[{"text":"hello"}]}]}"#,
        ),
    ];

    // When: provider-backed aliases are invoked.
    for (uri, body) in provider_routes {
        let (status, json) = request_json(Method::POST, uri, body).await?;

        // Then: each alias returns the same structured unsupported provider envelope.
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
        "/api/v1/messages/count_tokens",
        r#"{"messages":[{"content":"hello world"}]}"#,
    )
    .await?;
    let (voices_status, voices) = get_json("/api/v1/audio/voices").await?;

    assert_eq!(count_status, StatusCode::OK);
    assert_eq!(field(&count, "input_tokens")?, 3);
    assert_eq!(voices_status, StatusCode::OK);
    assert_eq!(field(&voices, "object")?, "list");
    Ok(())
}

#[actix_rt::test]
async fn api_v1_chat_aliases_return_json_and_sse_runtime_defaults() -> TestResult {
    // Given: chat-style aliases receive valid model-bearing requests.
    let json_body = r#"{"model":"openai/gpt-5","messages":[]}"#;
    let stream_body = r#"{"model":"openai/gpt-5","stream":true,"messages":[]}"#;

    // When: JSON and stream aliases are posted.
    let chat = request(Method::POST, "/api/v1/chat/completions", json_body).await?;
    let chat_stream = request(Method::POST, "/api/v1/chat/completions", stream_body).await?;
    let responses = request(Method::POST, "/api/v1/responses", json_body).await?;
    let messages = request(Method::POST, "/api/v1/messages", json_body).await?;
    let api_chat = request(Method::POST, "/api/v1/api/chat", json_body).await?;

    // Then: aliases retain the runtime JSON/SSE contracts instead of falling through to 404.
    assert_eq!(chat.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(chat.content_type.starts_with("application/json"));
    assert_eq!(chat_stream.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(chat_stream.content_type.starts_with("text/event-stream"));
    assert!(chat_stream.body.contains("data: [DONE]"));
    assert_eq!(responses.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(responses.content_type.starts_with("text/event-stream"));
    assert!(responses.body.contains("event: response.failed"));
    assert_eq!(messages.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(messages.content_type.starts_with("text/event-stream"));
    assert_eq!(api_chat.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(api_chat.content_type.starts_with("application/json"));
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
