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

struct JsonResponse {
    uri: &'static str,
    status: StatusCode,
    content_type: String,
    body_text: String,
    json: Value,
}

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

async fn request_json(method: Method, uri: &'static str, body: &str) -> TestResult<JsonResponse> {
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
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .ok_or_else(|| test_error(format!("{uri} missing content-type")))?
        .to_str()?
        .to_owned();
    let body = to_bytes(res.into_body()).await?;
    let body_text = String::from_utf8(body.to_vec())?;
    let json = serde_json::from_slice(&body)?;

    Ok(JsonResponse {
        uri,
        status,
        content_type,
        body_text,
        json,
    })
}

async fn get_json(uri: &'static str) -> TestResult<JsonResponse> {
    request_json(Method::GET, uri, "").await
}

fn assert_structured_json(response: &JsonResponse, expected_status: StatusCode) {
    assert_eq!(response.status, expected_status, "{}", response.uri);
    assert!(
        response.content_type.starts_with("application/json"),
        "{} returned content-type {}",
        response.uri,
        response.content_type
    );
    assert!(
        response.json.is_object(),
        "{} returned non-object JSON",
        response.uri
    );

    let body = response.body_text.to_ascii_lowercase();
    for marker in ["<!doctype", "<html", "</html>", "<body", "</body>"] {
        assert!(
            !body.contains(marker),
            "{} returned HTML marker {marker}",
            response.uri
        );
    }
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

#[actix_rt::test]
async fn api_sentry_routes_return_structured_json_when_state_is_empty() -> TestResult {
    // Given: the Actix API has no live provider, tunnel, or CLI host state.

    // When: nested dashboard parity sentinel routes are requested.
    let provider_models = get_json("/api/providers/openai/models").await?;
    let provider_test = request_json(Method::POST, "/api/providers/openai/test", "{}").await?;
    let provider_test_models =
        request_json(Method::POST, "/api/providers/openai/test-models", "{}").await?;
    let tts_voices = get_json("/api/media-providers/tts/voices").await?;
    let openai_tts_voices = get_json("/api/media-providers/tts/openai/voices").await?;
    let deepgram_tts_voices = get_json("/api/media-providers/tts/deepgram/voices?lang=en").await?;
    let model_aliases = get_json("/api/models/alias").await?;
    let headroom_proxy = get_json("/api/headroom/proxy/v1/models").await?;
    let tunnel_status = get_json("/api/tunnel/status").await?;
    let cli_all_statuses = get_json("/api/cli-tools/all-statuses").await?;
    let cli_codex_settings = get_json("/api/cli-tools/codex-settings").await?;

    // Then: every route returns structured JSON instead of HTML fallback or panic output.
    assert_structured_json(&provider_models, StatusCode::OK);
    assert_eq!(field(&provider_models.json, "provider")?, "openai");
    assert_eq!(
        field(&provider_models.json, "models")?,
        &serde_json::json!([])
    );

    // These two routes make a real upstream call now, so with state down they report
    // that they could not read the connection. 501/`unsupported: true` was the old
    // stub's answer; a 200-shaped "not valid" here would be a lie either way, and a
    // 404 would blame the user for a dependency this deployment cannot reach.
    assert_structured_json(&provider_test, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(field(&provider_test.json, "success")?, false);
    assert_eq!(field(&provider_test.json, "connectionId")?, "openai");
    assert!(
        field(&provider_test.json, "error")?
            .as_str()
            .is_some_and(|error| error.contains("state service is unreachable")),
        "{}",
        provider_test.body_text
    );

    assert_structured_json(&provider_test_models, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(field(&provider_test_models.json, "connectionId")?, "openai");
    assert_eq!(
        field(&provider_test_models.json, "results")?,
        &serde_json::json!([])
    );
    // No `unsupported` marker: model testing is implemented, so advertising it as
    // unsupported would send the dashboard back to hiding the control.
    assert!(
        provider_test_models.json.get("unsupported").is_none(),
        "{}",
        provider_test_models.body_text
    );

    for response in [&tts_voices, &openai_tts_voices, &deepgram_tts_voices] {
        assert_structured_json(response, StatusCode::OK);
        assert_eq!(field(&response.json, "voices")?, &serde_json::json!([]));
        assert_eq!(field(&response.json, "languages")?, &serde_json::json!([]));
        assert_eq!(field(&response.json, "byLang")?, &serde_json::json!({}));
    }

    assert_structured_json(&model_aliases, StatusCode::OK);
    assert_eq!(
        field(&model_aliases.json, "aliases")?,
        &serde_json::json!({})
    );

    assert_structured_json(&headroom_proxy, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&headroom_proxy.json, "success")?, false);
    assert_eq!(field(&headroom_proxy.json, "unsupported")?, true);
    assert_eq!(field(&headroom_proxy.json, "path")?, "v1/models");

    assert_structured_json(&tunnel_status, StatusCode::OK);
    assert_eq!(
        field(field(&tunnel_status.json, "tunnel")?, "enabled")?,
        false
    );
    assert_eq!(
        field(field(&tunnel_status.json, "download")?, "inProgress")?,
        false
    );

    assert_structured_json(&cli_all_statuses, StatusCode::OK);
    let codex = field(&cli_all_statuses.json, "codex")?;
    assert_eq!(field(codex, "installed")?, false);
    assert_eq!(field(codex, "hasRouter")?, false);

    assert_structured_json(&cli_codex_settings, StatusCode::OK);
    assert_eq!(field(&cli_codex_settings.json, "installed")?, false);
    assert_eq!(field(&cli_codex_settings.json, "hasRouter")?, false);
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
