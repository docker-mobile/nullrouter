pub mod support;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use actix_web::{
    App,
    http::{StatusCode, header},
    test,
};
use async_trait::async_trait;
use serde_json::Value;

use nullrouter_auth::{ApiKeyValidation, ApiKeyValidator, StateValidationError, configure};
use support::{ManualClock, default_lockout, extract_cookie, peer, service};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const API_KEY: &str = "nr_g017_fixture_active_key";

#[derive(Debug, Clone)]
enum ValidatorMode {
    Active,
    Inactive,
    Unavailable,
}

#[derive(Debug)]
struct FakeValidator {
    mode: ValidatorMode,
    observed: Mutex<Vec<String>>,
}

impl FakeValidator {
    const fn new(mode: ValidatorMode) -> Self {
        Self {
            mode,
            observed: Mutex::new(Vec::new()),
        }
    }

    fn observed(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        Ok(self
            .observed
            .lock()
            .map_err(|_| std::io::Error::other("validator lock poisoned"))?
            .clone())
    }
}

#[async_trait]
impl ApiKeyValidator for FakeValidator {
    async fn validate(&self, api_key: &str) -> Result<ApiKeyValidation, StateValidationError> {
        self.observed
            .lock()
            .map_err(|_| StateValidationError::Unavailable)?
            .push(api_key.to_owned());
        match self.mode {
            ValidatorMode::Active => Ok(ApiKeyValidation {
                valid: true,
                active: true,
                key_id: Some("key_fixture".to_owned()),
            }),
            ValidatorMode::Inactive => Ok(ApiKeyValidation {
                valid: true,
                active: false,
                key_id: Some("key_fixture".to_owned()),
            }),
            ValidatorMode::Unavailable => Err(StateValidationError::Unavailable),
        }
    }
}

#[actix_web::test]
async fn internal_authorize_accepts_valid_dashboard_session() -> TestResult {
    let clock = Arc::new(ManualClock::new(1_000));
    let app = test::init_service(App::new().configure(configure(service(
        clock,
        Arc::new(FakeValidator::new(ValidatorMode::Active)),
        Duration::from_secs(60),
        default_lockout(),
    )?)))
    .await;
    let login = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .peer_addr(peer(1))
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .set_payload(r#"{"password":"g017-test-password"}"#)
            .to_request(),
    )
    .await;
    let cookie = extract_cookie(&login)?;
    let token = cookie
        .strip_prefix("auth_token=")
        .ok_or_else(|| std::io::Error::other("unexpected cookie name"))?;

    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/internal/v1/authorize")
            .peer_addr(peer(1))
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .set_payload(format!(
                r#"{{"kind":"dashboard","sessionToken":"{token}"}}"#
            ))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json: Value = test::read_body_json(response).await;
    assert_eq!(json.get("authorized"), Some(&Value::Bool(true)));
    assert_eq!(
        json.get("principal"),
        Some(&Value::String("dashboard_session".to_owned()))
    );
    Ok(())
}

#[actix_web::test]
async fn internal_authorize_queries_state_without_leaking_key() -> TestResult {
    let validator = Arc::new(FakeValidator::new(ValidatorMode::Active));
    let app = test::init_service(App::new().configure(configure(service(
        Arc::new(ManualClock::new(1_000)),
        validator.clone(),
        Duration::from_secs(60),
        default_lockout(),
    )?)))
    .await;
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/internal/v1/authorize")
            .peer_addr(peer(1))
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .set_payload(format!(r#"{{"kind":"runtime","apiKey":"{API_KEY}"}}"#))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = test::read_body(response).await;
    let body = std::str::from_utf8(&bytes)?;
    assert!(!body.contains(API_KEY));
    let json: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(json.get("authorized"), Some(&Value::Bool(true)));
    assert_eq!(validator.observed()?, vec![API_KEY.to_owned()]);
    Ok(())
}

#[actix_web::test]
async fn internal_authorize_fails_closed_when_state_unavailable() -> TestResult {
    let app = test::init_service(App::new().configure(configure(service(
        Arc::new(ManualClock::new(1_000)),
        Arc::new(FakeValidator::new(ValidatorMode::Unavailable)),
        Duration::from_secs(60),
        default_lockout(),
    )?)))
    .await;
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/internal/v1/authorize")
            .peer_addr(peer(1))
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .set_payload(format!(r#"{{"kind":"runtime","apiKey":"{API_KEY}"}}"#))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json: Value = test::read_body_json(response).await;
    assert_eq!(json.get("authorized"), Some(&Value::Bool(false)));
    assert_eq!(
        json.get("reason"),
        Some(&Value::String("state_unavailable".to_owned()))
    );
    Ok(())
}

#[actix_web::test]
async fn internal_authorize_rejects_inactive_key() -> TestResult {
    let app = test::init_service(App::new().configure(configure(service(
        Arc::new(ManualClock::new(1_000)),
        Arc::new(FakeValidator::new(ValidatorMode::Inactive)),
        Duration::from_secs(60),
        default_lockout(),
    )?)))
    .await;
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/internal/v1/authorize")
            .peer_addr(peer(1))
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .set_payload(format!(r#"{{"kind":"runtime","apiKey":"{API_KEY}"}}"#))
            .to_request(),
    )
    .await;
    let json: Value = test::read_body_json(response).await;
    assert_eq!(json.get("authorized"), Some(&Value::Bool(false)));
    assert_eq!(
        json.get("reason"),
        Some(&Value::String("invalid_api_key".to_owned()))
    );
    Ok(())
}

#[actix_web::test]
async fn internal_authorize_rejects_non_loopback_peer() -> TestResult {
    let app = test::init_service(App::new().configure(configure(service(
        Arc::new(ManualClock::new(1_000)),
        Arc::new(FakeValidator::new(ValidatorMode::Active)),
        Duration::from_secs(60),
        default_lockout(),
    )?)))
    .await;
    let remote = "198.51.100.10:42000".parse()?;
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/internal/v1/authorize")
            .peer_addr(remote)
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .set_payload(r#"{"kind":"runtime","apiKey":"fixture"}"#)
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let json: Value = test::read_body_json(response).await;
    assert_eq!(
        json.pointer("/error/code"),
        Some(&Value::String("loopback_required".to_owned()))
    );
    Ok(())
}
