//! Persisting a refreshed OAuth credential.
//!
//! The endpoint existed but nothing called it, and it replaced `providerSpecificData`
//! wholesale. A refresh carries only `lastRefreshAt` and sometimes an `idToken`, so a
//! replacing write would drop the connection's `baseUrl`, region, and proxy settings
//! on the first token rotation — turning a working connection into one that dials
//! the wrong host.

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

async fn post(store: StateStore, uri: &str, body: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(store))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::POST)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body = to_bytes(res.into_body()).await?;
    Ok((status, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

/// Create an OAuth connection carrying settings a refresh must not disturb.
async fn oauth_connection(store: &StateStore) -> TestResult<String> {
    let (status, created) = post(
        store.clone(),
        "/api/providers",
        &json!({
            "provider": "claude",
            "name": "oauth account",
            "apiKey": "sk-placeholder",
            "providerSpecificData": {
                "baseUrl": "https://proxy.example/v1",
                "region": "sgp",
                "connectionProxyEnabled": true,
                "connectionProxyUrl": "socks5://127.0.0.1:1080",
            },
        })
        .to_string(),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    Ok(created
        .pointer("/connection/id")
        .and_then(Value::as_str)
        .ok_or("created connection has no id")?
        .to_owned())
}

/// The stored connection, secrets included.
fn stored(store: &StateStore, id: &str) -> Option<nullrouter_state::ProviderConnection> {
    store
        .list_connections_for_test()
        .into_iter()
        .find(|connection| connection.id == id)
}

#[actix_rt::test]
async fn a_refresh_replaces_the_tokens_and_merges_the_settings() -> TestResult {
    let store = StateStore::memory();
    let id = oauth_connection(&store).await?;

    let (status, body) = post(
        store.clone(),
        "/internal/v1/credentials/refresh",
        &json!({
            "connectionId": id,
            "accessToken": "new-access",
            "refreshToken": "new-refresh",
            "expiresAt": "2030-01-01T00:00:00Z",
            "providerSpecificData": {
                "lastRefreshAt": "2024-06-01T00:00:00Z",
                "idToken": "id-token",
            },
        })
        .to_string(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    let connection = stored(&store, &id).ok_or("connection vanished")?;
    assert_eq!(connection.access_token.as_deref(), Some("new-access"));
    assert_eq!(connection.refresh_token.as_deref(), Some("new-refresh"));
    assert_eq!(
        connection.expires_at.as_deref(),
        Some("2030-01-01T00:00:00Z")
    );

    let settings = connection
        .provider_specific_data
        .as_ref()
        .ok_or("settings vanished")?;
    // The refresh's own keys landed.
    assert_eq!(
        settings.get("lastRefreshAt"),
        Some(&json!("2024-06-01T00:00:00Z"))
    );
    assert_eq!(settings.get("idToken"), Some(&json!("id-token")));
    // And nothing else was lost. A replacing write would send every later request
    // to the provider's default host instead of the configured proxy.
    assert_eq!(
        settings.get("baseUrl"),
        Some(&json!("https://proxy.example/v1"))
    );
    assert_eq!(settings.get("region"), Some(&json!("sgp")));
    assert_eq!(settings.get("connectionProxyEnabled"), Some(&json!(true)));
    assert_eq!(
        settings.get("connectionProxyUrl"),
        Some(&json!("socks5://127.0.0.1:1080"))
    );
    Ok(())
}

#[actix_rt::test]
async fn a_refresh_that_rotates_no_token_leaves_the_current_one_in_place() -> TestResult {
    let store = StateStore::memory();
    let id = oauth_connection(&store).await?;

    // Seed a refresh token, then send an update that omits it.
    post(
        store.clone(),
        "/internal/v1/credentials/refresh",
        &json!({ "connectionId": id, "refreshToken": "refresh-1" }).to_string(),
    )
    .await?;
    let (status, body) = post(
        store.clone(),
        "/internal/v1/credentials/refresh",
        &json!({ "connectionId": id, "accessToken": "access-2" }).to_string(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    let connection = stored(&store, &id).ok_or("connection vanished")?;
    assert_eq!(connection.access_token.as_deref(), Some("access-2"));
    // Clearing it here would leave no way to refresh again.
    assert_eq!(connection.refresh_token.as_deref(), Some("refresh-1"));
    Ok(())
}

#[actix_rt::test]
async fn a_refresh_for_an_unknown_connection_is_a_404() -> TestResult {
    let store = StateStore::memory();
    let (status, body) = post(
        store,
        "/internal/v1/credentials/refresh",
        &json!({ "connectionId": "connection_gone", "accessToken": "a" }).to_string(),
    )
    .await?;
    // Not a silent success: the caller needs to know its token was not stored, or
    // it will assume a rotation landed that did not.
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    Ok(())
}

#[actix_rt::test]
async fn a_refreshed_token_is_handed_out_by_the_next_selection() -> TestResult {
    // The point of persisting: the next request must use the new token, not the
    // expired one it replaced.
    let store = StateStore::memory();
    let id = oauth_connection(&store).await?;

    post(
        store.clone(),
        "/internal/v1/credentials/refresh",
        &json!({
            "connectionId": id,
            "accessToken": "rotated-access",
            "refreshToken": "rotated-refresh",
            "expiresAt": "2030-01-01T00:00:00Z",
        })
        .to_string(),
    )
    .await?;

    let (status, selection) = post(
        store,
        "/internal/v1/credentials/select",
        r#"{"provider":"claude"}"#,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{selection}");
    assert_eq!(
        selection.pointer("/credentials/accessToken"),
        Some(&json!("rotated-access")),
        "{selection}"
    );
    assert_eq!(
        selection.pointer("/credentials/refreshToken"),
        Some(&json!("rotated-refresh")),
        "{selection}"
    );
    assert_eq!(
        selection.pointer("/credentials/expiresAt"),
        Some(&json!("2030-01-01T00:00:00Z")),
        "{selection}"
    );
    Ok(())
}
