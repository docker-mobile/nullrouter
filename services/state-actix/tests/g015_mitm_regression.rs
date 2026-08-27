#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_state::{StateStore, configure};
use serde_json::{Value, json};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Response {
    status: StatusCode,
    content_type: String,
    body: Vec<u8>,
}

async fn request(store: StateStore, method: Method, uri: &str, body: &str) -> TestResult<Response> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(store))
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
    let body = to_bytes(res.into_body()).await?.to_vec();
    Ok(Response {
        status,
        content_type,
        body,
    })
}

async fn get_json(store: StateStore, uri: &str) -> TestResult<(Response, Value)> {
    let response = request(store, Method::GET, uri, "").await?;
    let json = serde_json::from_slice(&response.body)?;
    Ok((response, json))
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

#[actix_rt::test]
async fn g015_state_does_not_own_mitm_routes_and_keeps_default_json() -> TestResult {
    // Given: the state service owns dashboard data, not direct MITM API control.
    let store = StateStore::memory();

    // When: direct MITM status and alias paths are requested from the state service.
    for (method, uri, body) in [
        (Method::GET, "/api/cli-tools/antigravity-mitm", ""),
        (Method::GET, "/api/cli-tools/antigravity-mitm/alias", ""),
        (
            Method::PUT,
            "/api/cli-tools/antigravity-mitm/alias",
            r#"{"tool":"antigravity","mappings":{}}"#,
        ),
    ] {
        let request_label = format!("{method} {uri}");
        let response = request(store.clone(), method, uri, body).await?;

        // Then: unowned paths are plain 404s, never dashboard HTML fallbacks.
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{request_label}");
        assert!(
            !response.content_type.starts_with("text/html"),
            "{request_label} returned HTML content type {}",
            response.content_type
        );
    }

    // When: the state-owned default settings and providers endpoints are requested.
    let (settings_response, settings) = get_json(store.clone(), "/api/settings").await?;
    let (providers_response, providers) = get_json(store, "/api/providers").await?;

    // Then: both remain successful structured JSON defaults.
    assert_eq!(settings_response.status, StatusCode::OK);
    assert!(
        settings_response
            .content_type
            .starts_with("application/json")
    );
    assert_eq!(field(&settings, "requireLogin")?, true);
    assert_eq!(field(&settings, "tunnelDashboardAccess")?, false);
    assert_eq!(field(&settings, "tunnelUrl")?, "");
    assert_eq!(field(&settings, "tailscaleUrl")?, "");
    assert_eq!(field(&settings, "outboundProxyEnabled")?, false);

    assert_eq!(providers_response.status, StatusCode::OK);
    assert!(
        providers_response
            .content_type
            .starts_with("application/json")
    );
    assert_eq!(providers, json!({ "connections": [] }));
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
