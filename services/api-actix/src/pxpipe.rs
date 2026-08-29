//! `/api/pxpipe/*`: the eight routes behind the Token Saver page.
//!
//! Ports `inspire/src/app/api/pxpipe/*/route.js`.
//!
//! The work is split by what each route needs. `stats` and `logs` read files, so they
//! are answered here directly. `status`, `health`, `start`, `stop` and `restart` need
//! the loaded worker, which lives in `nullrouter-runtime` because that is where the
//! transform runs — so those five proxy to `/internal/pxpipe/*` there. A worker in
//! this service would report itself running while every request bypassed.
//!
//! `install` is the exception in both directions: the install is a filesystem
//! operation this service can do, but a running worker holds the *old* module
//! afterwards, so it installs here and then asks the runtime to reload — which is
//! exactly what upstream's `unloadPxpipe()` after install achieves in-process.
//!
//! Every route is behind the dashboard session, as upstream's are. `install` runs
//! `npm install pxpipe-proxy@latest`, whose lifecycle scripts execute as this service
//! does; the package name is fixed and never taken from the request, so it is not an
//! arbitrary-code path, but it is a real install and is documented as one rather than
//! quietly performed.

use actix_web::{HttpResponse, http::StatusCode, web};
use nullrouter_pxpipe::{InstallOutcome, TokenSaver};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{responses, state_client::RuntimeClient};

/// Upstream's cap on how many events a single read returns.
const MAX_LIMIT: usize = 500;
const DEFAULT_LIMIT: usize = 100;

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/pxpipe/status")
                .route(web::get().to(status))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/api/pxpipe/health")
                // GET mirrors POST so the card can probe on load without a mutation,
                // as upstream's `export const GET = POST` does.
                .route(web::get().to(health))
                .route(web::post().to(health))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/api/pxpipe/install")
                .route(web::post().to(install))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/api/pxpipe/logs")
                .route(web::get().to(logs))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/api/pxpipe/stats")
                .route(web::get().to(stats))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/api/pxpipe/start")
                .route(web::post().to(start))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/api/pxpipe/stop")
                .route(web::post().to(stop))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/api/pxpipe/restart")
                .route(web::post().to(restart))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        );
}

async fn no_content() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}

/// How many events to return, clamped as upstream clamps.
#[derive(Debug, Deserialize)]
struct LimitQuery {
    #[serde(default)]
    limit: Option<u32>,
}

impl LimitQuery {
    fn resolved(&self) -> usize {
        self.limit
            .map_or(DEFAULT_LIMIT, |limit| {
                usize::try_from(limit).unwrap_or(DEFAULT_LIMIT)
            })
            .clamp(1, MAX_LIMIT)
    }
}

/// The install and worker state, plus the settings that govern it.
///
/// Upstream merges the settings into this response, and the Token Saver page reads
/// them from here rather than from `/api/settings`, so the merge is kept.
async fn status(runtime: web::Data<RuntimeClient>, saver: web::Data<TokenSaver>) -> HttpResponse {
    match runtime.pxpipe_get("status").await {
        Some(forwarded) => forwarded.into_response(),
        // The runtime being unreachable is reported as that. Answering from this
        // service's own install state would say "installed, not running" for a router
        // whose worker may be running perfectly — the status would be about the wrong
        // process.
        None => runtime_unreachable(&saver).await,
    }
}

/// Installed → loads → transforms.
async fn health(runtime: web::Data<RuntimeClient>, saver: web::Data<TokenSaver>) -> HttpResponse {
    match runtime.pxpipe_post("health", &[]).await {
        Some(forwarded) => forwarded.into_response(),
        None => responses::json(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({
                "healthy": false,
                "checks": [{
                    "id": "runtime",
                    "label": "Runtime service reachable",
                    "ok": false,
                    "detail": "The runtime service holds the transform and could not be reached",
                }],
                "error": "The runtime service is unreachable, so the token saver cannot be checked",
                "installed": saver.install_info().installed,
            }),
        ),
    }
}

/// Install or repair, then reload the runtime's worker and re-check health.
///
/// Blocking work moved off the worker thread: `npm install` on a cold cache
/// legitimately takes minutes, and parking an Actix worker on it would stall
/// unrelated requests.
async fn install(runtime: web::Data<RuntimeClient>, saver: web::Data<TokenSaver>) -> HttpResponse {
    let installer = saver.get_ref().clone();
    let outcome = match actix_web::rt::task::spawn_blocking(move || installer.install()).await {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::warn!(%error, "pxpipe install task failed");
            return responses::json(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({
                    "success": false,
                    "code": "INSTALL_FAILED",
                    "error": "The install could not be run",
                }),
            );
        }
    };

    let info = match outcome {
        InstallOutcome::Installed(info) => info,
        InstallOutcome::NpmMissing => {
            return responses::json(
                StatusCode::CONFLICT,
                &json!({
                    "success": false,
                    "code": "NPM_MISSING",
                    "error": "This host has no npm, so the token saver cannot be installed here",
                }),
            );
        }
        InstallOutcome::Failed { message } => {
            return responses::json(
                StatusCode::BAD_GATEWAY,
                &json!({
                    "success": false,
                    "code": "INSTALL_FAILED",
                    "error": message,
                    "installLog": saver.install_log_tail(),
                }),
            );
        }
    };

    // A worker started before this install holds the previous version. Reloading is
    // what makes "Repair" mean anything without a service restart.
    let health = match runtime.pxpipe_post("restart", &[]).await {
        Some(forwarded) => forwarded.json().unwrap_or_else(
            || json!({ "healthy": false, "error": "The runtime service gave an unreadable reply" }),
        ),
        None => json!({
            "healthy": false,
            "error": "Installed, but the runtime service could not be asked to load it",
        }),
    };

    responses::json(
        StatusCode::OK,
        &InstallReport {
            success: true,
            installed: info.installed,
            version: info.version,
            path: info.path,
            requires_node: info.requires_node,
            health,
        },
    )
}

/// The install log and the recent events.
async fn logs(saver: web::Data<TokenSaver>, query: web::Query<LimitQuery>) -> HttpResponse {
    responses::json(StatusCode::OK, &saver.logs(query.resolved()).await)
}

/// The aggregates behind the savings chart.
async fn stats(saver: web::Data<TokenSaver>, query: web::Query<LimitQuery>) -> HttpResponse {
    responses::json(StatusCode::OK, &saver.stats(query.resolved()))
}

async fn start(runtime: web::Data<RuntimeClient>, saver: web::Data<TokenSaver>) -> HttpResponse {
    forwarded_control(&runtime, &saver, "start").await
}

async fn stop(runtime: web::Data<RuntimeClient>, saver: web::Data<TokenSaver>) -> HttpResponse {
    forwarded_control(&runtime, &saver, "stop").await
}

async fn restart(runtime: web::Data<RuntimeClient>, saver: web::Data<TokenSaver>) -> HttpResponse {
    forwarded_control(&runtime, &saver, "restart").await
}

/// Relay one control action to the runtime, preserving its status and its message.
///
/// The runtime's own status code is kept rather than flattened: a 409 for "not
/// installed" and a 502 for "the package will not load" call for different actions
/// from the user, and turning both into 500 would hide which.
async fn forwarded_control(
    runtime: &RuntimeClient,
    saver: &TokenSaver,
    action: &str,
) -> HttpResponse {
    match runtime.pxpipe_post(action, &[]).await {
        Some(forwarded) => forwarded.into_response(),
        None => runtime_unreachable(saver).await,
    }
}

/// What to say when the service holding the worker cannot be reached.
///
/// Reports the install state, which this service can see, and says plainly that the
/// running state is unknown. `running: false` would be a claim about a process this
/// service never contacted.
async fn runtime_unreachable(saver: &TokenSaver) -> HttpResponse {
    let install = saver.install_info();
    responses::json(
        StatusCode::SERVICE_UNAVAILABLE,
        &json!({
            "success": false,
            "code": "RUNTIME_UNREACHABLE",
            "error": "The runtime service holds the transform and could not be reached, \
                      so the token saver's running state is unknown",
            "installed": install.installed,
            "version": install.version,
            "path": install.path,
            "requiresNode": install.requires_node,
            "npmAvailable": nullrouter_pxpipe::install::find_npm().is_some(),
            "nodeAvailable": nullrouter_pxpipe::install::find_node().is_some(),
            "mode": "worker",
        }),
    )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallReport {
    success: bool,
    installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requires_node: Option<String>,
    health: Value,
}
