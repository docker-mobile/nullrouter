use std::{net::SocketAddr, time::Duration};

use nullrouter_contracts::{AuthorizeRequest, AuthorizeResponse, INTERNAL_AUTHORIZE_PATH};
use thiserror::Error;

const AUTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Error)]
pub enum AuthClientError {
    #[error("failed to construct the Auth HTTP client")]
    ClientBuild(#[source] reqwest::Error),
    #[error("Auth authorization transport failed")]
    Transport(#[source] reqwest::Error),
    #[error("Auth authorization returned HTTP status {status}")]
    Status { status: u16 },
    #[error("Auth authorization response was invalid")]
    Decode(#[source] reqwest::Error),
}

#[derive(Debug, Clone)]
pub struct AuthClient {
    client: reqwest::Client,
    endpoint: String,
}

impl AuthClient {
    pub fn new(addr: SocketAddr) -> Result<Self, AuthClientError> {
        let client = reqwest::Client::builder()
            .connect_timeout(AUTH_REQUEST_TIMEOUT)
            .timeout(AUTH_REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(AuthClientError::ClientBuild)?;
        Ok(Self {
            client,
            endpoint: format!("http://{addr}{INTERNAL_AUTHORIZE_PATH}"),
        })
    }

    pub async fn authorize(
        &self,
        request: &AuthorizeRequest,
    ) -> Result<AuthorizeResponse, AuthClientError> {
        let response = self
            .client
            .post(&self.endpoint)
            .json(request)
            .send()
            .await
            .map_err(AuthClientError::Transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(AuthClientError::Status {
                status: status.as_u16(),
            });
        }
        response
            .json::<AuthorizeResponse>()
            .await
            .map_err(AuthClientError::Decode)
    }
}
