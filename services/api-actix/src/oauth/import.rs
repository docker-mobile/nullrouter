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

/// Overrides the Amazon Q host, so the verify-then-record sequence can be tested.
///
/// The same reasoning as [`GITLAB_BASE_VAR`], and the same limits: it is read from the process
/// environment rather than from a request, and the region a request names is still pattern-checked
/// before it is used — so this cannot become a way to send a caller's key somewhere the route would
/// otherwise refuse. Without it the success path could not be exercised at all, and an untested
/// verify-then-record sequence is worth less than no test.
const KIRO_Q_BASE_VAR: &str = "NULLROUTER_KIRO_Q_BASE";

/// The Amazon Q base for a region.
fn kiro_q_base(region: &str) -> String {
    std::env::var(KIRO_Q_BASE_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || format!("https://q.{region}.amazonaws.com"),
            |value| value.trim_end_matches('/').to_owned(),
        )
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
/// `POST /api/oauth/kiro/api-key`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KiroApiKeyRequest {
    api_key: Option<String>,
    /// AWS region. Checked before it reaches a hostname.
    region: Option<String>,
}

/// `POST /api/oauth/codex/import-token`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexImportRequest {
    access_token: Option<String>,
    /// Optional label; falls back to the token's email claim.
    name: Option<String>,
}

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

/// The AWS region an unqualified Kiro import defaults to.
const KIRO_DEFAULT_REGION: &str = "us-east-1";

/// How long an API-key credential is recorded as valid.
///
/// Upstream stores a year out. An API key has no refresh token and no scheduled expiry, so the value
/// exists only to keep the proactive-refresh path — which needs a refresh token — from selecting it.
const API_KEY_HORIZON_DAYS: i64 = 365;

/// `POST /api/oauth/kiro/api-key`.
///
/// A headless Kiro credential: a long-lived bearer token with no refresh token. Verified against the
/// same Amazon Q surface inference uses, then recorded — so a key that cannot list a single model is
/// rejected here rather than becoming a connection that fails on first use.
pub(super) async fn kiro_api_key(
    state: web::Data<crate::StateClient>,
    body: web::Bytes,
) -> HttpResponse {
    let request = match json_body::parse::<KiroApiKeyRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let api_key = request.api_key.as_deref().map(str::trim).unwrap_or_default();
    if api_key.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "API key is required");
    }

    let region = match request
        .region
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(supplied) => match checked_aws_region(supplied) {
            Ok(region) => region,
            Err(error) => return refuse(StatusCode::BAD_REQUEST, error),
        },
        None => KIRO_DEFAULT_REGION.to_owned(),
    };

    let client = match reqwest::Client::builder().timeout(VERIFY_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };

    // The region is already pattern-checked, which is what makes interpolating it into the host safe.
    let endpoint = format!(
        "{}/ListAvailableModels?origin=AI_EDITOR",
        kiro_q_base(&region)
    );
    let response = client
        .get(&endpoint)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
        // Amazon Q distinguishes an API key from a bearer access token by this header alone; without
        // it the key is read as an SSO token and rejected.
        .header("TokenType", "API_KEY")
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::USER_AGENT,
            "AWS-SDK-JS/3.0.0 kiro-ide/1.0.0",
        )
        .header("X-Amz-User-Agent", "aws-sdk-js/3.0.0 kiro-ide/1.0.0")
        .send()
        .await;

    let response = match response {
        Ok(response) => response,
        Err(error) => {
            return refuse(
                StatusCode::BAD_GATEWAY,
                format!("Could not reach Amazon Q in {region}: {error}"),
            );
        }
    };
    if !response.status().is_success() {
        // Upstream returns a fixed message and drops the body, calling it SSRF hardening. The
        // reasoning is sound in the other direction too: this body is an AWS error document that can
        // quote the credential back. The status is kept, since 403 and 503 mean different things to
        // whoever has to act on it.
        let status = response.status().as_u16();
        return refuse(
            StatusCode::UNAUTHORIZED,
            format!("API key validation failed (Amazon Q answered {status})"),
        );
    }

    // "Reachable" is not "usable": a key scoped to nothing answers 200 with an empty list, and
    // recording that would produce a connection that fails on its first real request.
    let listed: Value = response.json().await.unwrap_or(Value::Null);
    let models = listed
        .get("models")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if models == 0 {
        return refuse(
            StatusCode::UNAUTHORIZED,
            "API key validation failed: the key is accepted but has no available models",
        );
    }

    let email = email_from_jwt(api_key).unwrap_or_default();
    let expires_at = expires_in_days(API_KEY_HORIZON_DAYS);

    let created = state
        .create_provider_connection(&serde_json::json!({
            "provider": "kiro",
            "authType": "api_key",
            "accessToken": api_key,
            "refreshToken": Value::Null,
            "expiresAt": expires_at,
            "email": if email.is_empty() { Value::Null } else { Value::String(email.clone()) },
            "name": if email.is_empty() { "kiro".to_owned() } else { email },
            "testStatus": "active",
            "providerSpecificData": {
                "region": region,
                // Both keys, as upstream writes them: `authMethod` is what the refresh path reads to
                // skip a credential with no refresh token, and `provider` is the label the panel
                // shows for how this connection was authenticated.
                "authMethod": "api_key",
                "provider": "API Key",
            },
        }))
        .await;

    created.map_or_else(
        || {
            refuse(
                StatusCode::BAD_GATEWAY,
                "The API key was verified, but the connection could not be recorded because \
                 nullrouter-state did not answer.",
            )
        },
        |_connection| responses::json(StatusCode::OK, &serde_json::json!({ "success": true })),
    )
}

/// `POST /api/oauth/codex/import-token`.
///
/// A `ChatGPT` access token, created by the user on `chatgpt.com`. Entirely local: there is nothing
/// to verify it against: the credential is issued for the `ChatGPT` surface rather than for an API
/// endpoint that would accept it in a probe. So the route decodes what the token says about itself and
/// records it, which is what upstream does too.
///
/// The claims are read for display and for the plan label the panel shows. Nothing is verified — no
/// signature is checked — so none of it is treated as an identity, only as a label.
pub(super) async fn codex_import_token(
    state: web::Data<crate::StateClient>,
    body: web::Bytes,
) -> HttpResponse {
    let request = match json_body::parse::<CodexImportRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let token = request
        .access_token
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if token.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "Access token is required");
    }

    let claims = CodexClaims::from_token(token);
    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| claims.email.clone())
        .unwrap_or_else(|| "ChatGPT Access Token".to_owned());

    let mut specific = serde_json::Map::new();
    specific.insert(
        "authMethod".to_owned(),
        Value::String("access_token".to_owned()),
    );
    if let Some(account) = &claims.chatgpt_account_id {
        specific.insert(
            "chatgptAccountId".to_owned(),
            Value::String(account.clone()),
        );
    }
    if let Some(plan) = &claims.chatgpt_plan_type {
        specific.insert("chatgptPlanType".to_owned(), Value::String(plan.clone()));
    }
    if let Some(exp) = claims.expires_at {
        specific.insert("jwtExp".to_owned(), Value::from(exp));
    }

    let created = state
        .create_provider_connection(&serde_json::json!({
            "provider": "codex",
            "authType": "access_token",
            "accessToken": token,
            "name": name,
            "email": claims.email.clone().map_or(Value::Null, Value::String),
            "testStatus": "active",
            "providerSpecificData": Value::Object(specific),
        }))
        .await;

    created.map_or_else(
        || {
            refuse(
                StatusCode::BAD_GATEWAY,
                "The token could not be recorded because nullrouter-state did not answer.",
            )
        },
        |_connection| {
            // The token is not echoed back, as everywhere else here. The labels are, because the
            // panel shows which account and plan were imported.
            responses::json(
                StatusCode::OK,
                &serde_json::json!({
                    "success": true,
                    "connection": {
                        "provider": "codex",
                        "email": claims.email,
                        "name": name,
                        "workspace": claims.chatgpt_account_id,
                        "plan": claims.chatgpt_plan_type,
                    },
                }),
            )
        },
    )
}

/// What a `ChatGPT` access token says about itself, by its own claims.
#[derive(Debug, Default)]
struct CodexClaims {
    email: Option<String>,
    chatgpt_account_id: Option<String>,
    chatgpt_plan_type: Option<String>,
    expires_at: Option<i64>,
}

impl CodexClaims {
    /// Read the claims, or an empty set when the token is not a JWT.
    ///
    /// A non-JWT is imported anyway, exactly as upstream does: the credential may still work, and
    /// refusing it because it carries no readable metadata would reject a valid token over a label.
    fn from_token(token: &str) -> Self {
        /// OpenAI namespaces its claims, so neither is a bare key.
        const AUTH_CLAIM: &str = "https://api.openai.com/auth";
        /// The profile namespace, which is where the email usually is.
        const PROFILE_CLAIM: &str = "https://api.openai.com/profile";

        let Some(payload) = jwt_payload(token) else {
            return Self::default();
        };
        let auth = payload.get(AUTH_CLAIM);
        let profile = payload.get(PROFILE_CLAIM);

        let text = |value: Option<&Value>, key: &str| {
            value
                .and_then(|value| value.get(key))
                .and_then(Value::as_str)
                .filter(|found| !found.is_empty())
                .map(str::to_owned)
        };
        let top = |key: &str| {
            payload
                .get(key)
                .and_then(Value::as_str)
                .filter(|found| !found.is_empty())
                .map(str::to_owned)
        };

        Self {
            // The order upstream uses: profile, then the top-level claim, then the OIDC standard
            // name. A token from the web UI carries the first; one from a device flow carries a
            // different one, and the panel needs a label either way.
            email: text(profile, "email")
                .or_else(|| top("email"))
                .or_else(|| top("preferred_username")),
            chatgpt_account_id: text(auth, "chatgpt_account_id")
                .or_else(|| top("account_id")),
            chatgpt_plan_type: text(auth, "chatgpt_plan_type").or_else(|| top("plan_type")),
            expires_at: payload.get("exp").and_then(Value::as_i64),
        }
    }
}

/// The decoded payload of a JWT, without verifying anything.
///
/// The signature is neither checked nor checkable here — the issuer's key is not held — so every value
/// read from this is a label and never an authorisation.
fn jwt_payload(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    // Exactly three segments: two would be unsigned and four is not a JWT at all.
    let _signature = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    serde_json::from_slice(&base64url_decode(payload)?).ok()
}

/// A region that is safe to interpolate into a hostname.
///
/// Upstream's `AWS_REGION_PATTERN`, `^[a-z]{2}-[a-z]+-\d{1,2}$`, and it is doing real work: the region
/// becomes the first label of `q.<region>.amazonaws.com`, so an unchecked value is a way to choose
/// which host receives the caller's bearer token — including one the caller controls.
fn checked_aws_region(supplied: &str) -> Result<String, String> {
    let mut parts = supplied.split('-');
    let area = parts.next().unwrap_or_default();
    let direction = parts.next().unwrap_or_default();
    let index = parts.next().unwrap_or_default();
    let shaped = parts.next().is_none()
        && area.len() == 2
        && area.chars().all(|character| character.is_ascii_lowercase())
        && !direction.is_empty()
        && direction
            .chars()
            .all(|character| character.is_ascii_lowercase())
        && (1..=2).contains(&index.len())
        && index.chars().all(|character| character.is_ascii_digit());

    if shaped {
        Ok(supplied.to_owned())
    } else {
        Err("Invalid region".to_owned())
    }
}

/// The `email` claim of a JWT, when the credential happens to be one.
///
/// Display only, and entirely optional: a Kiro API key is often opaque. Nothing is verified here —
/// the signature is not checked and must not be trusted for anything — so the value is used as a
/// label and never as an identity.
fn email_from_jwt(token: &str) -> Option<String> {
    let email = jwt_payload(token)?
        .get("email")
        .and_then(Value::as_str)?
        .to_owned();
    (!email.is_empty()).then_some(email)
}

/// Decode unpadded base64url, which is what a JWT segment is.
fn base64url_decode(segment: &str) -> Option<Vec<u8>> {
    /// The base64url alphabet, in value order.
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    let mut out = Vec::with_capacity(segment.len() * 3 / 4);
    let mut accumulator = 0_u32;
    let mut bits = 0_u32;
    for byte in segment.bytes() {
        let value = ALPHABET.iter().position(|candidate| *candidate == byte)?;
        accumulator = (accumulator << 6) | u32::try_from(value).ok()?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            let shifted = accumulator >> bits;
            out.push(u8::try_from(shifted & 0xFF).ok()?);
        }
    }
    Some(out)
}

/// An RFC 3339 timestamp `days` from now.
///
/// Computed from the Unix epoch rather than through a date library, because this is the only place in
/// the service that needs a future timestamp and the arithmetic is exact.
fn expires_in_days(days: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0_i64, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        });
    let at = now.saturating_add(days.saturating_mul(24 * 60 * 60));
    format_rfc3339(at)
}

/// Render a Unix timestamp as `YYYY-MM-DDTHH:MM:SSZ`.
fn format_rfc3339(seconds: i64) -> String {
    /// Days in each month of a non-leap year.
    const MONTH_LENGTHS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    let days = seconds.div_euclid(86_400);
    let time_of_day = seconds.rem_euclid(86_400);
    let (hour, minute, second) = (
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60,
    );

    let mut year = 1970_i64;
    let mut remaining = days;
    loop {
        let length = if is_leap_year(year) { 366 } else { 365 };
        if remaining < length {
            break;
        }
        remaining -= length;
        year += 1;
    }

    let mut month = 1_i64;
    for (index, base) in MONTH_LENGTHS.iter().enumerate() {
        let length = if index == 1 && is_leap_year(year) {
            base + 1
        } else {
            *base
        };
        if remaining < length {
            break;
        }
        remaining -= length;
        month += 1;
    }
    let day = remaining + 1;

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Whether a year has 366 days, by the Gregorian rule.
const fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
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
