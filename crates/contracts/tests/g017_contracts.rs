use nullrouter_contracts::{
    AuthorizeRequest, AuthorizeResponse, INTERNAL_API_KEY_VALIDATE_PATH, INTERNAL_AUTHORIZE_PATH,
    SecretString, ValidateApiKeyRequest, ValidateApiKeyResponse,
};
use serde_json::json;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const SECRET_SENTINEL: &str = "g017-sensitive-fixture";

#[test]
fn authorize_request_round_trips_dashboard_and_runtime_variants() -> TestResult {
    assert_eq!(INTERNAL_AUTHORIZE_PATH, "/internal/v1/authorize");

    let dashboard = AuthorizeRequest::Dashboard {
        session_token: Some(SecretString::new(SECRET_SENTINEL)),
    };
    let runtime = AuthorizeRequest::Runtime {
        api_key: Some(SecretString::new(SECRET_SENTINEL)),
    };

    assert_eq!(
        serde_json::to_value(&dashboard)?,
        json!({"kind": "dashboard", "sessionToken": SECRET_SENTINEL})
    );
    assert_eq!(
        serde_json::to_value(&runtime)?,
        json!({"kind": "runtime", "apiKey": SECRET_SENTINEL})
    );
    assert_eq!(
        serde_json::from_value::<AuthorizeRequest>(serde_json::to_value(&dashboard)?)?,
        dashboard
    );
    assert_eq!(
        serde_json::from_value::<AuthorizeRequest>(serde_json::to_value(&runtime)?)?,
        runtime
    );
    Ok(())
}

#[test]
fn authorize_request_accepts_missing_variant_credential() -> TestResult {
    let dashboard: AuthorizeRequest = serde_json::from_value(json!({"kind": "dashboard"}))?;
    let runtime: AuthorizeRequest = serde_json::from_value(json!({"kind": "runtime"}))?;

    match dashboard {
        AuthorizeRequest::Dashboard { session_token } => assert_eq!(session_token, None),
        AuthorizeRequest::Runtime { .. } => {
            return Err(std::io::Error::other("wrong dashboard variant").into());
        }
    }
    match runtime {
        AuthorizeRequest::Runtime { api_key } => assert_eq!(api_key, None),
        AuthorizeRequest::Dashboard { .. } => {
            return Err(std::io::Error::other("wrong runtime variant").into());
        }
    }
    Ok(())
}

#[test]
fn authorize_response_omits_absent_optional_fields() -> TestResult {
    let response = AuthorizeResponse {
        authorized: false,
        principal: None,
        key_id: None,
        reason: Some("invalid_api_key".to_owned()),
    };

    assert_eq!(
        serde_json::to_value(response)?,
        json!({"authorized": false, "reason": "invalid_api_key"})
    );
    Ok(())
}

#[test]
fn authorize_request_debug_is_redacted() {
    let dashboard = AuthorizeRequest::Dashboard {
        session_token: Some(SecretString::new(SECRET_SENTINEL)),
    };
    let runtime = AuthorizeRequest::Runtime {
        api_key: Some(SecretString::new(SECRET_SENTINEL)),
    };

    assert!(!format!("{dashboard:?}").contains(SECRET_SENTINEL));
    assert!(!format!("{runtime:?}").contains(SECRET_SENTINEL));
}

#[test]
fn api_key_validation_contract_is_secret_safe() -> TestResult {
    assert_eq!(INTERNAL_API_KEY_VALIDATE_PATH, "/internal/v1/keys/validate");
    let request = ValidateApiKeyRequest {
        api_key: SecretString::new(SECRET_SENTINEL),
    };
    let response = ValidateApiKeyResponse {
        valid: true,
        active: true,
        key_id: Some("key_fixture".to_owned()),
    };

    assert!(!format!("{request:?}").contains(SECRET_SENTINEL));
    assert_eq!(
        serde_json::to_value(&request)?,
        json!({"apiKey": SECRET_SENTINEL})
    );
    let response_json = serde_json::to_string(&response)?;
    assert!(!response_json.contains(SECRET_SENTINEL));
    assert_eq!(
        serde_json::from_str::<ValidateApiKeyResponse>(&response_json)?,
        response
    );
    Ok(())
}

#[test]
fn api_key_validation_response_omits_absent_key_id() -> TestResult {
    let response = ValidateApiKeyResponse {
        valid: false,
        active: false,
        key_id: None,
    };

    assert_eq!(
        serde_json::to_value(response)?,
        json!({"valid": false, "active": false})
    );
    Ok(())
}

#[test]
fn secret_debug_is_redacted() {
    let secret = SecretString::new(SECRET_SENTINEL);

    let debug = format!("{secret:?}");
    assert!(!debug.contains(SECRET_SENTINEL));
    assert!(debug.contains("REDACTED"));
    assert_eq!(secret.expose_secret(), SECRET_SENTINEL);
}
