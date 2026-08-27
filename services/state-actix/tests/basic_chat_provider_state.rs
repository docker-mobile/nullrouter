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

#[actix_rt::test]
async fn basic_chat_gets_empty_connections_for_no_providers_connected_yet() -> TestResult {
    let store = StateStore::memory();

    let response = get_json(store, "/api/providers").await?;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(field(&response.body, "connections")?, &json!([]));
    missing_field(&response.body, "providers")?;
    missing_field(&response.body, "models")?;
    missing_field(&response.body, "openaiModels")?;
    Ok(())
}

#[actix_rt::test]
async fn basic_chat_gets_stateful_provider_connection_model_fields() -> TestResult {
    let store = StateStore::memory();

    let created = request_json(
        store.clone(),
        request(
            Method::POST,
            "/api/providers",
            r#"{
                "provider":"openai-compatible-1",
                "apiKey":"sk-state",
                "name":"State OpenAI Compatible",
                "defaultModel":"openai-compatible-1/gpt-5-mini",
                "globalPriority":10,
                "providerSpecificData":{
                    "apiType":"chat",
                    "baseUrl":"https://llm.example/v1",
                    "enabledModels":["gpt-5-mini","gpt-5-nano"],
                    "prefix":"openai-compatible-1"
                }
            }"#,
        ),
    )
    .await?;

    assert_eq!(created.status, StatusCode::CREATED);
    let connection = field(&created.body, "connection")?;
    assert_basic_chat_connection_shape(connection)?;
    let id = string_field(connection, "id")?;

    let listed = get_json(store.clone(), "/api/providers").await?;
    assert_eq!(listed.status, StatusCode::OK);
    let listed_connection = field(&listed.body, "connections")?
        .as_array()
        .and_then(|connections| connections.first())
        .ok_or_else(|| test_error("missing provider connection"))?;
    assert_eq!(field(listed_connection, "id")?, id.as_str());
    assert_basic_chat_connection_shape(listed_connection)?;

    let fetched = get_json(store.clone(), &format!("/api/providers/{id}")).await?;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_basic_chat_connection_shape(field(&fetched.body, "connection")?)?;

    let updated = request_json(
        store.clone(),
        request(
            Method::PUT,
            &format!("/api/providers/{id}"),
            r#"{"isActive":false,"defaultModel":"openai-compatible-1/gpt-5-nano"}"#,
        ),
    )
    .await?;
    assert_eq!(updated.status, StatusCode::OK);
    let updated_connection = field(&updated.body, "connection")?;
    assert_eq!(field(updated_connection, "isActive")?, false);
    assert_eq!(
        field(updated_connection, "defaultModel")?,
        "openai-compatible-1/gpt-5-nano"
    );
    missing_field(updated_connection, "apiKey")?;

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

    let empty = get_json(store, "/api/providers").await?;
    assert_eq!(field(&empty.body, "connections")?, &json!([]));
    Ok(())
}

fn assert_basic_chat_connection_shape(connection: &Value) -> TestResult {
    assert_eq!(field(connection, "provider")?, "openai-compatible-1");
    assert_eq!(field(connection, "name")?, "State OpenAI Compatible");
    assert_eq!(field(connection, "authType")?, "apikey");
    assert_eq!(field(connection, "isActive")?, true);
    assert_eq!(
        field(connection, "defaultModel")?,
        "openai-compatible-1/gpt-5-mini"
    );
    assert_eq!(field(connection, "globalPriority")?, 10);
    let provider_data = field(connection, "providerSpecificData")?;
    assert_eq!(field(provider_data, "apiType")?, "chat");
    assert_eq!(field(provider_data, "baseUrl")?, "https://llm.example/v1");
    assert_eq!(
        field(provider_data, "enabledModels")?,
        &json!(["gpt-5-mini", "gpt-5-nano"])
    );
    assert_eq!(field(provider_data, "prefix")?, "openai-compatible-1");
    missing_field(connection, "apiKey")?;
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
