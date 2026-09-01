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

/// Overrides iFlow's platform host, so the two-call sequence can be tested.
///
/// Same reasoning and same limits as [`GITLAB_BASE_VAR`]. This one matters more than most: the second
/// call rotates a key on the user's real iFlow account, so a test that could only run against the live
/// host would either not exist or would have side effects on someone's account.
const IFLOW_BASE_VAR: &str = "NULLROUTER_IFLOW_BASE";

/// iFlow's platform host. Fixed — no part of a request chooses it.
const IFLOW_DEFAULT_BASE: &str = "https://platform.iflow.cn";

fn iflow_base() -> String {
    std::env::var(IFLOW_BASE_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || IFLOW_DEFAULT_BASE.to_owned(),
            |value| value.trim_end_matches('/').to_owned(),
        )
}

/// Overrides Kiro's social-token refresh host for tests; requests cannot choose it.
const KIRO_SOCIAL_BASE_VAR: &str = "NULLROUTER_KIRO_SOCIAL_BASE";
const KIRO_SOCIAL_BASE: &str = "https://prod.us-east-1.auth.desktop.kiro.dev";
const KIRO_OIDC_BASE_VAR: &str = "NULLROUTER_KIRO_OIDC_BASE";
const KIRO_OIDC_BASE: &str = "https://oidc";

fn kiro_social_base() -> String {
    std::env::var(KIRO_SOCIAL_BASE_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || KIRO_SOCIAL_BASE.to_owned(),
            |value| value.trim_end_matches('/').to_owned(),
        )
}

/// AWS SSO-OIDC's base. The production host is `https://oidc.<region>.amazonaws.com`; a process-level
/// override lets the protocol be tested against a loopback stub without allowing a request to pick its
/// own credential destination.
fn kiro_oidc_base(region: &str) -> String {
    std::env::var(KIRO_OIDC_BASE_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || format!("{KIRO_OIDC_BASE}.{region}.amazonaws.com"),
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

/// `POST /api/oauth/kiro/import`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KiroImportRequest {
    refresh_token: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    region: Option<String>,
    profile_arn: Option<String>,
}

/// `POST /api/oauth/iflow/cookie`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IflowCookieRequest {
    /// A whole cookie header as pasted from a browser. Narrowed to the session field before use.
    cookie: Option<String>,
}

/// `POST /api/oauth/cursor/import`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorImportRequest {
    access_token: Option<String>,
    /// Cursor's `storage.serviceMachineId`. Sent with every later request, so it is not optional.
    machine_id: Option<String>,
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
        return refuse(StatusCode::BAD_REQUEST, "Personal Access Token is required");
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
    let api_key = request
        .api_key
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
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

/// Shortest a Cursor access token may plausibly be.
///
/// Upstream's check. It is a shape test rather than a validation, and it is honest about that: Cursor's
/// API speaks protobuf with no simple probe endpoint, so nothing here can tell a real token from a
/// well-shaped string. The credential is validated the first time it is used.
const CURSOR_MIN_TOKEN_LEN: usize = 50;

/// How long a Cursor token is assumed to last.
///
/// Upstream's 24 hours. Cursor publishes no refresh endpoint, so this is an expiry the panel can show
/// rather than a schedule anything acts on.
const CURSOR_EXPIRES_IN: i64 = 86_400;

/// Where Cursor keeps its token, per platform. Shown to the user, never read by this service.
const CURSOR_TOKEN_PATHS: [(&str, &str); 3] = [
    ("linux", "~/.config/Cursor/User/globalStorage/state.vscdb"),
    (
        "macos",
        "/Users/<user>/Library/Application Support/Cursor/User/globalStorage/state.vscdb",
    ),
    (
        "windows",
        "%APPDATA%\\Cursor\\User\\globalStorage\\state.vscdb",
    ),
];

/// `GET /api/oauth/cursor/import`.
///
/// The instructions for finding the token, which the panel renders as a form. Static: this service does
/// not read the user's Cursor database, it tells them which two values to copy out of it.
pub(super) async fn cursor_import_instructions() -> HttpResponse {
    let paths: serde_json::Map<String, Value> = CURSOR_TOKEN_PATHS
        .iter()
        .map(|(platform, path)| ((*platform).to_owned(), Value::String((*path).to_owned())))
        .collect();

    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "provider": "cursor",
            "method": "import_token",
            "instructions": {
                "title": "How to get your Cursor token",
                "steps": [
                    "1. Open Cursor IDE and make sure you're logged in",
                    "2. Find the state.vscdb file:",
                    format!("   - Linux: {}", CURSOR_TOKEN_PATHS[0].1),
                    format!("   - macOS: {}", CURSOR_TOKEN_PATHS[1].1),
                    format!("   - Windows: {}", CURSOR_TOKEN_PATHS[2].1),
                    "3. Open the database with SQLite browser or CLI:",
                    "   sqlite3 state.vscdb \"SELECT value FROM itemTable WHERE key='cursorAuth/accessToken'\"",
                    "4. Also get the machine ID:",
                    "   sqlite3 state.vscdb \"SELECT value FROM itemTable WHERE key='storage.serviceMachineId'\"",
                    "5. Paste both values in the form below",
                ],
                "alternativeMethod": [
                    "Or use this one-liner to get both values:",
                    "sqlite3 state.vscdb \"SELECT key, value FROM itemTable WHERE key IN ('cursorAuth/accessToken', 'storage.serviceMachineId')\"",
                ],
                "paths": paths,
            },
            "requiredFields": [
                {
                    "name": "accessToken",
                    "label": "Access Token",
                    "description": "From cursorAuth/accessToken in state.vscdb",
                    "type": "textarea",
                },
                {
                    "name": "machineId",
                    "label": "Machine ID",
                    "description": "From storage.serviceMachineId in state.vscdb",
                    "type": "text",
                },
            ],
        }),
    )
}

/// `POST /api/oauth/cursor/import`.
///
/// Records a token copied out of Cursor's local database. Local by necessity rather than by choice:
/// Cursor's API speaks protobuf and publishes no endpoint that would accept this token in a probe, so
/// the route checks the two values are the right *shape* and says so plainly. Upstream's own comment
/// says the same — "we don't validate against API because Cursor uses complex protobuf".
pub(super) async fn cursor_import(
    state: web::Data<crate::StateClient>,
    body: web::Bytes,
) -> HttpResponse {
    let request = match json_body::parse::<CursorImportRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let access_token = request
        .access_token
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    let machine_id = request
        .machine_id
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();

    if access_token.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "Access token is required");
    }
    if machine_id.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "Machine ID is required");
    }
    if access_token.len() < CURSOR_MIN_TOKEN_LEN {
        return refuse(
            StatusCode::BAD_REQUEST,
            "Invalid token format. Token appears too short.",
        );
    }
    if !is_machine_id(machine_id) {
        return refuse(
            StatusCode::BAD_REQUEST,
            "Invalid machine ID format. Expected UUID format.",
        );
    }

    // The token may be a JWT, in which case it names the account. Labels only — nothing is verified.
    let claims = jwt_payload(access_token);
    let claim = |key: &str| {
        claims
            .as_ref()
            .and_then(|payload| payload.get(key))
            .and_then(Value::as_str)
            .filter(|found| !found.is_empty())
            .map(str::to_owned)
    };
    let email = claim("email").or_else(|| claim("sub"));
    let user_id = claim("sub").or_else(|| claim("user_id"));

    let created = state
        .create_provider_connection(&serde_json::json!({
            "provider": "cursor",
            "authType": "oauth",
            "accessToken": access_token,
            // Cursor publishes no refresh endpoint, so there is nothing to store and nothing for the
            // refresh path to attempt. `crates/execute`'s refresh lists cursor as unsupported for the
            // same reason.
            "refreshToken": Value::Null,
            "expiresAt": expires_in_seconds(CURSOR_EXPIRES_IN),
            "email": email.clone().map_or(Value::Null, Value::String),
            "name": email.clone().unwrap_or_else(|| "cursor".to_owned()),
            "testStatus": "active",
            "providerSpecificData": {
                "machineId": machine_id,
                "authMethod": "imported",
                "provider": "Imported",
                "userId": user_id.map_or(Value::Null, Value::String),
            },
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
            responses::json(
                StatusCode::OK,
                &serde_json::json!({
                    "success": true,
                    "connection": { "provider": "cursor", "email": email },
                }),
            )
        },
    )
}

/// `POST /api/oauth/kiro/import`.
///
/// Records a refresh token taken out of a Kiro IDE login. Unlike the other imports here, the check is
/// not a probe: it *is* a refresh. Kiro has no endpoint that would accept a refresh token in a read-only
/// call, so the only way to know a token works is to spend it — which also means the connection is
/// recorded with a live access token rather than one that has to be minted on first use.
///
/// Two protocols, and which one runs is decided by the credentials present, never by a label in the
/// request. A `clientId` and `clientSecret` pair means AWS SSO-OIDC (a Builder ID or an organisation's
/// IDC), posted as camel-cased JSON to the regional host. Without them it is a social login, posted to
/// Kiro's own auth service. Sending one protocol's token to the other endpoint would burn it: a refused
/// refresh is not always reversible, and a refresh token is the whole credential.
///
/// This is also the protocol `crates/execute`'s generic refresh deliberately excludes for `kiro`, which
/// is why importing here does not imply anything later can renew it.
pub(super) async fn kiro_import(
    state: web::Data<crate::StateClient>,
    body: web::Bytes,
) -> HttpResponse {
    let request = match json_body::parse::<KiroImportRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let refresh_token = request
        .refresh_token
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if refresh_token.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "Refresh token is required");
    }

    let client_id = request
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let client_secret = request
        .client_secret
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    // Both or neither. One alone cannot authenticate an SSO-OIDC refresh, and treating a half pair as a
    // social login would send an organisation's token to the wrong service.
    let idc = match (client_id, client_secret) {
        (Some(id), Some(secret)) => Some((id, secret)),
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => {
            return refuse(
                StatusCode::BAD_REQUEST,
                "An IDC import needs both clientId and clientSecret. With only one of them there is \
                 no way to tell which refresh protocol this token belongs to.",
            );
        }
    };

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

    let refreshed = match idc {
        Some((id, secret)) => kiro_idc_refresh(&client, refresh_token, id, secret, &region).await,
        None => kiro_social_refresh(&client, refresh_token).await,
    };
    let refreshed = match refreshed {
        Ok(refreshed) => refreshed,
        Err(error) => return refuse(StatusCode::BAD_GATEWAY, error),
    };

    let email = email_from_jwt(&refreshed.access_token);
    let profile_arn = request
        .profile_arn
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or(refreshed.profile_arn);

    let mut specific = serde_json::json!({
        "profileArn": profile_arn.clone().map_or(Value::Null, Value::String),
        "authMethod": if idc.is_some() { "idc" } else { "imported" },
        "provider": if idc.is_some() { "Enterprise" } else { "Imported" },
    });
    if let Some((id, secret)) = idc
        && let Some(object) = specific.as_object_mut()
    {
        // Stored because the next refresh needs them: an IDC token cannot be renewed without its
        // client credentials, and a connection that cannot be renewed stops working within the hour.
        object.insert("clientId".to_owned(), Value::String(id.to_owned()));
        object.insert("clientSecret".to_owned(), Value::String(secret.to_owned()));
        object.insert("region".to_owned(), Value::String(region.clone()));
    }

    let created = state
        .create_provider_connection(&serde_json::json!({
            "provider": "kiro",
            "authType": "oauth",
            "accessToken": refreshed.access_token,
            // The refresh may or may not rotate the token. Keeping the one that came back when it does,
            // and the submitted one when it does not, is what makes a second refresh possible.
            "refreshToken": refreshed.refresh_token,
            "expiresAt": expires_in_seconds(refreshed.expires_in),
            "email": email.clone().map_or(Value::Null, Value::String),
            "name": email.clone().unwrap_or_else(|| "kiro".to_owned()),
            "testStatus": "active",
            "providerSpecificData": specific,
        }))
        .await;

    created.map_or_else(
        || {
            refuse(
                StatusCode::BAD_GATEWAY,
                "The token was refreshed but could not be recorded, because nullrouter-state did \
                 not answer. The submitted refresh token may already have been rotated by Kiro.",
            )
        },
        |_connection| {
            responses::json(
                StatusCode::OK,
                &serde_json::json!({
                    "success": true,
                    "connection": { "provider": "kiro", "email": email },
                }),
            )
        },
    )
}

/// What a Kiro refresh returned.
#[derive(Debug)]
struct KiroRefreshed {
    access_token: String,
    refresh_token: String,
    profile_arn: Option<String>,
    expires_in: i64,
}

/// AWS SSO-OIDC refresh: camel-cased JSON to the regional host.
///
/// Not a form-encoded OAuth grant. This endpoint reads `grantType`, not `grant_type`, and answers a
/// JSON document with `accessToken` — which is exactly why `crates/execute`'s generic refresh excludes
/// `kiro` rather than trying.
async fn kiro_idc_refresh(
    client: &reqwest::Client,
    refresh_token: &str,
    client_id: &str,
    client_secret: &str,
    region: &str,
) -> Result<KiroRefreshed, String> {
    // The region is pattern-checked before it gets here, which is what makes interpolating it into a
    // hostname safe.
    let endpoint = format!("{}/token", kiro_oidc_base(region));
    let sent = client
        .post(&endpoint)
        .json(&serde_json::json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "refreshToken": refresh_token,
            "grantType": "refresh_token",
        }))
        .send()
        .await;
    kiro_refresh_document(sent, refresh_token, &format!("AWS SSO-OIDC in {region}")).await
}

/// Social-login refresh: Kiro's own auth service, which holds the Google and GitHub sessions.
async fn kiro_social_refresh(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<KiroRefreshed, String> {
    let endpoint = format!("{}/refreshToken", kiro_social_base());
    let sent = client
        .post(&endpoint)
        .json(&serde_json::json!({ "refreshToken": refresh_token }))
        .send()
        .await;
    kiro_refresh_document(sent, refresh_token, "Kiro's auth service").await
}

/// Read a refresh answer, or turn it into a refusal.
///
/// The body is never reflected. Upstream returns `error: await response.text()` from both endpoints,
/// and an AWS OIDC error document quotes the request back — including, on some failures, the client
/// secret that was in it.
async fn kiro_refresh_document(
    result: Result<reqwest::Response, reqwest::Error>,
    submitted: &str,
    what: &str,
) -> Result<KiroRefreshed, String> {
    let response = result.map_err(|error| format!("Could not reach {what}: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "{what} refused the refresh with HTTP {status}. The token is most likely expired or was \
             issued for the other Kiro login method; sign in to Kiro IDE again and re-import."
        ));
    }
    let document: Value = response
        .json()
        .await
        .map_err(|error| format!("{what} did not answer with JSON: {error}"))?;

    let access_token = document
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{what} returned no access token, so there is nothing to record."))?
        .to_owned();

    Ok(KiroRefreshed {
        access_token,
        refresh_token: document
            .get("refreshToken")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(submitted)
            .to_owned(),
        profile_arn: document
            .get("profileArn")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        expires_in: document
            .get("expiresIn")
            .and_then(Value::as_i64)
            .filter(|seconds| *seconds > 0)
            .unwrap_or(DEFAULT_EXPIRES_IN),
    })
}

/// The cookie field iFlow's session lives in.
const IFLOW_SESSION_FIELD: &str = "BXAuth=";

/// iFlow's API-key endpoint, read then written.
const IFLOW_KEY_PATH: &str = "/api/openapi/apikey";

/// Longest cookie accepted. A session cookie is a few hundred bytes; this is generous.
const IFLOW_MAX_COOKIE: usize = 8 * 1024;

/// How much of the returned key is shown back to the caller.
const IFLOW_KEY_PREVIEW: usize = 10;

/// A browser `User-Agent`, which iFlow's platform API expects.
///
/// Not evasion — the credential is the user's own session cookie and the request is the one their
/// browser would make. The platform endpoint simply refuses a request that does not look like one.
const IFLOW_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                                (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36";

/// `POST /api/oauth/iflow/cookie`.
///
/// iFlow issues no OAuth credential a panel can hold, so upstream takes the session cookie out of the
/// user's browser and uses it to mint an API key. Two calls: read the current key to learn its name,
/// then post that name back, which **rotates the key on the user's account**. That is a mutation of
/// someone's real account, not a read, and it is the reason this route asks for a cookie at all.
///
/// Only the session field is kept. Everything else the user pasted — analytics ids, other sites'
/// cookies, whatever the clipboard carried — is dropped before anything is stored.
pub(super) async fn iflow_cookie(
    state: web::Data<crate::StateClient>,
    body: web::Bytes,
) -> HttpResponse {
    let request = match json_body::parse::<IflowCookieRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let session = match checked_session(request.cookie.as_deref().unwrap_or_default()) {
        Ok(session) => session,
        Err(error) => return refuse(StatusCode::BAD_REQUEST, error),
    };

    let client = match reqwest::Client::builder().timeout(VERIFY_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let base = iflow_base();
    let endpoint = format!("{base}{IFLOW_KEY_PATH}");

    // Step one: read the current key, for its name. Nothing is stored from this call.
    let read = client
        .get(&endpoint)
        .header(reqwest::header::COOKIE, &session)
        .header(reqwest::header::ACCEPT, "application/json, text/plain, */*")
        .header(reqwest::header::USER_AGENT, IFLOW_USER_AGENT)
        .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .send()
        .await;
    let current = match iflow_document(read, "read the iFlow API key").await {
        Ok(document) => document,
        Err(response) => return response,
    };
    let Some(name) = current
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
    else {
        return refuse(StatusCode::BAD_REQUEST, "Missing name in API key info");
    };

    // Step two: post the name back, which rotates the key. This is the mutation.
    let rotated = client
        .post(&endpoint)
        .header(reqwest::header::COOKIE, &session)
        .header(reqwest::header::ACCEPT, "application/json, text/plain, */*")
        .header(reqwest::header::USER_AGENT, IFLOW_USER_AGENT)
        .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .header(reqwest::header::ORIGIN, base.as_str())
        .header(reqwest::header::REFERER, format!("{base}/"))
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await;
    let issued = match iflow_document(rotated, "refresh the iFlow API key").await {
        Ok(document) => document,
        Err(response) => return response,
    };
    let Some(api_key) = issued
        .get("apiKey")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return refuse(StatusCode::BAD_GATEWAY, "Missing API key in response");
    };
    let label = issued
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&name)
        .to_owned();
    let expire_time = issued.get("expireTime").cloned().unwrap_or(Value::Null);

    let created = state
        .create_provider_connection(&serde_json::json!({
            "provider": "iflow",
            "authType": "cookie",
            "name": label,
            "email": label,
            "apiKey": api_key,
            "testStatus": "active",
            "isActive": true,
            "providerSpecificData": {
                // The session field only. The key was already minted, so this is kept for the next
                // rotation rather than for the requests themselves.
                "cookie": session,
                "expireTime": expire_time.clone(),
            },
        }))
        .await;

    created.map_or_else(
        || {
            refuse(
                StatusCode::BAD_GATEWAY,
                "The key was issued but could not be recorded, because nullrouter-state did not \
                 answer. The key on the iFlow account has already been rotated.",
            )
        },
        |_connection| {
            responses::json(
                StatusCode::OK,
                &serde_json::json!({
                    "success": true,
                    "connection": {
                        "provider": "iflow",
                        "email": label,
                        // A prefix, as upstream has it: enough for the user to recognise which key was
                        // issued, not enough to use.
                        "apiKey": format!("{}...", preview(api_key, IFLOW_KEY_PREVIEW)),
                        "expireTime": expire_time,
                    },
                }),
            )
        },
    )
}

/// Reads one of iFlow's two responses, or turns it into a refusal.
///
/// iFlow answers 200 with `success: false` for a rejected cookie, so the status alone does not say
/// whether a call worked. Neither body is reflected: both can quote the session cookie back, and one is
/// an error document from a host this route reached with someone's credential attached.
async fn iflow_document(
    result: Result<reqwest::Response, reqwest::Error>,
    what: &str,
) -> Result<Value, HttpResponse> {
    let response = result.map_err(|error| {
        refuse(
            StatusCode::BAD_GATEWAY,
            format!("Could not {what}: {error}"),
        )
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(refuse(
            // iFlow's own status, so a 401 reads as a stale cookie rather than as a generic failure.
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            format!("iFlow refused the request to {what} with HTTP {status}."),
        ));
    }
    let document: Value = response.json().await.map_err(|error| {
        refuse(
            StatusCode::BAD_GATEWAY,
            format!("iFlow's answer to {what} was not JSON: {error}"),
        )
    })?;
    if document.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(refuse(
            StatusCode::BAD_GATEWAY,
            format!(
                "iFlow declined to {what}. The cookie is most likely expired; sign in again and \
                 copy a fresh one."
            ),
        ));
    }
    document
        .get("data")
        .filter(|data| data.is_object())
        .cloned()
        .ok_or_else(|| {
            refuse(
                StatusCode::BAD_GATEWAY,
                format!("iFlow's answer to {what} carried no data object."),
            )
        })
}

/// The session field of a pasted cookie, checked and on its own.
///
/// Everything a caller pasted is dropped except the one field iFlow authenticates with. Upstream sends
/// the whole string and narrows it afterwards, just before storing — which discloses the unrelated
/// cookies in a clipboard paste to iFlow on the way. Narrowing first sends what the call needs.
fn checked_session(cookie: &str) -> Result<String, String> {
    let cookie = cookie.trim();
    if cookie.is_empty() {
        return Err("Cookie is required".to_owned());
    }
    if cookie.len() > IFLOW_MAX_COOKIE {
        return Err(format!(
            "The cookie is longer than {IFLOW_MAX_COOKIE} bytes, which no session cookie is."
        ));
    }
    // A header value cannot carry a newline. reqwest would reject it, but rejecting it here names the
    // problem instead of surfacing a builder error, and keeps the check beside the value it guards: a
    // cookie is caller-supplied text on its way into a request header.
    if cookie.chars().any(char::is_control) {
        return Err(
            "The cookie contains a control character, so it is not a cookie header.".to_owned(),
        );
    }
    // A field that is present but empty authenticates nothing, so it is refused here rather than sent.
    cookie
        .split(';')
        .map(str::trim)
        .find_map(|pair| pair.strip_prefix(IFLOW_SESSION_FIELD))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("{IFLOW_SESSION_FIELD}{value};"))
        .ok_or_else(|| "Cookie must contain BXAuth field".to_owned())
}

/// The first `count` characters of a credential, for a response the user reads.
///
/// Counts characters rather than bytes: slicing a byte offset would panic on a multi-byte boundary,
/// and a key is caller-influenced text.
fn preview(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}

/// Whether a machine id has the shape Cursor writes.
///
/// Upstream strips hyphens and requires at least 32 hex characters. Reproduced rather than tightened to
/// exactly 32: Cursor has written longer values, and rejecting a real machine id would block an import
/// that would have worked.
fn is_machine_id(value: &str) -> bool {
    let hex: String = value
        .chars()
        .filter(|character| *character != '-')
        .collect();
    hex.len() >= 32 && hex.chars().all(|character| character.is_ascii_hexdigit())
}

/// Fields a bulk-imported codex account may set.
///
/// Upstream spreads `...item` into the record and strips five names — `id`, `provider`, `authType`,
/// `createdAt`, `updatedAt`. That is a blocklist, so every field it did not think of is writable: a
/// caller can set `priority` and reorder someone's provider list, or set `isActive` on a credential
/// that should not be live, or write a key the store gains meaning for in a later version. This is an
/// allowlist instead. It is a deliberate divergence, and in the narrowing direction: an import decides
/// what a credential *is*, not where it sits in a routing order.
const BULK_IMPORT_FIELDS: [&str; 8] = [
    "accessToken",
    "refreshToken",
    "idToken",
    "email",
    "name",
    "expiresAt",
    "expiresIn",
    "accountId",
];

/// The most accounts one bulk import may carry.
///
/// Upstream has no cap. Each item is a serial round trip to the state service — serial because
/// `createProviderConnection` assigns priority inside a transaction and parallel writes would race on
/// it — so an unbounded list is a request that holds a worker for as long as the list is long.
const BULK_IMPORT_MAX: usize = 200;

/// `POST /api/oauth/codex/bulk-import`.
///
/// Imports several codex accounts in one call. Each item is recorded independently and reported by
/// index, so one bad entry in a pasted export does not discard the rest — which is the whole reason
/// the route exists.
pub(super) async fn codex_bulk_import(
    state: web::Data<crate::StateClient>,
    body: web::Bytes,
) -> HttpResponse {
    let parsed = match json_body::parse::<Value>(&body) {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };

    // Three shapes, as upstream: a bare array, `{accounts: [...]}`, or a single object.
    let accounts: Vec<&Value> = match &parsed {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_fields) => match parsed.get("accounts") {
            Some(Value::Array(items)) => items.iter().collect(),
            _other => vec![&parsed],
        },
        _other => Vec::new(),
    };

    if accounts.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "No accounts provided");
    }
    if accounts.len() > BULK_IMPORT_MAX {
        return refuse(
            StatusCode::BAD_REQUEST,
            format!(
                "No more than {BULK_IMPORT_MAX} accounts may be imported at once; {} were sent",
                accounts.len()
            ),
        );
    }

    let mut results = Vec::with_capacity(accounts.len());
    let mut succeeded = 0_usize;
    let mut failed = 0_usize;

    // Serial, for the reason upstream is serial: the store assigns priority by reading the current
    // maximum inside a transaction, and concurrent writes would race on it.
    for (index, raw) in accounts.into_iter().enumerate() {
        match bulk_import_one(&state, raw).await {
            Ok(()) => {
                succeeded += 1;
                results.push(serde_json::json!({ "index": index, "ok": true }));
            }
            Err(error) => {
                failed += 1;
                results.push(serde_json::json!({ "index": index, "ok": false, "error": error }));
            }
        }
    }

    // 200 even with failures, as upstream: the per-item results are the answer, and a status that
    // said "failed" would discard the report on the items that worked.
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "success": succeeded,
            "failed": failed,
            "results": results,
        }),
    )
}

/// Record one bulk-imported account.
async fn bulk_import_one(state: &crate::StateClient, raw: &Value) -> Result<(), String> {
    let Some(fields) = raw.as_object() else {
        return Err("Item is not an object".to_owned());
    };

    let mut record = serde_json::Map::new();
    for key in BULK_IMPORT_FIELDS {
        if let Some(value) = fields.get(key)
            && !value.is_null()
        {
            record.insert(key.to_owned(), value.clone());
        }
    }

    let access_token = record
        .get("accessToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if access_token.is_empty() {
        return Err("Missing accessToken".to_owned());
    }

    // Backfill the labels from the id token if there is one, then from the access token — the order
    // upstream uses, because an id token carries the claims more reliably.
    let claims = CodexClaims::from_token(
        fields
            .get("idToken")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .unwrap_or(access_token),
    );

    let mut specific = fields
        .get("providerSpecificData")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    // Only the two identity labels are taken from a caller's `providerSpecificData`; anything else it
    // carries is dropped for the same reason the top-level allowlist exists.
    specific.retain(|key, _value| matches!(key.as_str(), "chatgptAccountId" | "chatgptPlanType"));
    if !specific.contains_key("chatgptAccountId")
        && let Some(account) = claims.chatgpt_account_id
    {
        specific.insert("chatgptAccountId".to_owned(), Value::String(account));
    }
    if !specific.contains_key("chatgptPlanType")
        && let Some(plan) = claims.chatgpt_plan_type
    {
        specific.insert("chatgptPlanType".to_owned(), Value::String(plan));
    }
    if !record.contains_key("email")
        && let Some(email) = claims.email
    {
        record.insert("email".to_owned(), Value::String(email));
    }

    // An absolute expiry is computed from a stated lifetime, since the store keeps absolutes and a
    // lifetime is only meaningful at the moment it was issued.
    if !record.contains_key("expiresAt")
        && let Some(seconds) = record.get("expiresIn").and_then(Value::as_i64)
        && seconds > 0
    {
        record.insert(
            "expiresAt".to_owned(),
            Value::String(expires_in_seconds(seconds)),
        );
    }
    record.remove("expiresIn");

    record.insert("provider".to_owned(), Value::String("codex".to_owned()));
    record.insert("authType".to_owned(), Value::String("oauth".to_owned()));
    record.insert("testStatus".to_owned(), Value::String("active".to_owned()));
    if !specific.is_empty() {
        record.insert("providerSpecificData".to_owned(), Value::Object(specific));
    }

    state
        .create_provider_connection(&Value::Object(record))
        .await
        .map(|_connection| ())
        .ok_or_else(|| "nullrouter-state did not record the connection".to_owned())
}

/// Hosts a Microsoft token endpoint may be on.
///
/// This is the load-bearing check in the `CLIProxyAPI` import, and it is not about the import at all: the
/// endpoint is *stored*, and every later refresh posts the refresh token to whatever was stored. An
/// unvalidated value here is a way to have this service hand a long-lived credential to an endpoint of
/// the caller's choosing, on a schedule, forever. Upstream's allowlist, kept exactly.
const MICROSOFT_TOKEN_HOSTS: [&str; 3] = [
    "login.microsoftonline.com",
    "login.microsoft.com",
    "login.windows.net",
];

/// The default region for a `CLIProxyAPI` import that names none.
const CLI_PROXY_DEFAULT_REGION: &str = "us-east-1";

/// The lifetime assumed when an import states none and the token claims none.
const DEFAULT_EXPIRES_IN: i64 = 3600;

/// `POST /api/oauth/kiro/import-cli-proxy`.
///
/// Imports a `CLIProxyAPI` auth document for a Microsoft `external_idp` Kiro account. Local: everything
/// needed is in the document, and the credential is verified the next time it is refreshed rather than
/// here — there is no Kiro endpoint that would accept this token in a probe.
///
/// What the route does instead is refuse a document that could not work, and refuse one whose stored
/// endpoint would later leak the refresh token.
pub(super) async fn kiro_import_cli_proxy(
    state: web::Data<crate::StateClient>,
    body: web::Bytes,
) -> HttpResponse {
    let parsed = match json_body::parse::<Value>(&body) {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };

    // Upstream accepts the document at any of three keys, or as the body itself, or as a JSON string
    // holding the document — because a user pastes whatever their file gave them.
    let raw = ["cliProxyAuth", "auth", "json"]
        .iter()
        .find_map(|key| parsed.get(*key))
        .unwrap_or(&parsed);
    let document = match raw {
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(inner) => inner,
            Err(_error) => {
                return refuse(StatusCode::BAD_REQUEST, "CLIProxyAPI auth JSON is invalid");
            }
        },
        other => other.clone(),
    };

    let normalised = match normalise_external_idp(&document) {
        Ok(normalised) => normalised,
        // 400 throughout, as upstream: every failure here is a property of the pasted document.
        Err(error) => return refuse(StatusCode::BAD_REQUEST, error),
    };

    let created = state
        .create_provider_connection(&serde_json::json!({
            "provider": "kiro",
            "authType": "oauth",
            "accessToken": normalised.access_token,
            "refreshToken": normalised.refresh_token,
            "expiresAt": normalised.expires_at,
            "email": normalised.email.clone().map_or(Value::Null, Value::String),
            "name": normalised.email.clone().unwrap_or_else(|| "kiro".to_owned()),
            "testStatus": "active",
            "providerSpecificData": {
                "profileArn": normalised.profile_arn,
                "region": normalised.region,
                "authMethod": "external_idp",
                "provider": "CLIProxyAPI",
                "clientId": normalised.client_id,
                // Stored because the refresh needs it, and safe to store because it was checked
                // against the host allowlist above.
                "tokenEndpoint": normalised.token_endpoint,
                "scope": normalised.scope,
            },
        }))
        .await;

    created.map_or_else(
        || {
            refuse(
                StatusCode::BAD_GATEWAY,
                "The document was accepted, but the connection could not be recorded because \
                 nullrouter-state did not answer.",
            )
        },
        |_connection| {
            responses::json(
                StatusCode::OK,
                &serde_json::json!({
                    "success": true,
                    "connection": {
                        "provider": "kiro",
                        "email": normalised.email,
                    },
                }),
            )
        },
    )
}

/// A `CLIProxyAPI` document, checked and normalised.
#[derive(Debug)]
struct ExternalIdp {
    access_token: String,
    refresh_token: String,
    expires_at: String,
    email: Option<String>,
    client_id: String,
    token_endpoint: String,
    profile_arn: String,
    region: String,
    scope: String,
}

/// Read a `CLIProxyAPI` document, refusing anything that could not work.
///
/// Every field is accepted under both its `snake_case` and `camelCase` spelling, because the document
/// is written by a different tool and a user pastes it as they found it.
fn normalise_external_idp(document: &Value) -> Result<ExternalIdp, String> {
    if !document.is_object() {
        return Err("CLIProxyAPI auth JSON is required".to_owned());
    }
    let text = |snake: &str, camel: &str| -> String {
        [snake, camel]
            .iter()
            .find_map(|key| document.get(*key).and_then(Value::as_str))
            .unwrap_or_default()
            .trim()
            .to_owned()
    };

    // A document for a different Kiro auth method is refused rather than half-imported: the fields
    // below would be missing and the failure would name the wrong thing.
    let method = text("auth_method", "authMethod");
    if !method.is_empty() && method != "external_idp" {
        return Err("Only external_idp Kiro auth is supported by this importer".to_owned());
    }

    let access_token = text("access_token", "accessToken");
    let refresh_token = text("refresh_token", "refreshToken");
    let client_id = text("client_id", "clientId");
    let profile_arn = text("profile_arn", "profileArn");
    let region = match text("region", "region") {
        found if found.is_empty() => CLI_PROXY_DEFAULT_REGION.to_owned(),
        found => found,
    };
    let scope = normalise_scope(document);
    let token_endpoint = checked_microsoft_endpoint(&text("token_endpoint", "tokenEndpoint"))?;

    // Reported in upstream's order, so a user fixing one field at a time sees the same sequence.
    for (value, message) in [
        (&access_token, "access_token is required"),
        (&refresh_token, "refresh_token is required"),
        (&client_id, "client_id is required"),
        (&scope, "scopes is required"),
        (&profile_arn, "profile_arn is required"),
    ] {
        if value.is_empty() {
            return Err(message.to_owned());
        }
    }

    let claims = jwt_payload(&access_token);
    let claim = |key: &str| {
        claims
            .as_ref()
            .and_then(|payload| payload.get(key))
            .and_then(Value::as_str)
            .filter(|found| !found.is_empty())
            .map(str::to_owned)
    };
    // Upstream's order. `upn` and `sub` are Microsoft-specific and are the only labels a work account
    // often carries, so dropping them would leave the panel showing an unnamed connection.
    let email = document
        .get("email")
        .and_then(Value::as_str)
        .filter(|found| !found.is_empty())
        .map(str::to_owned)
        .or_else(|| claim("email"))
        .or_else(|| claim("preferred_username"))
        .or_else(|| claim("upn"))
        .or_else(|| claim("sub"));

    Ok(ExternalIdp {
        expires_at: resolve_expires_at(document, claims.as_ref()),
        access_token,
        refresh_token,
        email,
        client_id,
        token_endpoint,
        profile_arn,
        region,
        scope,
    })
}

/// The scope, from either a list or a space-separated string.
fn normalise_scope(document: &Value) -> String {
    for key in ["scopes", "scope"] {
        match document.get(key) {
            Some(Value::Array(items)) => {
                let joined = items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !joined.is_empty() {
                    return joined;
                }
            }
            Some(Value::String(text)) if !text.trim().is_empty() => {
                return text.trim().to_owned();
            }
            _other => {}
        }
    }
    String::new()
}

/// A token endpoint that may be stored and used for a later refresh.
fn checked_microsoft_endpoint(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("token_endpoint is required".to_owned());
    }
    let parsed = reqwest::Url::parse(trimmed)
        .map_err(|_error| "token_endpoint must be a valid URL".to_owned())?;
    if parsed.scheme() != "https" {
        return Err("token_endpoint must use https".to_owned());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "token_endpoint must be a valid URL".to_owned())?
        .to_ascii_lowercase();
    if !MICROSOFT_TOKEN_HOSTS.contains(&host.as_str()) {
        return Err("token_endpoint must be a Microsoft login endpoint".to_owned());
    }
    Ok(parsed.to_string())
}

/// When the imported credential expires.
///
/// Four sources in upstream's order: a stated absolute time, a stated lifetime, the token's own `exp`
/// claim, then an hour. The last is a floor rather than a guess that matters — a credential whose
/// expiry is unknown is refreshed sooner than needed, which is the harmless direction to be wrong in.
fn resolve_expires_at(document: &Value, claims: Option<&Value>) -> String {
    for key in ["expired", "expires_at", "expiresAt"] {
        if let Some(stated) = document.get(key).and_then(Value::as_str)
            && !stated.trim().is_empty()
        {
            return stated.trim().to_owned();
        }
    }
    for key in ["expires_in", "expiresIn"] {
        if let Some(seconds) = document.get(key).and_then(Value::as_i64)
            && seconds > 0
        {
            return expires_in_seconds(seconds);
        }
    }
    if let Some(exp) = claims
        .and_then(|payload| payload.get("exp"))
        .and_then(Value::as_i64)
    {
        return format_rfc3339(exp);
    }
    expires_in_seconds(DEFAULT_EXPIRES_IN)
}

/// An RFC 3339 timestamp `seconds` from now.
fn expires_in_seconds(seconds: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0_i64, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        });
    format_rfc3339(now.saturating_add(seconds))
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
            chatgpt_account_id: text(auth, "chatgpt_account_id").or_else(|| top("account_id")),
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
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

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
    expires_in_seconds(days.saturating_mul(24 * 60 * 60))
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
