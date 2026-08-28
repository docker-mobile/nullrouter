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
            "unexpected content type: {content_type}"
        )));
    }
    let body = to_bytes(res.into_body()).await?;
    Ok(JsonResponse {
        status,
        body: serde_json::from_slice(&body)?,
    })
}

async fn get_json(store: StateStore, uri: &str) -> TestResult<JsonResponse> {
    request_json(store, request(Method::GET, uri, "")).await
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

fn first_array_item<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    array_field(json, name)?
        .first()
        .ok_or_else(|| test_error(format!("missing {name} item")))
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

#[actix_rt::test]
async fn state_owned_routes_keep_structured_json_contracts_during_translator_work() -> TestResult {
    // Given: state data that exercises provider, combo, proxy-pool, and settings routes.
    let store = StateStore::memory();

    let created_pool = request_json(
        store.clone(),
        request(
            Method::POST,
            "/api/proxy-pools",
            r#"{"name":"translator-regression","proxyUrl":"http://127.0.0.1:8888","strictProxy":true}"#,
        ),
    )
    .await?;
    assert_eq!(created_pool.status, StatusCode::CREATED);
    let pool = field(&created_pool.body, "proxyPool")?;
    let pool_id = string_field(pool, "id")?;

    let provider_payload = format!(
        r#"{{"provider":"openai","apiKey":"sk-state","name":"Translator regression provider","proxyPoolId":"{pool_id}"}}"#
    );
    let created_provider = request_json(
        store.clone(),
        request(Method::POST, "/api/providers", &provider_payload),
    )
    .await?;
    assert_eq!(created_provider.status, StatusCode::CREATED);
    let connection = field(&created_provider.body, "connection")?;
    let connection_id = string_field(connection, "id")?;
    assert_eq!(field(connection, "provider")?, "openai");
    assert_eq!(
        field(field(connection, "providerSpecificData")?, "proxyPoolId")?,
        pool_id.as_str()
    );
    missing_field(connection, "apiKey")?;

    let created_combo = request_json(
        store.clone(),
        request(
            Method::POST,
            "/api/combos",
            r#"{"name":"translator_state_combo","kind":"llm","models":["openai/gpt-5"]}"#,
        ),
    )
    .await?;
    assert_eq!(created_combo.status, StatusCode::CREATED);
    let combo_id = string_field(&created_combo.body, "id")?;

    let updated_settings = request_json(
        store.clone(),
        request(
            Method::PUT,
            "/api/settings",
            r#"{"tunnelDashboardAccess":true,"tunnelUrl":"https://translator.example","outboundProxyEnabled":true,"outboundProxyUrl":"http://proxy.example:8080"}"#,
        ),
    )
    .await?;
    assert_eq!(updated_settings.status, StatusCode::OK);

    // When: translator-facing work has not touched these state-owned routes.
    let providers = get_json(store.clone(), "/api/providers").await?;
    let combos = get_json(store.clone(), "/api/combos").await?;
    let proxy_pools = get_json(store.clone(), "/api/proxy-pools?includeUsage=true").await?;
    let settings = get_json(store, "/api/settings").await?;

    // Then: each route still returns its state JSON envelope and core fields.
    assert_eq!(providers.status, StatusCode::OK);
    let listed_connection = first_array_item(&providers.body, "connections")?;
    assert_eq!(field(listed_connection, "id")?, connection_id.as_str());
    assert_eq!(
        field(
            field(listed_connection, "providerSpecificData")?,
            "proxyPoolId"
        )?,
        pool_id.as_str()
    );
    missing_field(&providers.body, "providers")?;

    assert_eq!(combos.status, StatusCode::OK);
    let listed_combo = first_array_item(&combos.body, "combos")?;
    assert_eq!(field(listed_combo, "id")?, combo_id.as_str());
    assert_eq!(field(listed_combo, "models")?, &json!(["openai/gpt-5"]));

    assert_eq!(proxy_pools.status, StatusCode::OK);
    let listed_pool = first_array_item(&proxy_pools.body, "proxyPools")?;
    assert_eq!(field(listed_pool, "id")?, pool_id.as_str());
    assert_eq!(field(listed_pool, "boundConnectionCount")?, 1);

    assert_eq!(settings.status, StatusCode::OK);
    assert_eq!(field(&settings.body, "tunnelDashboardAccess")?, true);
    assert_eq!(field(&settings.body, "outboundProxyEnabled")?, true);
    assert_eq!(
        field(&settings.body, "outboundProxyUrl")?,
        "http://proxy.example:8080"
    );
    // `requireLogin` is not part of the settings shape: dashboard login is
    // unconditional, so there is no flag for translator work to have disturbed.
    missing_field(&settings.body, "requireLogin")?;
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
