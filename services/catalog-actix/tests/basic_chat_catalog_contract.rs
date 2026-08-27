#![allow(clippy::future_not_send)]

use actix_web::{App, http::StatusCode, test};
use nullrouter_catalog::configure;
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const DASHBOARD_BASIC_CHAT_ROUTES: [&str; 24] = [
    "/dashboard/basic-chat",
    "/dashboard/providers",
    "/dashboard/providers/new",
    "/dashboard/providers/{id}",
    "/dashboard/proxy-pools",
    "/dashboard/translator",
    "/dashboard/usage",
    "/dashboard/status",
    "/dashboard/settings",
    "/dashboard/settings/pricing",
    "/dashboard/console-log",
    "/dashboard/media-providers/web",
    "/dashboard/media-providers/{kind}",
    "/dashboard/media-providers/{kind}/{id}",
    "/dashboard/media-providers/combo/{id}",
    "/dashboard/combos",
    "/dashboard/quota",
    "/dashboard/token-saver",
    "/dashboard/cli-tools",
    "/dashboard/cli-tools/{toolId}",
    "/dashboard/skills",
    "/dashboard/profile",
    "/dashboard/mitm",
    "/dashboard/endpoint",
];

const MODEL_DEFAULTS: [&str; 6] = [
    "openai/gpt-5",
    "anthropic/claude-sonnet-4.5",
    "gemini/gemini-2.5-pro",
    "github/gpt-4.1",
    "kiro/claude-sonnet-4.5",
    "opencode/sonnet",
];

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

fn assert_contains_all(items: &[Value], expected_items: &[&str]) {
    for expected in expected_items {
        assert!(
            items.iter().any(|item| item.as_str() == Some(expected)),
            "catalog is missing {expected}"
        );
    }
}

#[actix_web::test]
async fn route_catalog_includes_basic_chat_ownership_when_requested() -> TestResult {
    // Given: Basic Chat parity needs dashboard, API, and runtime route ownership.

    // When: the route catalog is requested.
    let (status, json) = get_json("/api/catalog/routes").await?;

    // Then: Basic Chat routes are assigned without dropping completed route families.
    assert_eq!(status, StatusCode::OK);
    let families = array_field(&json, "families")?;

    let api = item_by_id(families, "api")?;
    assert_eq!(field(api, "upstream")?, "nullrouter-api");
    assert_contains_all(
        array_field(api, "routes")?,
        &["/api/dashboard/chat/completions"],
    );

    let v1 = item_by_id(families, "v1")?;
    assert_eq!(field(v1, "upstream")?, "nullrouter-runtime");
    assert_contains_all(
        array_field(v1, "routes")?,
        &["/v1/chat/completions", "/v1/responses", "/v1/messages"],
    );

    let dashboard = item_by_id(families, "dashboard")?;
    assert_eq!(field(dashboard, "upstream")?, "nullrouter-dashboard-host");
    assert_contains_all(
        array_field(dashboard, "routes")?,
        &DASHBOARD_BASIC_CHAT_ROUTES,
    );
    Ok(())
}

#[actix_web::test]
async fn provider_catalog_keeps_basic_chat_model_defaults_when_requested() -> TestResult {
    // Given: Basic Chat model menus are derived from catalog defaults.

    // When: the provider catalog is requested.
    let (status, json) = get_json("/api/catalog/providers").await?;

    // Then: dashboard models and OpenAI-compatible model defaults stay in sync.
    assert_eq!(status, StatusCode::OK);
    let providers = array_field(&json, "providers")?;
    for provider_id in ["openai", "anthropic", "gemini", "openrouter"] {
        assert!(item_by_id(providers, provider_id).is_ok());
    }

    let models = array_field(&json, "models")?;
    assert_eq!(models.len(), MODEL_DEFAULTS.len());
    for model_id in MODEL_DEFAULTS {
        let model = item_by_id(models, model_id)?;
        assert_eq!(field(model, "fullModel")?, model_id);
        assert_eq!(field(model, "id")?, model_id);
    }

    let openai_models = field(&json, "openaiModels")?;
    assert_eq!(field(openai_models, "object")?, "list");
    let data = array_field(openai_models, "data")?;
    assert_eq!(data.len(), MODEL_DEFAULTS.len());
    for model_id in MODEL_DEFAULTS {
        assert_eq!(field(item_by_id(data, model_id)?, "object")?, "model");
    }
    Ok(())
}
