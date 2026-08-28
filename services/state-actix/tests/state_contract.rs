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

async fn get_json(store: StateStore, uri: &str) -> TestResult<(StatusCode, Value)> {
    request_json(store, Method::GET, uri, "").await
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
async fn key_crud_uses_upstream_envelopes_and_status_codes() -> TestResult {
    let store = StateStore::memory();

    let (create_status, created) = request_json(
        store.clone(),
        Method::POST,
        "/api/keys",
        r#"{"name":"local dev"}"#,
    )
    .await?;
    assert_eq!(create_status, StatusCode::CREATED);
    assert_eq!(field(&created, "name")?, "local dev");
    assert_eq!(field(&created, "machineId")?, "nullrouter-state");
    let id = string_field(&created, "id")?;

    let (list_status, list) = get_json(store.clone(), "/api/keys").await?;
    assert_eq!(list_status, StatusCode::OK);
    assert_eq!(field(&list, "keys")?.as_array().map(Vec::len), Some(1));

    let (get_status, fetched) = get_json(store.clone(), &format!("/api/keys/{id}")).await?;
    assert_eq!(get_status, StatusCode::OK);
    assert_eq!(field(field(&fetched, "key")?, "id")?, id.as_str());

    let (update_status, updated) = request_json(
        store.clone(),
        Method::PUT,
        &format!("/api/keys/{id}"),
        r#"{"isActive":false}"#,
    )
    .await?;
    assert_eq!(update_status, StatusCode::OK);
    assert_eq!(field(field(&updated, "key")?, "isActive")?, false);

    let (delete_status, deleted) = request_json(
        store.clone(),
        Method::DELETE,
        &format!("/api/keys/{id}"),
        "",
    )
    .await?;
    assert_eq!(delete_status, StatusCode::OK);
    assert_eq!(field(&deleted, "message")?, "Key deleted successfully");

    let (missing_status, _) = get_json(store, &format!("/api/keys/{id}")).await?;
    assert_eq!(missing_status, StatusCode::NOT_FOUND);
    Ok(())
}

#[actix_rt::test]
async fn provider_combo_proxy_and_settings_crud_are_stateful() -> TestResult {
    let store = StateStore::memory();

    let (combo_status, combo) = request_json(
        store.clone(),
        Method::POST,
        "/api/combos",
        r#"{"name":"fast_lane","kind":"llm","models":["openai/gpt-5"]}"#,
    )
    .await?;
    assert_eq!(combo_status, StatusCode::CREATED);
    let combo_id = string_field(&combo, "id")?;

    let (combo_update_status, combo_update) = request_json(
        store.clone(),
        Method::PUT,
        &format!("/api/combos/{combo_id}"),
        r#"{"name":"fast_lane_v2","models":["anthropic/claude"]}"#,
    )
    .await?;
    assert_eq!(combo_update_status, StatusCode::OK);
    assert_eq!(field(&combo_update, "name")?, "fast_lane_v2");

    let (pool_status, pool_body) = request_json(
        store.clone(),
        Method::POST,
        "/api/proxy-pools",
        r#"{"name":"corp","proxyUrl":"http://127.0.0.1:8888","strictProxy":true}"#,
    )
    .await?;
    assert_eq!(pool_status, StatusCode::CREATED);
    let pool = field(&pool_body, "proxyPool")?;
    let pool_id = string_field(pool, "id")?;

    let provider_payload = format!(
        r#"{{"provider":"openai","apiKey":"sk-test","name":"OpenAI primary","proxyPoolId":"{pool_id}"}}"#
    );
    let (provider_status, provider_body) = request_json(
        store.clone(),
        Method::POST,
        "/api/providers",
        &provider_payload,
    )
    .await?;
    assert_eq!(provider_status, StatusCode::CREATED);
    let connection = field(&provider_body, "connection")?;
    assert!(field(connection, "apiKey").is_err());
    let provider_id = string_field(connection, "id")?;

    let (usage_status, usage_body) =
        get_json(store.clone(), "/api/proxy-pools?includeUsage=true").await?;
    assert_eq!(usage_status, StatusCode::OK);
    assert_eq!(
        field(
            field(&usage_body, "proxyPools")?
                .as_array()
                .and_then(|items| items.first())
                .ok_or_else(|| test_error("missing proxy pool"))?,
            "boundConnectionCount",
        )?,
        1
    );

    let (provider_update_status, provider_update) = request_json(
        store.clone(),
        Method::PUT,
        &format!("/api/providers/{provider_id}"),
        r#"{"name":"OpenAI secondary","isActive":false}"#,
    )
    .await?;
    assert_eq!(provider_update_status, StatusCode::OK);
    assert_eq!(
        field(field(&provider_update, "connection")?, "name")?,
        "OpenAI secondary"
    );
    assert_eq!(
        field(field(&provider_update, "connection")?, "isActive")?,
        false
    );

    let (settings_status, settings) = request_json(
        store.clone(),
        Method::PUT,
        "/api/settings",
        r#"{"tunnelDashboardAccess":true,"tunnelUrl":"https://example.test"}"#,
    )
    .await?;
    assert_eq!(settings_status, StatusCode::OK);
    assert_eq!(field(&settings, "tunnelDashboardAccess")?, true);
    assert_eq!(field(&settings, "tunnelUrl")?, "https://example.test");

    // The written value is readable back from the collection route. There is no
    // `/api/settings/require-login` to read it from: login is unconditional, so
    // the route and the flag were both removed rather than left to mislead.
    let (fetched_status, fetched) = get_json(store.clone(), "/api/settings").await?;
    assert_eq!(fetched_status, StatusCode::OK);
    assert_eq!(field(&fetched, "tunnelDashboardAccess")?, true);

    let (provider_delete_status, _) = request_json(
        store.clone(),
        Method::DELETE,
        &format!("/api/providers/{provider_id}"),
        "",
    )
    .await?;
    assert_eq!(provider_delete_status, StatusCode::OK);

    let (pool_delete_status, pool_deleted) = request_json(
        store.clone(),
        Method::DELETE,
        &format!("/api/proxy-pools/{pool_id}"),
        "",
    )
    .await?;
    assert_eq!(pool_delete_status, StatusCode::OK);
    assert_eq!(field(&pool_deleted, "success")?, true);

    let (combo_delete_status, combo_deleted) = request_json(
        store,
        Method::DELETE,
        &format!("/api/combos/{combo_id}"),
        "",
    )
    .await?;
    assert_eq!(combo_delete_status, StatusCode::OK);
    assert_eq!(field(&combo_deleted, "success")?, true);
    Ok(())
}

#[actix_rt::test]
async fn file_store_reloads_state_across_service_instances() -> TestResult {
    let tempdir = tempfile::tempdir()?;
    let state_file = tempdir.path().join("state.json");
    let store = StateStore::file(&state_file)?;

    let (key_status, _) = request_json(
        store.clone(),
        Method::POST,
        "/api/keys",
        r#"{"name":"persisted"}"#,
    )
    .await?;
    let (combo_status, _) = request_json(
        store,
        Method::POST,
        "/api/combos",
        r#"{"name":"persisted_combo","models":["openai/gpt-5"]}"#,
    )
    .await?;
    assert_eq!(key_status, StatusCode::CREATED);
    assert_eq!(combo_status, StatusCode::CREATED);

    let reloaded = StateStore::file(&state_file)?;
    let (keys_status, keys) = get_json(reloaded.clone(), "/api/keys").await?;
    let (combos_status, combos) = get_json(reloaded, "/api/combos").await?;
    assert_eq!(keys_status, StatusCode::OK);
    assert_eq!(combos_status, StatusCode::OK);
    assert_eq!(field(&keys, "keys")?.as_array().map(Vec::len), Some(1));
    assert_eq!(field(&combos, "combos")?.as_array().map(Vec::len), Some(1));
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
