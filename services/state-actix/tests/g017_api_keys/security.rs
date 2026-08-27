use actix_web::http::{Method, StatusCode};
use nullrouter_state::StateStore;

use super::support::{
    PUBLIC_KEY_MASK, SECRET_PREFIX, STRONG_SECRET_LEN, TestResult, assert_no_verification_fields,
    create_key, field, request, request_json, string_field,
};

#[actix_rt::test]
async fn created_keys_are_distinct_and_have_256_bits_of_random_material() -> TestResult {
    // Given
    let store = StateStore::memory();

    // When
    let first = create_key(store.clone(), "first strong key").await?;
    let second = create_key(store, "second strong key").await?;

    // Then
    assert_ne!(first.secret, second.secret);
    for secret in [first.secret, second.secret] {
        assert!(secret.starts_with(SECRET_PREFIX));
        assert_eq!(secret.len(), STRONG_SECRET_LEN);
        assert!(secret.strip_prefix(SECRET_PREFIX).is_some_and(|random| {
            random
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        }));
    }
    Ok(())
}

#[actix_rt::test]
async fn list_discloses_only_the_public_key_mask() -> TestResult {
    // Given
    let store = StateStore::memory();
    let created = create_key(store.clone(), "list redaction").await?;

    // When
    let response = request_json(store, request(Method::GET, "/api/keys", "")).await?;

    // Then
    assert_eq!(response.status, StatusCode::OK);
    let body = response.json()?;
    let key = field(&body, "keys")?
        .as_array()
        .and_then(|keys| keys.first())
        .ok_or_else(|| super::support::test_error("missing listed key"))?;
    assert!(
        field(key, "key")? == PUBLIC_KEY_MASK,
        "list returned an unmasked key"
    );
    assert!(
        field(key, "key")? != created.secret.as_str(),
        "list repeated the create-only secret"
    );
    assert_no_verification_fields(&body)?;
    Ok(())
}

#[actix_rt::test]
async fn get_discloses_only_the_public_key_mask() -> TestResult {
    // Given
    let store = StateStore::memory();
    let created = create_key(store.clone(), "get redaction").await?;

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
    assert!(
        string_field(key, "key")? == PUBLIC_KEY_MASK,
        "get returned an unmasked key"
    );
    assert_no_verification_fields(&body)?;
    Ok(())
}

#[actix_rt::test]
async fn update_discloses_only_the_public_key_mask() -> TestResult {
    // Given
    let store = StateStore::memory();
    let created = create_key(store.clone(), "update redaction").await?;

    // When
    let response = request_json(
        store,
        request(
            Method::PUT,
            &format!("/api/keys/{}", created.id),
            r#"{"isActive":false}"#,
        ),
    )
    .await?;

    // Then
    assert_eq!(response.status, StatusCode::OK);
    let body = response.json()?;
    let key = field(&body, "key")?;
    assert!(
        field(key, "key")? == PUBLIC_KEY_MASK,
        "update returned an unmasked key"
    );
    assert_eq!(field(key, "isActive")?, false);
    assert_no_verification_fields(&body)?;
    Ok(())
}
