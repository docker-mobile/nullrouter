//! Authentication outcomes reach a log collector as structured records.
//!
//! Asserted by capturing what a subscriber actually receives, not by checking that a `tracing!`
//! line exists in the source: a macro call with no subscriber installed is silent, and a field
//! renamed in one place would still compile. What matters is the shape a SIEM ingests.
pub mod support;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use actix_web::{App, http::StatusCode, test};
use async_trait::async_trait;
use tracing_subscriber::layer::SubscriberExt as _;

use nullrouter_auth::{ApiKeyValidation, ApiKeyValidator, StateValidationError, configure};
use support::{ManualClock, PASSWORD, default_lockout, default_service, peer};

type TestResult = Result<(), Box<dyn std::error::Error>>;

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

/// Collects each record's rendered fields, so an assertion can look for a field by name.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<String>>>);

impl Captured {
    fn lines(&self) -> Vec<String> {
        self.0.lock().map(|guard| guard.clone()).unwrap_or_default()
    }

    /// Every captured record that mentions `event`.
    fn with_event(&self, event: &str) -> Vec<String> {
        self.lines()
            .into_iter()
            .filter(|line| line.contains(event))
            .collect()
    }
}

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut guard) = self.0.lock() {
            guard.push(String::from_utf8_lossy(buf).into_owned());
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[actix_web::test]
async fn sign_in_outcomes_are_recorded_as_audit_events() -> TestResult {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(captured.clone())
            .with_ansi(false),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    let clock = Arc::new(ManualClock::new(1_700_000_000));
    let app = test::init_service(App::new().configure(configure(default_service(
        Arc::clone(&clock),
        Arc::new(RejectAllValidator),
        Duration::from_secs(3_600),
        default_lockout(),
    )?)))
    .await;

    // A wrong password.
    let denied = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .peer_addr(peer(9))
            .set_json(serde_json::json!({ "password": "not-the-password" }))
            .to_request(),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    // Then the right one.
    let accepted = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .peer_addr(peer(9))
            .set_json(serde_json::json!({ "password": PASSWORD }))
            .to_request(),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);

    let failed = captured.with_event("auth.login.failed");
    let succeeded = captured.with_event("auth.login.succeeded");

    assert_eq!(failed.len(), 1, "captured: {:?}", captured.lines());
    assert_eq!(succeeded.len(), 1, "captured: {:?}", captured.lines());

    let failure = failed.first().map(String::as_str).unwrap_or_default();
    let success = succeeded.first().map(String::as_str).unwrap_or_default();

    for (label, line) in [("failure", failure), ("success", success)] {
        // The marker a collector selects on, rather than matching message text that will change.
        assert!(
            line.contains("audit=true"),
            "{label} lacks the audit marker: {line}"
        );
        // "From where" is the first question an incident review asks.
        assert!(
            line.contains("127.0.0.9"),
            "{label} lacks the peer address: {line}"
        );
    }

    // What turns a series of failures into a rate worth alerting on.
    assert!(
        failure.contains("remaining_before_lock="),
        "failure record cannot be turned into a rate: {failure}"
    );
    assert!(failure.contains("locked=false"), "{failure}");

    // The password must never appear, in any form. Its length alone narrows a brute-force search.
    for line in captured.lines() {
        assert!(
            !line.contains(PASSWORD),
            "an audit record leaked the password: {line}"
        );
        assert!(
            !line.contains("not-the-password"),
            "an audit record leaked a submitted password: {line}"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn a_locked_out_address_is_recorded_on_every_refusal() -> TestResult {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(captured.clone())
            .with_ansi(false),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    let clock = Arc::new(ManualClock::new(1_700_000_000));
    let lockout = nullrouter_auth::LockoutConfig {
        threshold: 2,
        window: Duration::from_secs(60),
        lock_duration: Duration::from_secs(120),
        capacity: 8,
    };
    let app = test::init_service(App::new().configure(configure(default_service(
        Arc::clone(&clock),
        Arc::new(RejectAllValidator),
        Duration::from_secs(3_600),
        lockout,
    )?)))
    .await;

    // Exhaust the threshold, then knock twice more while locked.
    for _ in 0..4 {
        let response = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/auth/login")
                .peer_addr(peer(11))
                .set_json(serde_json::json!({ "password": "wrong" }))
                .to_request(),
        )
        .await;
        assert!(response.status().is_client_error());
    }

    // Recorded on each refusal, not only when the lock is first applied: a lockout that keeps being
    // hit is an attack in progress, one hit once is a typo, and only repetition separates them.
    let locked = captured.with_event("auth.login.locked_out");
    assert!(
        locked.len() >= 2,
        "expected a record per refusal while locked, got {}: {:?}",
        locked.len(),
        captured.lines()
    );
    let first = locked.first().map(String::as_str).unwrap_or_default();
    assert!(first.contains("audit=true"), "{first}");
    assert!(first.contains("127.0.0.11"), "{first}");
    assert!(
        first.contains("retry_after_seconds="),
        "a reviewer cannot tell how long the lock lasts: {first}"
    );
    Ok(())
}
