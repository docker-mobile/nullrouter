use std::fmt;

use serde::{Deserialize, Serialize};

pub const INTERNAL_AUTHORIZE_PATH: &str = "/internal/v1/authorize";
pub const INTERNAL_API_KEY_VALIDATE_PATH: &str = "/internal/v1/keys/validate";

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
