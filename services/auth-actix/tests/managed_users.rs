//! Signing in as a managed account, and the three ways that must not work.
//!
//! The cases here are about which credential is accepted when, because that is where a user model
//! goes wrong in a way nobody notices: a shared password that keeps working after accounts exist is a
//! second door nobody audits, and a credential store outage that falls back to it is a way to open
//! that door on demand.
#![allow(clippy::future_not_send)]

pub mod support;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use actix_web::{App, body::to_bytes, http::StatusCode, test};
use async_trait::async_trait;
use serde_json::Value;

use nullrouter_auth::{
    ApiKeyValidation, ApiKeyValidator, AuthConfig, AuthService, PasswordConfig,
    StateValidationError, UserDirectory, UsersError, Verdict, VerifiedUser, configure,
};
use support::{ManualClock, NoSso, PASSWORD, default_lockout, peer};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug)]
struct RejectAllValidator;

#[async_trait]
impl ApiKeyValidator for RejectAllValidator {
    async fn validate(&self, _api_key: &str) -> Result<ApiKeyValidation, StateValidationError> {
        Ok(ApiKeyValidation {
            valid: false,
            active: false,
            key_id: None,
        })
    }
}

/// A directory holding one account, and recording what it was asked.
#[derive(Debug)]
struct OneUser {
    username: String,
    password: String,
    role: String,
    asked: Mutex<Vec<String>>,
}

impl OneUser {
    fn new(username: &str, password: &str, role: &str) -> Self {
        Self {
            username: username.to_owned(),
            password: password.to_owned(),
            role: role.to_owned(),
            asked: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl UserDirectory for OneUser {
    async fn verify(&self, username: &str, password: &str) -> Result<Verdict, UsersError> {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push(username.to_owned());
        }
        if username == self.username && password == self.password {
            return Ok(Verdict::Authenticated(VerifiedUser {
                user_id: "user_1".to_owned(),
                username: self.username.clone(),
                display_name: "Dana Reyes".to_owned(),
                role: self.role.clone(),
            }));
        }
        // Accounts exist, so a miss is a refusal rather than an invitation to try the shared password.
        Ok(Verdict::Rejected)
    }
}

/// A directory that cannot answer.
#[derive(Debug)]
struct Unavailable;

#[async_trait]
impl UserDirectory for Unavailable {
    async fn verify(&self, _username: &str, _password: &str) -> Result<Verdict, UsersError> {
        Err(UsersError::Unavailable)
    }
}

fn service(directory: Arc<dyn UserDirectory>) -> TestResult<AuthService> {
    let config = AuthConfig::new(
        b"g017-test-session-secret-32-bytes-minimum".to_vec(),
        PasswordConfig::Plaintext(PASSWORD.to_owned()),
    )?
    .with_session_ttl(Duration::from_secs(3_600))
    .with_lockout(default_lockout())
    .with_state_validation_url("http://127.0.0.1:9/internal/v1/keys/validate")?;
    Ok(AuthService::with_directory(
        config,
        Arc::new(ManualClock::new(1_700_000_000)),
        Arc::new(RejectAllValidator),
        Arc::new(NoSso),
        directory,
    )?)
}

struct Outcome {
    status: StatusCode,
    body: Value,
    cookie: Option<String>,
}

async fn sign_in(directory: Arc<dyn UserDirectory>, payload: Value) -> TestResult<Outcome> {
    let app = test::init_service(App::new().configure(configure(service(directory)?))).await;
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .peer_addr(peer(51))
            .set_json(payload)
            .to_request(),
    )
    .await;
    let status = response.status();
    let cookie = response
        .response()
        .cookies()
        .find(|cookie| cookie.name() == "auth_token")
        .map(|cookie| cookie.value().to_owned());
    let bytes = to_bytes(response.into_body()).await?;
    Ok(Outcome {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        cookie,
    })
}

/// Read `/api/auth/status` with a session cookie.
async fn status_for(directory: Arc<dyn UserDirectory>, token: &str) -> TestResult<Value> {
    let app = test::init_service(App::new().configure(configure(service(directory)?))).await;
    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/status")
            .cookie(actix_web::cookie::Cookie::new(
                "auth_token",
                token.to_owned(),
            ))
            .to_request(),
    )
    .await;
    let bytes = to_bytes(response.into_body()).await?;
    Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[actix_web::test]
async fn a_managed_account_signs_in_and_the_session_names_it() -> TestResult {
    let directory = Arc::new(OneUser::new(
        "dana",
        "a-sufficiently-long-password",
        "operator",
    ));
    let outcome = sign_in(
        Arc::clone(&directory) as Arc<dyn UserDirectory>,
        serde_json::json!({ "username": "dana", "password": "a-sufficiently-long-password" }),
    )
    .await?;
    assert_eq!(outcome.status, StatusCode::OK, "{:?}", outcome.body);
    let token = outcome
        .cookie
        .ok_or_else(|| std::io::Error::other("no session cookie"))?;

    // The session has to carry who signed in, or every downstream decision is back to
    // "authenticated" with no principal.
    let status = status_for(Arc::clone(&directory) as Arc<dyn UserDirectory>, &token).await?;
    assert_eq!(status.get("authenticated"), Some(&serde_json::json!(true)));
    assert_eq!(status.get("userId"), Some(&serde_json::json!("user_1")));
    assert_eq!(
        status.get("displayName"),
        Some(&serde_json::json!("Dana Reyes"))
    );
    assert_eq!(
        status.get("role"),
        Some(&serde_json::json!("operator")),
        "the role in the token was not reported: {status}"
    );
    Ok(())
}

#[actix_web::test]
async fn the_shared_password_stops_working_once_an_account_exists() -> TestResult {
    // The migration's whole point. A shared password that keeps working after accounts exist is a
    // second door that no user list shows and no deactivation closes.
    let directory = Arc::new(OneUser::new(
        "dana",
        "a-sufficiently-long-password",
        "admin",
    ));
    let outcome = sign_in(
        Arc::clone(&directory) as Arc<dyn UserDirectory>,
        serde_json::json!({ "password": PASSWORD }),
    )
    .await?;
    assert_eq!(
        outcome.status,
        StatusCode::UNAUTHORIZED,
        "the shared password still worked: {:?}",
        outcome.body
    );
    assert!(outcome.cookie.is_none(), "a session was minted anyway");

    // And with the username supplied, so this is not passing only because the username was absent.
    let named = sign_in(
        Arc::clone(&directory) as Arc<dyn UserDirectory>,
        serde_json::json!({ "username": "dana", "password": PASSWORD }),
    )
    .await?;
    assert_eq!(named.status, StatusCode::UNAUTHORIZED);
    assert!(named.cookie.is_none());
    Ok(())
}

#[actix_web::test]
async fn the_shared_password_works_while_no_account_exists() -> TestResult {
    // The other half of the migration: an install that upgrades must not lock its operator out.
    let outcome = sign_in(
        Arc::new(support::NoUsers),
        serde_json::json!({ "password": PASSWORD }),
    )
    .await?;
    assert_eq!(outcome.status, StatusCode::OK, "{:?}", outcome.body);
    let token = outcome
        .cookie
        .ok_or_else(|| std::io::Error::other("no session cookie"))?;

    // That session reports admin. It is the legacy full-access principal, and reporting less would
    // hide the very controls needed to create the first account.
    let status = status_for(Arc::new(support::NoUsers), &token).await?;
    assert_eq!(status.get("role"), Some(&serde_json::json!("admin")));
    assert_eq!(
        status.get("userId"),
        None,
        "a shared session named an account"
    );
    assert_eq!(
        status.get("usersConfigured"),
        Some(&serde_json::json!(false)),
        "the dashboard was not told the shared password is still in force"
    );
    Ok(())
}

#[actix_web::test]
async fn a_wrong_password_for_a_real_account_is_refused() -> TestResult {
    let directory = Arc::new(OneUser::new(
        "dana",
        "a-sufficiently-long-password",
        "viewer",
    ));
    let outcome = sign_in(
        directory as Arc<dyn UserDirectory>,
        serde_json::json!({ "username": "dana", "password": "not-the-password" }),
    )
    .await?;
    assert_eq!(outcome.status, StatusCode::UNAUTHORIZED);
    assert!(outcome.cookie.is_none());
    Ok(())
}

#[actix_web::test]
async fn an_unreachable_credential_store_is_not_a_wrong_password() -> TestResult {
    // Two things this protects. An operator hunting a credential that is actually fine, because the
    // router said "wrong password" when it meant "I could not check"; and an attacker who takes the
    // state service down to downgrade authentication back to one shared secret.
    let outcome = sign_in(
        Arc::new(Unavailable),
        serde_json::json!({ "username": "dana", "password": "a-sufficiently-long-password" }),
    )
    .await?;
    assert_eq!(
        outcome.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "an outage was reported as a credential failure: {:?}",
        outcome.body
    );
    assert!(outcome.cookie.is_none());

    // The shared password must not be accepted either: whether accounts exist is exactly what could
    // not be read, so falling back would be a bypass available on demand.
    let shared = sign_in(
        Arc::new(Unavailable),
        serde_json::json!({ "password": PASSWORD }),
    )
    .await?;
    assert_eq!(shared.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        shared.cookie.is_none(),
        "an outage let the shared password through"
    );
    Ok(())
}

#[actix_web::test]
async fn an_unknown_role_in_a_token_reads_as_the_least_privilege() -> TestResult {
    // A token minted by a newer build could name a role this one does not know. It must not read as
    // full access.
    let directory = Arc::new(OneUser::new(
        "dana",
        "a-sufficiently-long-password",
        "superuser",
    ));
    let outcome = sign_in(
        Arc::clone(&directory) as Arc<dyn UserDirectory>,
        serde_json::json!({ "username": "dana", "password": "a-sufficiently-long-password" }),
    )
    .await?;
    assert_eq!(outcome.status, StatusCode::OK);
    let token = outcome
        .cookie
        .ok_or_else(|| std::io::Error::other("no session cookie"))?;
    let status = status_for(directory as Arc<dyn UserDirectory>, &token).await?;
    assert_eq!(
        status.get("role"),
        Some(&serde_json::json!("viewer")),
        "an unrecognised role was not reduced: {status}"
    );
    Ok(())
}
