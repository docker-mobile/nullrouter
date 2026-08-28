#![allow(clippy::future_not_send)]

use actix_web::{App, http::StatusCode, test};
use nullrouter_catalog::{DEFAULT_HOST, DEFAULT_PORT, configure};
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[actix_web::test]
async fn package_metadata_uses_nullrouter_catalog_names() {
    // Given: the catalog crate is built as a Rust microservice.
    let package_name = env!("CARGO_PKG_NAME");
    let catalog_binary = option_env!("CARGO_BIN_EXE_nullrouter-catalog");

    // When: Cargo exposes package and binary metadata to integration tests.

    // Then: both public names follow the nullrouter-* service naming convention.
    assert_eq!(package_name, "nullrouter-catalog");
    assert!(catalog_binary.is_some());
    assert_eq!(DEFAULT_HOST, "127.0.0.1");
    assert_eq!(DEFAULT_PORT, 20131);
}

async fn get_json(uri: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(App::new().configure(configure)).await;
    let req = test::TestRequest::get().uri(uri).to_request();

    let res = test::call_service(&app, req).await;
    let status = res.status();
    let json = test::read_body_json(res).await;

    Ok((status, json))
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

fn array_field<'a>(json: &'a Value, name: &str) -> TestResult<&'a [Value]> {
    field(json, name)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| test_error(format!("{name} is an array")))
}

fn item_by_id<'a>(items: &'a [Value], id: &str) -> TestResult<&'a Value> {
    items
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
        .ok_or_else(|| test_error(format!("missing item {id}")))
}

fn contains_str(items: &[Value], expected: &str) -> bool {
    items.iter().any(|item| item.as_str() == Some(expected))
}

fn assert_routes_include(routes: &[Value], expected_routes: &[&str]) {
    for expected in expected_routes {
        assert!(
            contains_str(routes, expected),
            "route catalog is missing {expected}"
        );
    }
}

#[actix_web::test]
async fn health_returns_catalog_service_status_when_requested() -> TestResult {
    // Given: the catalog service routes are configured.

    // When: health is requested.
    let (status, json) = get_json("/health").await?;

    // Then: the service reports the catalog identity.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json,
        serde_json::json!({ "ok": true, "service": "nullrouter-catalog" })
    );
    Ok(())
}

#[actix_web::test]
async fn route_catalog_groups_inspire_inventory_when_requested() -> TestResult {
    // Given: inspire exposes control-plane, OpenAI-compatible, and dashboard route families.

    // When: the route catalog is requested.
    let (status, json) = get_json("/api/catalog/routes").await?;

    // Then: routes are grouped by the upstream service that should own them.
    assert_eq!(status, StatusCode::OK);
    let families = array_field(&json, "families")?;
    assert_eq!(families.len(), 3);

    let api = item_by_id(families, "api")?;
    assert_eq!(field(api, "upstream")?, "nullrouter-api");
    assert_routes_include(
        array_field(api, "routes")?,
        &["/api/settings", "/api/providers/client", "/api/usage/stats"],
    );

    let v1 = item_by_id(families, "v1")?;
    assert_eq!(field(v1, "upstream")?, "nullrouter-runtime");
    assert_eq!(field(v1, "gatewayPrefix")?, "/v1");
    assert_eq!(field(v1, "sourcePrefix")?, "/v1");
    assert_routes_include(
        array_field(v1, "routes")?,
        &["/v1/chat/completions", "/v1/responses"],
    );

    let dashboard = item_by_id(families, "dashboard")?;
    assert_eq!(field(dashboard, "upstream")?, "nullrouter-dashboard-host");
    assert_routes_include(array_field(dashboard, "routes")?, &["/dashboard/providers"]);
    Ok(())
}

#[actix_web::test]
async fn route_catalog_includes_g010_media_provider_routes_when_requested() -> TestResult {
    // Given: G010 media-provider parity spans dashboard, API, and runtime routes.

    // When: the route catalog is requested.
    let (status, json) = get_json("/api/catalog/routes").await?;

    // Then: the catalog exposes the representative media-provider route contract.
    assert_eq!(status, StatusCode::OK);
    let families = array_field(&json, "families")?;

    let api = item_by_id(families, "api")?;
    assert_routes_include(
        array_field(api, "routes")?,
        &[
            "/api/media-providers/tts/voices",
            "/api/media-providers/tts/{provider}/voices",
        ],
    );

    let v1 = item_by_id(families, "v1")?;
    assert_routes_include(
        array_field(v1, "routes")?,
        &[
            "/v1/embeddings",
            "/v1/audio/speech",
            "/v1/images/generations",
            "/v1/search",
            "/v1/web/fetch",
        ],
    );

    let dashboard = item_by_id(families, "dashboard")?;
    assert_routes_include(
        array_field(dashboard, "routes")?,
        &[
            "/dashboard/{path:.*}",
            "/dashboard/media-providers/{kind}",
            "/dashboard/media-providers/{kind}/{id}",
            "/dashboard/media-providers/combo/{id}",
        ],
    );
    Ok(())
}

#[actix_web::test]
async fn provider_catalog_matches_api_and_dashboard_defaults_when_requested() -> TestResult {
    // Given: the API and dashboard fixtures expose default providers and models.

    // When: the provider catalog is requested.
    let (status, json) = get_json("/api/catalog/providers").await?;

    // Then: provider and model defaults are deterministic and dashboard-compatible.
    assert_eq!(status, StatusCode::OK);

    let providers = array_field(&json, "providers")?;
    assert!(item_by_id(providers, "claude").is_ok());
    assert!(item_by_id(providers, "openai").is_ok());
    assert_eq!(
        field(item_by_id(providers, "openai")?, "authLabel")?,
        "API key"
    );

    let models = array_field(&json, "models")?;
    let openai = item_by_id(models, "openai/gpt-5")?;
    assert_eq!(field(openai, "provider")?, "openai");
    assert_eq!(field(openai, "fullModel")?, "openai/gpt-5");
    assert_eq!(field(openai, "alias")?, "gpt-5");

    let openai_models = field(&json, "openaiModels")?;
    assert_eq!(field(openai_models, "object")?, "list");
    let data = array_field(openai_models, "data")?;
    assert!(item_by_id(data, "anthropic/claude-sonnet-4.5").is_ok());
    Ok(())
}

#[actix_web::test]
async fn state_defaults_are_empty_and_zeroed_when_requested() -> TestResult {
    // Given: no persistence backend is configured for this service slice.

    // When: state routes are requested.
    let (settings_status, settings) = get_json("/api/state/settings").await?;
    let (keys_status, keys) = get_json("/api/state/keys").await?;
    let (usage_status, usage) = get_json("/api/state/usage").await?;

    // Then: dashboard state shapes are present with default values.
    assert_eq!(settings_status, StatusCode::OK);
    assert_eq!(field(&settings, "requireApiKey")?, false);
    assert_eq!(field(&settings, "hasPassword")?, false);
    assert_eq!(field(&settings, "enableTranslator")?, false);
    assert_eq!(field(&settings, "tunnelDashboardAccess")?, false);
    assert_eq!(field(&settings, "oidcConfigured")?, false);
    assert_eq!(field(&settings, "enableRequestLogs")?, false);
    // No `requireLogin`: dashboard login is unconditional in nullrouter, so the
    // shape must not carry a flag that would imply it can be turned off.
    assert!(
        settings.get("requireLogin").is_none(),
        "requireLogin must stay absent from the state shape"
    );

    assert_eq!(keys_status, StatusCode::OK);
    assert!(array_field(&keys, "keys")?.is_empty());

    assert_eq!(usage_status, StatusCode::OK);
    assert_eq!(field(&usage, "streamConnected")?, false);
    assert_eq!(field(&usage, "activeRequests")?, 0);
    assert_eq!(field(&usage, "requestsToday")?, 0);
    assert_eq!(field(&usage, "tokensToday")?, 0);
    assert_eq!(field(&usage, "estimatedCost")?, "$0.00");
    assert!(array_field(&usage, "recentRequests")?.is_empty());
    assert!(!array_field(&usage, "topologyProviders")?.is_empty());
    Ok(())
}
