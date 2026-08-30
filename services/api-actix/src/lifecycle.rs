//! Shutting the service down, and reporting what version is running.
//!
//! Ports `POST /api/shutdown`, `POST /api/version/shutdown` and `POST /api/version/update`.
//!
//! Two things here are deliberately unlike upstream, and both are about not misleading whoever
//! pressed the button.
//!
//! **Shutdown stops one service, not the router.** Upstream is a single process, so its
//! shutdown ends everything. This port runs eight, and this route can only stop the one it
//! lives in — nothing here supervises the others. A user who clicks "shut down" and is told
//! `success: true` would reasonably believe their router had stopped, while the gateway carried
//! on serving `/v1` and spending their provider credits. So the response says exactly which
//! service is stopping and which ports are still listening, and [`Shutdown`] refuses outright
//! unless a secret is configured.
//!
//! **Self-replacing updates are refused.** Upstream's updater spawns a detached process that
//! overwrites the binary and exits. This port does not own its binary — it is built and placed
//! by whoever deployed it — and a router that rewrites itself out from under a package manager
//! is a worse outcome than one that tells you to run your package manager.

use std::time::Duration;

use actix_web::{HttpRequest, HttpResponse, http::StatusCode, web};
use serde::Serialize;

use crate::responses;

/// How long to wait before stopping, so the reply reaches the caller first.
///
/// Upstream uses 500ms for the same reason. Stopping the server inside the handler would
/// close the connection the reply was travelling on, and the dashboard would show a network
/// error for a shutdown that in fact succeeded.
const SHUTDOWN_DELAY: Duration = Duration::from_millis(500);

/// The env var holding the shutdown secret.
///
/// Upstream reads `SHUTDOWN_SECRET` and additionally refuses outright when `NODE_ENV` is
/// production. This port has no equivalent of that flag, so the secret is the whole gate:
/// unset means the route is disabled. That is at least as strict as upstream in every
/// configuration — upstream with no secret also refuses — and it avoids inventing a
/// production-detection heuristic that would be wrong somewhere.
const SHUTDOWN_SECRET_VAR: &str = "NULLROUTER_SHUTDOWN_SECRET";

/// The other services' default ports, for the "still running" report.
///
/// Named rather than probed from a registry because there is no registry: each service is its
/// own binary with its own port variable. Reported as *configured* defaults, and each is
/// probed before being listed, so a non-default deployment under-reports rather than claiming
/// something is up that is not.
const SIBLING_SERVICES: [(&str, u16); 7] = [
    ("nullrouter-gateway", 20128),
    ("nullrouter-dashboard-host", 20130),
    ("nullrouter-catalog", 20131),
    ("nullrouter-runtime", 20132),
    ("nullrouter-events", 20133),
    ("nullrouter-state", 20134),
    ("nullrouter-auth", 20135),
];

#[derive(Debug, Clone, Serialize)]
struct NoopLifecycleResponse {
    success: bool,
    unsupported: bool,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShutdownResponse {
    success: bool,
    message: String,
    /// The service this route can actually stop.
    stopping: &'static str,
    /// Services that will keep running, because nothing here supervises them.
    ///
    /// The point of the whole response. `/v1` is served by the gateway and the runtime, so a
    /// caller seeing these listed knows their router is still live.
    still_running: Vec<&'static str>,
    /// Set when `still_running` is non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(web::resource("/api/shutdown").route(web::post().to(shutdown)))
        .service(web::resource("/api/version/update").route(web::post().to(update)))
        .service(web::resource("/api/version/shutdown").route(web::post().to(version_shutdown)));
}

/// A handle that can stop this server.
///
/// Held as app data so the handler can reach it, and behind a `OnceLock` because of an
/// ordering problem: `HttpServer::new` takes the closure that registers app data, but the
/// handle only exists once `run()` has been called on the built server. So `main` registers an
/// empty one, then fills it in.
///
/// A route that finds it unfilled reports that it cannot stop anything, rather than claiming
/// success — which is the whole point of not defaulting this to a no-op.
#[derive(Clone, Default)]
pub struct ShutdownHandle {
    handle: std::sync::Arc<std::sync::OnceLock<actix_web::dev::ServerHandle>>,
}

impl ShutdownHandle {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Supply the handle once the server exists. Later calls are ignored.
    pub fn set(&self, handle: actix_web::dev::ServerHandle) {
        let _ = self.handle.set(handle);
    }

    fn get(&self) -> Option<&actix_web::dev::ServerHandle> {
        self.handle.get()
    }
}

impl std::fmt::Debug for ShutdownHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ShutdownHandle")
    }
}

/// Is the caller authorised to stop this service?
fn authorised(request: &HttpRequest) -> Result<(), HttpResponse> {
    let Ok(secret) = std::env::var(SHUTDOWN_SECRET_VAR) else {
        return Err(responses::json(
            StatusCode::FORBIDDEN,
            &NoopLifecycleResponse {
                success: false,
                unsupported: false,
                message: format!(
                    "Shutdown is disabled: set {SHUTDOWN_SECRET_VAR} and send it as a bearer \
                     token to enable it"
                ),
            },
        ));
    };
    if secret.trim().is_empty() {
        return Err(responses::json(
            StatusCode::FORBIDDEN,
            &NoopLifecycleResponse {
                success: false,
                unsupported: false,
                message: format!("Shutdown is disabled: {SHUTDOWN_SECRET_VAR} is empty"),
            },
        ));
    }

    let presented = request
        .headers()
        .get(actix_web::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
        .trim();

    // Constant-time comparison: this is a shared secret presented over a local socket, and a
    // length-or-prefix leak is avoidable for free.
    if presented.len() == secret.trim().len()
        && presented
            .bytes()
            .zip(secret.trim().bytes())
            .fold(0_u8, |differences, (left, right)| differences | (left ^ right))
            == 0
    {
        Ok(())
    } else {
        Err(responses::json(
            StatusCode::UNAUTHORIZED,
            &NoopLifecycleResponse {
                success: false,
                unsupported: false,
                message: "Unauthorized".to_owned(),
            },
        ))
    }
}

/// How long to wait for a sibling to accept a connection before calling it absent.
///
/// Loopback, so this is generous. Under-reporting a live service is the failure that matters —
/// it would tell a user their router had stopped when it had not — but the probes run
/// concurrently, so the whole check costs one timeout rather than seven.
const PROBE_TIMEOUT: Duration = Duration::from_millis(150);

/// Which sibling services are listening right now.
///
/// Async and concurrent on purpose: the blocking `std` connect would hold an actix worker for up
/// to seven timeouts, and this runs on the shutdown path where the worker pool is about to
/// shrink to nothing.
async fn siblings_still_running() -> Vec<&'static str> {
    let probes = SIBLING_SERVICES.map(|(name, port)| async move {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let connected = actix_web::rt::time::timeout(
            PROBE_TIMEOUT,
            actix_web::rt::net::TcpStream::connect(address),
        )
        .await
        .is_ok_and(|result| result.is_ok());
        connected.then_some(name)
    });
    futures_util::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
        .collect()
}

/// Stop this service after the reply has been sent.
///
/// `false` when there is no handle to stop with, so the caller reports that rather than
/// claiming a shutdown that will not happen.
fn schedule_stop(handle: Option<&web::Data<ShutdownHandle>>, reason: &str) -> bool {
    let Some(handle) = handle.and_then(|data| data.get()) else {
        return false;
    };
    let handle = handle.clone();
    let reason = reason.to_owned();
    actix_web::rt::spawn(async move {
        actix_web::rt::time::sleep(SHUTDOWN_DELAY).await;
        tracing::info!(%reason, "stopping nullrouter-api");
        // `true` drains in-flight requests rather than cutting them off, which is the
        // difference between this and upstream's `process.exit(0)`.
        handle.stop(true).await;
    });
    true
}

async fn shutdown_reply(message: &str) -> HttpResponse {
    let still_running = siblings_still_running().await;
    let warning = if still_running.is_empty() {
        None
    } else {
        Some(format!(
            "Only nullrouter-api is stopping. {} still listening, so /v1 keeps serving \
             requests and provider credits can still be spent. Stop the remaining services \
             through whatever supervises them.",
            still_running.len()
        ))
    };
    responses::json(
        StatusCode::OK,
        &ShutdownResponse {
            success: true,
            message: message.to_owned(),
            stopping: "nullrouter-api",
            still_running,
            warning,
        },
    )
}

async fn shutdown(
    request: HttpRequest,
    handle: Option<web::Data<ShutdownHandle>>,
) -> HttpResponse {
    if let Err(refusal) = authorised(&request) {
        return refusal;
    }
    if !schedule_stop(handle.as_ref(), "POST /api/shutdown") {
        // No handle registered means nothing here can stop the process. Reporting success
        // would be the fake-completion this route exists to avoid.
        return responses::json(
            StatusCode::NOT_IMPLEMENTED,
            &NoopLifecycleResponse {
                success: false,
                unsupported: true,
                message: "This nullrouter-api was started without a shutdown handle, so it \
                          cannot stop itself"
                    .to_owned(),
            },
        );
    }
    shutdown_reply("Shutting down nullrouter-api…").await
}

/// Upstream's "stop so the files can be replaced by hand" route.
///
/// Same mechanism, same gate. Upstream also kills sibling processes to release Windows file
/// locks; nothing here does that, because nothing here started them.
async fn version_shutdown(
    request: HttpRequest,
    handle: Option<web::Data<ShutdownHandle>>,
) -> HttpResponse {
    if let Err(refusal) = authorised(&request) {
        return refusal;
    }
    if !schedule_stop(handle.as_ref(), "POST /api/version/shutdown") {
        return responses::json(
            StatusCode::NOT_IMPLEMENTED,
            &NoopLifecycleResponse {
                success: false,
                unsupported: true,
                message: "This nullrouter-api was started without a shutdown handle, so it \
                          cannot stop itself"
                    .to_owned(),
            },
        );
    }
    shutdown_reply("Shutting down nullrouter-api for a manual update…").await
}

/// Refused, with the reason.
///
/// Upstream spawns a detached updater that overwrites the binary and exits. This port does not
/// own its binary: it was built and placed by whoever deployed it, quite possibly a package
/// manager or an image build that would be silently defeated by a self-replacement. Reporting
/// success and doing nothing would be worse than refusing, and actually replacing the binary
/// is not this program's business.
///
/// `GET /api/version` reports the compiled version, and reports `latestVersion: null` rather
/// than claiming to be up to date — no update channel is checked, and saying "no update
/// available" without looking would be a claim this port cannot support.
async fn update(config: web::Data<crate::AppConfig>) -> HttpResponse {
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &serde_json::json!({
            "success": false,
            "unsupported": true,
            "currentVersion": config.version,
            "message": format!(
                "nullrouter {} does not replace its own binary. It is installed and updated by \
                 whatever placed it — a package manager, an image build, or `cargo install` — \
                 and rewriting itself would silently defeat that. Update through the same route \
                 you installed by. POST /api/version/shutdown stops this service so files can \
                 be replaced, if a secret is configured.",
                config.version
            ),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::{SHUTDOWN_SECRET_VAR, SIBLING_SERVICES};

    #[test]
    fn every_sibling_service_is_named_and_has_a_distinct_port() {
        // A duplicated port would silently under-report which services are still running,
        // which is the one thing the shutdown response exists to get right.
        let mut ports: Vec<u16> = SIBLING_SERVICES.iter().map(|(_, port)| *port).collect();
        ports.sort_unstable();
        let before = ports.len();
        ports.dedup();
        assert_eq!(before, ports.len(), "duplicate port in SIBLING_SERVICES");

        for (name, port) in SIBLING_SERVICES {
            assert!(name.starts_with("nullrouter-"), "odd service name {name}");
            assert!((20128..=20135).contains(&port), "{name} port {port} is outside the range");
        }
    }

    #[test]
    fn the_api_service_is_not_listed_as_a_sibling() {
        // 20129 is this service. Listing itself as "still running" while it shuts down would
        // be both wrong and confusing.
        assert!(
            !SIBLING_SERVICES.iter().any(|(_, port)| *port == 20129),
            "nullrouter-api should not be in its own sibling list"
        );
    }

    #[test]
    fn the_secret_variable_is_namespaced() {
        // Upstream reads a bare `SHUTDOWN_SECRET`. Namespacing avoids a variable set for some
        // other program on the same box enabling this route by accident.
        assert!(SHUTDOWN_SECRET_VAR.starts_with("NULLROUTER_"));
    }
}
