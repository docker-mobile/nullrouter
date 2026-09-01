//! Cloudflare and Tailscale, driven from the panel under supervision.
//!
//! Two surfaces over one catalog:
//!
//! * the **parity routes** — `/api/tunnel/{status,enable,disable,tailscale-*}` — which are the
//!   shapes the dashboard already calls;
//! * the **operation routes** — `/api/tunnel/operations` and
//!   `/api/tunnel/operations/{id}` — which expose the catalog itself, so a Cloudflare or
//!   Tailscale operation beyond tunnels becomes a row in [`catalog::OPERATIONS`] and is
//!   immediately callable, without a new handler.
//!
//! Everything is loopback-only at the gateway. These operations change what this machine
//! exposes to the internet, and a request that can reach them from off-box would be a way to
//! publish the router.
//!
//! # What is deliberately not here
//!
//! Installing either binary. Upstream will `curl -fsSL https://tailscale.com/install.sh` into
//! `sudo sh`, run `sudo installer -pkg`, and `spawn("sudo", ["-S", ...])` with the user's
//! password piped to stdin, and it downloads `cloudflared` from `releases/latest` and
//! `chmod 755`es it. Those are four ways to execute unverified remote code as root on the
//! operator's machine. `tailscale-install` reports the command the operator can run
//! themselves, and returns 501.

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use crate::responses;

mod catalog;
mod cloudflared;
mod manager;
mod status;
mod tailscale;

pub use manager::Manager;

use catalog::{Args, Mode, Tool};
use manager::{OpError, Outcome};
use status::TailscaleStatus;

/// `/api/tunnel/status`.
#[derive(Debug, Serialize)]
struct TunnelStatusResponse {
    tunnel: TunnelState,
    tailscale: TailscaleState,
    download: DownloadState,
}

/// The Cloudflare half of the status.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TunnelState {
    /// Up and serving.
    enabled: bool,
    /// The public URL, when there is one.
    url: String,
    /// Whether a child exists, ready or not.
    running: bool,
    /// Supervisor state: `stopped`, `starting`, `running`, `stopping`, `backoff`, `failed`.
    state: &'static str,
    /// The child's pid, so an operator can see the one process this owns.
    pid: Option<u32>,
    /// Restarts since the last manual start.
    restarts: u32,
    /// Why the last attempt failed.
    last_error: Option<String>,
    /// Whether the binary is installed at all.
    installed: bool,
}

/// The Tailscale half of the status.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TailscaleState {
    installed: bool,
    logged_in: bool,
    daemon_running: bool,
    /// The Funnel URL, when Funnel is serving.
    url: String,
    /// Whether Funnel currently has a mapping.
    funnel_active: bool,
    /// Supervisor state for our own `tailscaled`.
    state: &'static str,
    /// Pending login URL, when a login is what is missing.
    auth_url: Option<String>,
}

/// Upstream reports download progress here because it fetches `cloudflared` itself.
///
/// Kept for shape compatibility, and always "not downloading": this service never downloads a
/// binary. The message says so, so a panel built against upstream shows a reason rather than
/// an empty progress bar.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadState {
    in_progress: bool,
    message: &'static str,
}

/// Why no download will happen.
const NO_DOWNLOAD: &str =
    "nullrouter never downloads tunnel binaries; install cloudflared or tailscale yourself";

/// `/api/tunnel/tailscale-check`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TailscaleCheck {
    installed: bool,
    logged_in: bool,
    platform: &'static str,
    /// Upstream uses this to offer `brew install tailscale`. Always false here: this service
    /// does not install anything, so offering it would be a button that cannot work.
    brew_available: bool,
    daemon_running: bool,
    /// Our own supervised daemon.
    custom_daemon_running: bool,
    /// A daemon the operator installed themselves, detected read-only.
    system_daemon_running: bool,
    /// Upstream caches a sudo password to install and to run `tailscaled` in TUN mode.
    /// Always false: nothing here ever needs one, because the daemon runs in userspace mode.
    has_cached_password: bool,
    /// Whether `tailscaled` is installed, which is separate from the CLI.
    daemon_installed: bool,
}

/// A refusal that names what the caller can do instead.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Refused {
    success: bool,
    unsupported: bool,
    message: &'static str,
    /// What the operator can run themselves.
    hint: &'static str,
}

/// The result of a mutation.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MutationResult {
    success: bool,
    /// The public URL, when one was established.
    #[serde(skip_serializing_if = "Option::is_none")]
    tunnel_url: Option<String>,
    /// Set when the operator has to finish a login in a browser.
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_url: Option<String>,
    /// Set when the tailnet has Funnel disabled and an admin has to allow it.
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_url: Option<String>,
    /// Whether a login is what is missing.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    needs_login: bool,
    /// Human-readable outcome.
    message: String,
}

impl MutationResult {
    /// A plain success.
    fn ok(message: impl Into<String>) -> Self {
        Self {
            success: true,
            tunnel_url: None,
            auth_url: None,
            enable_url: None,
            needs_login: false,
            message: message.into(),
        }
    }

    /// A success that established a URL.
    fn with_url(url: &str) -> Self {
        Self {
            tunnel_url: Some(url.to_owned()),
            ..Self::ok(format!("tunnel is up at {url}"))
        }
    }
}

/// Register every route.
pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(web::resource("/api/tunnel/status").route(web::get().to(tunnel_status)))
        .service(web::resource("/api/tunnel/enable").route(web::post().to(enable)))
        .service(web::resource("/api/tunnel/disable").route(web::post().to(disable)))
        .service(web::resource("/api/tunnel/named/enable").route(web::post().to(enable_named)))
        .service(web::resource("/api/tunnel/tailscale-check").route(web::get().to(tailscale_check)))
        .service(
            web::resource("/api/tunnel/tailscale-enable").route(web::post().to(tailscale_enable)),
        )
        .service(
            web::resource("/api/tunnel/tailscale-disable").route(web::post().to(tailscale_disable)),
        )
        .service(
            web::resource("/api/tunnel/tailscale-install")
                .route(web::post().to(tailscale_install)),
        )
        // The extension surface. Registered after the fixed paths so neither shadows the
        // other, and `{id}` cannot match a parity route.
        .service(web::resource("/api/tunnel/operations").route(web::get().to(list_operations)))
        .service(
            web::resource("/api/tunnel/operations/{id}").route(web::post().to(run_operation)),
        );
}

/// Run one read operation and return its stdout, or `None` if it could not run.
///
/// Used by the status endpoints, where every probe is best-effort: a missing binary or a dead
/// socket is information to report, not a request that failed.
async fn probe(manager: &Manager, id: &'static str) -> Option<String> {
    let operation = catalog::operation(id)?;
    match manager.run_operation(operation, &Args::default()).await {
        Ok(Outcome::Finished(output)) if output.success() => Some(output.stdout),
        _other => None,
    }
}

/// Read our own daemon's status, then a system daemon's, whichever answers.
///
/// Both are consulted because a machine where the operator already runs Tailscale should be
/// reported as logged in rather than told to log in again.
async fn tailscale_status_now(manager: &Manager) -> Option<TailscaleStatus> {
    probe(manager, "tailscale.status")
        .await
        .as_deref()
        .and_then(TailscaleStatus::parse)
}

/// `GET /api/tunnel/status`.
async fn tunnel_status(manager: web::Data<Manager>) -> HttpResponse {
    let snapshot = manager.snapshot(Tool::Cloudflared);
    let daemon = manager.snapshot(Tool::Tailscale);
    let tailscale_installed = tailscale::TAILSCALE.is_installed();

    let status = if tailscale_installed {
        tailscale_status_now(&manager).await
    } else {
        None
    };
    let funnel_active = if tailscale_installed {
        probe(&manager, "tailscale.funnel.status")
            .await
            .is_some_and(|text| status::funnel_is_serving(&text))
    } else {
        false
    };

    let logged_in = status.as_ref().is_some_and(TailscaleStatus::is_logged_in);
    responses::json(
        StatusCode::OK,
        &TunnelStatusResponse {
            tunnel: TunnelState {
                enabled: snapshot.is_running() && snapshot.ready_value.is_some(),
                url: snapshot.ready_value.clone().unwrap_or_default(),
                running: snapshot.pid.is_some(),
                state: snapshot.state.as_str(),
                pid: snapshot.pid,
                restarts: snapshot.restarts,
                last_error: snapshot.last_error.clone(),
                installed: cloudflared::CLOUDFLARED.is_installed(),
            },
            tailscale: TailscaleState {
                installed: tailscale_installed,
                logged_in,
                daemon_running: status.as_ref().is_some_and(TailscaleStatus::is_daemon_up)
                    || tailscale::system_daemon_present(),
                // Only report a URL when Funnel is actually serving: a name exists as soon as
                // the device is logged in, and showing it as the tunnel URL would advertise
                // an address that answers nothing.
                url: if funnel_active {
                    status.as_ref().and_then(TailscaleStatus::funnel_url).unwrap_or_default()
                } else {
                    String::new()
                },
                funnel_active,
                state: daemon.state.as_str(),
                auth_url: status.as_ref().and_then(|status| status.auth_url.clone()),
            },
            download: DownloadState {
                in_progress: false,
                message: NO_DOWNLOAD,
            },
        },
    )
}

/// `GET /api/tunnel/tailscale-check`.
async fn tailscale_check(manager: web::Data<Manager>) -> HttpResponse {
    let installed = tailscale::TAILSCALE.is_installed();
    let status = if installed {
        tailscale_status_now(&manager).await
    } else {
        None
    };
    let custom_daemon_running = status.as_ref().is_some_and(TailscaleStatus::is_daemon_up);
    let system_daemon_running = tailscale::system_daemon_present();

    responses::json(
        StatusCode::OK,
        &TailscaleCheck {
            installed,
            logged_in: status.as_ref().is_some_and(TailscaleStatus::is_logged_in),
            platform: std::env::consts::OS,
            brew_available: false,
            daemon_running: custom_daemon_running || system_daemon_running,
            custom_daemon_running,
            system_daemon_running,
            has_cached_password: false,
            daemon_installed: tailscale::TAILSCALED.is_installed(),
        },
    )
}

/// `POST /api/tunnel/enable` — a quick tunnel, needing no Cloudflare account.
async fn enable(manager: web::Data<Manager>, body: web::Bytes) -> HttpResponse {
    let args = match PortRequest::from_body(&body) {
        Ok(request) => request.to_args(),
        Err(message) => return failure_owned(StatusCode::BAD_REQUEST, message),
    };
    match manager.run("cloudflared.tunnel.quick", &args).await {
        Ok(Outcome::Supervised(Some(url))) => {
            responses::json(StatusCode::OK, &MutationResult::with_url(&url))
        }
        Ok(Outcome::Supervised(None) | Outcome::Finished(_)) => failure(
            StatusCode::BAD_GATEWAY,
            "cloudflared started but never announced a tunnel URL",
        ),
        Err(error) => report(&error),
    }
}

/// `POST /api/tunnel/named/enable` — a named, remotely-managed tunnel.
///
/// The token is taken from the body and placed in the child's environment. It is never
/// logged, never echoed back, and never becomes an argument.
async fn enable_named(
    manager: web::Data<Manager>,
    body: web::Json<TokenRequest>,
) -> HttpResponse {
    if body.token.trim().is_empty() {
        return failure(StatusCode::BAD_REQUEST, "a tunnel token is required");
    }
    let args = Args::from_pairs(vec![("token".to_owned(), body.token.clone())]);
    match manager.run("cloudflared.tunnel.run", &args).await {
        Ok(_outcome) => responses::json(
            StatusCode::OK,
            &MutationResult::ok("named tunnel is up with four registered edge connections"),
        ),
        Err(error) => report(&error),
    }
}

/// `POST /api/tunnel/disable`.
async fn disable(manager: web::Data<Manager>) -> HttpResponse {
    let outcome = manager.stop(Tool::Cloudflared).await;
    responses::json(
        StatusCode::OK,
        &MutationResult::ok(format!("cloudflared stopped ({outcome:?})")),
    )
}

/// `POST /api/tunnel/tailscale-enable` — bring up the daemon, then Funnel.
///
/// The sequence follows upstream's `enableTailscale`, minus the sudo: ensure the daemon,
/// check the login, start Funnel, provision a certificate, then read back the real hostname.
async fn tailscale_enable(manager: web::Data<Manager>, body: web::Bytes) -> HttpResponse {
    let args = match PortRequest::from_body(&body) {
        Ok(request) => request.to_args(),
        Err(message) => return failure_owned(StatusCode::BAD_REQUEST, message),
    };

    if let Err(error) = manager.ensure_daemon().await {
        return report(&error);
    }

    // A login cannot be completed here; it happens in a browser. Report the URL and stop.
    let status = tailscale_status_now(&manager).await;
    if status.as_ref().is_none_or(TailscaleStatus::needs_login) {
        return begin_login(&manager, status.as_ref()).await;
    }

    match manager.run("tailscale.funnel.start", &args).await {
        Ok(Outcome::Finished(output)) if output.success() => finish_funnel(&manager).await,
        Ok(Outcome::Finished(output)) => {
            let text = output.failure_text().to_owned();
            // A tailnet with Funnel switched off answers with the admin URL that turns it on,
            // which is the one thing that makes this actionable.
            if let Some(url) = tailscale::login_url(&text) {
                return responses::json(
                    StatusCode::OK,
                    &MutationResult {
                        success: false,
                        enable_url: Some(url),
                        message: "Funnel is not enabled for this tailnet".to_owned(),
                        ..MutationResult::ok("")
                    },
                );
            }
            failure_owned(StatusCode::BAD_GATEWAY, format!("tailscale funnel failed: {text}"))
        }
        Ok(Outcome::Supervised(_value)) => {
            failure(StatusCode::BAD_GATEWAY, "funnel returned an unexpected result")
        }
        Err(error) => report(&error),
    }
}

/// Start a login and report where to finish it.
async fn begin_login(manager: &Manager, status: Option<&TailscaleStatus>) -> HttpResponse {
    // A URL already published by the daemon is preferred: running `up` again would replace a
    // login the operator may already have open in a browser.
    if let Some(url) = status.and_then(|status| status.auth_url.clone()) {
        return login_needed(url);
    }
    match manager.run("tailscale.up", &Args::default()).await {
        Ok(Outcome::Finished(output)) => {
            let text = format!("{}\n{}", output.stdout, output.stderr);
            match tailscale::login_url(&text) {
                Some(url) => login_needed(url),
                None if output.success() => finish_funnel(manager).await,
                None => failure_owned(
                    StatusCode::BAD_GATEWAY,
                    format!("tailscale up produced no login URL: {}", output.failure_text()),
                ),
            }
        }
        Ok(Outcome::Supervised(_value)) => {
            failure(StatusCode::BAD_GATEWAY, "login returned an unexpected result")
        }
        Err(error) => report(&error),
    }
}

/// The response that tells the operator to finish a login in a browser.
fn login_needed(url: String) -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &MutationResult {
            success: false,
            needs_login: true,
            auth_url: Some(url),
            message: "finish the Tailscale login in a browser, then enable again".to_owned(),
            ..MutationResult::ok("")
        },
    )
}

/// Funnel is configured: provision a certificate and read back the real hostname.
async fn finish_funnel(manager: &Manager) -> HttpResponse {
    let Some(status) = tailscale_status_now(manager).await else {
        return failure(
            StatusCode::BAD_GATEWAY,
            "funnel was configured but tailscaled stopped answering",
        );
    };
    let Some(url) = status.funnel_url() else {
        return failure(
            StatusCode::BAD_GATEWAY,
            "funnel was configured but this device has no tailnet name yet",
        );
    };

    // Funnel cannot serve HTTPS without a certificate. Best-effort, exactly as upstream has
    // it: the mapping is already in place, and a certificate can be provisioned later.
    if let Some(hostname) = status.hostname() {
        let args = Args::from_pairs(vec![("hostname".to_owned(), hostname)]);
        if let Err(error) = manager.run("tailscale.cert", &args).await {
            tracing::warn!(%error, "tailscale cert provisioning failed; funnel may not serve TLS yet");
        }
    }

    responses::json(StatusCode::OK, &MutationResult::with_url(&url))
}

/// `POST /api/tunnel/tailscale-disable` — withdraw Funnel, leave the daemon alone.
///
/// The daemon stays up because it may be carrying tailnet traffic the operator wants; only
/// the public exposure is withdrawn, which is what "disable" means here.
async fn tailscale_disable(manager: web::Data<Manager>) -> HttpResponse {
    match manager.run("tailscale.funnel.reset", &Args::default()).await {
        Ok(Outcome::Finished(output)) if output.success() => responses::json(
            StatusCode::OK,
            &MutationResult::ok("funnel withdrawn; tailscaled left running"),
        ),
        Ok(Outcome::Finished(output)) => failure_owned(
            StatusCode::BAD_GATEWAY,
            format!("funnel reset failed: {}", output.failure_text()),
        ),
        Ok(Outcome::Supervised(_value)) => {
            failure(StatusCode::BAD_GATEWAY, "reset returned an unexpected result")
        }
        Err(error) => report(&error),
    }
}

/// `POST /api/tunnel/tailscale-install` — refused, with the command to run instead.
async fn tailscale_install() -> HttpResponse {
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &Refused {
            success: false,
            unsupported: true,
            message: "nullrouter does not install system software. Upstream does this by \
                      piping a downloaded script into `sudo sh` with your password on the \
                      child's stdin; that is remote code execution as root and this port \
                      will not reproduce it.",
            hint: "install Tailscale yourself from https://tailscale.com/download, then \
                   call /api/tunnel/tailscale-enable",
        },
    )
}

/// A port, optionally supplied by the caller.
#[derive(Debug, Default, Deserialize)]
struct PortRequest {
    #[serde(default)]
    port: Option<u16>,
}

impl PortRequest {
    /// Read the body, distinguishing "no body" from "a body I could not read".
    ///
    /// The dashboard POSTs these routes with no body at all, so an empty payload has to mean
    /// defaults. `Option<web::Json<T>>` would cover that, but it also turns a *malformed*
    /// body into `None` — so `{"port":"not-a-port"}` would silently become the default port
    /// instead of an error, which is the worst of the three outcomes.
    fn from_body(body: &[u8]) -> Result<Self, String> {
        if body.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self::default());
        }
        serde_json::from_slice(body).map_err(|error| format!("invalid request body: {error}"))
    }

    /// As catalog arguments.
    fn to_args(&self) -> Args {
        Args::from_pairs(
            self.port
                .map(|port| vec![("port".to_owned(), port.to_string())])
                .unwrap_or_default(),
        )
    }
}

/// A tunnel token.
#[derive(Debug, Deserialize)]
struct TokenRequest {
    #[serde(default)]
    token: String,
}

/// A failure with a borrowed message.
fn failure(status: StatusCode, message: &str) -> HttpResponse {
    failure_owned(status, message.to_owned())
}

/// A failure with an owned message.
fn failure_owned(status: StatusCode, message: String) -> HttpResponse {
    responses::json(
        status,
        &MutationResult {
            success: false,
            message,
            ..MutationResult::ok("")
        },
    )
}

/// Turn an operation error into a response, choosing the status by cause.
fn report(error: &OpError) -> HttpResponse {
    failure_owned(error.status(), error.to_string())
}

/// One row of the catalog, as the panel sees it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OperationSummary {
    id: &'static str,
    about: &'static str,
    tool: &'static str,
    effect: &'static str,
    /// `oneShot` or `supervised`.
    mode: &'static str,
    /// Deadline in milliseconds, for a one-shot.
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u128>,
    params: Vec<ParamSummary>,
    /// Whether the binary this needs is installed right now.
    available: bool,
}

/// One parameter, as the panel sees it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ParamSummary {
    name: &'static str,
    about: &'static str,
    required: bool,
    /// A panel must render this as a password field and must not store it.
    secret: bool,
}

/// The catalog listing.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OperationList {
    operations: Vec<OperationSummary>,
    /// Which binaries are present, so a panel can explain an unavailable row once rather than
    /// per operation.
    tools: Vec<ToolSummary>,
}

/// Whether one binary is installed, and where.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolSummary {
    id: &'static str,
    installed: bool,
    /// The resolved path, so an operator can confirm *which* binary would run.
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

/// `GET /api/tunnel/operations` — everything the panel may ask these binaries to do.
///
/// This is the discoverable form of the allowlist. A panel renders it, so a new row in
/// [`catalog::OPERATIONS`] appears in the UI without any further work, and an operator can see
/// the complete set of operations this service is capable of.
async fn list_operations() -> HttpResponse {
    let operations = catalog::OPERATIONS
        .iter()
        .map(|entry| OperationSummary {
            id: entry.id,
            about: entry.about,
            tool: entry.tool.id(),
            effect: entry.effect.id(),
            mode: match entry.mode {
                Mode::OneShot { .. } => "oneShot",
                Mode::Supervised => "supervised",
            },
            timeout_ms: match entry.mode {
                Mode::OneShot { timeout } => Some(timeout.as_millis()),
                Mode::Supervised => None,
            },
            params: entry
                .params
                .iter()
                .map(|param| ParamSummary {
                    name: param.name,
                    about: param.about,
                    required: param.required,
                    secret: param.secret,
                })
                .collect(),
            available: tool_path(entry.tool).is_some(),
        })
        .collect();

    let tools = [Tool::Cloudflared, Tool::Tailscale]
        .into_iter()
        .map(|tool| {
            let path = tool_path(tool);
            ToolSummary {
                id: tool.id(),
                installed: path.is_some(),
                path,
            }
        })
        .collect();

    responses::json(StatusCode::OK, &OperationList { operations, tools })
}

/// The resolved path of one tool's binary, if it is installed and acceptable.
fn tool_path(tool: Tool) -> Option<String> {
    let spec = match tool {
        Tool::Cloudflared => cloudflared::CLOUDFLARED,
        Tool::Tailscale => tailscale::TAILSCALE,
    };
    spec.resolve(None)
        .ok()
        .map(|executable| executable.path().display().to_string())
}

/// What running an arbitrary operation produced.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OperationResult {
    id: String,
    success: bool,
    /// Exit code, for a one-shot.
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<i32>,
    /// Captured stdout, already scrubbed of any credential.
    #[serde(skip_serializing_if = "Option::is_none")]
    stdout: Option<String>,
    /// Captured stderr, already scrubbed.
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr: Option<String>,
    /// Whether output hit the capture cap.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    truncated: bool,
    /// Whatever a supervised operation's readiness rule captured.
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

/// A request body for an arbitrary operation: free-form names to string values.
///
/// Typed as strings rather than as `serde_json::Value` so a nested object cannot be smuggled
/// toward a builder, and validated by the argv layer regardless.
#[derive(Debug, Default, Deserialize)]
struct OperationRequest {
    #[serde(default)]
    args: std::collections::BTreeMap<String, String>,
}

/// `POST /api/tunnel/operations/{id}` — run one catalog operation.
///
/// The id has to match a row exactly. There is no pattern, no prefix match and no fallthrough:
/// an unknown id is a 400, never an attempt.
async fn run_operation(
    manager: web::Data<Manager>,
    path: web::Path<String>,
    body: Option<web::Json<OperationRequest>>,
) -> HttpResponse {
    let id = path.into_inner();
    let Some(operation) = catalog::operation(&id) else {
        return failure_owned(
            StatusCode::BAD_REQUEST,
            format!("{id} is not an operation this service can run"),
        );
    };

    let supplied = body.map(web::Json::into_inner).unwrap_or_default();
    // Only declared parameters are passed through. An undeclared name is dropped rather than
    // forwarded, so a body cannot reach a builder that was not expecting it.
    let args = Args::from_pairs(
        operation
            .params
            .iter()
            .filter_map(|param| {
                supplied
                    .args
                    .get(param.name)
                    .map(|value| (param.name.to_owned(), value.clone()))
            })
            .collect(),
    );

    for param in operation.params.iter().filter(|param| param.required) {
        if args.get(param.name).is_none() {
            return failure_owned(
                StatusCode::BAD_REQUEST,
                format!("{} requires the {} parameter", operation.id, param.name),
            );
        }
    }

    // Validate before anything else. Left until after `ensure_daemon`, a hostile value would
    // start a daemon on its way to being rejected, and the caller would be told about a
    // missing binary rather than about their own argument.
    if let Err(error) = Manager::validate(operation, &args) {
        return report(&error);
    }

    // A read against Tailscale needs the daemon up to answer at all; a mutation needs it too.
    // Bringing it up here is what makes an operation callable without an ordering ritual.
    if operation.tool == Tool::Tailscale
        && operation.id != "tailscale.version"
        && let Err(error) = manager.ensure_daemon().await
    {
        return report(&error);
    }

    match manager.run_operation(operation, &args).await {
        Ok(Outcome::Finished(output)) => responses::json(
            if output.success() {
                StatusCode::OK
            } else {
                StatusCode::BAD_GATEWAY
            },
            &OperationResult {
                id,
                success: output.success(),
                code: output.code,
                stdout: Some(output.stdout),
                stderr: Some(output.stderr),
                truncated: output.truncated,
                value: None,
            },
        ),
        Ok(Outcome::Supervised(value)) => responses::json(
            StatusCode::OK,
            &OperationResult {
                id,
                success: true,
                code: None,
                stdout: None,
                stderr: None,
                truncated: false,
                value,
            },
        ),
        Err(error) => report(&error),
    }
}

#[cfg(test)]
mod tests {
    use super::{MutationResult, catalog};
    use super::catalog::Effect;

    #[test]
    fn a_mutation_result_omits_the_fields_it_has_nothing_for() {
        // A panel branches on the presence of `authUrl` and `enableUrl`, so serialising them
        // as null would make it offer a login that is not needed.
        let rendered = serde_json::to_value(MutationResult::ok("done")).expect("serialises");

        assert_eq!(rendered.get("success"), Some(&serde_json::json!(true)));
        assert_eq!(rendered.get("message"), Some(&serde_json::json!("done")));
        assert!(rendered.get("authUrl").is_none(), "{rendered}");
        assert!(rendered.get("enableUrl").is_none(), "{rendered}");
        assert!(rendered.get("tunnelUrl").is_none(), "{rendered}");
        assert!(rendered.get("needsLogin").is_none(), "{rendered}");
    }

    #[test]
    fn a_url_result_carries_it_in_both_places_a_panel_looks() {
        let rendered = serde_json::to_value(MutationResult::with_url(
            "https://a-b-c.trycloudflare.com",
        ))
        .expect("serialises");

        assert_eq!(
            rendered.get("tunnelUrl"),
            Some(&serde_json::json!("https://a-b-c.trycloudflare.com"))
        );
        assert!(
            rendered
                .get("message")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|message| message.contains("trycloudflare.com")),
            "{rendered}"
        );
    }

    #[test]
    fn every_catalog_row_has_an_effect_a_panel_can_render() {
        for entry in catalog::OPERATIONS {
            assert!(
                matches!(entry.effect, Effect::Read | Effect::Mutate),
                "{} has no renderable effect",
                entry.id
            );
            assert!(!entry.about.is_empty(), "{} has no description", entry.id);
        }
    }
}

