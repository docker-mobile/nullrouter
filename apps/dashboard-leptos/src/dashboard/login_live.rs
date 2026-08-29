//! Login panel state: what it reads, what it sends, and where it may navigate.
//!
//! This was 162 lines of inline JavaScript in the actix host's login shell. The
//! logic is the same; it lives here so it is type-checked and unit-tested rather
//! than only exercised by a browser.
//!
//! The derivations that matter for safety are [`dashboard_target`] (a redirect
//! sanitiser) and [`skips_login`] (an auth decision). Both are pure functions
//! over untrusted input, which is exactly why they are worth having out of the
//! markup.

use serde::Deserialize;

/// Where the panel navigates after a successful sign-in when no `?next=` applies.
pub const DEFAULT_TARGET: &str = "/dashboard";

/// Upper bound on a lockout countdown, in seconds.
///
/// A hostile or broken `Retry-After` must not park the button for a week.
const MAX_RETRY_SECONDS: u32 = 3600;

/// Shown when repeated failures have locked the address out.
pub const RESET_HINT: &str =
    "Forgot password? Reset to default via 9Router CLI -> Settings -> Reset Password to Default.";

/// `GET /api/auth/status`, as the login panel reads it.
///
/// Every field is optional: this is read before authenticating, so a partial or
/// unexpected body must degrade to "show the password form" rather than break the
/// only screen from which the user could recover.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    #[serde(default)]
    pub authenticated: bool,
    #[serde(default)]
    pub oidc_configured: bool,
    #[serde(default)]
    pub auth_mode: Option<String>,
    #[serde(default)]
    pub oidc_login_label: Option<String>,
}

impl AuthStatus {
    /// Whether an OIDC button should be offered.
    ///
    /// Both halves are required: a configured provider that the mode does not
    /// select would send the user into a flow this router will not complete.
    pub fn oidc_ready(&self) -> bool {
        self.oidc_configured && matches!(self.auth_mode.as_deref(), Some("oidc" | "both"))
    }

    /// Whether the password form should be hidden entirely.
    pub fn password_hidden(&self) -> bool {
        self.auth_mode.as_deref() == Some("oidc") && self.oidc_ready()
    }

    /// The label for the OIDC button, falling back to a generic one.
    pub fn oidc_label(&self) -> &str {
        self.oidc_login_label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or("Sign in with OIDC")
    }
}

/// Whether this status permits skipping the login screen.
///
/// An existing session is the ONLY thing that does. This deliberately ignores any
/// `requireLogin` field: dashboard login is unconditional in nullrouter, so a body
/// carrying that flag is either a stale build or a spoofed one, and acting on it
/// would be an auth bypass driven by a JSON field.
pub const fn skips_login(status: &AuthStatus) -> bool {
    status.authenticated
}

/// Parse a status body. `None` when it is not an object at all.
pub fn parse_status(body: &str) -> Option<AuthStatus> {
    serde_json::from_str::<AuthStatus>(body).ok()
}

/// Resolve a post-login navigation target from a raw `?next=` value.
///
/// Only a same-origin path under `/dashboard` is honoured. Everything else —
/// another origin, a scheme-relative `//evil.example`, a path outside the
/// dashboard, a `javascript:` URL — falls back to [`DEFAULT_TARGET`]. The caller
/// passes `origin` so this is testable without a browser.
///
/// Returns a path-and-query only, never an absolute URL, so the result cannot
/// carry an origin even if a future caller forgets to check.
pub fn dashboard_target(raw_next: Option<&str>, origin: &str) -> String {
    let Some(raw) = raw_next.map(str::trim).filter(|next| !next.is_empty()) else {
        return DEFAULT_TARGET.to_owned();
    };
    // A scheme-relative reference resolves to another host while looking like a
    // path, so it is rejected before any parsing.
    if raw.starts_with("//") {
        return DEFAULT_TARGET.to_owned();
    }
    // An absolute URL is only acceptable when it names this exact origin.
    let path_and_rest = if let Some(rest) = raw.strip_prefix(origin) {
        if !rest.starts_with('/') {
            return DEFAULT_TARGET.to_owned();
        }
        rest
    } else if raw.starts_with('/') {
        raw
    } else {
        // Relative or opaque (`javascript:`, `data:`, `dashboard/x`): refused
        // rather than resolved, since resolution depends on the current path.
        return DEFAULT_TARGET.to_owned();
    };

    let path = path_and_rest
        .split(['?', '#'])
        .next()
        .unwrap_or(path_and_rest);
    // Backslashes are path separators to some URL parsers but not others, so a
    // value containing one is not worth trying to agree about.
    if path.contains('\\') || !is_dashboard_path(path) {
        return DEFAULT_TARGET.to_owned();
    }
    path_and_rest.to_owned()
}

/// Whether a path is the dashboard or inside it.
///
/// `/dashboardsomething` must not match: a prefix test alone would accept it.
fn is_dashboard_path(path: &str) -> bool {
    path == DEFAULT_TARGET || path.starts_with("/dashboard/")
}

/// What the panel is currently asking the user for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Sign in with the existing password.
    #[default]
    SignIn,
    /// A password change is required before the dashboard opens.
    ChangePassword,
}

impl Mode {
    /// The endpoint this mode posts to.
    pub const fn path(self) -> &'static str {
        match self {
            Self::SignIn => "/api/auth/login",
            Self::ChangePassword => "/api/settings",
        }
    }

    /// The HTTP method this mode uses.
    pub const fn method(self) -> &'static str {
        match self {
            Self::SignIn => "POST",
            Self::ChangePassword => "PATCH",
        }
    }

    /// The submit button's resting label.
    pub const fn submit_label(self) -> &'static str {
        match self {
            Self::SignIn => "Login",
            Self::ChangePassword => "Set password",
        }
    }

    /// The submit button's in-flight label.
    pub const fn pending_label(self) -> &'static str {
        match self {
            Self::SignIn => "Logging in...",
            Self::ChangePassword => "Saving...",
        }
    }

    /// The panel's instruction copy.
    pub const fn copy(self) -> &'static str {
        match self {
            Self::SignIn => "Enter your password to access the dashboard",
            Self::ChangePassword => "Set a new password before accessing the dashboard remotely",
        }
    }
}

/// The JSON body for a submit in this mode.
///
/// Built with `serde_json` so a password containing a quote or backslash cannot
/// break out of the payload.
pub fn submit_body(mode: Mode, password: &str, new_password: &str) -> String {
    let body = match mode {
        Mode::SignIn => serde_json::json!({ "password": password }),
        Mode::ChangePassword => serde_json::json!({
            "currentPassword": password,
            "newPassword": new_password,
        }),
    };
    serde_json::to_string(&body).unwrap_or_else(|_error| String::from("{}"))
}

/// The user-facing message for a failed sign-in.
///
/// The remaining-attempts count is bounded before it is shown: it comes from the
/// server and is rendered into the page, so an absurd value would be an odd claim
/// rather than a useful one.
pub fn login_error(status: u16, remaining_before_lock: Option<i64>) -> String {
    match status {
        401 => match remaining_before_lock {
            Some(remaining) if (0..=100).contains(&remaining) => {
                format!("Invalid password. {remaining} attempt(s) left before lockout.")
            }
            _ => String::from("Invalid password."),
        },
        429 => String::from("Too many failed attempts. Try again later."),
        403 => String::from("Password login is unavailable."),
        400 => String::from("Enter a valid password."),
        _ => String::from("Unable to sign in. Please try again."),
    }
}

/// Seconds to hold the submit button after a lockout.
///
/// Reads the `Retry-After` header first, then the body's own `retryAfter`.
/// Rounds up, floors at zero, and caps at [`MAX_RETRY_SECONDS`].
pub fn retry_after_seconds(header: Option<&str>, body_value: Option<f64>) -> u32 {
    let raw = header
        .and_then(|value| value.trim().parse::<f64>().ok())
        .or(body_value);
    let Some(seconds) = raw.filter(|seconds| seconds.is_finite() && *seconds > 0.0) else {
        return 0;
    };
    let rounded = seconds.ceil();
    if rounded >= f64::from(MAX_RETRY_SECONDS) {
        return MAX_RETRY_SECONDS;
    }
    // Bounded above by the check above and below by the `> 0.0` filter.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded to 0..MAX_RETRY_SECONDS by the guards above"
    )]
    let bounded = rounded as u32;
    bounded
}

/// The submit button's label for the current state.
pub fn button_label(mode: Mode, submitting: bool, retry_after: u32) -> String {
    if retry_after > 0 {
        return format!("Wait {retry_after}s");
    }
    if submitting {
        return mode.pending_label().to_owned();
    }
    mode.submit_label().to_owned()
}

/// Whether the submit button is disabled.
pub const fn button_disabled(submitting: bool, retry_after: u32) -> bool {
    submitting || retry_after > 0
}

/// How a submit ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Submitted {
    /// Signed in: navigate to this target.
    Navigate(String),
    /// The server requires a password change first.
    RequireChange,
    /// Refused, with a message and an optional lockout countdown.
    Refused { message: String, retry_after: u32 },
}

/// Settle a submit response.
///
/// `ok` is whether the status was 2xx, and `must_change` is the reply's
/// `mustChangePassword` flag. A 2xx that asks for a change wins over navigating:
/// sending the user to the dashboard would strand them on a password they were
/// told to replace.
pub fn settle_submit(
    ok: bool,
    status: u16,
    must_change: bool,
    remaining_before_lock: Option<i64>,
    retry_after: u32,
    target: &str,
) -> Submitted {
    if ok {
        if must_change {
            return Submitted::RequireChange;
        }
        return Submitted::Navigate(target.to_owned());
    }
    Submitted::Refused {
        message: login_error(status, remaining_before_lock),
        retry_after: if status == 429 { retry_after } else { 0 },
    }
}
