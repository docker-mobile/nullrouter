//! Managed accounts persist, and the rules that keep an install administrable hold.
//!
//! Two of these are the ones worth having. A user has to survive a restart, because an operator who
//! creates an account has been told a credential exists. And the last admin cannot be removed,
//! disabled, or demoted -- with no admin left, nobody can create one, and the only way back is editing
//! the state file by hand.
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

/// Long enough to satisfy the minimum, and obviously not a real secret.
const PASSWORD: &str = "a-sufficiently-long-password";

struct Outcome {
    status: StatusCode,
    body: Value,
}

async fn call(store: &StateStore, method: Method, uri: &str, body: &str) -> TestResult<Outcome> {
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
    Ok(Outcome {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    })
}

async fn get(store: &StateStore, uri: &str) -> TestResult<Outcome> {
    call(store, Method::GET, uri, "").await
}

async fn post(store: &StateStore, uri: &str, body: &Value) -> TestResult<Outcome> {
    call(store, Method::POST, uri, &body.to_string()).await
}

async fn put(store: &StateStore, uri: &str, body: &Value) -> TestResult<Outcome> {
    call(store, Method::PUT, uri, &body.to_string()).await
}

async fn delete(store: &StateStore, uri: &str) -> TestResult<Outcome> {
    call(store, Method::DELETE, uri, "").await
}

fn user_id(created: &Value) -> String {
    created
        .get("user")
        .and_then(|user| user.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[actix_web::test]
async fn a_user_survives_a_reload_and_the_hash_never_leaves() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("state.json");
    let store = StateStore::file(&path)?;

    let empty = get(&store, "/api/users").await?;
    assert_eq!(empty.status, StatusCode::OK);
    assert_eq!(empty.body.get("users"), Some(&json!([])));
    // With no account, the shared password is still the way in. The dashboard says so rather than
    // leaving an operator to discover it.
    assert_eq!(
        empty.body.get("sharedPasswordActive"),
        Some(&json!(true)),
        "{:?}",
        empty.body
    );

    let created = post(
        &store,
        "/api/users",
        &json!({
            "username": "Dana",
            "password": PASSWORD,
            "displayName": "Dana Reyes",
            "email": "dana@example.com",
            "role": "admin",
        }),
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let user = created.body.get("user").cloned().unwrap_or(Value::Null);
    // Lowercased on the way in, so one person cannot hold `Dana` and `dana`.
    assert_eq!(user.get("username"), Some(&json!("dana")));
    assert_eq!(user.get("role"), Some(&json!("admin")));
    assert_eq!(user.get("isActive"), Some(&json!(true)));

    // The hash must not appear in any response, on any route.
    let rendered = created.body.to_string();
    assert!(
        !rendered.contains("passwordHash") && !rendered.contains("$2"),
        "a response carried the password hash: {rendered}"
    );
    let listed = get(&store, "/api/users").await?.body.to_string();
    assert!(
        !listed.contains("passwordHash") && !listed.contains("$2"),
        "the list carried the password hash: {listed}"
    );
    // Nor the password itself, in any form.
    assert!(!rendered.contains(PASSWORD) && !listed.contains(PASSWORD));

    // The assertion an in-memory store could not make.
    store.flush_if_dirty()?;
    let reopened = StateStore::file(&path)?;
    let after_restart = get(&reopened, "/api/users").await?;
    let users = after_restart
        .body
        .get("users")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(users.len(), 1, "the account did not survive a reload");
    assert_eq!(
        after_restart.body.get("sharedPasswordActive"),
        Some(&json!(false)),
        "the shared password was still reported active with an account stored"
    );
    Ok(())
}

#[actix_web::test]
async fn a_credential_verifies_only_when_the_account_is_usable() -> TestResult {
    let store = StateStore::memory();
    let created = post(
        &store,
        "/api/users",
        &json!({ "username": "dana", "password": PASSWORD, "role": "operator" }),
    )
    .await?;
    let id = user_id(&created.body);

    let good = post(
        &store,
        "/internal/v1/users/verify",
        &json!({ "username": "dana", "password": PASSWORD }),
    )
    .await?;
    assert_eq!(good.body.get("authenticated"), Some(&json!(true)));
    assert_eq!(good.body.get("role"), Some(&json!("operator")));
    assert_eq!(good.body.get("userId"), Some(&json!(id.clone())));

    // A wrong password, an unknown user, and a disabled account must all answer the same way. Telling
    // them apart turns the sign-in form into a way to enumerate accounts.
    for (username, password) in [("dana", "not-the-password"), ("nobody", PASSWORD), ("", "")] {
        let denied = post(
            &store,
            "/internal/v1/users/verify",
            &json!({ "username": username, "password": password }),
        )
        .await?;
        assert_eq!(
            denied.body.get("authenticated"),
            Some(&json!(false)),
            "{username} was authenticated"
        );
        assert_eq!(denied.body.get("role"), None, "a refusal carried a role");
    }

    // Disabling has to actually stop a sign-in, with the right password, or it is a label rather than
    // a control.
    let disabled = put(
        &store,
        &format!("/api/users/{id}"),
        &json!({ "isActive": false }),
    )
    .await?;
    assert_eq!(disabled.status, StatusCode::OK, "{:?}", disabled.body);
    let after = post(
        &store,
        "/internal/v1/users/verify",
        &json!({ "username": "dana", "password": PASSWORD }),
    )
    .await?;
    assert_eq!(
        after.body.get("authenticated"),
        Some(&json!(false)),
        "a disabled account signed in"
    );
    Ok(())
}

#[actix_web::test]
async fn the_last_admin_cannot_be_removed_disabled_or_demoted() -> TestResult {
    // With no admin left nobody can create one, and the only way back is hand-editing the state file.
    // The store refuses rather than leaving this to the UI, because a UI check is bypassed by curl.
    let store = StateStore::memory();
    let admin = post(
        &store,
        "/api/users",
        &json!({ "username": "root", "password": PASSWORD, "role": "admin" }),
    )
    .await?;
    let admin_id = user_id(&admin.body);

    for body in [
        json!({ "isActive": false }),
        json!({ "role": "operator" }),
        json!({ "role": "viewer" }),
    ] {
        let refused = put(&store, &format!("/api/users/{admin_id}"), &body).await?;
        assert_eq!(
            refused.status,
            StatusCode::CONFLICT,
            "the last admin was changed by {body}"
        );
    }
    let refused_delete = delete(&store, &format!("/api/users/{admin_id}")).await?;
    assert_eq!(refused_delete.status, StatusCode::CONFLICT);

    // Still an active admin afterwards.
    let verify = post(
        &store,
        "/internal/v1/users/verify",
        &json!({ "username": "root", "password": PASSWORD }),
    )
    .await?;
    assert_eq!(verify.body.get("role"), Some(&json!("admin")));

    // With a second admin, the first becomes changeable.
    post(
        &store,
        "/api/users",
        &json!({ "username": "second", "password": PASSWORD, "role": "admin" }),
    )
    .await?;
    let now_allowed = put(
        &store,
        &format!("/api/users/{admin_id}"),
        &json!({ "role": "viewer" }),
    )
    .await?;
    assert_eq!(
        now_allowed.status,
        StatusCode::OK,
        "a demotion was refused with another admin present: {:?}",
        now_allowed.body
    );
    Ok(())
}

#[actix_web::test]
async fn a_duplicate_username_and_a_weak_password_are_refused() -> TestResult {
    let store = StateStore::memory();
    post(
        &store,
        "/api/users",
        &json!({ "username": "dana", "password": PASSWORD, "role": "admin" }),
    )
    .await?;

    // Case-insensitively duplicate: `Dana` normalises onto the stored `dana`.
    let duplicate = post(
        &store,
        "/api/users",
        &json!({ "username": "DANA", "password": PASSWORD }),
    )
    .await?;
    assert_eq!(
        duplicate.status,
        StatusCode::CONFLICT,
        "{:?}",
        duplicate.body
    );

    let short = post(
        &store,
        "/api/users",
        &json!({ "username": "bob", "password": "short" }),
    )
    .await?;
    assert_eq!(short.status, StatusCode::BAD_REQUEST);

    let bad_name = post(
        &store,
        "/api/users",
        &json!({ "username": "has space", "password": PASSWORD }),
    )
    .await?;
    assert_eq!(bad_name.status, StatusCode::BAD_REQUEST);

    // An unrecognised role is refused rather than defaulted: silently becoming viewer would look like
    // a working restriction, and silently becoming admin would be escalation by typo.
    let bad_role = post(
        &store,
        "/api/users",
        &json!({ "username": "carol", "password": PASSWORD, "role": "superuser" }),
    )
    .await?;
    assert_eq!(bad_role.status, StatusCode::BAD_REQUEST);

    // Only the first account exists.
    let users = get(&store, "/api/users").await?;
    let count = users
        .body
        .get("users")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    assert_eq!(
        count, 1,
        "a refused create stored something: {:?}",
        users.body
    );
    Ok(())
}

#[actix_web::test]
async fn a_password_change_takes_effect_and_the_old_one_stops_working() -> TestResult {
    let store = StateStore::memory();
    let created = post(
        &store,
        "/api/users",
        &json!({ "username": "dana", "password": PASSWORD, "role": "admin" }),
    )
    .await?;
    let id = user_id(&created.body);

    let changed = put(
        &store,
        &format!("/api/users/{id}"),
        &json!({ "password": "an-entirely-different-password" }),
    )
    .await?;
    assert_eq!(changed.status, StatusCode::OK, "{:?}", changed.body);

    let with_new = post(
        &store,
        "/internal/v1/users/verify",
        &json!({ "username": "dana", "password": "an-entirely-different-password" }),
    )
    .await?;
    assert_eq!(with_new.body.get("authenticated"), Some(&json!(true)));

    // The point of a rotation: the old credential has to stop working.
    let with_old = post(
        &store,
        "/internal/v1/users/verify",
        &json!({ "username": "dana", "password": PASSWORD }),
    )
    .await?;
    assert_eq!(
        with_old.body.get("authenticated"),
        Some(&json!(false)),
        "the old password still worked after a change"
    );
    Ok(())
}
