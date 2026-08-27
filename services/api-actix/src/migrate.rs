//! Dashboard-facing 9Router import.
//!
//! nullrouter-specific: 9Router has no equivalent route because it *is* the
//! source. The work happens in `nullrouter-state`; this forwards to it and
//! relays the report.
//!
//! The gateway classifies `/api/*` as session-gated, so this is not reachable
//! without an authenticated dashboard session — importing provider credentials
//! must never be an unauthenticated operation.

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Deserialize;
use serde_json::json;

use crate::{responses, state_client::StateClient};

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config.service(
        web::resource("/api/migrate/9router")
            .route(web::post().to(migrate))
            .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
    );
}

async fn options() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}

/// Import request.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrateRequest {
    /// Explicit 9Router data directory. Discovered from `DATA_DIR` and
    /// `~/.9router` when omitted.
    #[serde(default)]
    data_dir: Option<String>,
    /// Preview what would be imported without writing.
    #[serde(default)]
    dry_run: bool,
}

async fn migrate(state: web::Data<StateClient>, body: web::Bytes) -> HttpResponse {
    // An absent body means "discover and import", the common case.
    let request = if body.is_empty() {
        MigrateRequest::default()
    } else {
        match serde_json::from_slice::<MigrateRequest>(&body) {
            Ok(request) => request,
            Err(_) => {
                return responses::json(
                    StatusCode::BAD_REQUEST,
                    &responses::error("Invalid JSON body"),
                );
            }
        }
    };

    let Some((status, report)) = state
        .migrate_from_9router(request.data_dir.as_deref(), request.dry_run)
        .await
    else {
        return responses::json(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({
                "ok": false,
                "error": "state_unavailable",
                "message": "The state service is unreachable, so no import was attempted.",
            }),
        );
    };

    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    responses::json(status, &report)
}
