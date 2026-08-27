use actix_web::http::{Method, StatusCode};
use nullrouter_state::StateStore;

use super::support::{
    TestResult, assert_json_response, assert_public_key_shape, create_key, field, object_len,
    request, request_json, string_field,
};

#[actix_rt::test]
async fn create_key_preserves_current_metadata_shape() -> TestResult {
    // Given
    let store = StateStore::memory();

    // When
    let created = create_key(store, "source characterization").await?;

    // Then
    assert_public_key_shape(&created.body)?;
    assert_eq!(object_len(&created.body)?, 6);
    assert_eq!(field(&created.body, "name")?, "source characterization");
    assert_eq!(field(&created.body, "machineId")?, "nullrouter-state");
    assert_eq!(field(&created.body, "isActive")?, true);
    Ok(())
}

#[actix_rt::test]
async fn list_key_preserves_current_envelope() -> TestResult {
    // Given
    let store = StateStore::memory();
    let created = create_key(store.clone(), "listed").await?;

    // When
    let response = request_json(store, request(Method::GET, "/api/keys", "")).await?;

    // Then
    assert_eq!(response.status, StatusCode::OK);
    assert_json_response(&response);
    let body = response.json()?;
    let key = field(&body, "keys")?
        .as_array()
        .and_then(|keys| keys.first())
        .ok_or_else(|| super::support::test_error("missing listed key"))?;
    assert_public_key_shape(key)?;
    assert_eq!(field(key, "id")?, created.id.as_str());
    Ok(())
}

#[actix_rt::test]
async fn get_key_preserves_current_envelope() -> TestResult {
    // Given
    let store = StateStore::memory();
    let created = create_key(store.clone(), "fetched").await?;

    // When
    let response = request_json(
        store,
        request(Method::GET, &format!("/api/keys/{}", created.id), ""),
    )
    .await?;

    // Then
    assert_eq!(response.status, StatusCode::OK);
    let body = response.json()?;
    let key = field(&body, "key")?;
    assert_public_key_shape(key)?;
    assert_eq!(field(key, "id")?, created.id.as_str());
    Ok(())
}

#[actix_rt::test]
async fn update_and_delete_key_preserve_current_status_contracts() -> TestResult {
    // Given
    let store = StateStore::memory();
    let created = create_key(store.clone(), "mutable").await?;
    let key_uri = format!("/api/keys/{}", created.id);

    // When
    let updated = request_json(
        store.clone(),
        request(Method::PUT, key_uri.as_str(), r#"{"isActive":false}"#),
    )
    .await?;

    // Then
    assert_eq!(updated.status, StatusCode::OK);
    let updated_body = updated.json()?;
    let key = field(&updated_body, "key")?;
    assert_public_key_shape(key)?;
    assert_eq!(field(key, "isActive")?, false);

    let deleted = request_json(store, request(Method::DELETE, key_uri.as_str(), "")).await?;
    assert_eq!(deleted.status, StatusCode::OK);
    assert_eq!(
        field(&deleted.json()?, "message")?,
        "Key deleted successfully"
    );
    Ok(())
}

#[actix_rt::test]
async fn provider_create_and_update_responses_keep_credentials_redacted() -> TestResult {
    // Given
    let store = StateStore::memory();
    let provider_secret = ["provider", "-credential"].concat();
    let create_payload = serde_json::json!({
        "provider": "openai",
        "apiKey": provider_secret,
        "name": "redaction pin"
    })
    .to_string();
    let created = request_json(
        store.clone(),
        request(Method::POST, "/api/providers", create_payload.as_str()),
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED);
    let created_body = created.json()?;
    let connection = field(&created_body, "connection")?;
    assert!(connection.get("apiKey").is_none());
    let id = string_field(connection, "id")?;

    // When
    let updated = request_json(
        store,
        request(
            Method::PUT,
            &format!("/api/providers/{id}"),
            r#"{"isActive":false}"#,
        ),
    )
    .await?;

    // Then
    assert_eq!(updated.status, StatusCode::OK);
    assert!(
        field(&updated.json()?, "connection")?
            .get("apiKey")
            .is_none()
    );
    Ok(())
}
