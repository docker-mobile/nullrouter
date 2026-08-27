use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, StatusCode, Url};
use thiserror::Error;

use crate::{
    AuthConfigError,
    contracts::{ValidateApiKeyRequest, ValidateApiKeyResponse},
};

const MAX_API_KEY_BYTES: usize = 4_096;
const MAX_STATE_RESPONSE_BYTES: usize = 8_192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyValidation {
    pub valid: bool,
    pub active: bool,
    pub key_id: Option<String>,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum StateValidationError {
    #[error("state validation service is unavailable")]
    Unavailable,
    #[error("state validation response is invalid")]
    InvalidResponse,
}

#[async_trait]
pub trait ApiKeyValidator: Send + Sync {
    async fn validate(&self, api_key: &str) -> Result<ApiKeyValidation, StateValidationError>;
}

pub(crate) struct HttpApiKeyValidator {
    client: Client,
    endpoint: Url,
}

impl HttpApiKeyValidator {
    pub(crate) fn new(endpoint: Url, timeout: Duration) -> Result<Self, AuthConfigError> {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|_| AuthConfigError::StateClient)?;
        Ok(Self { client, endpoint })
    }
}

#[async_trait]
impl ApiKeyValidator for HttpApiKeyValidator {
    async fn validate(&self, api_key: &str) -> Result<ApiKeyValidation, StateValidationError> {
        if api_key.is_empty() || api_key.len() > MAX_API_KEY_BYTES {
            return Ok(ApiKeyValidation {
                valid: false,
                active: false,
                key_id: None,
            });
        }
        let response = self
            .client
            .post(self.endpoint.clone())
            .json(&ValidateApiKeyRequest { api_key })
            .send()
            .await
            .map_err(|_| StateValidationError::Unavailable)?;
        if response.status() != StatusCode::OK {
            return Err(StateValidationError::Unavailable);
        }
        if response.content_length().is_some_and(|length| {
            length > u64::try_from(MAX_STATE_RESPONSE_BYTES).unwrap_or(u64::MAX)
        }) {
            return Err(StateValidationError::InvalidResponse);
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| StateValidationError::Unavailable)?;
        if bytes.len() > MAX_STATE_RESPONSE_BYTES {
            return Err(StateValidationError::InvalidResponse);
        }
        let response: ValidateApiKeyResponse =
            serde_json::from_slice(&bytes).map_err(|_| StateValidationError::InvalidResponse)?;
        Ok(ApiKeyValidation {
            valid: response.valid,
            active: response.active,
            key_id: response.key_id,
        })
    }
}
