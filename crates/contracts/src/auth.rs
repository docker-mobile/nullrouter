use std::fmt;

use serde::{Deserialize, Serialize};

pub const INTERNAL_AUTHORIZE_PATH: &str = "/internal/v1/authorize";
pub const INTERNAL_API_KEY_VALIDATE_PATH: &str = "/internal/v1/keys/validate";
/// One-round-trip `/v1` admission decision: is a key required, and is this one good?
///
/// Exists because the two questions have to be answered together and neither may be cached.
/// `requireApiKey` is a live dashboard setting, so a stale `false` is an authorization bypass;
/// key validity is equally live, because a revoked key must stop working at once. Asking
/// separately meant two loopback round trips on the hottest path in the router.
pub const INTERNAL_API_KEY_GATE_PATH: &str = "/internal/v1/keys/gate";

/// The state service's console-log buffer: `POST` a batch, `GET` since a cursor, `DELETE` to clear.
///
/// Internal, so the gateway refuses it from outside. Anything able to write here can put arbitrary
/// text in front of an operator reading what they believe are their own router's logs.
pub const INTERNAL_CONSOLE_LOGS_PATH: &str = "/internal/v1/console-logs";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub const fn expose_secret(&self) -> &str {
        self.0.as_str()
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SecretString")
            .field(&"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthorizeRequest {
    Dashboard {
        #[serde(rename = "sessionToken", skip_serializing_if = "Option::is_none")]
        session_token: Option<SecretString>,
    },
    Runtime {
        #[serde(rename = "apiKey", skip_serializing_if = "Option::is_none")]
        api_key: Option<SecretString>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizeResponse {
    pub authorized: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateApiKeyRequest {
    pub api_key: SecretString,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateApiKeyResponse {
    pub valid: bool,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
}

/// Ask whether a `/v1` request may proceed.
///
/// The key is optional because the caller does not yet know whether one is needed: it sends
/// whatever the client presented, including nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyGateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<SecretString>,
}

/// The admission decision, read from one snapshot of state.
///
/// `requireApiKey` and the key verdict come from the same read, so they cannot disagree — a
/// gate that turned on between two separate calls used to be answerable "not required" and
/// "not a valid key" at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyGateResponse {
    /// Whether `/v1` currently requires a managed key at all.
    pub require_api_key: bool,
    /// Whether the presented key exists. False when none was presented.
    pub valid: bool,
    /// Whether that key is also enabled. Only meaningful with `valid`.
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
}
