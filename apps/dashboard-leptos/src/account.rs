use serde::Deserialize;

const DISPLAY_NAME_LIMIT: usize = 32;
const LOGIN_METHOD_LIMIT: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountState {
    authentication: AuthenticationState,
    logout: LogoutState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AuthenticationState {
    Checking,
    Authenticated(AccountIdentity),
    Anonymous,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AccountIdentity {
    display_name: String,
    login_method: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum LogoutState {
    #[default]
    Idle,
    Pending,
    Failed,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatusResponse {
    #[serde(default)]
    authenticated: bool,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    login_method: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LogoutResponse {
    success: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountStatusError {
    InvalidPayload,
}

impl AccountState {
    pub const fn checking() -> Self {
        Self {
            authentication: AuthenticationState::Checking,
            logout: LogoutState::Idle,
        }
    }

    pub fn apply_status_json(&mut self, json: &str) -> Result<(), AccountStatusError> {
        let response = serde_json::from_str::<AuthStatusResponse>(json)
            .map_err(|_error| AccountStatusError::InvalidPayload)?;
        self.logout = LogoutState::Idle;
        self.authentication = if response.authenticated {
            AuthenticationState::Authenticated(AccountIdentity {
                display_name: bounded_text(
                    response.display_name.as_deref(),
                    "Password user",
                    DISPLAY_NAME_LIMIT,
                ),
                login_method: bounded_text(
                    response.login_method.as_deref(),
                    "Password",
                    LOGIN_METHOD_LIMIT,
                ),
            })
        } else {
            AuthenticationState::Anonymous
        };
        Ok(())
    }

    pub fn status_failed(&mut self) {
        self.authentication = AuthenticationState::Unavailable;
        self.logout = LogoutState::Idle;
    }

    pub const fn can_logout(&self) -> bool {
        matches!(&self.authentication, AuthenticationState::Authenticated(_))
            && !matches!(self.logout, LogoutState::Pending)
    }

    pub const fn begin_logout(&mut self) -> bool {
        if !self.can_logout() {
            return false;
        }
        self.logout = LogoutState::Pending;
        true
    }

    pub const fn logout_failed(&mut self) {
        if matches!(&self.authentication, AuthenticationState::Authenticated(_)) {
            self.logout = LogoutState::Failed;
        }
    }

    pub const fn is_logout_pending(&self) -> bool {
        matches!(self.logout, LogoutState::Pending)
    }

    pub fn display_name(&self) -> &str {
        match &self.authentication {
            AuthenticationState::Authenticated(identity) => &identity.display_name,
            AuthenticationState::Checking | AuthenticationState::Unavailable => "Rust host",
            AuthenticationState::Anonymous => "Signed out",
        }
    }

    pub fn login_method(&self) -> &str {
        match &self.authentication {
            AuthenticationState::Authenticated(identity) => &identity.login_method,
            AuthenticationState::Checking
            | AuthenticationState::Anonymous
            | AuthenticationState::Unavailable => "",
        }
    }

    pub const fn account_status(&self) -> &'static str {
        match &self.authentication {
            AuthenticationState::Checking => "Checking session",
            AuthenticationState::Authenticated(_) => "Authenticated",
            AuthenticationState::Anonymous => "Signed out",
            AuthenticationState::Unavailable => "Session unavailable",
        }
    }

    pub const fn logout_status(&self) -> &'static str {
        match (&self.authentication, self.logout) {
            (AuthenticationState::Authenticated(_), LogoutState::Idle) => "Available",
            (AuthenticationState::Authenticated(_), LogoutState::Pending) => "Signing out",
            (AuthenticationState::Authenticated(_), LogoutState::Failed) => "Logout failed",
            (
                AuthenticationState::Checking
                | AuthenticationState::Anonymous
                | AuthenticationState::Unavailable,
                LogoutState::Idle | LogoutState::Pending | LogoutState::Failed,
            ) => "Unavailable",
        }
    }

    pub const fn logout_announcement(&self) -> &'static str {
        match self.logout {
            LogoutState::Idle => "",
            LogoutState::Pending => "Signing out",
            LogoutState::Failed => "Logout failed",
        }
    }
}

pub fn logout_response_succeeded(json: &str) -> bool {
    serde_json::from_str::<LogoutResponse>(json).is_ok_and(|response| response.success)
}

fn bounded_text(value: Option<&str>, fallback: &str, limit: usize) -> String {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return fallback.to_owned();
    };
    let bounded = value.chars().take(limit).collect::<String>();
    if bounded.is_empty() {
        fallback.to_owned()
    } else {
        bounded
    }
}
