#![allow(clippy::future_not_send)]

use actix_web::{App, http::StatusCode, test};
use nullrouter_catalog::configure;
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const CONSOLE_LOG_API_ROUTES: [&str; 2] = [
    "/api/translator/console-logs",
    "/api/translator/console-logs/stream",
];
const CONSOLE_LOG_DASHBOARD_ROUTES: [&str; 1] = ["/dashboard/console-log"];

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

fn assert_routes_exclude(routes: &[Value], unexpected_routes: &[&str]) {
    for unexpected in unexpected_routes {
        assert!(
            !contains_str(routes, unexpected),
            "route catalog assigns {unexpected} to the wrong family"
        );
    }
}

#[actix_web::test]
async fn route_catalog_includes_g014_console_log_ownership_as_json_when_requested() -> TestResult {
    // Given: Console Log parity needs dashboard navigation plus API and SSE ownership.

    // When: the route catalog is requested through the canonical route and gateway alias.
    let (status, content_type, json) = get_json("/api/catalog/routes").await?;
    let (alias_status, alias_content_type, alias_json) = get_json("/api/catalog").await?;

    // Then: the catalog responds with structured JSON and assigns routes to their owners.
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
    assert_eq!(field(api, "upstream")?, "nullrouter-api");
    assert_eq!(field(api, "gatewayPrefix")?, "/api");
    assert_eq!(field(api, "sourcePrefix")?, "/api");
    let api_routes = array_field(api, "routes")?;
    assert_routes_include(api_routes, &CONSOLE_LOG_API_ROUTES);
    assert_routes_exclude(api_routes, &CONSOLE_LOG_DASHBOARD_ROUTES);

    let v1 = item_by_id(families, "v1")?;
    assert_eq!(field(v1, "upstream")?, "nullrouter-runtime");
    assert_routes_exclude(array_field(v1, "routes")?, &CONSOLE_LOG_API_ROUTES);

    let dashboard = item_by_id(families, "dashboard")?;
    assert_eq!(field(dashboard, "upstream")?, "nullrouter-dashboard-host");
    assert_eq!(field(dashboard, "gatewayPrefix")?, "/");
    assert_eq!(field(dashboard, "sourcePrefix")?, "/");
    let dashboard_routes = array_field(dashboard, "routes")?;
    assert_routes_include(dashboard_routes, &CONSOLE_LOG_DASHBOARD_ROUTES);
    assert_routes_exclude(dashboard_routes, &CONSOLE_LOG_API_ROUTES);

    Ok(())
}
