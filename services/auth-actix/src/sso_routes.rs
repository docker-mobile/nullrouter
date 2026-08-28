//! `/api/auth/oidc/*` and `/api/auth/saml/*` handlers.
//!
//! Split out of [`crate::routes`] so the session-minting path is in one place and
//! easy to audit. There is exactly one call to `session().create_token` in this
//! file, in [`oidc_callback`], and it sits after a verified `id_token`.

use actix_web::{
    HttpRequest, HttpResponse,
    cookie::{Cookie, SameSite, time},
    http::{StatusCode, header},
    web,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    AuthService,
    oidc::{
        self, AuthorizationRequest, IdTokenExpectations, OidcConfig, OidcError, TokenExchange,
        create_pkce_pair, create_random_token,
    },
    responses,
    saml::{self, AssertionPost, SamlConfig, SamlError},
};

/// How long the browser may take to come back from the provider.
const FLOW_COOKIE_TTL_SECONDS: i64 = 10 * 60;

pub(crate) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(web::resource("/api/auth/oidc/start").route(web::get().to(oidc_start)))
        .service(web::resource("/api/auth/oidc/callback").route(web::get().to(oidc_callback)))
        .service(web::resource("/api/auth/oidc/test").route(web::post().to(oidc_test)))
        .service(web::resource("/api/auth/saml/start").route(web::get().to(saml_start)))
        .service(web::resource("/api/auth/saml/acs").route(web::post().to(saml_acs)))
        .service(web::resource("/api/auth/saml/metadata").route(web::get().to(saml_metadata)))
        .service(web::resource("/api/auth/saml/test").route(web::post().to(saml_test)));
}

/// The origin this request appears to have arrived on.
///
/// Honours `X-Forwarded-Proto`/`X-Forwarded-Host` because the gateway sits in
/// front. Only ever used as a fallback: an explicitly configured public origin
/// wins, since a forwarded header is caller-controlled and the OIDC
/// `redirect_uri` has to match what the provider registered.
fn request_origin(request: &HttpRequest) -> Option<String> {
    let header_value = |name: header::HeaderName| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let host = header_value(header::HeaderName::from_static("x-forwarded-host"))
        .or_else(|| header_value(header::HOST))?;
    let scheme = header_value(header::HeaderName::from_static("x-forwarded-proto"))
        .unwrap_or_else(|| request.connection_info().scheme().to_owned());
    Some(format!("{scheme}://{host}"))
}

fn origin_for(service: &AuthService, request: &HttpRequest) -> String {
    service
        .config()
        .public_origin_or(request_origin(request).as_deref())
}

/// A short-lived cookie carrying one leg of the flow.
fn flow_cookie(service: &AuthService, name: &'static str, value: String) -> Cookie<'static> {
    Cookie::build(name, value)
        .http_only(true)
        .secure(service.config().secure_cookie())
        // `Lax` so the cookie survives the provider's top-level redirect back.
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::seconds(FLOW_COOKIE_TTL_SECONDS))
        .finish()
}

fn cleared_cookie(service: &AuthService, name: &'static str) -> Cookie<'static> {
    Cookie::build(name, "")
        .http_only(true)
        .secure(service.config().secure_cookie())
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::ZERO)
        .expires(time::OffsetDateTime::UNIX_EPOCH)
        .finish()
}

/// A 302 to `path`, clearing every in-flight OIDC cookie on the way.
fn redirect_clearing_oidc(service: &AuthService, origin: &str, path: &str) -> HttpResponse {
    let mut response = HttpResponse::build(StatusCode::FOUND);
    response.insert_header((header::LOCATION, format!("{origin}{path}")));
    for name in [
        oidc::STATE_COOKIE,
        oidc::NONCE_COOKIE,
        oidc::VERIFIER_COOKIE,
    ] {
        response.cookie(cleared_cookie(service, name));
    }
    response.finish()
}

/// `?error=` on `/login`, percent-encoded.
fn login_error_path(code: &str) -> String {
    format!(
        "/login?error={}",
        code.chars()
            .flat_map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
                    vec![ch]
                } else {
                    format!("%{:02X}", ch as u32 & 0xFF).chars().collect()
                }
            })
            .collect::<String>()
    )
}

/// Load the SSO configuration, mapping "state is down" to its own error.
///
/// A state outage must not read as "OIDC is not configured": one is a transient
/// fault, the other tells the operator to go and set it up.
async fn oidc_config(service: &AuthService) -> Result<OidcConfig, OidcError> {
    let settings = service
        .auth_settings()
        .await
        .map_err(|_| OidcError::StateUnavailable)?;
    OidcConfig::from_settings(&settings).ok_or(OidcError::NotConfigured)
}

async fn saml_config(service: &AuthService) -> Result<SamlConfig, SamlError> {
    let settings = service
        .auth_settings()
        .await
        .map_err(|_| SamlError::StateUnavailable)?;
    SamlConfig::from_settings(&settings).ok_or(SamlError::NotConfigured)
}

/// Begin the OIDC flow: mint PKCE + state + nonce, then redirect to the provider.
///
/// Not in the task list, but the callback is unreachable without it — nothing
/// else sets the `oidc_state` cookie the callback checks, and the login page
/// already links here.
async fn oidc_start(service: web::Data<AuthService>, request: HttpRequest) -> HttpResponse {
    let service = service.into_inner();
    let origin = origin_for(&service, &request);

    let config = match oidc_config(&service).await {
        Ok(config) => config,
        Err(error) => {
            return redirect_clearing_oidc(&service, &origin, &login_error_path(error.code()));
        }
    };
    let Some(http) = service.oidc_http() else {
        return redirect_clearing_oidc(
            &service,
            &origin,
            &login_error_path(OidcError::StateUnavailable.code()),
        );
    };
    let discovery = match http.discovery(&config.issuer_url).await {
        Ok(discovery) => discovery,
        Err(error) => {
            return redirect_clearing_oidc(&service, &origin, &login_error_path(error.code()));
        }
    };

    let pkce = create_pkce_pair();
    let state = create_random_token();
    let nonce = create_random_token();
    let redirect_uri = format!("{origin}/api/auth/oidc/callback");
    let Some(authorize_url) = oidc::authorization_url(&AuthorizationRequest {
        authorization_endpoint: &discovery.authorization_endpoint,
        client_id: &config.client_id,
        redirect_uri: &redirect_uri,
        scopes: &config.scopes,
        state: &state,
        nonce: &nonce,
        code_challenge: &pkce.challenge,
    }) else {
        return redirect_clearing_oidc(
            &service,
            &origin,
            &login_error_path(
                OidcError::Discovery("authorization_endpoint is not a URL".to_owned()).code(),
            ),
        );
    };

    let mut response = HttpResponse::build(StatusCode::FOUND);
    response.insert_header((header::LOCATION, authorize_url));
    response.cookie(flow_cookie(&service, oidc::STATE_COOKIE, state));
    response.cookie(flow_cookie(&service, oidc::NONCE_COOKIE, nonce));
    response.cookie(flow_cookie(&service, oidc::VERIFIER_COOKIE, pkce.verifier));
    response.finish()
}

/// Callback query parameters.
#[derive(Debug, Default, Deserialize)]
struct CallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    /// The provider's own error, when it declined instead of returning a code.
    #[serde(default)]
    error: Option<String>,
}

/// Finish the OIDC flow.
///
/// The order of checks is the security-relevant part, and it follows upstream:
/// a provider-reported error short-circuits; a missing `code`/`state` is
/// rejected; the returned `state` must equal the `oidc_state` cookie **and** the
/// nonce and verifier cookies must be present; only then is the code exchanged;
/// and the resulting `id_token` must verify against the provider's JWKS before a
/// session cookie is set. Every failure clears the three flow cookies and
/// redirects to `/login?error=…`, and no failure path reaches the session below.
async fn oidc_callback(
    service: web::Data<AuthService>,
    request: HttpRequest,
    query: Option<web::Query<CallbackQuery>>,
) -> HttpResponse {
    let service = service.into_inner();
    let origin = origin_for(&service, &request);
    let query = query.map(web::Query::into_inner).unwrap_or_default();

    // The provider declined. Upstream forwards its code verbatim.
    if let Some(error) = query.error.as_deref().filter(|error| !error.is_empty()) {
        return redirect_clearing_oidc(&service, &origin, &login_error_path(error));
    }

    let cookie_value = |name: &str| request.cookie(name).map(|cookie| cookie.value().to_owned());
    let stored_state = cookie_value(oidc::STATE_COOKIE);
    let stored_nonce = cookie_value(oidc::NONCE_COOKIE);
    let verifier = cookie_value(oidc::VERIFIER_COOKIE);

    let outcome = oidc_callback_inner(
        &service,
        &origin,
        &query,
        stored_state.as_deref(),
        stored_nonce.as_deref(),
        verifier.as_deref(),
    )
    .await;

    match outcome {
        Ok(token) => {
            let mut response = HttpResponse::build(StatusCode::FOUND);
            response.insert_header((header::LOCATION, format!("{origin}/dashboard")));
            for name in [
                oidc::STATE_COOKIE,
                oidc::NONCE_COOKIE,
                oidc::VERIFIER_COOKIE,
            ] {
                response.cookie(cleared_cookie(&service, name));
            }
            response.cookie(service.session().session_cookie(token));
            response.finish()
        }
        Err(error) => redirect_clearing_oidc(&service, &origin, &login_error_path(error.code())),
    }
}

/// The callback's decision, returning a session token only on full success.
///
/// Returning the token rather than a response keeps every early return an
/// `Err`, so there is no way to fall out of this function with a session by
/// accident.
async fn oidc_callback_inner(
    service: &AuthService,
    origin: &str,
    query: &CallbackQuery,
    stored_state: Option<&str>,
    stored_nonce: Option<&str>,
    verifier: Option<&str>,
) -> Result<String, OidcError> {
    fn non_empty(value: Option<&str>) -> Option<&str> {
        value.filter(|value| !value.is_empty())
    }
    let (Some(code), Some(state)) = (
        non_empty(query.code.as_deref()),
        non_empty(query.state.as_deref()),
    ) else {
        return Err(OidcError::MissingCode);
    };

    // All three cookies must be present, and the state must match. A missing
    // cookie is as fatal as a mismatch: without the nonce there is nothing to
    // bind the id_token to, and without the verifier PKCE is not being used.
    let (Some(stored_state), Some(stored_nonce), Some(verifier)) = (
        non_empty(stored_state),
        non_empty(stored_nonce),
        non_empty(verifier),
    ) else {
        return Err(OidcError::InvalidState);
    };
    if stored_state != state {
        return Err(OidcError::InvalidState);
    }

    let config = oidc_config(service).await?;
    let http = service.oidc_http().ok_or(OidcError::StateUnavailable)?;
    let discovery = http.discovery(&config.issuer_url).await?;
    let issuer = if discovery.issuer.is_empty() {
        config.issuer_url.clone()
    } else {
        discovery.issuer.clone()
    };

    let tokens = http
        .exchange_code(&TokenExchange {
            token_endpoint: &discovery.token_endpoint,
            client_id: &config.client_id,
            client_secret: &config.client_secret,
            code,
            redirect_uri: &format!("{origin}/api/auth/oidc/callback"),
            code_verifier: verifier,
        })
        .await?;
    let id_token = tokens
        .id_token
        .filter(|token| !token.is_empty())
        .ok_or(OidcError::MissingIdToken)?;

    let jwks = http.jwks(&discovery.jwks_uri).await?;
    let claims = oidc::verify_id_token(
        &id_token,
        &jwks,
        &IdTokenExpectations {
            issuer: &issuer,
            audience: &config.client_id,
            nonce: stored_nonce,
            now_seconds: service.now(),
        },
    )?;
    // Claims are read only to be dropped for now: the session token carries no
    // identity beyond "authenticated", so recording them would be storing state
    // nothing reads. Verification is the part that matters, and it has happened.
    drop((oidc::pick_display_name(&claims), oidc::pick_email(&claims)));

    service
        .session()
        .create_token(service.now())
        .ok_or(OidcError::StateUnavailable)
}

/// Body for `POST /api/auth/oidc/test`.
///
/// Each field overrides the stored setting, so an operator can validate a change
/// before saving it. `clientSecret` present-but-empty means "test without a
/// secret", which is why it is an `Option` rather than defaulted.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OidcTestRequest {
    #[serde(default)]
    issuer_url: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    scopes: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OidcTestResponse {
    ok: bool,
    discovery_ok: bool,
    issuer_url: String,
    client_id: String,
    scopes: String,
    redirect_uri: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    /// Whether the discovery document advertises everything the flow needs.
    ready_for_login: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Validate the configured OIDC provider by fetching its discovery document.
///
/// Requires a dashboard session: it reaches out to a caller-influenced URL and
/// reports what it found, so it is not something an unauthenticated visitor
/// should be able to drive. Upstream allows it unauthenticated when
/// `requireLogin === false`; that branch does not exist here, since login is
/// always required.
///
/// Reports only what it observed. The client secret is **not** probed — upstream
/// sends a deliberately invalid authorization code and reads the error code back,
/// and inferring "secret is valid" from an error string is a guess this will not
/// make. `readyForLogin` therefore means "discovery advertises the endpoints the
/// flow needs", not "sign-in will succeed".
async fn oidc_test(
    service: web::Data<AuthService>,
    request: HttpRequest,
    body: web::Bytes,
) -> HttpResponse {
    let service = service.into_inner();
    if !is_authenticated(&service, &request) {
        return unauthorized();
    }
    let overrides: OidcTestRequest = if body.is_empty() {
        OidcTestRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(parsed) => parsed,
            Err(_) => {
                return responses::json(
                    StatusCode::BAD_REQUEST,
                    &json!({ "error": "Invalid JSON body" }),
                );
            }
        }
    };

    let stored = match service.auth_settings().await {
        Ok(settings) => settings,
        Err(_) => {
            return responses::json(
                StatusCode::SERVICE_UNAVAILABLE,
                &json!({ "error": "Router state is unavailable, so OIDC settings could not be read" }),
            );
        }
    };
    let pick = |override_value: Option<String>, stored: &str| {
        override_value
            .unwrap_or_else(|| stored.to_owned())
            .trim()
            .to_owned()
    };
    let issuer_url =
        oidc::trim_trailing_slashes(&pick(overrides.issuer_url, &stored.oidc_issuer_url));
    let client_id = pick(overrides.client_id, &stored.oidc_client_id);
    let scopes = oidc::normalize_scopes(&pick(overrides.scopes, &stored.oidc_scopes));
    let client_secret = pick(overrides.client_secret, &stored.oidc_client_secret);

    if issuer_url.is_empty() {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Issuer URL is required" }),
        );
    }
    if client_id.is_empty() {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "Client ID is required" }),
        );
    }

    let origin = origin_for(&service, &request);
    let redirect_uri = format!("{origin}/api/auth/oidc/callback");
    let Some(http) = service.oidc_http() else {
        return responses::json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "error": "The outbound HTTP client could not be created" }),
        );
    };

    match http.discovery(&issuer_url).await {
        Ok(discovery) => {
            let ready = !discovery.authorization_endpoint.is_empty()
                && !discovery.token_endpoint.is_empty()
                && !discovery.jwks_uri.is_empty();
            let message = if ready {
                if client_secret.is_empty() {
                    "Discovery loaded. No client secret is configured, so the token exchange will \
                     be attempted without one."
                } else {
                    "Discovery loaded and advertises the authorization, token, and JWKS endpoints. \
                     The client secret was not tested: it can only be checked by a real sign-in."
                }
            } else {
                "Discovery loaded but does not advertise every endpoint the login flow needs."
            };
            responses::json(
                StatusCode::OK,
                &OidcTestResponse {
                    ok: ready,
                    discovery_ok: true,
                    issuer_url,
                    client_id,
                    scopes,
                    redirect_uri,
                    authorization_endpoint: discovery.authorization_endpoint,
                    token_endpoint: discovery.token_endpoint,
                    jwks_uri: discovery.jwks_uri,
                    ready_for_login: ready,
                    message: Some(message.to_owned()),
                    error: None,
                },
            )
        }
        Err(error) => responses::json(
            StatusCode::OK,
            &OidcTestResponse {
                ok: false,
                discovery_ok: false,
                issuer_url,
                client_id,
                scopes,
                redirect_uri,
                authorization_endpoint: String::new(),
                token_endpoint: String::new(),
                jwks_uri: String::new(),
                ready_for_login: false,
                message: None,
                error: Some(error.to_string()),
            },
        ),
    }
}

/// Begin the SAML flow.
///
/// Implemented even though the assertion cannot yet be verified, because it is
/// what makes the metadata and the IdP-side configuration testable end to end.
/// The browser will come back to `/api/auth/saml/acs`, which refuses — that
/// refusal is the honest outcome, and it is far better than an ACS that accepts
/// whatever it is posted.
async fn saml_start(service: web::Data<AuthService>, request: HttpRequest) -> HttpResponse {
    let service = service.into_inner();
    let origin = origin_for(&service, &request);
    let config = match saml_config(&service).await {
        Ok(config) => config,
        Err(error) => return saml_login_redirect(&service, &origin, error.code()),
    };

    let Some(built) = saml::authn_request(&origin, &config, &rfc3339(service.now())) else {
        return saml_login_redirect(&service, &origin, "saml_request_build_failed");
    };
    let mut response = HttpResponse::build(StatusCode::FOUND);
    response.insert_header((header::LOCATION, built.redirect_url));
    response.cookie(flow_cookie(&service, saml::STATE_COOKIE, built.request_id));
    response.finish()
}

fn saml_login_redirect(service: &AuthService, origin: &str, code: &str) -> HttpResponse {
    let mut response = HttpResponse::build(StatusCode::FOUND);
    response.insert_header((
        header::LOCATION,
        format!("{origin}{}", login_error_path(code)),
    ));
    response.cookie(cleared_cookie(service, saml::STATE_COOKIE));
    response.finish()
}

/// Consume a SAML assertion — or rather, refuse to.
///
/// See the [`crate::saml`] module docs. Every check that can be made is made,
/// and then the assertion is rejected because its signature cannot be verified.
/// This handler has no path that mints a session; that is deliberate and must
/// stay that way until XML-DSig verification exists.
async fn saml_acs(
    service: web::Data<AuthService>,
    request: HttpRequest,
    body: web::Bytes,
) -> HttpResponse {
    let service = service.into_inner();
    let origin = origin_for(&service, &request);
    let expected_request_id = request
        .cookie(saml::STATE_COOKIE)
        .map(|cookie| cookie.value().to_owned())
        .unwrap_or_default();

    let form = parse_form(&body);
    let config = saml_config(&service).await.ok();
    let error = match saml::consume_response(
        &AssertionPost {
            saml_response: form.get("SAMLResponse").map(String::as_str),
            expected_request_id: &expected_request_id,
        },
        config.as_ref(),
    ) {
        // `consume_response` returns `Infallible` on success, so this arm cannot
        // be constructed: there is no way to reach a session from here.
        Ok(never) => match never {},
        Err(error) => error,
    };

    // The refusal is a 501 with a body, not a bare redirect, when the caller
    // asked for JSON — an operator testing their IdP needs to see *why*. A
    // browser POST still gets the redirect upstream produces.
    if wants_json(&request) {
        return responses::json(
            StatusCode::NOT_IMPLEMENTED,
            &json!({
                "ok": false,
                "error": error.code(),
                "message": error.to_string(),
                "accepted": false,
            }),
        );
    }
    saml_login_redirect(&service, &origin, error.code())
}

/// Serve SP metadata XML.
///
/// Upstream answers a failure with an XML `<Error>` document, and so does this.
async fn saml_metadata(service: web::Data<AuthService>, request: HttpRequest) -> HttpResponse {
    let service = service.into_inner();
    let origin = origin_for(&service, &request);
    match saml_config(&service).await {
        Ok(config) => HttpResponse::build(StatusCode::OK)
            .insert_header((header::CONTENT_TYPE, "application/xml"))
            .insert_header((header::CACHE_CONTROL, "no-cache"))
            .body(saml::metadata_xml(&origin, &config)),
        Err(error) => HttpResponse::build(StatusCode::SERVICE_UNAVAILABLE)
            .insert_header((header::CONTENT_TYPE, "application/xml"))
            .body(format!(
                "<?xml version=\"1.0\"?><Error>{}</Error>",
                saml::escape_xml(&error.to_string())
            )),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SamlTestResponse {
    ok: bool,
    configured: bool,
    entry_point: String,
    issuer: String,
    certificate_present: bool,
    acs_url: String,
    metadata_url: String,
    /// Whether an assertion could actually be accepted. Always `false`.
    assertion_verification_available: bool,
    message: String,
}

/// Report what is configured for SAML, and what this build will do with it.
///
/// Reports `assertionVerificationAvailable: false` rather than a cheerful "ok":
/// an operator has to know that sign-in will not complete before they point an
/// IdP at this router.
async fn saml_test(service: web::Data<AuthService>, request: HttpRequest) -> HttpResponse {
    let service = service.into_inner();
    if !is_authenticated(&service, &request) {
        return unauthorized();
    }
    let origin = origin_for(&service, &request);
    let settings = match service.auth_settings().await {
        Ok(settings) => settings,
        Err(_) => {
            return responses::json(
                StatusCode::SERVICE_UNAVAILABLE,
                &json!({ "error": "Router state is unavailable, so SAML settings could not be read" }),
            );
        }
    };
    let config = SamlConfig::from_settings(&settings);
    let certificate_present = !saml::format_x509_certificate(&settings.saml_cert).is_empty();

    responses::json(
        StatusCode::OK,
        &SamlTestResponse {
            // Never `true`: a configuration that cannot complete a login is not
            // a working configuration.
            ok: false,
            configured: config.is_some(),
            entry_point: settings.saml_entry_point.trim().to_owned(),
            issuer: saml::normalize_issuer(&settings.saml_issuer),
            certificate_present,
            acs_url: saml::acs_url(&origin),
            metadata_url: format!("{origin}/api/auth/saml/metadata"),
            assertion_verification_available: false,
            message: if config.is_some() {
                SamlError::VerificationUnavailable.to_string()
            } else {
                "SAML needs both an IdP sign-on URL and an IdP signing certificate. Note that \
                 even once configured, this build cannot verify an assertion, so sign-in will \
                 not complete."
                    .to_owned()
            },
        },
    )
}

/// Whether this request carries a valid dashboard session.
fn is_authenticated(service: &AuthService, request: &HttpRequest) -> bool {
    request
        .cookie(crate::session::SessionCodec::cookie_name())
        .is_some_and(|cookie| service.session().verify(cookie.value(), service.now()))
}

fn unauthorized() -> HttpResponse {
    responses::json(
        StatusCode::UNAUTHORIZED,
        &json!({ "error": "Unauthorized" }),
    )
}

/// Whether the caller would rather have JSON than a redirect.
fn wants_json(request: &HttpRequest) -> bool {
    request
        .headers()
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("application/json"))
}

/// Parse an `application/x-www-form-urlencoded` body.
fn parse_form(body: &[u8]) -> std::collections::BTreeMap<String, String> {
    let text = String::from_utf8_lossy(body);
    form_urlencoded::parse(text.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

/// Epoch seconds as an RFC 3339 timestamp, for `IssueInstant`.
///
/// Written out by hand rather than pulling in a date library for one format
/// string. Uses the civil-from-days algorithm, so it is correct for any epoch
/// second, leap years included.
fn rfc3339(epoch_seconds: u64) -> String {
    let days = epoch_seconds / 86_400;
    let seconds_of_day = epoch_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Days since the Unix epoch to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}
