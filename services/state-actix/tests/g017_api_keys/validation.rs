use actix_web::http::{Method, StatusCode};
use nullrouter_contracts::INTERNAL_API_KEY_VALIDATE_PATH;
use nullrouter_state::StateStore;

use super::support::{
    TestResult, assert_denied, assert_denied_json, assert_inactive, assert_json_response,
    create_key, field, gate, gate_key, loopback_addr, remote_addr, request, request_json,
    validate_key,
};

#[actix_rt::test]
async fn active_key_returns_only_the_minimum_identity_decision() -> TestResult {
    // Given
    let store = StateStore::memory();
    let created = create_key(store.clone(), "active validation").await?;

    // When
    let response = validate_key(store, &created.secret).await?;

    // Then
    assert_eq!(response.status, StatusCode::OK);
    assert_json_response(&response);
    let decision = response.validation()?;
    assert!(decision.valid);
    assert!(decision.active);
    assert_eq!(decision.key_id.as_deref(), Some(created.id.as_str()));
    assert_eq!(super::support::object_len(&response.json()?)?, 3);
    Ok(())
}

#[actix_rt::test]
async fn inactive_key_returns_valid_but_inactive_identity() -> TestResult {
    // Given
    let store = StateStore::memory();
    let inactive = create_key(store.clone(), "inactive validation").await?;
    let inactive_uri = format!("/api/keys/{}", inactive.id);
    request_json(
        store.clone(),
        request(Method::PUT, inactive_uri.as_str(), r#"{"isActive":false}"#),
    )
    .await?;

    // When
    let response = validate_key(store, &inactive.secret).await?;

    // Then
    assert_eq!(response.status, StatusCode::OK);
    assert_inactive(&response.validation()?, &inactive.id);
    assert_eq!(super::support::object_len(&response.json()?)?, 3);
    Ok(())
}

#[actix_rt::test]
async fn deleted_unknown_and_malformed_keys_share_the_deny_shape() -> TestResult {
    // Given
    let store = StateStore::memory();
    let deleted = create_key(store.clone(), "deleted validation").await?;
    let deleted_uri = format!("/api/keys/{}", deleted.id);
    request_json(
        store.clone(),
        request(Method::DELETE, deleted_uri.as_str(), ""),
    )
    .await?;
    let unknown = [super::support::SECRET_PREFIX, &"0".repeat(64)].concat();

    // When
    let decisions = [
        validate_key(store.clone(), &deleted.secret).await?,
        validate_key(store.clone(), &unknown).await?,
        validate_key(store, "malformed").await?,
    ];

    // Then
    let mut prior = None;
    for response in decisions {
        assert_eq!(response.status, StatusCode::OK);
        let typed = response.validation()?;
        assert_denied(&typed);
        let decision = response.json()?;
        assert_denied_json(&decision)?;
        if let Some(prior) = prior.as_ref() {
            assert_eq!(prior, &decision);
        }
        prior = Some(decision);
    }
    Ok(())
}

#[actix_rt::test]
async fn internal_validation_rejects_non_loopback_peers_before_parsing() -> TestResult {
    // Given
    let store = StateStore::memory();

    // When
    let response = request_json(
        store,
        request(Method::POST, INTERNAL_API_KEY_VALIDATE_PATH, "not-json").peer_addr(remote_addr()?),
    )
    .await?;

    // Then
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_json_response(&response);
    assert_eq!(
        field(&response.json()?, "error")?,
        "Internal route requires loopback peer"
    );
    Ok(())
}

#[actix_rt::test]
async fn internal_validation_requires_json_content_type() -> TestResult {
    // Given
    let store = StateStore::memory();

    // When
    let response = request_json(
        store,
        request(
            Method::POST,
            INTERNAL_API_KEY_VALIDATE_PATH,
            r#"{"apiKey":"malformed"}"#,
        )
        .without_content_type()
        .peer_addr(loopback_addr()?),
    )
    .await?;

    // Then
    assert_eq!(response.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_json_response(&response);
    assert_eq!(
        field(&response.json()?, "error")?,
        "Content-Type must be application/json"
    );
    Ok(())
}

#[actix_rt::test]
async fn internal_validation_rejects_malformed_or_missing_json_structurally() -> TestResult {
    // Given
    let store = StateStore::memory();

    // When
    let malformed = request_json(
        store.clone(),
        request(Method::POST, INTERNAL_API_KEY_VALIDATE_PATH, "{").peer_addr(loopback_addr()?),
    )
    .await?;
    let missing = request_json(
        store,
        request(Method::POST, INTERNAL_API_KEY_VALIDATE_PATH, "{}").peer_addr(loopback_addr()?),
    )
    .await?;

    // Then
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&malformed.json()?, "error")?, "Invalid JSON body");
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&missing.json()?, "error")?, "Invalid JSON body");
    Ok(())
}

#[actix_rt::test]
async fn internal_validation_wrong_method_returns_structured_405() -> TestResult {
    // Given
    let store = StateStore::memory();

    // When
    let response = request_json(
        store,
        request(Method::GET, INTERNAL_API_KEY_VALIDATE_PATH, "").peer_addr(loopback_addr()?),
    )
    .await?;

    // Then
    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_json_response(&response);
    assert_eq!(field(&response.json()?, "error")?, "Method not allowed");
    Ok(())
}

#[actix_rt::test]
async fn gate_reads_requirement_and_key_verdict_from_one_snapshot() -> TestResult {
    let store = StateStore::memory();
    let key = create_key(store.clone(), "gate").await?;

    let initially_public = gate_key(store.clone(), None).await?;
    assert_eq!(initially_public.status, StatusCode::OK);
    let decision = gate(&initially_public)?;
    assert!(!decision.require_api_key);
    assert!(!decision.valid);
    assert!(!decision.active);

    request_json(
        store.clone(),
        request(Method::PUT, "/api/settings", r#"{"requireApiKey":true}"#),
    )
    .await?;
    let permitted = gate_key(store.clone(), Some(&key.secret)).await?;
    let decision = gate(&permitted)?;
    assert!(decision.require_api_key);
    assert!(decision.valid);
    assert!(decision.active);
    assert_eq!(decision.key_id.as_deref(), Some(key.id.as_str()));

    let missing = gate_key(store, None).await?;
    let decision = gate(&missing)?;
    assert!(decision.require_api_key);
    assert!(!decision.valid);
    assert!(!decision.active);
    assert!(decision.key_id.is_none());
    Ok(())
}
