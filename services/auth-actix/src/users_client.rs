//! Asking the state service whether a credential is good.
//!
//! The password hash lives in `nullrouter-state` and stays there. This client sends a username and
//! password to an endpoint under `/internal`, which the gateway refuses outright, and gets back an
//! identity or a refusal. Nothing here can read the password table, so a compromise of this process
//! does not yield a set of hashes to crack offline.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};

use crate::AuthConfigError;

/// Enough for the verify response, which is a handful of short strings.
const MAX_RESPONSE_BYTES: usize = 8 * 1_024;

/// A verified account.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedUser {
    pub user_id: String,
    pub username: String,
    #[serde(default)]
    pub display_name: String,
    pub role: String,
}

/// The outcome of a credential check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The credential matched.
    Authenticated(VerifiedUser),
    /// It did not. One verdict for a wrong password, an unknown user and a disabled account alike:
    /// telling them apart turns the sign-in form into a way to enumerate accounts.
    Rejected,
    /// No account exists yet, so the shared password is still the way in.
    ///
    /// Distinct from `Rejected` because the caller has to fall back rather than refuse. Collapsing the
    /// two would make an un-migrated install unable to sign in at all.
    NoUsersConfigured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsersError {
    /// The state service could not be reached, or answered something unusable.
    ///
    /// Never treated as a rejection: "the credential store is down" and "that password is wrong" have
    /// to stay distinguishable, or an outage silently becomes a lockout with a misleading message.
    Unavailable,
}

/// Where credentials are checked.
///
/// A trait so a test can drive the login route without a running state service, which is what makes
/// the sign-in cases testable at all.
#[async_trait]
pub trait UserDirectory: Send + Sync {
    async fn verify(&self, username: &str, password: &str) -> Result<Verdict, UsersError>;
}

#[derive(Serialize)]
struct VerifyRequest<'a> {
    username: &'a str,
    password: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyResponse {
    #[serde(default)]
    authenticated: bool,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    shared_password_active: bool,
}

pub(crate) struct HttpUserDirectory {
    client: Client,
    endpoint: Url,
}

impl HttpUserDirectory {
    pub(crate) fn new(endpoint: Url, timeout: Duration) -> Result<Self, AuthConfigError> {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|_| AuthConfigError::StateClient)?;
        Ok(Self { client, endpoint })
    }
}

#[async_trait]
impl UserDirectory for HttpUserDirectory {
    async fn verify(&self, username: &str, password: &str) -> Result<Verdict, UsersError> {
        let response = self
            .client
            .post(self.endpoint.clone())
            .json(&VerifyRequest { username, password })
            .send()
            .await
            .map_err(|_| UsersError::Unavailable)?;
        if response.status() != StatusCode::OK {
            return Err(UsersError::Unavailable);
        }
        if response
            .content_length()
            .is_some_and(|length| length > u64::try_from(MAX_RESPONSE_BYTES).unwrap_or(u64::MAX))
        {
            return Err(UsersError::Unavailable);
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| UsersError::Unavailable)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(UsersError::Unavailable);
        }
        let parsed: VerifyResponse =
            serde_json::from_slice(&bytes).map_err(|_| UsersError::Unavailable)?;

        if parsed.authenticated {
            // An `authenticated: true` with no id or role is a malformed answer, not a pass. Treating
            // it as one would mint a session with no identity and, downstream, no role -- which reads
            // as the legacy full-access principal.
            let (Some(user_id), Some(username), Some(role)) =
                (parsed.user_id, parsed.username, parsed.role)
            else {
                return Err(UsersError::Unavailable);
            };
            return Ok(Verdict::Authenticated(VerifiedUser {
                user_id,
                username,
                display_name: parsed.display_name.unwrap_or_default(),
                role,
            }));
        }
        if parsed.shared_password_active {
            return Ok(Verdict::NoUsersConfigured);
        }
        Ok(Verdict::Rejected)
    }
}
