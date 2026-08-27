#![allow(clippy::future_not_send)]

use actix_web::{App, http::StatusCode, test};
use nullrouter_catalog::configure;
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const TRANSLATOR_API_ROUTES: [&str; 6] = [
    "/api/translator/load",
    "/api/translator/save",
    "/api/translator/translate",
    "/api/translator/send",
    "/api/translator/console-logs",
    "/api/translator/console-logs/stream",
];

async fn get_json(uri: &str) -> TestResult<(StatusCode, String, Value)> {
    let app = test::init_service(App::new().configure(configure)).await;
    let req = test::TestRequest::get().uri(uri).to_request();

    let res = test::call_service(&app, req).await;
    let status = res.status();
    let content_type = res
        .headers()
        .get(actix_web::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = test::read_body(res).await;
    let json = serde_json::from_slice(&body)?;

    Ok((status, content_type, json))
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

fn assert_routes_include(routes: &[Value], expected_routes: &[&str]) {
    for expected in expected_routes {
        assert!(
            routes.iter().any(|route| route.as_str() == Some(expected)),
            "route catalog is missing {expected}"
        );
    }
}

#[actix_web::test]
async fn route_catalog_includes_translator_ownership_as_json_when_requested() -> TestResult {
    // Given: translator parity needs dashboard and API route ownership in catalog JSON.

    // When: the route catalog is requested through the canonical route and gateway alias.
    let (status, content_type, json) = get_json("/api/catalog/routes").await?;
    let (alias_status, alias_content_type, alias_json) = get_json("/api/catalog").await?;

    // Then: the catalog responds with structured JSON, not dashboard HTML.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(alias_status, StatusCode::OK);
    assert!(
        content_type.starts_with("application/json"),
        "catalog routes returned non-JSON content type {content_type}"
    );
    assert!(
        alias_content_type.starts_with("application/json"),
        "catalog alias returned non-JSON content type {alias_content_type}"
    );
    assert_eq!(alias_json, json);

    let families = array_field(&json, "families")?;

    let api = item_by_id(families, "api")?;
    let api_routes = array_field(api, "routes")?;
    assert_routes_include(api_routes, &TRANSLATOR_API_ROUTES);
    assert_routes_include(api_routes, &["/api/proxy-pools"]);

    let dashboard = item_by_id(families, "dashboard")?;
    assert_routes_include(
        array_field(dashboard, "routes")?,
        &["/dashboard/translator", "/dashboard/proxy-pools"],
    );

    Ok(())
}
