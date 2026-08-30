use std::net::SocketAddr;

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_contracts::{
    ApiKeyGateRequest, ApiKeyGateResponse, INTERNAL_API_KEY_GATE_PATH,
    INTERNAL_API_KEY_VALIDATE_PATH, SecretString, ValidateApiKeyRequest, ValidateApiKeyResponse,
};
use nullrouter_state::{StateStore, configure};
use serde_json::Value;

pub(crate) const PUBLIC_KEY_MASK: &str = "nr_nullrouter_state_...redacted";
pub(crate) const SECRET_PREFIX: &str = "nr_nullrouter_state_";
pub(crate) const STRONG_SECRET_LEN: usize = 84;

pub(crate) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub(crate) struct JsonRequest<'a> {
    method: Method,
    uri: &'a str,
    body: &'a str,
    content_type: Option<&'a str>,
    peer_addr: Option<SocketAddr>,
}

impl JsonRequest<'_> {
    pub(crate) const fn peer_addr(mut self, peer_addr: SocketAddr) -> Self {
        self.peer_addr = Some(peer_addr);
        self
    }

    pub(crate) const fn without_content_type(mut self) -> Self {
        self.content_type = None;
        self
    }
}

pub(crate) struct JsonResponse {
    pub(crate) status: StatusCode,
    pub(crate) content_type: String,
    body: Vec<u8>,
}

impl JsonResponse {
    pub(crate) fn json(&self) -> TestResult<Value> {
        serde_json::from_slice(&self.body).map_err(Into::into)
    }

    pub(crate) fn validation(&self) -> TestResult<ValidateApiKeyResponse> {
        serde_json::from_slice(&self.body).map_err(Into::into)
    }
}

pub(crate) struct CreatedKey {
    pub(crate) id: String,
    pub(crate) secret: String,
    pub(crate) body: Value,
}

pub(crate) const fn request<'a>(method: Method, uri: &'a str, body: &'a str) -> JsonRequest<'a> {
    JsonRequest {
        method,
        uri,
        body,
        content_type: Some("application/json"),
        peer_addr: None,
    }
}

pub(crate) fn loopback_addr() -> TestResult<SocketAddr> {
    "127.0.0.1:43117".parse().map_err(Into::into)
}

pub(crate) fn remote_addr() -> TestResult<SocketAddr> {
    "198.51.100.17:43117".parse().map_err(Into::into)
}

pub(crate) async fn request_json(
    store: StateStore,
    request: JsonRequest<'_>,
) -> TestResult<JsonResponse> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(store))
            .configure(configure),
    )
    .await;
    let mut builder = test::TestRequest::default()
        .method(request.method)
        .uri(request.uri)
        .set_payload(request.body.to_owned());
    if let Some(content_type) = request.content_type {
        builder = builder.insert_header((header::CONTENT_TYPE, content_type));
    }
    if let Some(peer_addr) = request.peer_addr {
        builder = builder.peer_addr(peer_addr);
    }
    let response = test::call_service(&app, builder.to_request()).await;
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = to_bytes(response.into_body()).await?.to_vec();
    Ok(JsonResponse {
        status,
        content_type,
        body,
    })
}

pub(crate) async fn create_key(store: StateStore, name: &str) -> TestResult<CreatedKey> {
    let payload = serde_json::json!({ "name": name }).to_string();
    let response =
        request_json(store, request(Method::POST, "/api/keys", payload.as_str())).await?;
    assert_eq!(response.status, StatusCode::CREATED);
    assert_json_response(&response);
    let body = response.json()?;
    Ok(CreatedKey {
        id: string_field(&body, "id")?,
        secret: string_field(&body, "key")?,
        body,
    })
}

pub(crate) async fn gate_key(store: StateStore, secret: Option<&str>) -> TestResult<JsonResponse> {
    let payload = serde_json::to_string(&ApiKeyGateRequest {
        api_key: secret.map(SecretString::new),
    })?;
    request_json(
        store,
        request(Method::POST, INTERNAL_API_KEY_GATE_PATH, payload.as_str())
            .peer_addr(loopback_addr()?),
    )
    .await
}

pub(crate) fn gate(response: &JsonResponse) -> TestResult<ApiKeyGateResponse> {
    serde_json::from_slice(&response.body).map_err(Into::into)
}

pub(crate) async fn validate_key(store: StateStore, secret: &str) -> TestResult<JsonResponse> {
    let payload = serde_json::to_string(&ValidateApiKeyRequest {
        api_key: SecretString::new(secret),
    })?;
    request_json(
        store,
        request(
            Method::POST,
            INTERNAL_API_KEY_VALIDATE_PATH,
            payload.as_str(),
        )
        .peer_addr(loopback_addr()?),
    )
    .await
}

pub(crate) fn assert_json_response(response: &JsonResponse) {
    assert!(
        response.content_type.starts_with("application/json"),
        "unexpected content type: {}",
        response.content_type
    );
}

pub(crate) fn assert_public_key_shape(key: &Value) -> TestResult {
    for field_name in ["id", "key", "name", "machineId", "isActive", "createdAt"] {
        field(key, field_name)?;
    }
    Ok(())
}

pub(crate) fn assert_denied(decision: &ValidateApiKeyResponse) {
    assert!(!decision.valid);
    assert!(!decision.active);
    assert!(decision.key_id.is_none());
}

pub(crate) fn assert_inactive(decision: &ValidateApiKeyResponse, key_id: &str) {
    assert!(decision.valid);
    assert!(!decision.active);
    assert_eq!(decision.key_id.as_deref(), Some(key_id));
}

pub(crate) fn assert_denied_json(decision: &Value) -> TestResult {
    assert_eq!(field(decision, "valid")?, false);
    assert_eq!(field(decision, "active")?, false);
    assert!(decision.get("keyId").is_none());
    assert_eq!(object_len(decision)?, 2);
    Ok(())
}

pub(crate) fn assert_no_verification_fields(value: &Value) -> TestResult {
    match value {
        Value::Object(object) => {
            for forbidden in [
                "verification",
                "verificationHash",
                "keyHash",
                "secretHash",
                "legacyKey",
            ] {
                if object.contains_key(forbidden) {
                    return Err(test_error(format!("unexpected field {forbidden}")));
                }
            }
            for nested in object.values() {
                assert_no_verification_fields(nested)?;
            }
        }
        Value::Array(values) => {
            for nested in values {
                assert_no_verification_fields(nested)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

pub(crate) fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

pub(crate) fn string_field(json: &Value, name: &str) -> TestResult<String> {
    field(json, name)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| test_error(format!("{name} is not a string")))
}

pub(crate) fn object_len(json: &Value) -> TestResult<usize> {
    json.as_object()
        .map(serde_json::Map::len)
        .ok_or_else(|| test_error("value is not an object"))
}

pub(crate) fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
