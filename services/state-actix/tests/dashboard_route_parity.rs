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
async fn api_keys_remain_state_owned_json_crud_routes() -> TestResult {
    let store = StateStore::memory();

    let empty = get_json(store.clone(), "/api/keys").await?;
    assert_eq!(empty.status, StatusCode::OK);
    assert_eq!(field(&empty.body, "keys")?, &json!([]));

    let created = request_json(
        store.clone(),
        request(Method::POST, "/api/keys", r#"{"name":"dashboard key"}"#),
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(field(&created.body, "name")?, "dashboard key");
    assert_eq!(field(&created.body, "machineId")?, "nullrouter-state");
    assert_eq!(field(&created.body, "isActive")?, true);
    assert!(
        string_field(&created.body, "key")?.starts_with("nr_nullrouter_state_"),
        "state service should mint state-owned keys"
    );
    let id = string_field(&created.body, "id")?;

    let listed = get_json(store.clone(), "/api/keys").await?;
    assert_eq!(listed.status, StatusCode::OK);
    let listed_key = first_array_item(&listed.body, "keys")?;
    assert_eq!(field(listed_key, "id")?, id.as_str());
    assert_eq!(field(listed_key, "machineId")?, "nullrouter-state");

    let fetched = get_json(store.clone(), &format!("/api/keys/{id}")).await?;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(field(field(&fetched.body, "key")?, "id")?, id.as_str());

    let updated = request_json(
        store.clone(),
        request(
            Method::PUT,
            &format!("/api/keys/{id}"),
            r#"{"isActive":false}"#,
        ),
    )
    .await?;
    assert_eq!(updated.status, StatusCode::OK);
    assert_eq!(field(field(&updated.body, "key")?, "isActive")?, false);

    let deleted = request_json(
        store.clone(),
        request(Method::DELETE, &format!("/api/keys/{id}"), ""),
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::OK);
    assert_eq!(field(&deleted.body, "message")?, "Key deleted successfully");

    let empty_after_delete = get_json(store, "/api/keys").await?;
    assert_eq!(field(&empty_after_delete.body, "keys")?, &json!([]));
    Ok(())
}

#[actix_rt::test]
async fn api_providers_remain_state_owned_connection_crud_routes() -> TestResult {
    let store = StateStore::memory();

    let empty = get_json(store.clone(), "/api/providers").await?;
    assert_eq!(empty.status, StatusCode::OK);
    assert_eq!(field(&empty.body, "connections")?, &json!([]));
    missing_field(&empty.body, "providers")?;
    missing_field(&empty.body, "models")?;
    missing_field(&empty.body, "openaiModels")?;

    let created = request_json(
        store.clone(),
        request(
            Method::POST,
            "/api/providers",
            r#"{"provider":"openai","apiKey":"sk-state","name":"State OpenAI"}"#,
        ),
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED);
    let connection = field(&created.body, "connection")?;
    assert_eq!(field(connection, "provider")?, "openai");
    assert_eq!(field(connection, "name")?, "State OpenAI");
    assert_eq!(field(connection, "authType")?, "apikey");
    assert_eq!(field(connection, "isActive")?, true);
    missing_field(connection, "apiKey")?;
    let id = string_field(connection, "id")?;

    let listed = get_json(store.clone(), "/api/providers").await?;
    assert_eq!(listed.status, StatusCode::OK);
    let listed_connection = first_array_item(&listed.body, "connections")?;
    assert_eq!(field(listed_connection, "id")?, id.as_str());
    missing_field(listed_connection, "apiKey")?;

    let fetched = get_json(store.clone(), &format!("/api/providers/{id}")).await?;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(
        field(field(&fetched.body, "connection")?, "id")?,
        id.as_str()
    );

    let updated = request_json(
        store.clone(),
        request(
            Method::PUT,
            &format!("/api/providers/{id}"),
            r#"{"name":"State OpenAI inactive","isActive":false}"#,
        ),
    )
    .await?;
    assert_eq!(updated.status, StatusCode::OK);
    let updated_connection = field(&updated.body, "connection")?;
    assert_eq!(field(updated_connection, "name")?, "State OpenAI inactive");
    assert_eq!(field(updated_connection, "isActive")?, false);

    let deleted = request_json(
        store.clone(),
        request(Method::DELETE, &format!("/api/providers/{id}"), ""),
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::OK);
    assert_eq!(
        field(&deleted.body, "message")?,
        "Connection deleted successfully"
    );

    let empty_after_delete = get_json(store, "/api/providers").await?;
    assert_eq!(field(&empty_after_delete.body, "connections")?, &json!([]));
    Ok(())
}

#[actix_rt::test]
async fn api_settings_remain_state_owned_default_and_update_routes() -> TestResult {
    let store = StateStore::memory();

    let defaults = get_json(store.clone(), "/api/settings").await?;
    assert_eq!(defaults.status, StatusCode::OK);
    assert_eq!(field(&defaults.body, "requireLogin")?, true);
    assert_eq!(field(&defaults.body, "tunnelDashboardAccess")?, false);
    assert_eq!(field(&defaults.body, "tunnelUrl")?, "");
    assert_eq!(field(&defaults.body, "tailscaleUrl")?, "");
    assert_eq!(field(&defaults.body, "outboundProxyEnabled")?, false);
    assert_eq!(field(&defaults.body, "outboundProxyUrl")?, "");
    assert_eq!(field(&defaults.body, "outboundNoProxy")?, "");
    missing_field(&defaults.body, "requireApiKey")?;
    missing_field(&defaults.body, "hasPassword")?;
    missing_field(&defaults.body, "oidcConfigured")?;

    let put = request_json(
        store.clone(),
        request(
            Method::PUT,
            "/api/settings",
            r#"{"requireLogin":false,"outboundProxyEnabled":true,"outboundProxyUrl":"http://127.0.0.1:8888","outboundNoProxy":"localhost"}"#,
        ),
    )
    .await?;
    assert_eq!(put.status, StatusCode::OK);
    assert_eq!(field(&put.body, "requireLogin")?, false);
    assert_eq!(field(&put.body, "outboundProxyEnabled")?, true);

    let post = request_json(
        store.clone(),
        request(
            Method::POST,
            "/api/settings",
            r#"{"tunnelDashboardAccess":true,"tunnelUrl":"https://state.example","tailscaleUrl":"https://tail.example"}"#,
        ),
    )
    .await?;
    assert_eq!(post.status, StatusCode::OK);
    assert_eq!(field(&post.body, "requireLogin")?, false);
    assert_eq!(field(&post.body, "tunnelDashboardAccess")?, true);
    assert_eq!(field(&post.body, "tunnelUrl")?, "https://state.example");
    assert_eq!(field(&post.body, "tailscaleUrl")?, "https://tail.example");
    assert_eq!(field(&post.body, "outboundProxyEnabled")?, true);

    let fetched = get_json(store, "/api/settings").await?;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(field(&fetched.body, "requireLogin")?, false);
    assert_eq!(field(&fetched.body, "tunnelDashboardAccess")?, true);
    assert_eq!(
        field(&fetched.body, "outboundProxyUrl")?,
        "http://127.0.0.1:8888"
    );
    assert_eq!(field(&fetched.body, "outboundNoProxy")?, "localhost");
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
