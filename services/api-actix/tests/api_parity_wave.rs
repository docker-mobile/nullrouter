#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    http::{StatusCode, header},
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

async fn request_json(
    method: actix_web::http::Method,
    uri: &str,
    body: &str,
) -> TestResult<(StatusCode, Value)> {
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
    request_json(actix_web::http::Method::GET, uri, "").await
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

#[actix_rt::test]
async fn provider_routes_return_empty_defaults_and_json_validation_errors() -> TestResult {
    // Given: no provider connections or remote catalog fetches are available in this slice.

    // When: dashboard provider endpoints are requested.
    let (providers_status, providers) = get_json("/api/providers").await?;
    let (suggest_status, suggest) = get_json(
        "/api/providers/suggested-models?url=https://example.invalid&type=openrouter-free",
    )
    .await?;
    let (missing_status, missing) = get_json("/api/providers/suggested-models").await?;
    let (validate_status, validate) = request_json(
        actix_web::http::Method::POST,
        "/api/providers/validate",
        "{}",
    )
    .await?;

    // Then: registered JSON endpoints expose upstream-compatible empty/error envelopes.
    assert_eq!(providers_status, StatusCode::OK);
    assert_eq!(field(&providers, "connections")?, &serde_json::json!([]));
    assert_eq!(suggest_status, StatusCode::OK);
    assert_eq!(field(&suggest, "data")?, &serde_json::json!([]));
    assert_eq!(missing_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&missing, "error")?, "Missing url or type");
    assert_eq!(validate_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&validate, "error")?, "Provider and API key required");
    Ok(())
}

#[actix_rt::test]
async fn provider_mutation_and_dynamic_routes_return_structured_errors() -> TestResult {
    // Given: the Rust slice has no persisted provider connections.

    // When: provider creation and dynamic provider routes hit validation boundaries.
    let (invalid_status, invalid) = request_json(
        actix_web::http::Method::POST,
        "/api/providers",
        r#"{"provider":"unknown","apiKey":"sk-test","name":"Bad"}"#,
    )
    .await?;
    let (missing_key_status, missing_key) = request_json(
        actix_web::http::Method::POST,
        "/api/providers",
        r#"{"provider":"openai","name":"OpenAI"}"#,
    )
    .await?;
    let (unknown_status, unknown) = get_json("/api/providers/missing").await?;

    // Then: upstream-style JSON errors are returned instead of route misses.
    assert_eq!(invalid_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&invalid, "error")?, "Invalid provider");
    assert_eq!(missing_key_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&missing_key, "error")?, "API Key is required");
    assert_eq!(unknown_status, StatusCode::NOT_FOUND);
    assert_eq!(field(&unknown, "error")?, "Connection not found");
    Ok(())
}

#[actix_rt::test]
async fn key_routes_return_creation_boundaries_and_unknown_id_errors() -> TestResult {
    // Given: no API keys are persisted in this deterministic slice.

    // When: key mutation and dynamic key routes are requested.
    let (missing_name_status, missing_name) =
        request_json(actix_web::http::Method::POST, "/api/keys", "{}").await?;
    let (unknown_status, unknown) = get_json("/api/keys/missing").await?;
    let (delete_status, deleted) =
        request_json(actix_web::http::Method::DELETE, "/api/keys/missing", "").await?;

    // Then: keys keep the upstream JSON envelopes at the boundary.
    assert_eq!(missing_name_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&missing_name, "error")?, "Name is required");
    assert_eq!(unknown_status, StatusCode::NOT_FOUND);
    assert_eq!(field(&unknown, "error")?, "Key not found");
    assert_eq!(delete_status, StatusCode::NOT_FOUND);
    assert_eq!(field(&deleted, "error")?, "Key not found");
    Ok(())
}

#[actix_rt::test]
async fn combo_routes_return_empty_defaults_and_json_boundary_errors() -> TestResult {
    // Given: the Rust API has no persisted combos.
    let valid_combo = r#"{"name":"fast_lane","models":["openai/gpt-5"],"kind":"fallback"}"#;

    // When: combo collection, creation, and unknown dynamic routes are requested.
    let (list_status, list) = get_json("/api/combos").await?;
    let (create_status, created) =
        request_json(actix_web::http::Method::POST, "/api/combos", valid_combo).await?;
    let (missing_name_status, missing_name) =
        request_json(actix_web::http::Method::POST, "/api/combos", "{}").await?;
    let (malformed_status, malformed) =
        request_json(actix_web::http::Method::POST, "/api/combos", "{").await?;
    let (unknown_status, unknown) = get_json("/api/combos/missing").await?;
    let (update_status, updated) = request_json(
        actix_web::http::Method::PUT,
        "/api/combos/missing",
        r#"{"name":"renamed"}"#,
    )
    .await?;
    let (delete_status, deleted) =
        request_json(actix_web::http::Method::DELETE, "/api/combos/missing", "").await?;

    // Then: success and failure bodies are JSON, not default 404/HTML responses.
    assert_eq!(list_status, StatusCode::OK);
    assert_eq!(field(&list, "combos")?, &serde_json::json!([]));
    assert_eq!(create_status, StatusCode::CREATED);
    assert_eq!(field(&created, "name")?, "fast_lane");
    assert_eq!(
        field(&created, "models")?,
        &serde_json::json!(["openai/gpt-5"])
    );
    assert_eq!(missing_name_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&missing_name, "error")?, "Name is required");
    assert_eq!(malformed_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&malformed, "error")?, "Invalid JSON body");
    assert_eq!(unknown_status, StatusCode::NOT_FOUND);
    assert_eq!(field(&unknown, "error")?, "Combo not found");
    assert_eq!(update_status, StatusCode::NOT_FOUND);
    assert_eq!(field(&updated, "error")?, "Combo not found");
    assert_eq!(delete_status, StatusCode::NOT_FOUND);
    assert_eq!(field(&deleted, "error")?, "Combo not found");
    Ok(())
}

#[actix_rt::test]
async fn proxy_pool_routes_return_empty_defaults_and_validation_errors() -> TestResult {
    // Given: no proxy pools exist.

    // When: proxy pool collection and dynamic routes are requested.
    let (list_status, list) = get_json("/api/proxy-pools").await?;
    let (missing_name_status, missing_name) =
        request_json(actix_web::http::Method::POST, "/api/proxy-pools", "{}").await?;
    let (missing_url_status, missing_url) = request_json(
        actix_web::http::Method::POST,
        "/api/proxy-pools",
        r#"{"name":"primary"}"#,
    )
    .await?;
    let (unknown_status, unknown) = get_json("/api/proxy-pools/missing").await?;

    // Then: proxy pool responses are structured JSON.
    assert_eq!(list_status, StatusCode::OK);
    assert_eq!(field(&list, "proxyPools")?, &serde_json::json!([]));
    assert_eq!(missing_name_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&missing_name, "error")?, "Name is required");
    assert_eq!(missing_url_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&missing_url, "error")?, "Proxy URL is required");
    assert_eq!(unknown_status, StatusCode::NOT_FOUND);
    assert_eq!(field(&unknown, "error")?, "Proxy pool not found");
    Ok(())
}

#[actix_rt::test]
async fn usage_routes_return_empty_metrics_and_validate_periods() -> TestResult {
    // Given: no request usage has been recorded.

    // When: usage dashboard endpoints are requested.
    let (stats_status, stats) = get_json("/api/usage/stats").await?;
    let (history_status, history) = get_json("/api/usage/history").await?;
    let (chart_status, chart) = get_json("/api/usage/chart").await?;
    let (invalid_chart_status, invalid_chart) = get_json("/api/usage/chart?period=bad").await?;
    let (providers_status, providers) = get_json("/api/usage/providers").await?;

    // Then: they return deterministic empty JSON shapes.
    assert_eq!(stats_status, StatusCode::OK);
    assert_eq!(field(&stats, "totalRequests")?, 0);
    assert_eq!(field(&stats, "recentRequests")?, &serde_json::json!([]));
    assert_eq!(history_status, StatusCode::OK);
    assert_eq!(field(&history, "totalRequests")?, 0);
    assert_eq!(chart_status, StatusCode::OK);
    assert!(chart.as_array().is_some_and(Vec::is_empty));
    assert_eq!(invalid_chart_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&invalid_chart, "error")?, "Invalid period");
    assert_eq!(providers_status, StatusCode::OK);
    assert_eq!(field(&providers, "providers")?, &serde_json::json!([]));

    for uri in ["/api/usage/logs", "/api/usage/request-logs"] {
        let (status, json) = get_json(uri).await?;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(json.as_array().is_some_and(Vec::is_empty), "{uri}");
    }
    Ok(())
}

#[actix_rt::test]
async fn catalog_and_model_routes_return_default_shapes_and_mutation_boundaries() -> TestResult {
    // Given: model catalog mutations are not persisted in this deterministic slice.

    // When: catalog and model metadata endpoints are requested.
    let (pricing_status, pricing) = get_json("/api/pricing").await?;
    let (tags_status, tags) = get_json("/api/tags").await?;
    let (availability_status, availability) = get_json("/api/models/availability").await?;
    let (disabled_status, disabled) = get_json("/api/models/disabled").await?;
    let (disabled_one_status, disabled_one) =
        get_json("/api/models/disabled?providerAlias=openai").await?;
    let (custom_status, custom) = get_json("/api/models/custom").await?;
    let (alias_status, aliases) = get_json("/api/models/alias").await?;
    let (alias_put_status, alias_put) =
        request_json(actix_web::http::Method::PUT, "/api/models/alias", "{").await?;

    // Then: all registered route families answer with JSON contract shapes.
    assert_eq!(pricing_status, StatusCode::OK);
    assert!(field(&pricing, "gh").is_ok());
    assert_eq!(tags_status, StatusCode::OK);
    assert!(
        field(&tags, "models")?
            .as_array()
            .is_some_and(|models| !models.is_empty())
    );
    assert_eq!(availability_status, StatusCode::OK);
    assert_eq!(field(&availability, "models")?, &serde_json::json!([]));
    assert_eq!(field(&availability, "unavailableCount")?, 0);
    assert_eq!(disabled_status, StatusCode::OK);
    assert_eq!(field(&disabled, "disabled")?, &serde_json::json!({}));
    assert_eq!(disabled_one_status, StatusCode::OK);
    assert_eq!(field(&disabled_one, "ids")?, &serde_json::json!([]));
    assert_eq!(custom_status, StatusCode::OK);
    assert_eq!(field(&custom, "models")?, &serde_json::json!([]));
    assert_eq!(alias_status, StatusCode::OK);
    assert_eq!(field(&aliases, "aliases")?, &serde_json::json!({}));
    assert_eq!(alias_put_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&alias_put, "error")?, "Invalid JSON body");
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
