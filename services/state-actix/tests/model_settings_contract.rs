//! Model settings are stored, not acknowledged and dropped.
//!
//! These four endpoint families previously answered `200 {"success": true}` from the API service and
//! wrote nothing, so every `GET` came back empty however many writes preceded it. The assertion that
//! matters here is therefore not the status code -- the stubs returned the same one -- but that a read
//! after a write reflects it, and that the value is still there after the store is reopened from disk.
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

fn test_error(message: String) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message))
}

struct JsonResponse {
    status: StatusCode,
    body: Value,
}

async fn call(
    store: &StateStore,
    method: Method,
    uri: &str,
    body: &str,
) -> TestResult<JsonResponse> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(store.clone()))
            .configure(configure),
    )
    .await;
    let request = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
        .to_request();
    let response = test::call_service(&app, request).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body()).await?;
    Ok(JsonResponse {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    })
}

async fn get(store: &StateStore, uri: &str) -> TestResult<JsonResponse> {
    call(store, Method::GET, uri, "").await
}

async fn post(store: &StateStore, uri: &str, body: &Value) -> TestResult<JsonResponse> {
    call(store, Method::POST, uri, &body.to_string()).await
}

async fn put(store: &StateStore, uri: &str, body: &Value) -> TestResult<JsonResponse> {
    call(store, Method::PUT, uri, &body.to_string()).await
}

async fn delete(store: &StateStore, uri: &str) -> TestResult<JsonResponse> {
    call(store, Method::DELETE, uri, "").await
}

#[actix_web::test]
async fn disabling_a_model_survives_a_reload() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("state.json");
    let store = StateStore::file(&path)?;

    let empty = get(&store, "/api/models/disabled").await?;
    assert_eq!(empty.status, StatusCode::OK);
    assert_eq!(empty.body, json!({ "disabled": {} }));

    let saved = post(
        &store,
        "/api/models/disabled",
        &json!({ "providerAlias": "openai-main", "ids": ["gpt-4o-mini", "o1-preview"] }),
    )
    .await?;
    assert_eq!(saved.status, StatusCode::OK);
    assert_eq!(saved.body, json!({ "success": true }));

    // The read the stub failed: same process, immediately after the write.
    let listed = get(&store, "/api/models/disabled").await?;
    assert_eq!(
        listed.body,
        json!({ "disabled": { "openai-main": ["gpt-4o-mini", "o1-preview"] } }),
        "the write was acknowledged and not stored"
    );

    // Narrowed to one provider, which upstream answers with a bare `ids` array rather than a map.
    let narrowed = get(&store, "/api/models/disabled?providerAlias=openai-main").await?;
    assert_eq!(
        narrowed.body,
        json!({ "ids": ["gpt-4o-mini", "o1-preview"] })
    );
    let other = get(&store, "/api/models/disabled?providerAlias=anthropic").await?;
    assert_eq!(
        other.body,
        json!({ "ids": [] }),
        "a provider with nothing disabled must read as empty, not as the other provider's set"
    );

    // The assertion the in-memory store could not make: an operator who disables a model and restarts
    // the router must not find it enabled again.
    store.flush_if_dirty()?;
    let reopened = StateStore::file(&path)?;
    let after_restart = get(&reopened, "/api/models/disabled").await?;
    assert_eq!(
        after_restart.body,
        json!({ "disabled": { "openai-main": ["gpt-4o-mini", "o1-preview"] } }),
        "the setting did not survive a reload from disk"
    );

    // Re-enabling removes the key rather than leaving an empty list, so the shape matches a provider
    // that was never touched.
    let cleared = delete(&reopened, "/api/models/disabled?providerAlias=openai-main").await?;
    assert_eq!(cleared.status, StatusCode::OK);
    assert_eq!(
        get(&reopened, "/api/models/disabled").await?.body,
        json!({ "disabled": {} })
    );
    Ok(())
}

#[actix_web::test]
async fn an_empty_id_list_re_enables_everything() -> TestResult {
    // An editor that sends the whole list has no other way to remove the last entry. This is the case
    // that separates "ids is absent" (malformed) from "ids is empty" (disable nothing).
    let store = StateStore::memory();
    post(
        &store,
        "/api/models/disabled",
        &json!({ "providerAlias": "openai-main", "ids": ["gpt-4o-mini"] }),
    )
    .await?;
    let emptied = post(
        &store,
        "/api/models/disabled",
        &json!({ "providerAlias": "openai-main", "ids": [] }),
    )
    .await?;
    assert_eq!(emptied.status, StatusCode::OK);
    assert_eq!(
        get(&store, "/api/models/disabled").await?.body,
        json!({ "disabled": {} })
    );

    let malformed = post(
        &store,
        "/api/models/disabled",
        &json!({ "providerAlias": "openai-main" }),
    )
    .await?;
    assert_eq!(
        malformed.status,
        StatusCode::BAD_REQUEST,
        "an absent ids[] is a malformed request, not an empty set"
    );
    Ok(())
}

#[actix_web::test]
async fn a_custom_model_is_stored_and_re_adding_it_is_an_edit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("state.json");
    let store = StateStore::file(&path)?;

    assert_eq!(
        get(&store, "/api/models/custom").await?.body,
        json!({ "models": [] })
    );

    let added = post(
        &store,
        "/api/models/custom",
        &json!({ "providerAlias": "local", "id": "qwen3-32b", "type": "chat", "name": "Qwen3 32B" }),
    )
    .await?;
    assert_eq!(added.status, StatusCode::OK);
    assert_eq!(added.body, json!({ "success": true, "added": true }));

    let listed = get(&store, "/api/models/custom").await?;
    let models = listed
        .body
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| test_error("models is not an array".to_owned()))?;
    assert_eq!(models.len(), 1, "{listed:?}", listed = listed.body);
    let model = models
        .first()
        .ok_or_else(|| test_error("no model".to_owned()))?;
    assert_eq!(model.get("providerAlias"), Some(&json!("local")));
    assert_eq!(model.get("id"), Some(&json!("qwen3-32b")));
    // `type`, not `modelType`: the wire name a client sends.
    assert_eq!(model.get("type"), Some(&json!("chat")));
    assert_eq!(model.get("name"), Some(&json!("Qwen3 32B")));

    // Re-adding the same pair edits it rather than duplicating, and says so with `added: false`.
    let edited = post(
        &store,
        "/api/models/custom",
        &json!({ "providerAlias": "local", "id": "qwen3-32b", "name": "Qwen3 32B Instruct" }),
    )
    .await?;
    assert_eq!(edited.body, json!({ "success": true, "added": false }));
    let after_edit = get(&store, "/api/models/custom").await?;
    let models = after_edit
        .body
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| test_error("models is not an array".to_owned()))?;
    assert_eq!(models.len(), 1, "re-adding the same pair duplicated it");
    assert_eq!(
        models.first().and_then(|model| model.get("name")),
        Some(&json!("Qwen3 32B Instruct"))
    );

    // The same id on a different provider is a different model.
    post(
        &store,
        "/api/models/custom",
        &json!({ "providerAlias": "other", "id": "qwen3-32b" }),
    )
    .await?;
    store.flush_if_dirty()?;
    let reopened = StateStore::file(&path)?;
    let after_restart = get(&reopened, "/api/models/custom").await?;
    let models = after_restart
        .body
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| test_error("models is not an array".to_owned()))?;
    assert_eq!(
        models.len(),
        2,
        "the same id on two providers must be two models"
    );

    let removed = delete(
        &reopened,
        "/api/models/custom?providerAlias=local&id=qwen3-32b",
    )
    .await?;
    assert_eq!(removed.status, StatusCode::OK);
    let missing = delete(
        &reopened,
        "/api/models/custom?providerAlias=local&id=qwen3-32b",
    )
    .await?;
    assert_eq!(
        missing.status,
        StatusCode::NOT_FOUND,
        "deleting something absent must not report success"
    );
    Ok(())
}

#[actix_web::test]
async fn an_alias_is_stored_and_post_is_still_rejected() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("state.json");
    let store = StateStore::file(&path)?;

    assert_eq!(
        get(&store, "/api/models/alias").await?.body,
        json!({ "aliases": {} })
    );

    let set = put(
        &store,
        "/api/models/alias",
        &json!({ "model": "gpt-4o", "alias": "fast" }),
    )
    .await?;
    assert_eq!(set.status, StatusCode::OK);
    assert_eq!(
        set.body,
        json!({ "success": true, "model": "gpt-4o", "alias": "fast" })
    );

    // Keyed by alias, because a lookup arrives naming the alias. Two aliases may share one model.
    put(
        &store,
        "/api/models/alias",
        &json!({ "model": "gpt-4o", "alias": "default" }),
    )
    .await?;
    store.flush_if_dirty()?;
    let reopened = StateStore::file(&path)?;
    assert_eq!(
        get(&reopened, "/api/models/alias").await?.body,
        json!({ "aliases": { "default": "gpt-4o", "fast": "gpt-4o" } })
    );

    // `POST` is a 405 here, as it is upstream. A client written against upstream relies on `PUT`.
    let posted = post(
        &reopened,
        "/api/models/alias",
        &json!({ "model": "gpt-4o", "alias": "quick" }),
    )
    .await?;
    assert_eq!(posted.status, StatusCode::METHOD_NOT_ALLOWED);

    assert_eq!(
        delete(&reopened, "/api/models/alias?alias=fast")
            .await?
            .status,
        StatusCode::OK
    );
    assert_eq!(
        delete(&reopened, "/api/models/alias?alias=fast")
            .await?
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&reopened, "/api/models/alias").await?.body,
        json!({ "aliases": { "default": "gpt-4o" } })
    );
    Ok(())
}

#[actix_web::test]
async fn incomplete_writes_are_refused() -> TestResult {
    let store = StateStore::memory();
    for (uri, body) in [
        ("/api/models/custom", json!({ "id": "no-provider" })),
        ("/api/models/custom", json!({ "providerAlias": "local" })),
        // Whitespace is not a value: trimming to empty must be refused, not stored as a blank key.
        (
            "/api/models/custom",
            json!({ "providerAlias": "   ", "id": "x" }),
        ),
    ] {
        let response = post(&store, uri, &body).await?;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "{uri} accepted {body}"
        );
    }
    for body in [
        json!({ "alias": "no-model" }),
        json!({ "model": "gpt-4o" }),
        json!({ "model": "gpt-4o", "alias": "  " }),
    ] {
        let response = put(&store, "/api/models/alias", &body).await?;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "alias accepted {body}"
        );
    }
    // Nothing was stored by any of the refusals.
    assert_eq!(
        get(&store, "/api/models/custom").await?.body,
        json!({ "models": [] })
    );
    assert_eq!(
        get(&store, "/api/models/alias").await?.body,
        json!({ "aliases": {} })
    );
    Ok(())
}
