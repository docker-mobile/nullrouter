use actix_web::http::StatusCode;
use nullrouter_state::StateStore;
use serde_json::{Value, json};

use super::support::{TestResult, assert_inactive, create_key, field, validate_key};

#[actix_rt::test]
async fn snapshot_persists_verification_material_without_the_created_secret() -> TestResult {
    // Given
    let directory = tempfile::tempdir()?;
    let state_file = directory.path().join("state.json");
    let store = StateStore::file(&state_file)?;
    let created = create_key(store, "persist safely").await?;

    // When
    let snapshot_text = std::fs::read_to_string(&state_file)?;
    let snapshot: Value = serde_json::from_str(&snapshot_text)?;

    // Then
    assert!(!snapshot_text.contains(&created.secret));
    let record = field(&snapshot, "apiKeys")?
        .as_array()
        .and_then(|keys| keys.first())
        .ok_or_else(|| super::support::test_error("missing persisted key"))?;
    assert!(record.get("key").is_none());
    assert!(record.get("verification").is_some());
    assert!(record.get("legacyKey").is_none());
    Ok(())
}

#[actix_rt::test]
async fn active_key_validation_survives_store_restart() -> TestResult {
    // Given
    let directory = tempfile::tempdir()?;
    let state_file = directory.path().join("state.json");
    let created = create_key(StateStore::file(&state_file)?, "restart validation").await?;
    let reloaded = StateStore::file(&state_file)?;

    // When
    let response = validate_key(reloaded, &created.secret).await?;

    // Then
    assert_eq!(response.status, StatusCode::OK);
    let decision = response.validation()?;
    assert!(decision.valid);
    assert!(decision.active);
    assert_eq!(decision.key_id.as_deref(), Some(created.id.as_str()));
    Ok(())
}

#[actix_rt::test]
async fn legacy_raw_key_snapshot_is_migrated_and_remains_valid() -> TestResult {
    // Given
    let directory = tempfile::tempdir()?;
    let state_file = directory.path().join("legacy.json");
    let legacy_secret = ["nr_nullrouter_", "state_legacy"].concat();
    let legacy_id = "key_legacy_1";
    let snapshot = json!({
        "apiKeys": [{
            "id": legacy_id,
            "key": legacy_secret,
            "name": "legacy",
            "machineId": "nullrouter-state",
            "isActive": true,
            "createdAt": "unix-ms:1"
        }],
        "providerConnections": [],
        "providerNodes": [],
        "combos": [],
        "proxyPools": [],
        "settings": {
            "requireLogin": true,
            "tunnelDashboardAccess": false,
            "tunnelUrl": "",
            "tailscaleUrl": "",
            "outboundProxyEnabled": false,
            "outboundProxyUrl": "",
            "outboundNoProxy": ""
        }
    });
    std::fs::write(&state_file, serde_json::to_vec_pretty(&snapshot)?)?;

    // When
    let migrated = StateStore::file(&state_file)?;
    let decision = validate_key(migrated, &legacy_secret).await?;

    // Then
    assert_eq!(decision.status, StatusCode::OK);
    let body = decision.validation()?;
    assert!(body.valid);
    assert!(body.active);
    assert_eq!(body.key_id.as_deref(), Some(legacy_id));
    let migrated_text = std::fs::read_to_string(&state_file)?;
    assert!(!migrated_text.contains(&legacy_secret));
    let migrated_snapshot: Value = serde_json::from_str(&migrated_text)?;
    let record = field(&migrated_snapshot, "apiKeys")?
        .as_array()
        .and_then(|keys| keys.first())
        .ok_or_else(|| super::support::test_error("missing migrated key"))?;
    assert!(record.get("key").is_none());
    assert!(record.get("verification").is_some());
    Ok(())
}

#[actix_rt::test]
async fn legacy_inactive_key_migrates_but_stays_denied() -> TestResult {
    // Given
    let directory = tempfile::tempdir()?;
    let state_file = directory.path().join("inactive-legacy.json");
    let legacy_secret = ["nr_nullrouter_", "state_inactive"].concat();
    let snapshot = json!({
        "apiKeys": [{
            "id": "key_legacy_inactive",
            "key": legacy_secret,
            "name": "legacy inactive",
            "machineId": "nullrouter-state",
            "isActive": false,
            "createdAt": "unix-ms:1"
        }],
        "providerConnections": [],
        "providerNodes": [],
        "combos": [],
        "proxyPools": [],
        "settings": {
            "requireLogin": true,
            "tunnelDashboardAccess": false,
            "tunnelUrl": "",
            "tailscaleUrl": "",
            "outboundProxyEnabled": false,
            "outboundProxyUrl": "",
            "outboundNoProxy": ""
        }
    });
    std::fs::write(&state_file, serde_json::to_vec_pretty(&snapshot)?)?;
    let store = StateStore::file(&state_file)?;

    // When
    let response = validate_key(store, &legacy_secret).await?;

    // Then
    assert_eq!(response.status, StatusCode::OK);
    assert_inactive(&response.validation()?, "key_legacy_inactive");
    Ok(())
}
