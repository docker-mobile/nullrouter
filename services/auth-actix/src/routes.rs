use std::{
    future::{Ready, ready},
    net::IpAddr,
};

use actix_web::{
    HttpRequest, HttpResponse, ResponseError as _,
    http::{Method, StatusCode, header},
    web,
};

use crate::{
    AuthService, SERVICE_NAME,
    contracts::{
        AuthStatusResponse, AuthorizationKind, AuthorizeRequest, AuthorizeResponse, HealthResponse,
        LoginDeniedResponse, LoginLockedResponse, LoginRequest, LoginSuccessResponse,
        LogoutResponse,
    },
    errors::{ApiError, protocol_error},
    lockout::{FailureState, LockState},
    oidc::{self, OidcConfig},
    responses,
    saml::SamlConfig,
    session::SessionCodec,
};

const MAX_REQUEST_BODY_BYTES: usize = 8 * 1_024;
const MAX_PASSWORD_BYTES: usize = 1_024;
const MAX_API_KEY_BYTES: usize = 4_096;
const RESET_HINT: &str = "Forgot password? Reset to default via nullrouter CLI -> Settings -> Reset Password to Default.";

pub(crate) fn configure(config: &mut web::ServiceConfig, service: AuthService) {
    config
        .app_data(web::Data::new(service))
        .app_data(web::PayloadConfig::new(MAX_REQUEST_BODY_BYTES))
        .service(
            web::resource("/health")
                .route(web::get().to(health))
                .route(web::method(Method::OPTIONS).to(options))
                .route(web::route().to(|| ready(method_not_allowed("GET, OPTIONS")))),
        )
        .service(
            web::resource("/api/auth/status")
                .route(web::get().to(status))
                .route(web::method(Method::OPTIONS).to(options))
                .route(web::route().to(|| ready(method_not_allowed("GET, OPTIONS")))),
        )
        .service(
            web::resource("/api/auth/login")
                .route(web::post().to(login))
                .route(web::method(Method::OPTIONS).to(options))
                .route(web::route().to(|| ready(method_not_allowed("POST, OPTIONS")))),
        )
        .service(
            web::resource("/api/auth/logout")
                .route(web::post().to(logout))
                .route(web::method(Method::OPTIONS).to(options))
                .route(web::route().to(|| ready(method_not_allowed("POST, OPTIONS")))),
        )
        .service(
            web::resource("/internal/v1/authorize")
                .route(web::post().to(authorize))
                .route(web::method(Method::OPTIONS).to(options))
                .route(web::route().to(|| ready(method_not_allowed("POST, OPTIONS")))),
        )
        .configure(crate::sso_routes::configure)
        .default_service(web::route().to(not_found));
}

fn health() -> Ready<HttpResponse> {
    ready(responses::json(
        StatusCode::OK,
        &HealthResponse {
            ok: true,
            service: SERVICE_NAME,
        },
    ))
}

/// Report whether this caller holds a valid dashboard session.
///
/// `requireLogin` is a hard-coded `true` **by design, not by omission**.
/// Dashboard login is unconditional in nullrouter: there is no setting behind
/// this field, `GET /api/settings/require-login` has been removed, and
/// `Settings` carries no `requireLogin` to read. Turning dashboard auth off
/// entirely is deliberately not offered — the dashboard holds provider
/// credentials, so an unauthenticated one is a credential leak with a UI.
///
/// So please do not "fix" this back into a settings lookup. The field stays only
/// because the login page reads it, and it must never report `false` — a client
/// that sees `requireLogin: false` would skip the login screen.
/// Whether single sign-on is usable, answered by the same predicates the flows themselves use.
///
/// This is deliberately not a separate configuration flag. `oidc_configured` used to be a hardcoded
/// `false`, which meant an operator could set a valid issuer, client id and secret, have
/// `/api/auth/oidc/start` work perfectly, and still never see a sign-in button — the login screen
/// reads this field to decide whether to offer one. Asking `OidcConfig::from_settings` and
/// `SamlConfig::from_settings` is what keeps the answer and the behaviour from drifting apart:
/// if a flow can run, the button appears, because the same function decided both.
async fn sso_availability(service: &AuthService) -> (bool, bool, String) {
    // A state service that cannot be reached is reported as "no SSO" rather than as an error. The
    // password form still works, so degrading to it beats a login screen that will not render.
    let Ok(settings) = service.auth_settings().await else {
        return (false, false, oidc::normalize_login_label(""));
    };
    let oidc = OidcConfig::from_settings(&settings);
    let saml = SamlConfig::from_settings(&settings).is_some();
    let label = oidc.as_ref().map_or_else(
        || oidc::normalize_login_label(""),
        |config| config.login_label.clone(),
    );
    (oidc.is_some(), saml, label)
}

async fn status(service: web::Data<AuthService>, request: HttpRequest) -> HttpResponse {
    let service = service.into_inner();
    let authenticated = request
        .cookie(SessionCodec::cookie_name())
        .is_some_and(|cookie| service.session().verify(cookie.value(), service.now()));
    drop(request);
    let (oidc_configured, saml_configured, oidc_login_label) = sso_availability(&service).await;
    responses::json(
        StatusCode::OK,
        &AuthStatusResponse {
            authenticated,
            require_login: true,
            auth_mode: "password",
            oidc_configured,
            oidc_login_label,
            saml_configured,
            has_password: service.config().has_configured_password_hash(),
            display_name: "Password user",
            login_method: "Password",
            oidc_name: None,
            oidc_email: None,
            oidc_login: false,
        },
    )
}

fn login(
    service: web::Data<AuthService>,
    request: HttpRequest,
    body: web::Bytes,
) -> Ready<HttpResponse> {
    let service = service.into_inner();
    let response = match login_inner(&service, &request, &body) {
        Ok(response) => response,
        Err(error) => error.error_response(),
    };
    drop((request, body));
    ready(response)
}

fn login_inner(
    service: &AuthService,
    request: &HttpRequest,
    body: &[u8],
) -> Result<HttpResponse, ApiError> {
    let peer = peer_ip(request)?;
    let now = service.now();
    let current_lock = service
        .lockout()
        .lock()
        .map_err(|_| ApiError::InternalStateUnavailable)?
        .check(peer, now);
    if let LockState::Locked { retry_after } = current_lock {
        return Ok(locked_response(retry_after));
    }

    let request = parse_json::<LoginRequest>(body)?;
    let Some(password) = request
        .password
        .as_deref()
        .map(str::trim)
        .filter(|password| !password.is_empty() && password.len() <= MAX_PASSWORD_BYTES)
    else {
        return Err(ApiError::PasswordRequired);
    };

    if service.password().verify(password) {
        service
            .lockout()
            .lock()
            .map_err(|_| ApiError::InternalStateUnavailable)?
            .record_success(peer);
        let token = service
            .session()
            .create_token(now)
            .ok_or(ApiError::InternalStateUnavailable)?;
        return Ok(responses::json_with_cookie(
            StatusCode::OK,
            &LoginSuccessResponse {
                success: true,
                must_change_password: false,
            },
            service.session().session_cookie(token),
        ));
    }

    let failure = service
        .lockout()
        .lock()
        .map_err(|_| ApiError::InternalStateUnavailable)?
        .record_failure(peer, now);
    Ok(failed_login_response(failure))
}

fn logout(service: web::Data<AuthService>) -> Ready<HttpResponse> {
    let service = service.into_inner();
    ready(responses::json_with_cookie(
        StatusCode::OK,
        &LogoutResponse { success: true },
        service.session().clear_cookie(),
    ))
}

fn authorize(
    service: web::Data<AuthService>,
    request: HttpRequest,
    body: web::Bytes,
) -> impl std::future::Future<Output = HttpResponse> {
    let prepared = peer_ip(&request).and_then(|peer| {
        if peer.is_loopback() {
            parse_json::<AuthorizeRequest>(&body).map(|request| (peer, request))
        } else {
            Err(ApiError::LoopbackRequired)
        }
    });
    drop((request, body));
    async move {
        match prepared {
            Ok((peer, request)) => {
                let response = authorize_inner(&service, peer, request).await;
                responses::json(StatusCode::OK, &response)
            }
            Err(error) => error.error_response(),
        }
    }
}

async fn authorize_inner(
    service: &AuthService,
    _peer: IpAddr,
    request: AuthorizeRequest,
) -> AuthorizeResponse {
    match request.kind() {
        AuthorizationKind::Dashboard => {
            let token = request
                .into_dashboard_token()
                .filter(|token| !token.is_empty());
            if token.is_some_and(|token| service.session().verify(&token, service.now())) {
                AuthorizeResponse {
                    authorized: true,
                    principal: Some("dashboard_session"),
                    key_id: None,
                    reason: None,
                }
            } else {
                AuthorizeResponse::denied("invalid_session")
            }
        }
        AuthorizationKind::Runtime => {
            let Some(api_key) = request
                .into_runtime_key()
                .map(|key| key.trim().to_owned())
                .filter(|key| !key.is_empty() && key.len() <= MAX_API_KEY_BYTES)
            else {
                return AuthorizeResponse::denied("invalid_api_key");
            };
            match service.validate_api_key(&api_key).await {
                Ok(validation) if validation.valid && validation.active => AuthorizeResponse {
                    authorized: true,
                    principal: Some("api_key"),
                    key_id: validation.key_id,
                    reason: None,
                },
                Ok(_) => AuthorizeResponse::denied("invalid_api_key"),
                Err(_) => AuthorizeResponse::denied("state_unavailable"),
            }
        }
    }
}

fn options() -> Ready<HttpResponse> {
    ready(responses::no_content())
}

fn not_found() -> Ready<HttpResponse> {
    ready(protocol_error(
        StatusCode::NOT_FOUND,
        "not_found",
        "Route not found",
    ))
}

fn method_not_allowed(allow: &'static str) -> HttpResponse {
    let mut response = responses::builder(StatusCode::METHOD_NOT_ALLOWED);
    response.insert_header((header::ALLOW, allow));
    response.json(&crate::contracts::ErrorEnvelope {
        error: crate::contracts::ErrorBody {
            code: "method_not_allowed",
            message: "Method not allowed",
            error_type: "request_error",
        },
    })
}

fn peer_ip(request: &HttpRequest) -> Result<IpAddr, ApiError> {
    request
        .peer_addr()
        .map(|address| address.ip())
        .ok_or(ApiError::PeerIdentityUnavailable)
}

fn parse_json<T>(body: &[u8]) -> Result<T, ApiError>
where
    T: serde::de::DeserializeOwned,
{
    if body.len() > MAX_REQUEST_BODY_BYTES {
        return Err(ApiError::BodyTooLarge);
    }
    serde_json::from_slice(body).map_err(|_| ApiError::InvalidJson)
}

fn failed_login_response(failure: FailureState) -> HttpResponse {
    match failure.lock_state {
        LockState::Allowed => responses::json(
            StatusCode::UNAUTHORIZED,
            &LoginDeniedResponse {
                error: format!(
                    "Invalid password. {} attempt(s) left before lockout.",
                    failure.remaining_before_lock
                ),
                remaining_before_lock: failure.remaining_before_lock,
            },
        ),
        LockState::Locked { retry_after } => locked_response(retry_after),
    }
}

fn locked_response(retry_after: u64) -> HttpResponse {
    let mut response = responses::builder(StatusCode::TOO_MANY_REQUESTS);
    response.insert_header((header::RETRY_AFTER, retry_after.to_string()));
    response.json(&LoginLockedResponse {
        error: format!("Too many failed attempts. Try again in {retry_after}s. {RESET_HINT}"),
        retry_after,
        reset_hint: RESET_HINT,
    })
}
