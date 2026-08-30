//! Per-combo strategy overrides: `settings.comboStrategies`.
//!
//! A combo without an entry uses the global `comboStrategy`; a combo with one ignores
//! it for itself alone. The runtime's own use of that is tested in
//! `runtime-actix/tests/fusion_combo.rs`; what this file owns is the storage contract —
//! that an override round-trips through `/api/settings`, that it reaches the runtime on
//! `/internal/v1/routing-context`, and that removing one is possible at all.
//!
//! That last point is why the map is replaced rather than merged. Upstream's dashboard
//! prunes an entry when a combo returns to the default, so a merging write would let a
//! combo be given an override and never have it taken away.

#![allow(clippy::future_not_send)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions read clearer with direct expect than with error plumbing"
)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_state::{StateStore, configure};
use serde_json::{Value, json};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn call(
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
    Ok((status, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

#[actix_web::test]
async fn an_override_round_trips_through_the_settings_route() -> TestResult {
    let store = StateStore::memory();
    let (status, written) = call(
        store.clone(),
        Method::PUT,
        "/api/settings",
        &json!({
            "comboStrategies": {
                "coding-fallback": {
                    "fallbackStrategy": "fusion",
                    "minPanel": 3,
                    "stragglerGraceMs": 2000,
                    "panelHardTimeoutMs": 45000,
                },
            },
        })
        .to_string(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        written.pointer("/comboStrategies/coding-fallback/fallbackStrategy"),
        Some(&Value::String("fusion".to_owned()))
    );
    assert_eq!(
        written
            .pointer("/comboStrategies/coding-fallback/minPanel")
            .and_then(Value::as_u64),
        Some(3)
    );

    // Readable back, which is what the dashboard's own page load depends on.
    let (status, fetched) = call(store, Method::GET, "/api/settings", "").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        fetched
            .pointer("/comboStrategies/coding-fallback/panelHardTimeoutMs")
            .and_then(Value::as_u64),
        Some(45_000)
    );
    Ok(())
}

#[actix_web::test]
async fn a_fresh_store_reports_an_empty_map_rather_than_omitting_it() -> TestResult {
    // The dashboard reads `settings.comboStrategies || {}`; an absent key works there,
    // but a present empty map is what lets a typed client parse the field without
    // treating it as optional.
    let store = StateStore::memory();
    let (status, settings) = call(store, Method::GET, "/api/settings", "").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        settings.get("comboStrategies"),
        Some(&json!({})),
        "expected an empty map, got {:?}",
        settings.get("comboStrategies")
    );
    Ok(())
}

#[actix_web::test]
async fn a_write_replaces_the_map_so_an_override_can_be_removed() -> TestResult {
    let store = StateStore::memory();
    call(
        store.clone(),
        Method::PUT,
        "/api/settings",
        &json!({
            "comboStrategies": {
                "one": { "fallbackStrategy": "fusion" },
                "two": { "fallbackStrategy": "round-robin" },
            },
        })
        .to_string(),
    )
    .await?;

    // Upstream's dashboard sends the whole pruned map when a combo returns to the
    // default. A merge here would make "two" permanent.
    let (status, written) = call(
        store.clone(),
        Method::PUT,
        "/api/settings",
        &json!({ "comboStrategies": { "one": { "fallbackStrategy": "fusion" } } }).to_string(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert!(
        written.pointer("/comboStrategies/one").is_some(),
        "the kept entry survived"
    );
    assert_eq!(
        written.pointer("/comboStrategies/two"),
        None,
        "a pruned entry must actually be removed, or an override could never be undone"
    );

    // And clearing the map entirely works.
    let (_, cleared) = call(
        store,
        Method::PUT,
        "/api/settings",
        &json!({ "comboStrategies": {} }).to_string(),
    )
    .await?;
    assert_eq!(cleared.get("comboStrategies"), Some(&json!({})));
    Ok(())
}

#[actix_web::test]
async fn a_settings_write_that_does_not_mention_the_map_leaves_it_alone() -> TestResult {
    // The distinction that makes the replace safe: only a write that *names*
    // `comboStrategies` replaces it. An unrelated settings change must not wipe every
    // override a user configured.
    let store = StateStore::memory();
    call(
        store.clone(),
        Method::PUT,
        "/api/settings",
        &json!({ "comboStrategies": { "one": { "fallbackStrategy": "fusion" } } }).to_string(),
    )
    .await?;
    let (_, after) = call(
        store,
        Method::PUT,
        "/api/settings",
        &json!({ "tunnelDashboardAccess": true }).to_string(),
    )
    .await?;
    assert_eq!(
        after.pointer("/comboStrategies/one/fallbackStrategy"),
        Some(&Value::String("fusion".to_owned())),
        "an unrelated settings write wiped the overrides"
    );
    Ok(())
}

#[actix_web::test]
async fn the_override_reaches_the_runtime_on_the_internal_route() -> TestResult {
    // The runtime never reads `/api/settings`; it reads the routing context. An
    // override that round-trips publicly but is missing here would be configurable and
    // inert.
    let store = StateStore::memory();
    call(
        store.clone(),
        Method::PUT,
        "/api/settings",
        &json!({
            "comboStrategy": "fallback",
            "comboStrategies": { "panel": { "fallbackStrategy": "fusion", "minPanel": 4 } },
        })
        .to_string(),
    )
    .await?;

    let (status, context) = call(store, Method::GET, "/internal/v1/routing-context", "").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        context
            .pointer("/settings/comboStrategy")
            .and_then(Value::as_str),
        Some("fallback")
    );
    assert_eq!(
        context
            .pointer("/settings/comboStrategies/panel/fallbackStrategy")
            .and_then(Value::as_str),
        Some("fusion")
    );
    assert_eq!(
        context
            .pointer("/settings/comboStrategies/panel/minPanel")
            .and_then(Value::as_u64),
        Some(4)
    );
    Ok(())
}

#[actix_web::test]
async fn an_entry_carrying_only_tuning_is_stored_without_a_strategy() -> TestResult {
    // Upstream persists only what the user changed. An entry with tuning and no
    // strategy must not gain one, or setting a grace window would silently change how
    // the combo routes.
    let store = StateStore::memory();
    let (status, written) = call(
        store,
        Method::PUT,
        "/api/settings",
        &json!({ "comboStrategies": { "panel": { "stragglerGraceMs": 1500 } } }).to_string(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        written
            .pointer("/comboStrategies/panel/stragglerGraceMs")
            .and_then(Value::as_u64),
        Some(1_500)
    );
    assert_eq!(
        written.pointer("/comboStrategies/panel/fallbackStrategy"),
        None,
        "an unset strategy must stay unset rather than being written as a default"
    );
    Ok(())
}
