use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub(crate) struct LoginRequest {
    pub(crate) password: Option<String>,
    /// Absent on a shared-password sign-in, which is still accepted while no account exists.
    #[serde(default)]
    pub(crate) username: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginSuccessResponse {
    pub(crate) success: bool,
    pub(crate) must_change_password: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginDeniedResponse {
    pub(crate) error: String,
    pub(crate) remaining_before_lock: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginLockedResponse {
    pub(crate) error: String,
    pub(crate) retry_after: u64,
    pub(crate) reset_hint: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct LogoutResponse {
    pub(crate) success: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthStatusResponse {
    pub(crate) authenticated: bool,
    pub(crate) require_login: bool,
    pub(crate) auth_mode: &'static str,
    pub(crate) oidc_configured: bool,
    /// Owned, not `&'static str`: an operator sets this label in settings, so it is runtime data.
    /// It was a literal while `oidc_configured` was hardcoded `false` and the field could never be
    /// anything but the placeholder.
    pub(crate) oidc_login_label: String,
    pub(crate) saml_configured: bool,
    pub(crate) has_password: bool,
    /// Owned since a managed account supplies it. `"Password user"` on a shared-password session,
    /// which is what those sessions were before accounts existed.
    pub(crate) display_name: String,
    pub(crate) login_method: &'static str,
    pub(crate) oidc_name: Option<&'static str>,
    pub(crate) oidc_email: Option<&'static str>,
    pub(crate) oidc_login: bool,
    /// The signed-in account's id, absent on a shared-password session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) user_id: Option<String>,
    /// `admin`, `operator` or `viewer`.
    ///
    /// A shared-password session reports `admin`: it is the legacy full-access principal, and calling
    /// it anything less would hide controls from an operator who has not migrated yet.
    pub(crate) role: &'static str,
    /// Whether any managed account exists. False means the shared password is still the way in, which
    /// the dashboard says out loud rather than leaving an operator to discover.
    pub(crate) users_configured: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct HealthResponse {
    pub(crate) ok: bool,
    pub(crate) service: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct ErrorEnvelope {
    pub(crate) error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub(crate) struct ErrorBody {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
    #[serde(rename = "type")]
    pub(crate) error_type: &'static str,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AuthorizeRequest {
    Dashboard {
        #[serde(rename = "sessionToken")]
        session_token: Option<String>,
    },
    Runtime {
        #[serde(rename = "apiKey")]
        api_key: Option<String>,
    },
}

impl AuthorizeRequest {
    pub(crate) fn into_dashboard_token(self) -> Option<String> {
        match self {
            Self::Dashboard { session_token } => session_token,
            Self::Runtime { .. } => None,
        }
    }

    pub(crate) fn into_runtime_key(self) -> Option<String> {
        match self {
            Self::Runtime { api_key } => api_key,
            Self::Dashboard { .. } => None,
        }
    }

    pub(crate) const fn kind(&self) -> AuthorizationKind {
        match self {
            Self::Dashboard { .. } => AuthorizationKind::Dashboard,
            Self::Runtime { .. } => AuthorizationKind::Runtime,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum AuthorizationKind {
    Dashboard,
    Runtime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthorizeResponse {
    pub(crate) authorized: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) principal: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<&'static str>,
    /// The session's role, so the gateway can enforce it. Absent for an API key and for a session
    /// minted before managed users existed; the gateway reads absent as full access.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<String>,
}

impl AuthorizeResponse {
    pub(crate) const fn denied(reason: &'static str) -> Self {
        Self {
            authorized: false,
            principal: None,
            key_id: None,
            reason: Some(reason),
            role: None,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ValidateApiKeyRequest<'a> {
    pub(crate) api_key: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ValidateApiKeyResponse {
    pub(crate) valid: bool,
    pub(crate) active: bool,
    pub(crate) key_id: Option<String>,
}
