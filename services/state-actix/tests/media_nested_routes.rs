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

struct RawResponse {
    status: StatusCode,
    content_type: String,
    body: String,
}

struct JsonResponse {
    status: StatusCode,
    body: Value,
}

struct JsonRequest<'a> {
    method: Method,
    uri: &'a str,
    body: &'a str,
}

const fn request<'a>(method: Method, uri: &'a str, body: &'a str) -> JsonRequest<'a> {
    JsonRequest { method, uri, body }
}

async fn request_raw(store: StateStore, request: JsonRequest<'_>) -> TestResult<RawResponse> {
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
    let bytes = to_bytes(res.into_body()).await?;
    Ok(RawResponse {
        status,
        content_type,
        body: String::from_utf8(bytes.to_vec())?,
    })
}

async fn request_json(store: StateStore, request: JsonRequest<'_>) -> TestResult<JsonResponse> {
    let uri = request.uri;
    let response = request_raw(store, request).await?;
    if !response.content_type.starts_with("application/json") {
        return Err(test_error(format!(
            "{uri} returned non-json content type {:?} with body {:?}",
            response.content_type, response.body
        )));
    }
    Ok(JsonResponse {
        status: response.status,
        body: serde_json::from_str(&response.body)?,
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
async fn custom_embedding_provider_nodes_are_state_owned_json_crud_routes() -> TestResult {
    let store = StateStore::memory();

    let empty = get_json(store.clone(), "/api/provider-nodes").await?;
    assert_eq!(empty.status, StatusCode::OK);
    assert_eq!(field(&empty.body, "nodes")?, &json!([]));

    let created = request_json(
        store.clone(),
        request(
            Method::POST,
            "/api/provider-nodes",
            r#"{"type":"custom-embedding","name":"Voyage","prefix":"voyage","baseUrl":"https://embed.example/v1/embeddings"}"#,
        ),
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED);
    let node = field(&created.body, "node")?;
    assert_eq!(field(node, "type")?, "custom-embedding");
    assert_eq!(field(node, "name")?, "Voyage");
    assert_eq!(field(node, "prefix")?, "voyage");
    assert_eq!(field(node, "baseUrl")?, "https://embed.example/v1");
    missing_field(node, "apiType")?;
    let id = string_field(node, "id")?;
    assert!(
        id.starts_with("custom-embedding-"),
        "custom embedding nodes must use the dashboard provider-id prefix"
    );

    let listed = get_json(store.clone(), "/api/provider-nodes").await?;
    assert_eq!(listed.status, StatusCode::OK);
    let listed_node = field(&listed.body, "nodes")?
        .as_array()
        .and_then(|nodes| nodes.first())
        .ok_or_else(|| test_error("missing provider node"))?;
    assert_eq!(field(listed_node, "id")?, id.as_str());

    let fetched = get_json(store.clone(), &format!("/api/provider-nodes/{id}")).await?;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(field(field(&fetched.body, "node")?, "id")?, id.as_str());

    let updated = request_json(
        store.clone(),
        request(
            Method::PUT,
            &format!("/api/provider-nodes/{id}"),
            r#"{"name":"Voyage updated","prefix":"voyage2","baseUrl":"https://embed.example/v2/embeddings"}"#,
        ),
    )
    .await?;
    assert_eq!(updated.status, StatusCode::OK);
    let updated_node = field(&updated.body, "node")?;
    assert_eq!(field(updated_node, "name")?, "Voyage updated");
    assert_eq!(field(updated_node, "prefix")?, "voyage2");
    assert_eq!(field(updated_node, "baseUrl")?, "https://embed.example/v2");

    let deleted = request_json(
        store.clone(),
        request(Method::DELETE, &format!("/api/provider-nodes/{id}"), ""),
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::OK);
    assert_eq!(field(&deleted.body, "success")?, true);

    let missing = get_json(store, &format!("/api/provider-nodes/{id}")).await?;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(field(&missing.body, "error")?, "Provider node not found");
    Ok(())
}

#[actix_rt::test]
async fn provider_node_errors_and_validation_are_structured_json() -> TestResult {
    let store = StateStore::memory();
    let cases = [
        (
            Method::POST,
            "/api/provider-nodes",
            "{",
            StatusCode::BAD_REQUEST,
            "Invalid JSON body",
        ),
        (
            Method::POST,
            "/api/provider-nodes",
            "{}",
            StatusCode::BAD_REQUEST,
            "Name is required",
        ),
        (
            Method::POST,
            "/api/provider-nodes",
            r#"{"name":"OpenAI compatible","prefix":"oa"}"#,
            StatusCode::BAD_REQUEST,
            "Invalid OpenAI compatible API type",
        ),
        (
            Method::POST,
            "/api/provider-nodes/validate",
            "{}",
            StatusCode::BAD_REQUEST,
            "Base URL and API key required",
        ),
    ];

    for (method, uri, body, expected_status, expected_error) in cases {
        let response = request_json(store.clone(), request(method, uri, body)).await?;
        assert_eq!(response.status, expected_status, "{uri}");
        assert_eq!(field(&response.body, "error")?, expected_error, "{uri}");
    }

    let missing = get_json(store, "/api/provider-nodes/custom-embedding-missing").await?;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(field(&missing.body, "error")?, "Provider node not found");
    Ok(())
}

#[actix_rt::test]
async fn combo_detail_routes_are_json_for_combo_1_missing_and_created_paths() -> TestResult {
    let store = StateStore::memory();

    let missing = get_json(store.clone(), "/api/combos/combo_1").await?;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(field(&missing.body, "error")?, "Combo not found");

    let created = request_json(
        store.clone(),
        request(
            Method::POST,
            "/api/combos",
            r#"{"name":"embedding_combo","kind":"embedding","models":["custom-embedding-1/text-embedding-3-large"]}"#,
        ),
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(field(&created.body, "id")?, "combo_1");
    assert_eq!(field(&created.body, "kind")?, "embedding");

    let fetched = get_json(store.clone(), "/api/combos/combo_1").await?;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(field(&fetched.body, "id")?, "combo_1");
    assert_eq!(
        field(&fetched.body, "models")?,
        &json!(["custom-embedding-1/text-embedding-3-large"])
    );

    let deleted = request_json(store, request(Method::DELETE, "/api/combos/combo_1", "")).await?;
    assert_eq!(deleted.status, StatusCode::OK);
    assert_eq!(field(&deleted.body, "success")?, true);
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
