//! `preferredConnectionId` pins credential selection to one account.
//!
//! Async video jobs are account-bound upstream: only the account that created a job
//! can poll it. Selection therefore has to honour a caller's pin — but not at the
//! cost of the cooldowns that protect a failing account, so a pin to an unavailable
//! connection falls back to the normal strategy rather than being obeyed.

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
    Ok((status, serde_json::from_slice(&body)?))
}

/// Create two xAI connections with distinct priorities.
///
/// `high` has the lower priority number, so fill-first selects it by default; any
/// selection of `low` is therefore the pin doing something.
async fn two_connections(store: &StateStore) -> TestResult<(String, String)> {
    let mut ids = Vec::new();
    for (name, key) in [("high", "sk-high"), ("low", "sk-low")] {
        let (status, created) = post(
            store.clone(),
            "/api/providers",
            &serde_json::json!({ "provider": "xai", "name": name, "apiKey": key }).to_string(),
        )
        .await?;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        ids.push(
            created
                .pointer("/connection/id")
                .and_then(Value::as_str)
                .ok_or("created connection has no id")?
                .to_owned(),
        );
    }
    let mut ids = ids.into_iter();
    let first = ids.next().ok_or("no first connection")?;
    let second = ids.next().ok_or("no second connection")?;
    Ok((first, second))
}

/// The connection a selection chose.
fn chosen(selection: &Value) -> Option<&str> {
    selection
        .pointer("/credentials/connectionId")
        .and_then(Value::as_str)
}

#[actix_rt::test]
async fn a_pin_selects_the_named_connection_instead_of_the_strategys_choice() -> TestResult {
    let store = StateStore::memory();
    let (first, second) = two_connections(&store).await?;

    // Unpinned: the strategy picks. Whichever it is, it is deterministic.
    let (status, unpinned) = post(
        store.clone(),
        "/internal/v1/credentials/select",
        r#"{"provider":"xai"}"#,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{unpinned}");
    let default_choice = chosen(&unpinned)
        .ok_or("no connection selected")?
        .to_owned();

    // Pinning to the *other* connection must move the selection.
    let other = if default_choice == first {
        second.clone()
    } else {
        first.clone()
    };
    let (status, pinned) = post(
        store.clone(),
        "/internal/v1/credentials/select",
        &serde_json::json!({ "provider": "xai", "preferredConnectionId": other }).to_string(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{pinned}");
    assert_eq!(
        chosen(&pinned),
        Some(other.as_str()),
        "the pin was ignored: {pinned}"
    );

    // And pinning to the default choice still yields it.
    let (_, repinned) = post(
        store.clone(),
        "/internal/v1/credentials/select",
        &serde_json::json!({ "provider": "xai", "preferredConnectionId": default_choice })
            .to_string(),
    )
    .await?;
    assert_eq!(chosen(&repinned), Some(default_choice.as_str()));
    Ok(())
}

#[actix_rt::test]
async fn a_pin_to_an_excluded_connection_falls_back_rather_than_being_obeyed() -> TestResult {
    let store = StateStore::memory();
    let (first, second) = two_connections(&store).await?;

    // Excluding a connection means it already failed in this request. Honouring a
    // pin to it would retry the account that just failed.
    let (status, selection) = post(
        store.clone(),
        "/internal/v1/credentials/select",
        &serde_json::json!({
            "provider": "xai",
            "exclude": [first.clone()],
            "preferredConnectionId": first.clone(),
        })
        .to_string(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{selection}");
    assert_eq!(
        chosen(&selection),
        Some(second.as_str()),
        "an excluded pin was honoured: {selection}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_pin_to_a_cooling_connection_falls_back_rather_than_being_obeyed() -> TestResult {
    let store = StateStore::memory();
    let (first, second) = two_connections(&store).await?;

    // Lock `first` the way a 429 would.
    let (status, locked) = post(
        store.clone(),
        "/internal/v1/credentials/unavailable",
        &serde_json::json!({
            "connectionId": first.clone(),
            "status": 429,
            "reason": "rate limited",
            "cooldownMs": 600_000,
        })
        .to_string(),
    )
    .await?;
    assert!(
        status.is_success(),
        "could not lock the connection: {status} {locked}"
    );

    let (status, selection) = post(
        store.clone(),
        "/internal/v1/credentials/select",
        &serde_json::json!({ "provider": "xai", "preferredConnectionId": first }).to_string(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{selection}");
    assert_eq!(
        chosen(&selection),
        Some(second.as_str()),
        "a pin defeated a cooldown: {selection}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_pin_to_an_unknown_connection_is_ignored_not_an_error() -> TestResult {
    let store = StateStore::memory();
    let (first, second) = two_connections(&store).await?;

    // A client echoing back a stale connection id must still get service.
    let (status, selection) = post(
        store.clone(),
        "/internal/v1/credentials/select",
        r#"{"provider":"xai","preferredConnectionId":"connection_gone"}"#,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{selection}");
    let picked = chosen(&selection).ok_or("nothing selected")?;
    assert!(
        picked == first || picked == second,
        "expected one of the real connections, got {picked}"
    );
    Ok(())
}
