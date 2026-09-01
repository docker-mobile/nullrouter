//! Importing a credential the user already holds.
//!
//! Most of what sits under `/api/oauth/` is not an authorisation flow. A Personal Access Token, an
//! API key or a session cookie is something the user obtained themselves and pastes in; the route's
//! job is to check it works and record a connection. No browser, no consent screen, and no client
//! credentials of this router's own.
//!
//! That distinction is why these are implemented while the rest of the family is not. The two
//! families that genuinely need a provider's consent screen — `kiro/social-*` and the generic
//! `{provider}/{action}` PKCE flows — still answer 501, and say so.
//!
//! # Verifying before recording
//!
//! Each import calls the provider once with the credential and stores nothing if that call fails.
//! Upstream does the same, and it is the difference between a connection that works and one that
//! looks configured: a mistyped token recorded without a check produces a provider that fails on
//! first real use, at which point the cause is several steps away.
//!
//! # What is not echoed back
//!
//! The credential is sent to the provider and stored for later use, but never returned in a
//! response. A dashboard that displayed it would put a long-lived token into a browser history and a
//! screenshot.

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Deserialize;
use serde_json::Value;

use crate::{json_body, responses};

/// GitLab's default host, when the caller does not name a self-managed one.
const GITLAB_DEFAULT_BASE: &str = "https://gitlab.com";

/// Overrides [`GITLAB_DEFAULT_BASE`], so the verify-then-record sequence can be tested.
///
/// Read from the process environment, which only whoever starts the service controls. It replaces
/// the *default* only: a base named in a request still goes through [`verified_base`], so this is
/// not a way to make the route send a caller's token somewhere it would otherwise refuse.
const GITLAB_BASE_VAR: &str = "NULLROUTER_GITLAB_BASE";

fn gitlab_default_base() -> String {
    std::env::var(GITLAB_BASE_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim_end_matches('/').to_owned())
        .unwrap_or_else(|| GITLAB_DEFAULT_BASE.to_owned())
}

/// One provider call must not hang the route.
const VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitlabPatRequest {
    token: Option<String>,
    /// For a self-managed GitLab. Absent means gitlab.com.
    base_url: Option<String>,
}

/// `POST /api/oauth/gitlab/pat` — verify a Personal Access Token and record the connection.
///
/// The token goes in `Private-Token`, which is GitLab's own header for a PAT; a bearer token there
/// is rejected, so this is not interchangeable with the OAuth header.
pub(super) async fn gitlab_pat(
    state: web::Data<crate::StateClient>,
    body: web::Bytes,
) -> HttpResponse {
    let request = match json_body::parse::<GitlabPatRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let token = request.token.as_deref().map(str::trim).unwrap_or_default();
    if token.is_empty() {
        return refuse(
            StatusCode::BAD_REQUEST,
            "Personal Access Token is required",
        );
    }

    let base = match request
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(supplied) => match verified_base(supplied) {
            Ok(base) => base,
            Err(error) => return refuse(StatusCode::BAD_REQUEST, error),
        },
        None => gitlab_default_base(),
    };

    let client = match reqwest::Client::builder().timeout(VERIFY_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };

    let response = client
        .get(format!("{base}/api/v4/user"))
        .header("Private-Token", token)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            return refuse(
                StatusCode::BAD_GATEWAY,
                format!("Could not reach GitLab at {base}: {error}"),
            );
        }
    };
    if !response.status().is_success() {
        // 401 regardless of what GitLab said, matching upstream: every failure here means the token
        // was not accepted, and the body is GitLab's HTML error page as often as not.
        let detail = response.text().await.unwrap_or_default();
        let detail = detail.chars().take(200).collect::<String>();
        return refuse(
            StatusCode::UNAUTHORIZED,
            format!("GitLab token verification failed: {detail}"),
        );
    }

    let user: Value = response.json().await.unwrap_or(Value::Null);
    let text = |key: &str| {
        user.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    // `email` is only present when the token carries the `read_user` scope; `public_email` is the
    // fallback and is often empty too, so a blank one is stored rather than treated as a failure.
    let email = match text("email") {
        found if !found.is_empty() => found,
        _ => text("public_email"),
    };
    let display = [text("name"), text("username"), email.clone()]
        .into_iter()
        .find(|candidate| !candidate.is_empty())
        .unwrap_or_else(|| "gitlab".to_owned());

    let created = state
        .create_provider_connection(&serde_json::json!({
            "provider": "gitlab",
            "apiKey": token,
            "name": display,
            "testStatus": "active",
            "providerSpecificData": {
                "username": text("username"),
                "email": email,
                "name": text("name"),
                "baseUrl": base,
                // Records how this connection was authenticated, which is what tells the refresh
                // path to leave it alone: a PAT has no refresh token and never expires on a
                // schedule, so trying to refresh one would fail on every request.
                "authKind": "personal_access_token",
            },
        }))
        .await;

    match created {
        Some(_connection) => {
            // The token is deliberately not in this response. It is stored, and it went to GitLab;
            // returning it would put a long-lived credential into a browser history.
            responses::json(StatusCode::OK, &serde_json::json!({ "success": true }))
        }
        None => refuse(
            StatusCode::BAD_GATEWAY,
            "The token was verified, but the connection could not be recorded because \
             nullrouter-state did not answer.",
        ),
    }
}

/// A base URL that is safe to send a credential to.
///
/// The caller names the host — a self-managed GitLab is the point — so it is checked rather than
/// trusted. `https` only, because a PAT sent over plaintext is disclosed to the network, and no
/// loopback or private address: this service can reach the internal services and the host's networks
/// and the caller cannot, so an unchecked base would make this route a way to post a header of the
/// caller's choosing to them.
fn verified_base(supplied: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(supplied)
        .map_err(|error| format!("GitLab base URL is not valid: {error}"))?;
    if parsed.scheme() != "https" {
        return Err(format!(
            "GitLab base URL must use https, not {:?} — a Personal Access Token sent in clear is \
             disclosed to anything on the path.",
            parsed.scheme()
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "GitLab base URL has no host".to_owned())?;
    if crate::proxy_test::is_local_target(host) {
        return Err(format!(
            "Refusing to send a token to {host}: it is a loopback or private address, which this \
             service can reach and the caller cannot."
        ));
    }
    Ok(supplied.trim_end_matches('/').to_owned())
}

fn refuse(status: StatusCode, error: impl Into<String>) -> HttpResponse {
    responses::json(
        status,
        &serde_json::json!({ "success": false, "error": error.into() }),
    )
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test assertions read clearer with expect than with error plumbing"
)]
mod tests {
    use super::verified_base;

    #[test]
    fn a_base_that_would_disclose_or_pivot_is_refused() {
        // The caller names this host and a credential is sent to it, so each of these is a way to
        // either leak the token or reach something only this service can.
        for base in [
            "http://gitlab.example.com",
            "http://127.0.0.1:20134",
            "https://127.0.0.1/gitlab",
            "https://localhost/gitlab",
            "https://10.0.0.5",
            "https://[::1]",
            "not a url",
            "ftp://gitlab.example.com",
        ] {
            assert!(verified_base(base).is_err(), "{base:?} should be refused");
        }
    }

    #[test]
    fn a_self_managed_https_host_is_accepted_with_its_trailing_slash_trimmed() {
        assert_eq!(
            verified_base("https://gitlab.example.com/").as_deref(),
            Ok("https://gitlab.example.com")
        );
        assert_eq!(
            verified_base("https://gitlab.example.com").as_deref(),
            Ok("https://gitlab.example.com")
        );
    }
}
