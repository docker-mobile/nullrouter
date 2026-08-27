#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_state::{StateStore, configure};
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn request_json(
    store: StateStore,
    method: Method,
    uri: &str,
    body: &str,
) -> TestResult<(StatusCode, Value)> {
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
    let body = to_bytes(res.into_body()).await?;
    Ok((status, serde_json::from_slice(&body)?))
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
async fn malformed_missing_and_unknown_inputs_return_structured_json() -> TestResult {
    let store = StateStore::memory();

    let cases = [
        (
            Method::POST,
            "/api/keys",
            "{",
            StatusCode::BAD_REQUEST,
            "Invalid JSON body",
        ),
        (
            Method::POST,
            "/api/keys",
            "{}",
            StatusCode::BAD_REQUEST,
            "Name is required",
        ),
        (
            Method::POST,
            "/api/providers",
            r#"{"provider":"openai"}"#,
            StatusCode::BAD_REQUEST,
            "API Key is required",
        ),
        (
            Method::POST,
            "/api/proxy-pools",
            r#"{"name":"missing-url"}"#,
            StatusCode::BAD_REQUEST,
            "Proxy URL is required",
        ),
        (
            Method::PUT,
            "/api/settings",
            "{",
            StatusCode::BAD_REQUEST,
            "Invalid JSON body",
        ),
    ];

    for (method, uri, body, expected_status, expected_error) in cases {
        let (status, json) = request_json(store.clone(), method, uri, body).await?;
        assert_eq!(status, expected_status, "{uri}");
        assert_eq!(field(&json, "error")?, expected_error, "{uri}");
    }

    let (missing_status, missing) =
        request_json(store, Method::DELETE, "/api/keys/missing", "").await?;
    assert_eq!(missing_status, StatusCode::NOT_FOUND);
    assert_eq!(field(&missing, "error")?, "Key not found");
    Ok(())
}

#[actix_rt::test]
async fn duplicate_combos_missing_proxy_pools_and_in_use_deletes_are_rejected() -> TestResult {
    let store = StateStore::memory();

    let (first_status, first_combo) = request_json(
        store.clone(),
        Method::POST,
        "/api/combos",
        r#"{"name":"dupe","models":[]}"#,
    )
    .await?;
    assert_eq!(first_status, StatusCode::CREATED);
    let combo_id = string_field(&first_combo, "id")?;

    let (dupe_status, dupe) = request_json(
        store.clone(),
        Method::POST,
        "/api/combos",
        r#"{"name":"dupe","models":[]}"#,
    )
    .await?;
    assert_eq!(dupe_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&dupe, "error")?, "Combo name already exists");

    let (invalid_update_status, invalid_update) = request_json(
        store.clone(),
        Method::PUT,
        &format!("/api/combos/{combo_id}"),
        r#"{"name":"bad name"}"#,
    )
    .await?;
    assert_eq!(invalid_update_status, StatusCode::BAD_REQUEST);
    assert_eq!(
        field(&invalid_update, "error")?,
        "Name can only contain letters, numbers, -, _ and ."
    );

    let (missing_pool_status, missing_pool) = request_json(
        store.clone(),
        Method::POST,
        "/api/providers",
        r#"{"provider":"openai","apiKey":"sk-test","name":"with missing pool","proxyPoolId":"nope"}"#,
    )
    .await?;
    assert_eq!(missing_pool_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&missing_pool, "error")?, "Proxy pool not found");

    let (pool_status, pool_body) = request_json(
        store.clone(),
        Method::POST,
        "/api/proxy-pools",
        r#"{"name":"bound","proxyUrl":"http://127.0.0.1:8888"}"#,
    )
    .await?;
    assert_eq!(pool_status, StatusCode::CREATED);
    let pool_id = string_field(field(&pool_body, "proxyPool")?, "id")?;
    let provider_payload = format!(
        r#"{{"provider":"openai","apiKey":"sk-test","name":"bound provider","proxyPoolId":"{pool_id}"}}"#
    );
    let (provider_status, _) = request_json(
        store.clone(),
        Method::POST,
        "/api/providers",
        &provider_payload,
    )
    .await?;
    assert_eq!(provider_status, StatusCode::CREATED);

    let (delete_status, delete_body) = request_json(
        store,
        Method::DELETE,
        &format!("/api/proxy-pools/{pool_id}"),
        "",
    )
    .await?;
    assert_eq!(delete_status, StatusCode::CONFLICT);
    assert_eq!(
        field(&delete_body, "error")?,
        "Proxy pool is currently in use"
    );
    assert_eq!(field(&delete_body, "boundConnectionCount")?, 1);
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
