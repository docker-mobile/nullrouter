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

async fn request_json(method: Method, uri: &str, body: &str) -> TestResult<(StatusCode, Value)> {
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
    let body = to_bytes(res.into_body()).await?;
    let json = serde_json::from_slice(&body)?;
    Ok((status, json))
}

async fn get_json(uri: &str) -> TestResult<(StatusCode, Value)> {
    request_json(Method::GET, uri, "").await
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

#[actix_rt::test]
async fn locale_and_oauth_gap_routes_return_json_contracts() -> TestResult {
    // Given: no locale persistence or OAuth credential store is available.

    // When: remaining locale and OAuth helper routes are requested.
    let (locale_status, locale) = get_json("/api/locale").await?;
    let (locale_post_status, locale_post) =
        request_json(Method::POST, "/api/locale", r#"{"locale":"en"}"#).await?;
    let (oauth_get_status, oauth_get) = get_json("/api/oauth/cursor/import").await?;
    let (oauth_post_status, oauth_post) =
        request_json(Method::POST, "/api/oauth/codex/import-token", "{}").await?;

    // Then: each route returns structured JSON instead of a framework 404 or HTML page.
    assert_eq!(locale_status, StatusCode::OK);
    assert_eq!(field(&locale, "locale")?, "en");
    assert_eq!(locale_post_status, StatusCode::OK);
    assert_eq!(field(&locale_post, "success")?, true);
    assert_eq!(oauth_get_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&oauth_get, "unsupported")?, true);
    assert_eq!(oauth_post_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&oauth_post, "unsupported")?, true);
    Ok(())
}

#[actix_rt::test]
async fn provider_proxy_model_and_usage_gap_routes_return_json_contracts() -> TestResult {
    // Given: provider connections, proxy pools, model executors, and usage DB are empty.

    // When: remaining provider helper, proxy helper, model test, and usage routes are requested.
    let (provider_models_status, provider_models) =
        get_json("/api/providers/openai/models").await?;
    let (provider_test_status, provider_test) =
        request_json(Method::POST, "/api/providers/openai/test", "{}").await?;
    let (test_models_status, test_models) =
        request_json(Method::POST, "/api/providers/openai/test-models", "{}").await?;
    let (free_models_status, free_models) = get_json("/api/providers/kilo/free-models").await?;
    let (batch_status, batch) = request_json(
        Method::POST,
        "/api/providers/test-batch",
        r#"{"mode":"all"}"#,
    )
    .await?;
    let (proxy_test_status, proxy_test) =
        request_json(Method::POST, "/api/proxy-pools/pool/test", "{}").await?;
    let (vercel_status, vercel) = request_json(
        Method::POST,
        "/api/proxy-pools/vercel-deploy",
        r#"{"vercelToken":"t"}"#,
    )
    .await?;
    let (deno_status, deno) = request_json(
        Method::POST,
        "/api/proxy-pools/deno-deploy",
        r#"{"orgDomain":"example","denoToken":"t"}"#,
    )
    .await?;
    let (cloudflare_status, cloudflare) = request_json(
        Method::POST,
        "/api/proxy-pools/cloudflare-deploy",
        r#"{"accountId":"a","apiToken":"t"}"#,
    )
    .await?;
    let (model_test_status, model_test) = request_json(
        Method::POST,
        "/api/models/test",
        r#"{"model":"openai/gpt-5"}"#,
    )
    .await?;
    let (usage_detail_status, usage_detail) =
        get_json("/api/usage/request-details?page=1&pageSize=20").await?;
    let (usage_connection_status, usage_connection) = get_json("/api/usage/connection_1").await?;
    let (reset_status, reset) = request_json(
        Method::POST,
        "/api/usage/connection_1/codex-reset-credits",
        "{}",
    )
    .await?;

    // Then: deterministic JSON shapes cover every upstream route family in this wave.
    assert_eq!(provider_models_status, StatusCode::OK);
    assert_eq!(field(&provider_models, "models")?, &serde_json::json!([]));
    assert_eq!(provider_test_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&provider_test, "valid")?, false);
    assert_eq!(test_models_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&test_models, "results")?, &serde_json::json!([]));
    assert_eq!(free_models_status, StatusCode::OK);
    assert_eq!(field(&free_models, "models")?, &serde_json::json!([]));
    assert_eq!(batch_status, StatusCode::OK);
    assert_eq!(field(&batch, "results")?, &serde_json::json!([]));
    assert_eq!(proxy_test_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&proxy_test, "ok")?, false);
    assert_eq!(vercel_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&vercel, "success")?, false);
    assert_eq!(deno_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&deno, "success")?, false);
    assert_eq!(cloudflare_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&cloudflare, "success")?, false);
    assert_eq!(model_test_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&model_test, "ok")?, false);
    assert_eq!(usage_detail_status, StatusCode::OK);
    assert_eq!(field(&usage_detail, "requests")?, &serde_json::json!([]));
    assert_eq!(usage_connection_status, StatusCode::NOT_FOUND);
    assert_eq!(field(&usage_connection, "error")?, "Connection not found");
    assert_eq!(reset_status, StatusCode::NOT_FOUND);
    assert_eq!(field(&reset, "error")?, "Connection not found");
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
