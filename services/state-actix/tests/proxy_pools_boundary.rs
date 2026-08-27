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

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

fn string_field(json: &Value, name: &str) -> TestResult<String> {
    field(json, name)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| test_error(format!("{name} is not a string")))
}

#[actix_rt::test]
async fn proxy_pool_input_and_unknown_id_errors_return_structured_json() -> TestResult {
    let store = StateStore::memory();
    let cases = [
        (
            Method::POST,
            "/api/proxy-pools",
            "{",
            StatusCode::BAD_REQUEST,
            "Invalid JSON body",
        ),
        (
            Method::POST,
            "/api/proxy-pools",
            r#"{"proxyUrl":"http://127.0.0.1:8888"}"#,
            StatusCode::BAD_REQUEST,
            "Name is required",
        ),
        (
            Method::POST,
            "/api/proxy-pools",
            r#"{"name":"missing-url"}"#,
            StatusCode::BAD_REQUEST,
            "Proxy URL is required",
        ),
        (
            Method::GET,
            "/api/proxy-pools/missing",
            "",
            StatusCode::NOT_FOUND,
            "Proxy pool not found",
        ),
        (
            Method::PUT,
            "/api/proxy-pools/missing",
            r#"{"name":"Updated","proxyUrl":"http://127.0.0.1:8888"}"#,
            StatusCode::NOT_FOUND,
            "Proxy pool not found",
        ),
        (
            Method::DELETE,
            "/api/proxy-pools/missing",
            "",
            StatusCode::NOT_FOUND,
            "Proxy pool not found",
        ),
        (
            Method::POST,
            "/api/providers",
            r#"{"provider":"openai","apiKey":"sk-test","name":"bad pool","proxyPoolId":"missing"}"#,
            StatusCode::BAD_REQUEST,
            "Proxy pool not found",
        ),
    ];

    for (method, uri, body, expected_status, expected_error) in cases {
        let response = request_json(store.clone(), request(method, uri, body)).await?;
        assert_eq!(response.status, expected_status, "{uri}");
        assert_eq!(field(&response.body, "error")?, expected_error, "{uri}");
    }
    Ok(())
}

#[actix_rt::test]
async fn deleting_in_use_proxy_pool_returns_conflict_with_bound_count() -> TestResult {
    let store = StateStore::memory();
    let create_pool_body = json!({
        "name": "Bound pool",
        "proxyUrl": "http://127.0.0.1:8888",
    })
    .to_string();
    let created = request_json(
        store.clone(),
        request(Method::POST, "/api/proxy-pools", &create_pool_body),
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED);
    let pool_id = string_field(field(&created.body, "proxyPool")?, "id")?;

    for name in ["Bound provider one", "Bound provider two"] {
        let provider_body = json!({
            "provider": "openai",
            "apiKey": "sk-test",
            "name": name,
            "proxyPoolId": pool_id,
        })
        .to_string();
        let provider = request_json(
            store.clone(),
            request(Method::POST, "/api/providers", &provider_body),
        )
        .await?;
        assert_eq!(provider.status, StatusCode::CREATED);
    }

    let deleted = request_json(
        store,
        request(Method::DELETE, &format!("/api/proxy-pools/{pool_id}"), ""),
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::CONFLICT);
    assert_eq!(
        field(&deleted.body, "error")?,
        "Proxy pool is currently in use"
    );
    assert_eq!(field(&deleted.body, "boundConnectionCount")?, 2);
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
