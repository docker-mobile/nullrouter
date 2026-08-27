pub mod support;

use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use actix_web::{
    App,
    http::{Method, StatusCode, header},
    test,
};
use async_trait::async_trait;
use serde_json::Value;

use nullrouter_auth::{
    ApiKeyValidation, ApiKeyValidator, LockoutConfig, StateValidationError, configure,
};
use support::{ManualClock, default_lockout, extract_cookie, peer, service};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Debug)]
struct AcceptAllValidator;

#[async_trait]
impl ApiKeyValidator for AcceptAllValidator {
    async fn validate(&self, _api_key: &str) -> Result<ApiKeyValidation, StateValidationError> {
        Ok(ApiKeyValidation {
            valid: true,
            active: true,
            key_id: Some("key_fixture".to_owned()),
        })
    }
}

#[actix_web::test]
async fn login_cookie_status_logout_round_trip() -> TestResult {
    let clock = Arc::new(ManualClock::new(1_700_000_000));
    let validator = Arc::new(AcceptAllValidator);
    let app = test::init_service(App::new().configure(configure(service(
        Arc::clone(&clock),
        validator,
        Duration::from_secs(3_600),
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
    assert_eq!(login.status(), StatusCode::OK);
    let set_cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .ok_or_else(|| std::io::Error::other("missing login cookie"))?
        .to_str()?
        .to_owned();
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("Path=/"));
    assert!(set_cookie.contains("SameSite=Lax"));
    assert!(set_cookie.contains("Secure"));
    let cookie = extract_cookie(&login)?;

    let status = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/status")
            .peer_addr(peer(1))
            .insert_header((header::COOKIE, cookie.clone()))
            .to_request(),
    )
    .await;
    assert_eq!(status.status(), StatusCode::OK);
    let status_json: Value = test::read_body_json(status).await;
    assert_eq!(status_json.get("authenticated"), Some(&Value::Bool(true)));

    let logout = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/logout")
            .peer_addr(peer(1))
            .insert_header((header::COOKIE, cookie))
            .to_request(),
    )
    .await;
    assert_eq!(logout.status(), StatusCode::OK);
    let clear_cookie = logout
        .headers()
        .get(header::SET_COOKIE)
        .ok_or_else(|| std::io::Error::other("missing logout cookie"))?
        .to_str()?;
    assert!(clear_cookie.contains("auth_token="));
    assert!(clear_cookie.contains("Max-Age=0"));
    Ok(())
}

#[actix_web::test]
async fn tampered_and_expired_cookies_are_rejected() -> TestResult {
    let clock = Arc::new(ManualClock::new(10_000));
    let app = test::init_service(App::new().configure(configure(service(
        Arc::clone(&clock),
        Arc::new(AcceptAllValidator),
        Duration::from_secs(10),
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

    let mut tampered = cookie.clone();
    tampered.push('x');
    let tampered_status = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/status")
            .peer_addr(peer(1))
            .insert_header((header::COOKIE, tampered))
            .to_request(),
    )
    .await;
    let tampered_json: Value = test::read_body_json(tampered_status).await;
    assert_eq!(
        tampered_json.get("authenticated"),
        Some(&Value::Bool(false))
    );

    clock.now.fetch_add(11, Ordering::SeqCst);
    let expired_status = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/status")
            .peer_addr(peer(1))
            .insert_header((header::COOKIE, cookie))
            .to_request(),
    )
    .await;
    let expired_json: Value = test::read_body_json(expired_status).await;
    assert_eq!(expired_json.get("authenticated"), Some(&Value::Bool(false)));
    Ok(())
}

#[actix_web::test]
async fn malformed_login_json_is_structured_400() -> TestResult {
    let app = test::init_service(App::new().configure(configure(service(
        Arc::new(ManualClock::new(1)),
        Arc::new(AcceptAllValidator),
        Duration::from_secs(60),
        default_lockout(),
    )?)))
    .await;
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .peer_addr(peer(1))
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .set_payload("{")
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json: Value = test::read_body_json(response).await;
    assert_eq!(
        json.pointer("/error/code"),
        Some(&Value::String("invalid_json".to_owned()))
    );
    Ok(())
}

#[actix_web::test]
async fn fixed_peer_lockout_ignores_forwarded_headers() -> TestResult {
    let app = test::init_service(App::new().configure(configure(service(
        Arc::new(ManualClock::new(50)),
        Arc::new(AcceptAllValidator),
        Duration::from_secs(60),
        default_lockout(),
    )?)))
    .await;
    for attempt in 1..=5 {
        let response = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/auth/login")
                .peer_addr(peer(1))
                .insert_header((header::CONTENT_TYPE, "application/json"))
                .insert_header(("x-forwarded-for", format!("203.0.113.{attempt}")))
                .insert_header(("forwarded", format!("for=198.51.100.{attempt}")))
                .set_payload(r#"{"password":"wrong"}"#)
                .to_request(),
        )
        .await;
        if attempt < 5 {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        } else {
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert!(response.headers().get(header::RETRY_AFTER).is_some());
        }
    }
    Ok(())
}

#[actix_web::test]
async fn lockout_storage_is_bounded_and_expiring() -> TestResult {
    let clock = Arc::new(ManualClock::new(100));
    let lockout = LockoutConfig {
        threshold: 3,
        window: Duration::from_secs(60),
        lock_duration: Duration::from_secs(120),
        capacity: 2,
    };
    let app = test::init_service(App::new().configure(configure(service(
        Arc::clone(&clock),
        Arc::new(AcceptAllValidator),
        Duration::from_secs(60),
        lockout,
    )?)))
    .await;

    for ip in [1, 2, 3] {
        let response = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/auth/login")
                .peer_addr(peer(ip))
                .insert_header((header::CONTENT_TYPE, "application/json"))
                .set_payload(r#"{"password":"wrong"}"#)
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    for _ in 0..2 {
        let response = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/auth/login")
                .peer_addr(peer(1))
                .insert_header((header::CONTENT_TYPE, "application/json"))
                .set_payload(r#"{"password":"wrong"}"#)
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    for expected in [
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED,
        StatusCode::TOO_MANY_REQUESTS,
    ] {
        let response = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/auth/login")
                .peer_addr(peer(4))
                .insert_header((header::CONTENT_TYPE, "application/json"))
                .set_payload(r#"{"password":"wrong"}"#)
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), expected);
    }
    clock.now.fetch_add(121, Ordering::SeqCst);
    let expired = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .peer_addr(peer(4))
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .set_payload(r#"{"password":"wrong"}"#)
            .to_request(),
    )
    .await;
    assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[actix_web::test]
async fn health_and_structured_protocol_errors() -> TestResult {
    let app = test::init_service(App::new().configure(configure(service(
        Arc::new(ManualClock::new(1)),
        Arc::new(AcceptAllValidator),
        Duration::from_secs(60),
        default_lockout(),
    )?)))
    .await;
    let health = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/health")
            .peer_addr(peer(1))
            .to_request(),
    )
    .await;
    assert_eq!(health.status(), StatusCode::OK);

    let method = test::call_service(
        &app,
        test::TestRequest::default()
            .method(Method::PUT)
            .uri("/api/auth/status")
            .peer_addr(peer(1))
            .to_request(),
    )
    .await;
    assert_eq!(method.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(method.headers().get(header::ALLOW).is_some());
    let method_json: Value = test::read_body_json(method).await;
    assert_eq!(
        method_json.pointer("/error/code"),
        Some(&Value::String("method_not_allowed".to_owned()))
    );

    let missing = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/not-a-route")
            .peer_addr(peer(1))
            .to_request(),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let missing_json: Value = test::read_body_json(missing).await;
    assert_eq!(
        missing_json.pointer("/error/code"),
        Some(&Value::String("not_found".to_owned()))
    );
    Ok(())
}
