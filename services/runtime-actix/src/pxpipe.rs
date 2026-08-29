//! PXPIPE control, on the service that owns the transform.
//!
//! The eight `/api/pxpipe/*` routes a dashboard calls live in `nullrouter-api`, but
//! the worker cannot: the transform runs on the request path, so the process holding
//! it has to be this one. A worker in `nullrouter-api` would report itself as running
//! while every request here bypassed — the status page and the router disagreeing
//! about whether compression is happening, which is worse than either being wrong.
//!
//! So the loaded state lives here and `nullrouter-api` proxies to these routes. They
//! are `/internal/*`, which the gateway refuses from outside (pinned by
//! `internal_paths_are_not_publicly_routable`), so the only way in is from another
//! service on the loopback.
//!
//! Reads that need no worker — the event log and its aggregates — are served directly
//! by `nullrouter-api` from the same files, because proxying a file read would add a
//! hop for nothing.

use actix_web::{HttpResponse, http::StatusCode, web};
use nullrouter_pxpipe::bridge::StartError;
use serde::Serialize;

use crate::{Runtime, handlers, responses};

pub(crate) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/internal/pxpipe/status")
                .route(web::get().to(status))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/internal/pxpipe/health")
                .route(web::get().to(health))
                .route(web::post().to(health))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/internal/pxpipe/start")
                .route(web::post().to(start))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/internal/pxpipe/stop")
                .route(web::post().to(stop))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/internal/pxpipe/restart")
                .route(web::post().to(restart))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        );
}

async fn no_content() -> HttpResponse {
    handlers::no_content().await
}

/// The install and worker state.
async fn status(runtime: web::Data<Runtime>) -> HttpResponse {
    responses::json(StatusCode::OK, &runtime.token_saver().status().await)
}

/// Installed, then loads, then transforms.
async fn health(runtime: web::Data<Runtime>) -> HttpResponse {
    responses::json(StatusCode::OK, &runtime.token_saver().health().await)
}

/// Warm the worker.
///
/// Auto-install is honoured here rather than in `nullrouter-api`, because the setting
/// that governs it is read here anyway and the worker being warmed is this one.
async fn start(runtime: web::Data<Runtime>) -> HttpResponse {
    let saver = runtime.token_saver().clone();
    if !saver.install_info().installed {
        let settings = runtime.state_client().routing_context().await.settings;
        if !settings.pxpipe_auto_install {
            return refused(
                StatusCode::CONFLICT,
                "NOT_INSTALLED",
                "PXPIPE is not installed, and automatic installation is turned off",
            );
        }
        let installer = saver.clone();
        // npm is a subprocess with a five-minute budget; never on a worker thread.
        match actix_web::rt::task::spawn_blocking(move || installer.install()).await {
            Ok(nullrouter_pxpipe::InstallOutcome::Installed(_)) => {}
            Ok(nullrouter_pxpipe::InstallOutcome::NpmMissing) => {
                return refused(
                    StatusCode::CONFLICT,
                    "NPM_MISSING",
                    "PXPIPE is not installed and this host has no npm to install it with",
                );
            }
            Ok(nullrouter_pxpipe::InstallOutcome::Failed { message }) => {
                return refused_owned(StatusCode::BAD_GATEWAY, "INSTALL_FAILED", message);
            }
            Err(error) => {
                tracing::warn!(%error, "pxpipe install task failed");
                return refused(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "INSTALL_FAILED",
                    "The install could not be run",
                );
            }
        }
    }
    match saver.start().await {
        Ok(_) => responses::json(StatusCode::OK, &saver.status().await),
        Err(error) => start_failure(&error),
    }
}

/// Drop the worker. Requests then bypass rather than fail.
async fn stop(runtime: web::Data<Runtime>) -> HttpResponse {
    let saver = runtime.token_saver();
    let stopped = saver.stop().await;
    responses::json(
        StatusCode::OK,
        &StopReport {
            stopped,
            status: saver.status().await,
        },
    )
}

/// Reload, so an upgraded install takes effect without restarting the service.
async fn restart(runtime: web::Data<Runtime>) -> HttpResponse {
    let saver = runtime.token_saver();
    match saver.restart().await {
        Ok(_) => responses::json(StatusCode::OK, &saver.status().await),
        Err(error) => start_failure(&error),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StopReport {
    stopped: bool,
    #[serde(flatten)]
    status: nullrouter_pxpipe::Status,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Refusal<'a> {
    success: bool,
    code: &'a str,
    error: &'a str,
}

fn refused(status: StatusCode, code: &'static str, error: &'static str) -> HttpResponse {
    responses::json(
        status,
        &Refusal {
            success: false,
            code,
            error,
        },
    )
}

fn refused_owned(status: StatusCode, code: &'static str, error: String) -> HttpResponse {
    responses::json(
        status,
        &Refusal {
            success: false,
            code,
            error: &error,
        },
    )
}

/// A failed load, with the status code that matches the cause.
///
/// Not all one code: a missing install is the caller's to fix by installing, an
/// unsupported Node is the host's to fix by upgrading, and a broken package is
/// neither. Collapsing them into one 500 would hide which.
fn start_failure(error: &StartError) -> HttpResponse {
    let status = match error {
        StartError::NotInstalled => StatusCode::CONFLICT,
        StartError::NodeMissing | StartError::UnsupportedNode(_) => StatusCode::CONFLICT,
        StartError::Failed(_) => StatusCode::BAD_GATEWAY,
    };
    refused_owned(status, error.code(), error.to_string())
}
