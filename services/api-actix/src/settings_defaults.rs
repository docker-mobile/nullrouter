use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use crate::{json_body, responses};

#[derive(Debug, Deserialize)]
struct DatabaseImportRequest {
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProxyTestRequest {
    proxy_url: Option<String>,
    test_url: Option<String>,
    timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct DatabaseExport {
    success: bool,
    settings: SettingsSnapshot,
    providers: &'static [serde_json::Value],
    keys: &'static [serde_json::Value],
    combos: &'static [serde_json::Value],
    proxy_pools: &'static [serde_json::Value],
    provider_nodes: &'static [serde_json::Value],
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsSnapshot {
    tunnel_dashboard_access: bool,
    tunnel_url: &'static str,
    tailscale_url: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ProxyTestResponse {
    ok: bool,
    unsupported: bool,
    error: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct UnsupportedMutation {
    success: bool,
    unsupported: bool,
    error: &'static str,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/settings/database")
                .route(web::get().to(database_export))
                .route(web::post().to(database_import)),
        )
        // `GET /api/settings/require-login` used to be served here. It is gone:
        // dashboard login is always required, so nothing is left to report.
        .service(web::resource("/api/settings/proxy-test").route(web::post().to(proxy_test)));
}

async fn database_export() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &DatabaseExport {
            success: true,
            settings: SettingsSnapshot {
                tunnel_dashboard_access: false,
                tunnel_url: "",
                tailscale_url: "",
            },
            providers: &[],
            keys: &[],
            combos: &[],
            proxy_pools: &[],
            provider_nodes: &[],
        },
    )
}

async fn database_import(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<DatabaseImportRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let _ = request.password;
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &UnsupportedMutation {
            success: false,
            unsupported: true,
            error: "Database import is not supported by nullrouter-api",
        },
    )
}

async fn proxy_test(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<ProxyTestRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.proxy_url.as_deref().is_none_or(str::is_empty) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Proxy URL is required"),
        );
    }
    let _ = (request.test_url, request.timeout_ms);
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &ProxyTestResponse {
            ok: false,
            unsupported: true,
            error: "Proxy testing is not supported by nullrouter-api",
        },
    )
}
