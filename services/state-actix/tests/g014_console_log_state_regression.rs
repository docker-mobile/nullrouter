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

struct JsonRequest<'a> {
    method: Method,
    uri: &'a str,
    body: &'a str,
}

struct JsonResponse {
    status: StatusCode,
    body: Value,
}

const fn request<'a>(method: Method, uri: &'a str, body: &'a str) -> JsonRequest<'a> {
    JsonRequest { method, uri, body }
}

async fn request_json(store: StateStore, request: JsonRequest<'_>) -> TestResult<JsonResponse> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(store))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(request.method)
        .uri(request.uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(request.body.to_owned())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if !content_type.starts_with("application/json") {
        return Err(test_error(format!(
            "unexpected content type for {}: {content_type}",
            request.uri
        )));
    }
    let body = to_bytes(res.into_body()).await?;
    Ok(JsonResponse {
        status,
        body: serde_json::from_slice(&body)?,
    })
}

async fn request_status(store: StateStore, request: JsonRequest<'_>) -> StatusCode {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(store))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(request.method)
        .uri(request.uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(request.body.to_owned())
        .to_request();
    test::call_service(&app, req).await.status()
}

async fn get_json(store: StateStore, uri: &str) -> TestResult<JsonResponse> {
    request_json(store, request(Method::GET, uri, "")).await
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

fn missing_field(json: &Value, name: &str) -> TestResult {
    if json.get(name).is_some() {
        return Err(test_error(format!("unexpected field {name}")));
    }
    Ok(())
}

fn string_field(json: &Value, name: &str) -> TestResult<String> {
    field(json, name)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| test_error(format!("{name} is not a string")))
}

fn first_array_item<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    field(json, name)?
        .as_array()
        .and_then(|items| items.first())
        .ok_or_else(|| test_error(format!("{name} is empty")))
}

#[actix_rt::test]
async fn g014_console_log_work_keeps_state_json_routes_structured_and_stateful() -> TestResult {
    // Given: console-log API and SSE routes are not owned by the state service.
    let store = StateStore::memory();
    for uri in [
        "/api/translator/console-logs",
        "/api/translator/console-logs/stream",
    ] {
        let status = request_status(store.clone(), request(Method::GET, uri, "")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }

    // When: state-owned dashboard JSON routes mutate settings, proxy pools, providers, and keys.
    let settings = request_json(
        store.clone(),
        request(
            Method::PUT,
            "/api/settings",
            r#"{
                "tunnelDashboardAccess":true,
                "tunnelUrl":"https://state.example",
                "outboundProxyEnabled":true,
                "outboundProxyUrl":"http://127.0.0.1:8888",
                "outboundNoProxy":"localhost"
            }"#,
        ),
    )
    .await?;
    assert_eq!(settings.status, StatusCode::OK);
    assert_eq!(field(&settings.body, "tunnelDashboardAccess")?, true);

    let pool = request_json(
        store.clone(),
        request(
            Method::POST,
            "/api/proxy-pools",
            r#"{
                "name":"G014 pool",
                "proxyUrl":"http://127.0.0.1:8888",
                "strictProxy":true,
                "type":"http"
            }"#,
        ),
    )
    .await?;
    assert_eq!(pool.status, StatusCode::CREATED);
    let pool_id = string_field(field(&pool.body, "proxyPool")?, "id")?;

    let provider_body = json!({
        "provider": "openai",
        "apiKey": "sk-g014",
        "name": "G014 OpenAI",
        "proxyPoolId": pool_id,
    })
    .to_string();
    let provider = request_json(
        store.clone(),
        request(Method::POST, "/api/providers", &provider_body),
    )
    .await?;
    assert_eq!(provider.status, StatusCode::CREATED);
    let connection = field(&provider.body, "connection")?;
    let provider_id = string_field(connection, "id")?;
    assert_eq!(field(connection, "provider")?, "openai");
    missing_field(connection, "apiKey")?;

    let key = request_json(
        store.clone(),
        request(Method::POST, "/api/keys", r#"{"name":"G014 key"}"#),
    )
    .await?;
    assert_eq!(key.status, StatusCode::CREATED);
    let key_id = string_field(&key.body, "id")?;

    // Then: the state-owned JSON routes keep their upstream envelopes and persisted state.
    let fetched_settings = get_json(store.clone(), "/api/settings").await?;
    assert_eq!(fetched_settings.status, StatusCode::OK);
    assert_eq!(
        field(&fetched_settings.body, "tunnelDashboardAccess")?,
        true
    );
    assert_eq!(
        field(&fetched_settings.body, "outboundProxyUrl")?,
        "http://127.0.0.1:8888"
    );
    missing_field(&fetched_settings.body, "requireApiKey")?;

    let providers = get_json(store.clone(), "/api/providers").await?;
    assert_eq!(providers.status, StatusCode::OK);
    let listed_connection = first_array_item(&providers.body, "connections")?;
    assert_eq!(field(listed_connection, "id")?, provider_id.as_str());
    missing_field(&providers.body, "providers")?;
    missing_field(listed_connection, "apiKey")?;

    let proxy_pools = get_json(store.clone(), "/api/proxy-pools?includeUsage=true").await?;
    assert_eq!(proxy_pools.status, StatusCode::OK);
    let listed_pool = first_array_item(&proxy_pools.body, "proxyPools")?;
    assert_eq!(field(listed_pool, "id")?, pool_id.as_str());
    assert_eq!(field(listed_pool, "boundConnectionCount")?, 1);

    let keys = get_json(store, "/api/keys").await?;
    assert_eq!(keys.status, StatusCode::OK);
    let listed_key = first_array_item(&keys.body, "keys")?;
    assert_eq!(field(listed_key, "id")?, key_id.as_str());
    assert_eq!(field(listed_key, "machineId")?, "nullrouter-state");
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
