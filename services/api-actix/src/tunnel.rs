use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Serialize;

use crate::responses;

const TUNNEL_UNSUPPORTED: &str = "Tunnel control is not supported by nullrouter-api";

#[derive(Debug, Clone, Copy, Serialize)]
struct TunnelStatusResponse {
    tunnel: TunnelState,
    tailscale: TailscaleState,
    download: DownloadState,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct TunnelState {
    enabled: bool,
    url: &'static str,
    running: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct TailscaleState {
    installed: bool,
    logged_in: bool,
    daemon_running: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadState {
    in_progress: bool,
    message: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct TailscaleCheck {
    installed: bool,
    logged_in: bool,
    platform: &'static str,
    brew_available: bool,
    daemon_running: bool,
    custom_daemon_running: bool,
    system_daemon_running: bool,
    has_cached_password: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct UnsupportedMutation {
    success: bool,
    unsupported: bool,
    message: &'static str,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(web::resource("/api/tunnel/status").route(web::get().to(status)))
        .service(web::resource("/api/tunnel/tailscale-check").route(web::get().to(tailscale_check)))
        .service(web::resource("/api/tunnel/enable").route(web::post().to(unsupported_mutation)))
        .service(web::resource("/api/tunnel/disable").route(web::post().to(unsupported_mutation)))
        .service(
            web::resource("/api/tunnel/tailscale-enable")
                .route(web::post().to(unsupported_mutation)),
        )
        .service(
            web::resource("/api/tunnel/tailscale-disable")
                .route(web::post().to(unsupported_mutation)),
        )
        .service(
            web::resource("/api/tunnel/tailscale-install")
                .route(web::post().to(unsupported_mutation)),
        );
}

async fn status() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &TunnelStatusResponse {
            tunnel: TunnelState {
                enabled: false,
                url: "",
                running: false,
            },
            tailscale: TailscaleState {
                installed: false,
                logged_in: false,
                daemon_running: false,
            },
            download: DownloadState {
                in_progress: false,
                message: TUNNEL_UNSUPPORTED,
            },
        },
    )
}

async fn tailscale_check() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &TailscaleCheck {
            installed: false,
            logged_in: false,
            platform: "unknown",
            brew_available: false,
            daemon_running: false,
            custom_daemon_running: false,
            system_daemon_running: false,
            has_cached_password: false,
        },
    )
}

async fn unsupported_mutation() -> HttpResponse {
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &UnsupportedMutation {
            success: false,
            unsupported: true,
            message: TUNNEL_UNSUPPORTED,
        },
    )
}
