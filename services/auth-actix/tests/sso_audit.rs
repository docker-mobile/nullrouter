//! A sign-in through an identity provider names the human in the audit trail.
//!
//! Driven end-to-end against a stub provider rather than by calling the audit helper directly: the
//! record sits after four sequential checks (state cookie, code exchange, JWKS signature, claim
//! validation), and a test that skipped them would still pass if the callback stopped reaching the
//! line. So the provider here is a real HTTP server, and the `id_token` below is a real RS256
//! signature over the claims it carries.
//!
//! The token is a fixed vector, signed once with a throwaway key whose public half is served as the
//! JWKS. That makes the flow deterministic: the clock is pinned inside the token's validity window,
//! and the nonce cookie matches the nonce claim, so a failure means the code changed rather than
//! that time passed.
pub mod support;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use actix_web::{App, HttpResponse, http::StatusCode, test, web};
use async_trait::async_trait;
use tracing_subscriber::layer::SubscriberExt as _;

use nullrouter_auth::{
    ApiKeyValidation, ApiKeyValidator, AuthConfig, AuthService, AuthSettings, AuthSettingsProvider,
    PasswordConfig, SettingsError, StateValidationError, configure,
};
use support::{ManualClock, default_lockout, peer};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The issuer inside the signed token. The stub advertises this in its discovery document, so the
/// ephemeral port the server binds to never has to appear in a claim.
const ISSUER: &str = "https://idp.test";
const CLIENT_ID: &str = "nullrouter-dashboard";
const NONCE: &str = "g017-test-nonce";
const STATE: &str = "g017-test-state";
const VERIFIER: &str = "g017-test-code-verifier";

/// Identity asserted by the token, and the three fields the audit record must carry.
const SUBJECT: &str = "idp-subject-8fbc21";
const EMAIL: &str = "dana.reyes@example.com";
const USERNAME: &str = "dana.reyes";

/// Signed at `iat` 1700000000, expiring at 1700003600, over the claims above.
const ID_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCIsImtpZCI6ImlkcC10ZXN0LWtleSJ9.eyJpc3MiOiJodHRwczovL2lkcC50ZXN0IiwiYXVkIjoibnVsbHJvdXRlci1kYXNoYm9hcmQiLCJzdWIiOiJpZHAtc3ViamVjdC04ZmJjMjEiLCJub25jZSI6ImcwMTctdGVzdC1ub25jZSIsImVtYWlsIjoiZGFuYS5yZXllc0BleGFtcGxlLmNvbSIsInByZWZlcnJlZF91c2VybmFtZSI6ImRhbmEucmV5ZXMiLCJuYW1lIjoiRGFuYSBSZXllcyIsImlhdCI6MTcwMDAwMDAwMCwiZXhwIjoxNzAwMDAzNjAwfQ.WRWetPeFTD1o-QpuYty4Xx2LqKgMr7N4xqaUnYWT59dY-P0T3WgCwjy-IExv6PkZxy4sMgCkErcF2r497NZarNlo4TGz65iZRiPbjCYg_AUmFSo6yvR4tT-JxnG8NRSAtRmNNJvKKj6Vymw_sPBx196Bpxjw5-1bF3UwJWhPxRZs5-dW4Hl9Uz2sZnxMkMDyfIWSIQ61W1D-b8EN8R5G34UonQgtS_d1huu9tmWspLPu46b_17jBVje8fGLqqIXEJ1qD48gDykLuPrjYxrriez7MxhzGoKPu3RjaK1U6sYQkWZVf6vX220OJn7nA3K5HcRLd_7ObzbCagjWIfE5RyQ";

/// Public half of the key that signed `ID_TOKEN`, as a JWKS `n`.
const JWKS_MODULUS: &str = "4ScRzv_-016-i_W0I58AruduxDSNZ6wrfYemZPVDB2rlzNO0OJm2VlDPYe4k7cE_Ts0cwlsow8lSpWGu3xxfOZS1HgzcAnmYWSuESXfWUgdfixV7ac69PfIvWJ4_EyFMYWgkfmLHJDP0TnZOHZoqhoCY1UA5Fk0Y4YtBaBURQb0iPQ4Jve-cI5gyTFc-PWyIFFuWka4GrReRaj5-_s39O9drIxCL6SPAq7IzfbCZvou9UYIVXISMwWsiSlXBRypA2EuO2iKvJFztKnn3X1siBzW0VXqcBDoef00qMPOEtrvKwq1yFxDXitOMoXwWuCMP4RuqKKEf1Y-9hlRJkoXHUw";

/// Inside the token's window, and far enough from both edges that clock skew cannot decide it.
const NOW: u64 = 1_700_000_100;

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

/// Supplies the OIDC settings a running state service would.
#[derive(Debug)]
struct StubSettings {
    issuer_url: String,
}

#[async_trait]
impl AuthSettingsProvider for StubSettings {
    async fn settings(&self) -> Result<AuthSettings, SettingsError> {
        Ok(AuthSettings {
            oidc_issuer_url: self.issuer_url.clone(),
            oidc_client_id: CLIENT_ID.to_owned(),
            oidc_client_secret: "idp-client-secret".to_owned(),
            oidc_scopes: "openid email profile".to_owned(),
            oidc_login_label: "Example SSO".to_owned(),
            ..AuthSettings::default()
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

/// The value of one `key=value` field in a rendered record.
///
/// Exact rather than a substring search, because the values here overlap: `dana.reyes` is a prefix of
/// `dana.reyes@example.com`, so `record.contains("dana.reyes")` would pass with `display_name`
/// deleted entirely. The default renderer quotes a value only when it needs to, so both forms are
/// accepted and the quotes are stripped.
fn field(record: &str, key: &str) -> Option<String> {
    let after = record.split(&format!(" {key}=")).nth(1)?;
    let raw = if let Some(quoted) = after.strip_prefix('"') {
        quoted.split('"').next()?
    } else {
        after.split_whitespace().next()?
    };
    Some(raw.to_owned())
}

/// A stub provider: discovery, JWKS, and a token endpoint that returns `ID_TOKEN`.
///
/// Runs on a real socket because the callback reaches it through `reqwest`, which no in-process
/// actix test client intercepts.
/// Not `async`: the server is spawned onto the caller's runtime rather than awaited, so the socket is
/// listening by the time this returns and the callback cannot race it.
fn start_stub_idp() -> Result<String, Box<dyn std::error::Error>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let base = format!("http://127.0.0.1:{port}");
    let advertised = base.clone();

    let server = actix_web::HttpServer::new(move || {
        let advertised = advertised.clone();
        App::new()
            .app_data(web::Data::new(advertised))
            .route(
                "/.well-known/openid-configuration",
                web::get().to(|base: web::Data<String>| async move {
                    HttpResponse::Ok().json(serde_json::json!({
                        "issuer": ISSUER,
                        "authorization_endpoint": format!("{}/authorize", base.as_str()),
                        "token_endpoint": format!("{}/token", base.as_str()),
                        "jwks_uri": format!("{}/jwks", base.as_str()),
                    }))
                }),
            )
            .route(
                "/jwks",
                web::get().to(|| async {
                    HttpResponse::Ok().json(serde_json::json!({
                        "keys": [{
                            "kty": "RSA",
                            "kid": "idp-test-key",
                            "alg": "RS256",
                            "use": "sig",
                            "n": JWKS_MODULUS,
                            "e": "AQAB",
                        }],
                    }))
                }),
            )
            .route(
                "/token",
                web::post().to(|| async {
                    HttpResponse::Ok().json(serde_json::json!({
                        "access_token": "stub-access-token",
                        "token_type": "Bearer",
                        "id_token": ID_TOKEN,
                    }))
                }),
            )
    })
    .listen(listener)?
    .workers(1)
    .run();
    actix_web::rt::spawn(server);
    Ok(base)
}

fn service(
    issuer_url: String,
    clock: Arc<ManualClock>,
) -> Result<AuthService, Box<dyn std::error::Error>> {
    let config = AuthConfig::new(
        b"g017-test-session-secret-32-bytes-minimum".to_vec(),
        PasswordConfig::Plaintext(support::PASSWORD.to_owned()),
    )?
    .with_session_ttl(Duration::from_secs(3_600))
    .with_lockout(default_lockout())
    .with_state_validation_url("http://127.0.0.1:9/internal/v1/keys/validate")?;

    Ok(AuthService::with_settings_provider(
        config,
        clock,
        Arc::new(RejectAllValidator),
        Arc::new(StubSettings { issuer_url }),
    )?)
}

#[actix_web::test]
async fn an_oidc_sign_in_records_who_authenticated() -> TestResult {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(captured.clone())
            .with_ansi(false),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    let idp = start_stub_idp()?;
    let clock = Arc::new(ManualClock::new(NOW));
    let app = test::init_service(App::new().configure(configure(service(idp, clock)?))).await;

    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&format!(
                "/api/auth/oidc/callback?code=stub-code&state={STATE}"
            ))
            .peer_addr(peer(23))
            .cookie(actix_web::cookie::Cookie::new("oidc_state", STATE))
            .cookie(actix_web::cookie::Cookie::new("oidc_nonce", NONCE))
            .cookie(actix_web::cookie::Cookie::new(
                "oidc_code_verifier",
                VERIFIER,
            ))
            .to_request(),
    )
    .await;

    // The flow has to have actually completed, or the assertions below would pass vacuously against
    // an audit record emitted on some other path.
    let location = response
        .headers()
        .get(actix_web::http::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert_eq!(response.status(), StatusCode::FOUND, "location: {location}");
    assert!(
        location.ends_with("/dashboard"),
        "the callback did not reach a session; it redirected to {location}"
    );

    let records = captured.with_event("auth.sso.succeeded");
    assert_eq!(
        records.len(),
        1,
        "expected one SSO audit record, captured: {:?}",
        captured.lines()
    );
    let record = records.first().map(String::as_str).unwrap_or_default();

    assert!(
        record.contains("audit=true"),
        "the record lacks the marker a collector selects on: {record}"
    );
    assert_eq!(field(record, "method").as_deref(), Some("oidc"), "{record}");
    // The stable identifier. A display name or an email can be reassigned between people; `sub` is
    // what still distinguishes them at review time.
    assert_eq!(
        field(record, "subject").as_deref(),
        Some(SUBJECT),
        "the record cannot name who signed in: {record}"
    );
    assert_eq!(field(record, "email").as_deref(), Some(EMAIL), "{record}");
    assert_eq!(
        field(record, "display_name").as_deref(),
        Some(USERNAME),
        "the record lacks the display name: {record}"
    );
    // "From where" is the first question an incident review asks.
    assert_eq!(
        field(record, "peer").as_deref(),
        Some("127.0.0.23"),
        "the record lacks the peer address: {record}"
    );

    // Credentials must not reach the log. The id_token is a bearer credential for its lifetime, and
    // the client secret would let an attacker complete a flow of their own.
    //
    // Checked across every captured line, not just the audit record: the HTTP client logs at TRACE,
    // and this subscriber has no filter, so a secret rendered into a request body would be caught
    // here too.
    for line in captured.lines() {
        assert!(
            !line.contains(ID_TOKEN),
            "an audit record leaked the id_token: {line}"
        );
        assert!(
            !line.contains("idp-client-secret"),
            "an audit record leaked the client secret: {line}"
        );
        assert!(
            !line.contains("stub-access-token"),
            "an audit record leaked the access token: {line}"
        );
    }
    Ok(())
}

/// A callback that never becomes a session still has to appear in the trail.
///
/// Two distinct refusals, because they arrive by different routes: the provider declining before a
/// code exists, and a code that fails this router's own checks. Recording only one would leave a
/// blind spot on whichever half an attacker used.
#[actix_web::test]
async fn a_refused_sso_callback_is_recorded() -> TestResult {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(captured.clone())
            .with_ansi(false),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    let idp = start_stub_idp()?;
    let clock = Arc::new(ManualClock::new(NOW));
    let app = test::init_service(App::new().configure(configure(service(idp, clock)?))).await;

    // The provider declined.
    let declined = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/oidc/callback?error=access_denied")
            .peer_addr(peer(31))
            .to_request(),
    )
    .await;
    assert_eq!(declined.status(), StatusCode::FOUND);

    // A returned state that does not match the cookie: the CSRF check this flow exists to make.
    let mismatched = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/oidc/callback?code=stub-code&state=not-the-stored-state")
            .peer_addr(peer(32))
            .cookie(actix_web::cookie::Cookie::new("oidc_state", STATE))
            .cookie(actix_web::cookie::Cookie::new("oidc_nonce", NONCE))
            .cookie(actix_web::cookie::Cookie::new(
                "oidc_code_verifier",
                VERIFIER,
            ))
            .to_request(),
    )
    .await;
    assert_eq!(mismatched.status(), StatusCode::FOUND);

    let records = captured.with_event("auth.sso.failed");
    assert_eq!(
        records.len(),
        2,
        "expected a record per refusal, captured: {:?}",
        captured.lines()
    );

    let by_peer = |address: &str| -> Option<String> {
        records
            .iter()
            .find(|line| field(line, "peer").as_deref() == Some(address))
            .cloned()
    };
    let declined_record = by_peer("127.0.0.31").unwrap_or_default();
    let mismatched_record = by_peer("127.0.0.32").unwrap_or_default();

    for (label, record) in [
        ("provider refusal", declined_record.as_str()),
        ("state mismatch", mismatched_record.as_str()),
    ] {
        assert!(
            record.contains("audit=true"),
            "{label} lacks the audit marker: {record}"
        );
        assert!(
            record.contains("WARN"),
            "{label} is not at a level that alerts: {record}"
        );
        assert_eq!(field(record, "method").as_deref(), Some("oidc"), "{record}");
    }

    // The provider's own code, kept verbatim: only it separates a cancelled sign-in from a client the
    // IdP has disabled.
    assert_eq!(
        field(&declined_record, "provider_error").as_deref(),
        Some("access_denied"),
        "{declined_record}"
    );
    // The stable reason, which is what a rule matches on.
    assert_eq!(
        field(&mismatched_record, "reason").as_deref(),
        Some("oidc_invalid_state"),
        "{mismatched_record}"
    );

    // A refusal must never be mistakable for a sign-in.
    assert!(
        captured.with_event("auth.sso.succeeded").is_empty(),
        "a refused callback recorded a success: {:?}",
        captured.lines()
    );
    Ok(())
}

/// A SAML assertion cannot mint a session in this build, and the refusal is recorded.
#[actix_web::test]
async fn a_refused_saml_assertion_is_recorded() -> TestResult {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(captured.clone())
            .with_ansi(false),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    let idp = start_stub_idp()?;
    let clock = Arc::new(ManualClock::new(NOW));
    let app = test::init_service(App::new().configure(configure(service(idp, clock)?))).await;

    // A browser POST, which is refused with a redirect to `/login?error=…` rather than a status code.
    // The security-relevant assertion is not the status but the absence of a session cookie.
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/saml/acs")
            .peer_addr(peer(41))
            .insert_header((
                actix_web::http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            ))
            .set_payload("SAMLResponse=not-base64")
            .to_request(),
    )
    .await;
    let minted_session = response
        .headers()
        .get_all(actix_web::http::header::SET_COOKIE)
        .filter_map(|value| value.to_str().ok())
        .any(|cookie| cookie.starts_with("auth_token=") && !cookie.starts_with("auth_token=;"));
    assert!(
        !minted_session,
        "a SAML assertion minted a session: {:?}",
        response
            .headers()
            .get_all(actix_web::http::header::SET_COOKIE)
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>()
    );
    let location = response
        .headers()
        .get(actix_web::http::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(
        location.contains("/login?error=saml_"),
        "the refusal did not name a SAML cause: status {}, location {location}",
        response.status()
    );

    let records = captured.with_event("auth.sso.failed");
    assert_eq!(
        records.len(),
        1,
        "expected one SAML refusal record, captured: {:?}",
        captured.lines()
    );
    let record = records.first().map(String::as_str).unwrap_or_default();
    assert!(record.contains("audit=true"), "{record}");
    assert_eq!(field(record, "method").as_deref(), Some("saml"), "{record}");
    assert_eq!(
        field(record, "peer").as_deref(),
        Some("127.0.0.41"),
        "{record}"
    );
    assert!(
        field(record, "reason").is_some_and(|reason| reason.starts_with("saml_")),
        "the record carries no SAML-specific reason: {record}"
    );
    assert!(
        captured.with_event("auth.sso.succeeded").is_empty(),
        "a refused SAML assertion recorded a success: {:?}",
        captured.lines()
    );
    Ok(())
}
