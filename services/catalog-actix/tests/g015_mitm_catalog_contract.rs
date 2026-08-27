#![allow(clippy::future_not_send)]

use actix_web::{App, http::StatusCode, test};
use nullrouter_catalog::configure;
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

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
        .ok_or_else(|| test_error(format!("{name} is not an array")))
}

fn item_by_id<'a>(items: &'a [Value], id: &str) -> TestResult<&'a Value> {
    items
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
        .ok_or_else(|| test_error(format!("missing item {id}")))
}

fn contains_route(routes: &[Value], expected: &str) -> bool {
    routes.iter().any(|route| route.as_str() == Some(expected))
}

#[actix_web::test]
async fn g015_mitm_catalog_keeps_api_and_dashboard_families_separate() -> TestResult {
    // Given: the real catalog service exposes its route-family inventory.

    // When: the public catalog endpoint is requested.
    let (status, json) = get_json("/api/catalog").await?;

    // Then: MITM API routes belong to nullrouter-api and dashboard MITM remains dashboard-owned.
    assert_eq!(status, StatusCode::OK);
    let families = array_field(&json, "families")?;

    let api = item_by_id(families, "api")?;
    assert_eq!(field(api, "upstream")?, "nullrouter-api");
    let api_routes = array_field(api, "routes")?;
    assert!(
        api_routes
            .iter()
            .all(|entry| entry.as_str().is_some_and(|path| path.starts_with("/api/")))
    );
    for route in [
        "/api/cli-tools/antigravity-mitm",
        "/api/cli-tools/antigravity-mitm/alias",
    ] {
        assert!(
            contains_route(api_routes, route),
            "missing API route {route}"
        );
    }
    assert!(!contains_route(api_routes, "/dashboard/mitm"));

    let dashboard = item_by_id(families, "dashboard")?;
    assert_eq!(field(dashboard, "upstream")?, "nullrouter-dashboard-host");
    let dashboard_routes = array_field(dashboard, "routes")?;
    assert!(contains_route(dashboard_routes, "/dashboard/mitm"));
    for route in [
        "/api/cli-tools/antigravity-mitm",
        "/api/cli-tools/antigravity-mitm/alias",
    ] {
        assert!(
            !contains_route(dashboard_routes, route),
            "leaked API route {route}"
        );
    }

    Ok(())
}
