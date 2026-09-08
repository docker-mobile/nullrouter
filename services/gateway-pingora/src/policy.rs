use std::net::IpAddr;

use http::{Method, StatusCode, header};
use nullrouter_contracts::{AuthorizeRequest, SecretString};
use pingora_core::Result as PingoraResult;
use pingora_http::RequestHeader;

use crate::RouteKind;

const TRUSTED_REAL_IP_HEADER: &str = "x-9r-real-ip";
const TRUSTED_PROXY_HEADER: &str = "x-9r-via-proxy";

const HOST_ONLY_PREFIXES: &[&str] = &[
    "/api/cli-tools/cowork-settings",
    "/api/cli-tools/antigravity-mitm",
    "/api/mcp",
    "/api/tunnel/tailscale-install",
    "/api/tunnel/tailscale-enable",
    "/api/tunnel/tailscale-disable",
    "/api/tunnel/tailscale-check",
    "/api/tunnel/enable",
    "/api/tunnel/disable",
    // Everything under `/api/tunnel` is host-only, including the read-only status and the
    // operation catalog. These operations decide what this machine publishes to the internet,
    // and both binaries have subcommands beyond tunnels; a session cookie stolen from a
    // browser elsewhere must not be able to open a public route into this host, or to
    // enumerate what could be opened. The narrower entries above stay for the record of which
    // routes upstream exposes to any authenticated session.
    //
    // No trailing slash: `matches_prefix` appends its own separator, so `/api/tunnel/` here
    // would look for `/api/tunnel//` and match nothing at all.
    "/api/tunnel",
    "/api/oauth/cursor/auto-import",
    "/api/oauth/kiro/auto-import",
    "/api/auth/reset-password",
    "/api/headroom/start",
    "/api/headroom/stop",
    // Added when restart stopped being a refusal: it signals a process on this host, which is
    // the same authority `start` and `stop` carry.
    "/api/headroom/restart",
    "/api/headroom/proxy",
    // `npm install pxpipe-proxy@latest`, whose lifecycle scripts run as the API
    // service. The package name is fixed and never taken from the request, so it is
    // not an arbitrary-code path, but it installs and executes third-party code on
    // the host — the same reason `tailscale-install` is here. Upstream allows it from
    // any authenticated dashboard session; a session cookie stolen from a browser on
    // another machine should not be able to install software on this one.
    "/api/pxpipe/install",
    // `start` can install too, when `pxpipeAutoInstall` is on, so it is held to the
    // same rule. Leaving it open would make the restriction above decorative.
    "/api/pxpipe/start",
];

/// Paths whose *mutating* methods are host-only while reads stay open to a session.
///
/// `/api/cli-tools/{tool}` writes files under the operator's home directory —
/// `~/.claude/settings.json`, `~/.codex/config.toml`, VS Code's user settings. A session
/// cookie lifted from a browser on another machine must not be able to rewrite dotfiles on
/// this host, for the same reason `tailscale-install` is above.
///
/// Reads are deliberately left reachable. `GET` reports which tools are installed and
/// whether each points here, which is what the dashboard's status pane is; holding that to
/// loopback would blank the pane for every remote user while protecting nothing, since a
/// read spawns nothing and writes nothing.
///
/// This is additive: a path already in [`HOST_ONLY_PREFIXES`] stays host-only for every
/// method. `cowork-settings` and `antigravity-mitm` are there and are not relaxed by being
/// covered here too.
const HOST_ONLY_WRITE_PREFIXES: &[&str] = &[
    "/api/cli-tools",
    // `POST` installs packages into a Python interpreter on this host and `DELETE` removes them;
    // `GET` reports which extras are present, which is the panel's compression pane. Splitting by
    // method is what lets the pane stay readable remotely without exposing the install.
    "/api/headroom/extras",
];

/// What a signed-in principal is allowed to do.
///
/// Ordered, so "at least this much" is a comparison rather than a match. The gateway is where this has
/// to be enforced: it is the only component that sees a request's route and its session's role at the
/// same moment, and a check inside each service would be one implementation per service to keep in
/// agreement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PrincipalRole {
    /// Reads only.
    Viewer,
    /// Everything about how the router runs, but not who may sign in.
    Operator,
    /// Everything.
    Admin,
}

impl PrincipalRole {
    /// Parse the role a session token carried.
    ///
    /// Anything unrecognised is `Viewer`, not `Admin` and not an error. A token minted by a newer
    /// build could name a role this one has never heard of, and the safe reading of an unknown claim
    /// is the least privilege -- an unknown role that read as full access would be an escalation
    /// delivered by a version skew.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "admin" => Self::Admin,
            "operator" => Self::Operator,
            _ => Self::Viewer,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Admin => "admin",
        }
    }

    /// Rank, so `satisfies` can compare inside a `const fn`.
    ///
    /// The derived `Ord` is not const, and `decision` is, which is worth keeping: it makes the whole
    /// access decision a compile-time-checkable function of its inputs with no allocation.
    const fn rank(self) -> u8 {
        match self {
            Self::Viewer => 0,
            Self::Operator => 1,
            Self::Admin => 2,
        }
    }

    /// Whether this role is at least `needed`.
    pub const fn satisfies(self, needed: Self) -> bool {
        self.rank() >= needed.rank()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessRequirement {
    Public,
    DashboardSession,
    /// An API session holding at least `least`.
    ApiSession {
        least: PrincipalRole,
    },
    RuntimeApiKey,
    Forbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationState {
    /// Authorized, carrying what the principal may do.
    ///
    /// `None` is a principal with no role claim: a `/v1` API key, or a dashboard session minted before
    /// managed users existed. Both are treated as `Admin`, which is what they were -- reducing them
    /// would break every un-migrated install and every runtime key at once.
    Authorized {
        role: Option<PrincipalRole>,
    },
    Denied,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessDecision {
    Allow,
    RedirectToLogin,
    Unauthorized,
    Forbidden,
}

impl AccessRequirement {
    pub fn for_request(
        path: &str,
        method: &Method,
        route: RouteKind,
        peer_ip: Option<IpAddr>,
        managed_api_keys_required: bool,
    ) -> Self {
        if is_internal_path(path) {
            return Self::Forbidden;
        }
        let host_only = is_host_only_path(path) || is_host_only_write(path, method);
        if host_only && !is_loopback_peer(peer_ip) {
            return Self::Forbidden;
        }
        if is_public_path(path) {
            return Self::Public;
        }

        match route {
            RouteKind::Runtime if managed_api_keys_required => Self::RuntimeApiKey,
            RouteKind::Runtime | RouteKind::Auth => Self::Public,
            RouteKind::Dashboard => Self::DashboardSession,
            RouteKind::Api | RouteKind::Catalog | RouteKind::Events | RouteKind::State => {
                Self::ApiSession {
                    least: least_role(path, method),
                }
            }
        }
    }

    pub const fn decision(self, state: AuthorizationState) -> AccessDecision {
        match self {
            Self::Public => AccessDecision::Allow,
            Self::Forbidden => AccessDecision::Forbidden,
            Self::DashboardSession => match state {
                AuthorizationState::Authorized { .. } => AccessDecision::Allow,
                AuthorizationState::Denied | AuthorizationState::Unavailable => {
                    AccessDecision::RedirectToLogin
                }
            },
            Self::RuntimeApiKey => match state {
                AuthorizationState::Authorized { .. } => AccessDecision::Allow,
                AuthorizationState::Denied | AuthorizationState::Unavailable => {
                    AccessDecision::Unauthorized
                }
            },
            Self::ApiSession { least } => match state {
                // A principal with no role claim is the legacy full-access one; see
                // `AuthorizationState::Authorized`.
                AuthorizationState::Authorized { role: None } => AccessDecision::Allow,
                AuthorizationState::Authorized { role: Some(role) } => {
                    if role.satisfies(least) {
                        AccessDecision::Allow
                    } else {
                        // Forbidden, not Unauthorized: the session is valid and signing in again with
                        // the same account will not help. A 401 here would send the dashboard to the
                        // login screen in a loop.
                        AccessDecision::Forbidden
                    }
                }
                AuthorizationState::Denied | AuthorizationState::Unavailable => {
                    AccessDecision::Unauthorized
                }
            },
        }
    }
}

impl AccessDecision {
    pub const fn status(self) -> StatusCode {
        match self {
            Self::Allow => StatusCode::OK,
            Self::RedirectToLogin => StatusCode::TEMPORARY_REDIRECT,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
        }
    }

    pub const fn location(self) -> Option<&'static str> {
        match self {
            Self::RedirectToLogin => Some("/login"),
            Self::Allow | Self::Unauthorized | Self::Forbidden => None,
        }
    }

    pub const fn body(self) -> Option<&'static str> {
        match self {
            Self::Unauthorized => Some(r#"{"error":"Unauthorized"}"#),
            Self::Forbidden => Some(r#"{"error":"Forbidden"}"#),
            Self::Allow | Self::RedirectToLogin => None,
        }
    }
}

pub fn authorization_request(
    request: &RequestHeader,
    requirement: AccessRequirement,
) -> Option<AuthorizeRequest> {
    match requirement {
        AccessRequirement::DashboardSession | AccessRequirement::ApiSession { .. } => {
            Some(AuthorizeRequest::Dashboard {
                session_token: session_token(request),
            })
        }
        AccessRequirement::RuntimeApiKey => Some(AuthorizeRequest::Runtime {
            api_key: runtime_api_key(request),
        }),
        AccessRequirement::Public | AccessRequirement::Forbidden => None,
    }
}

pub fn stamp_trusted_identity_headers(
    request: &mut RequestHeader,
    peer_ip: IpAddr,
) -> PingoraResult<()> {
    let spoofable_headers = request
        .headers
        .keys()
        .filter(|name| {
            let name = name.as_str();
            name == header::FORWARDED.as_str()
                || name == "x-real-ip"
                || name.starts_with("x-forwarded-")
                || name == TRUSTED_REAL_IP_HEADER
                || name == TRUSTED_PROXY_HEADER
        })
        .cloned()
        .collect::<Vec<_>>();

    for name in spoofable_headers {
        request.remove_header(&name);
    }
    request.insert_header(TRUSTED_REAL_IP_HEADER, peer_ip.to_string())?;
    Ok(())
}

fn is_public_path(path: &str) -> bool {
    path == "/"
        || path == "/login"
        || path == "/landing"
        || path == "/callback"
        || path == "/favicon.svg"
        || path.starts_with("/pkg/")
        || path.starts_with("/providers/")
        || path.starts_with("/assets/")
        || path == "/api/health"
        || path == "/api/auth"
        || path.starts_with("/api/auth/")
}

/// The least role that may make this request.
///
/// Three rules, in order. Everything under `/api/users` is admin-only, because managing who can sign in
/// is the one capability that can grant every other. Any mutating method needs at least operator, since
/// a viewer that could change routing is not a viewer. Everything else is a read, which viewer covers.
///
/// `OPTIONS` counts as a read: a CORS preflight carries no body and changes nothing, and failing it
/// makes the browser report a network error instead of the 403 the real request would return.
fn least_role(path: &str, method: &Method) -> PrincipalRole {
    if path == "/api/users" || path.starts_with("/api/users/") {
        return PrincipalRole::Admin;
    }
    let mutating = !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    if mutating {
        PrincipalRole::Operator
    } else {
        PrincipalRole::Viewer
    }
}

fn is_internal_path(path: &str) -> bool {
    path == "/internal" || path.starts_with("/internal/")
}

fn is_host_only_path(path: &str) -> bool {
    matches_prefix(HOST_ONLY_PREFIXES, path)
}

/// A write to a path whose reads are open but whose writes touch the host.
///
/// `OPTIONS` is treated as a read: a CORS preflight carries no body and changes nothing, and
/// failing it would make the browser report a network error instead of the 403 the real
/// request would return.
fn is_host_only_write(path: &str, method: &Method) -> bool {
    let mutating = !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    mutating && matches_prefix(HOST_ONLY_WRITE_PREFIXES, path)
}

fn matches_prefix(prefixes: &[&str], path: &str) -> bool {
    prefixes
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
}

fn is_loopback_peer(peer_ip: Option<IpAddr>) -> bool {
    peer_ip.is_some_and(|ip| ip.is_loopback())
}

fn session_token(request: &RequestHeader) -> Option<SecretString> {
    let cookie = request.headers.get(header::COOKIE)?.to_str().ok()?;
    cookie.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == "auth_token").then(|| SecretString::new(value.to_owned()))
    })
}

fn runtime_api_key(request: &RequestHeader) -> Option<SecretString> {
    let authorization = request
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let key = authorization
        .or_else(|| header_text(request, "x-api-key"))
        .or_else(|| header_text(request, "x-goog-api-key"))
        .or_else(|| query_key(request));
    key.map(|value| SecretString::new(value.to_owned()))
}

fn header_text<'a>(request: &'a RequestHeader, name: &str) -> Option<&'a str> {
    request.headers.get(name)?.to_str().ok()
}

fn query_key(request: &RequestHeader) -> Option<&str> {
    request.uri.query()?.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == "key").then_some(value)
    })
}
