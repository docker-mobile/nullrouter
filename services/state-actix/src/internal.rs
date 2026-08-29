//! Loopback-internal endpoints used by `nullrouter-runtime` to execute
//! provider calls.
//!
//! These return **unredacted credentials** and must never be reachable from the
//! public gateway port. `nullrouter-gateway` rejects `/internal/*` from outside;
//! `internal_paths_are_not_publicly_routable` in the gateway tests pins that.

mod usage_query;

use std::collections::BTreeMap;

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    responses,
    store::{
        ConnectionSelection, CredentialUpdate, FallbackStrategy, MarkUnavailableRequest,
        ProviderConnection, SelectConnectionRequest, StateStore,
    },
    usage::{MAX_REQUEST_LOG, UsageInput},
};

pub(crate) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/internal/v1/credentials/select")
                .route(web::post().to(select_credentials))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/credentials/unavailable")
                .route(web::post().to(mark_unavailable))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/credentials/clear-error")
                .route(web::post().to(clear_error))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/credentials/refresh")
                .route(web::post().to(store_refreshed_credentials))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/usage")
                .route(web::post().to(record_usage))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/usage/stats")
                .route(web::get().to(usage_stats))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/usage/records")
                .route(web::get().to(usage_records))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/usage/details")
                .route(web::get().to(usage_details))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/usage/providers")
                .route(web::get().to(usage_providers))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/usage/live")
                .route(web::get().to(usage_live))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/usage/aggregate")
                .route(web::get().to(usage_aggregate))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/usage/connection/{connection_id}")
                .route(web::get().to(usage_connection))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/routing-context")
                .route(web::get().to(routing_context))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/auth-settings")
                .route(web::get().to(auth_settings))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/internal/v1/migrate/9router")
                .route(web::post().to(migrate_from_9router))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        );
}

async fn options() -> HttpResponse {
    responses::no_content()
}

/// Request body for credential selection.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectRequest {
    provider: String,
    #[serde(default)]
    model: Option<String>,
    /// Connections already tried in this request.
    #[serde(default)]
    exclude: Vec<String>,
}

/// Credentials handed to the runtime, mirroring `nullrouter-execute`'s shape.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CredentialsResponse {
    connection_id: String,
    connection_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<String>,
    provider_specific_data: serde_json::Map<String, serde_json::Value>,
}

impl CredentialsResponse {
    fn from_connection(connection: &ProviderConnection) -> Self {
        let mut settings: serde_json::Map<String, serde_json::Value> = connection
            .provider_specific_data
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
        // Resolve the connection's proxy pool into the flat keys the executor
        // reads, so it never has to know about pools.
        settings
            .entry("connectionProxyEnabled".to_owned())
            .or_insert_with(|| json!(false));

        Self {
            connection_id: connection.id.clone(),
            connection_name: connection
                .email
                .clone()
                .filter(|email| !email.is_empty())
                .unwrap_or_else(|| {
                    if connection.name.is_empty() {
                        connection.id.clone()
                    } else {
                        connection.name.clone()
                    }
                }),
            api_key: connection.api_key.clone(),
            access_token: connection.access_token.clone(),
            refresh_token: connection.refresh_token.clone(),
            expires_at: connection.expires_at.clone(),
            provider_specific_data: settings,
        }
    }
}

async fn select_credentials(
    store: web::Data<StateStore>,
    request: web::Json<SelectRequest>,
) -> HttpResponse {
    let Ok(settings) = store.settings() else {
        return internal_error();
    };
    // Strategy/sticky limit come from settings; both have upstream defaults.
    let strategy = if settings.fallback_strategy == "round-robin" {
        FallbackStrategy::RoundRobin
    } else {
        FallbackStrategy::FillFirst
    };

    let selection = store.select_connection(&SelectConnectionRequest {
        provider: &request.provider,
        model: request.model.as_deref(),
        exclude: &request.exclude,
        strategy,
        sticky_limit: settings.sticky_round_robin_limit,
    });

    match selection {
        Ok(ConnectionSelection::Selected(connection)) => responses::json(
            StatusCode::OK,
            &json!({
                "status": "selected",
                "credentials": CredentialsResponse::from_connection(&connection),
            }),
        ),
        Ok(ConnectionSelection::NoCredentials) => responses::json(
            StatusCode::NOT_FOUND,
            &json!({
                "status": "no_credentials",
                "message": format!(
                    "No active credentials for provider: {}",
                    request.provider
                ),
            }),
        ),
        Ok(ConnectionSelection::AllRateLimited {
            retry_at_ms,
            last_error,
            last_error_code,
        }) => responses::json(
            StatusCode::OK,
            &json!({
                "status": "all_rate_limited",
                "retryAtMs": retry_at_ms,
                "lastError": last_error,
                "lastErrorCode": last_error_code,
            }),
        ),
        Ok(ConnectionSelection::Exhausted) => {
            responses::json(StatusCode::OK, &json!({ "status": "exhausted" }))
        }
        Err(_) => internal_error(),
    }
}

/// Request body for locking a failing connection.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnavailableRequest {
    connection_id: String,
    #[serde(default)]
    model: Option<String>,
    status: u16,
    #[serde(default)]
    reason: String,
    cooldown_ms: u64,
    #[serde(default)]
    backoff_level: Option<u32>,
}

async fn mark_unavailable(
    store: web::Data<StateStore>,
    request: web::Json<UnavailableRequest>,
) -> HttpResponse {
    let updated = store.mark_connection_unavailable(&MarkUnavailableRequest {
        connection_id: &request.connection_id,
        model: request.model.as_deref(),
        status: request.status,
        reason: &request.reason,
        cooldown_ms: request.cooldown_ms,
        backoff_level: request.backoff_level,
    });
    match updated {
        Ok(true) => responses::json(StatusCode::OK, &json!({ "ok": true })),
        Ok(false) => responses::json(
            StatusCode::NOT_FOUND,
            &responses::error("connection not found"),
        ),
        Err(_) => internal_error(),
    }
}

/// Request body for clearing a connection's error state.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClearErrorRequest {
    connection_id: String,
    #[serde(default)]
    model: Option<String>,
}

async fn clear_error(
    store: web::Data<StateStore>,
    request: web::Json<ClearErrorRequest>,
) -> HttpResponse {
    match store.clear_connection_error(&request.connection_id, request.model.as_deref()) {
        Ok(true) => responses::json(StatusCode::OK, &json!({ "ok": true })),
        Ok(false) => responses::json(
            StatusCode::NOT_FOUND,
            &responses::error("connection not found"),
        ),
        Err(_) => internal_error(),
    }
}

/// Request body for persisting refreshed OAuth credentials.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshRequest {
    connection_id: String,
    #[serde(flatten)]
    credentials: CredentialUpdate,
}

async fn store_refreshed_credentials(
    store: web::Data<StateStore>,
    request: web::Json<RefreshRequest>,
) -> HttpResponse {
    match store.update_connection_credentials(&request.connection_id, &request.credentials) {
        Ok(true) => responses::json(StatusCode::OK, &json!({ "ok": true })),
        Ok(false) => responses::json(
            StatusCode::NOT_FOUND,
            &responses::error("connection not found"),
        ),
        Err(_) => internal_error(),
    }
}

async fn record_usage(
    store: web::Data<StateStore>,
    request: web::Json<UsageInput>,
) -> HttpResponse {
    match store.record_usage(request.into_inner()) {
        Ok(record) => responses::json(StatusCode::OK, &json!({ "ok": true, "id": record.id })),
        Err(_) => internal_error(),
    }
}

/// Aggregate usage stats, for the dashboard's usage surface.
async fn usage_stats(store: web::Data<StateStore>) -> HttpResponse {
    store.usage_stats().map_or_else(
        |_| internal_error(),
        |stats| responses::json(StatusCode::OK, &stats),
    )
}

/// Query parameters for the usage record listing.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordsQuery {
    /// Only records at or after this epoch-millis timestamp.
    #[serde(default)]
    since_ms: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

/// Recent request records, newest first.
async fn usage_records(
    store: web::Data<StateStore>,
    query: web::Query<RecordsQuery>,
) -> HttpResponse {
    // Bounded so a caller cannot ask for an unbounded response.
    let limit = query.limit.unwrap_or(100).min(1000);
    store
        .usage_records(query.since_ms.unwrap_or(0), limit)
        .map_or_else(
            |_| internal_error(),
            |records| responses::json(StatusCode::OK, &json!({ "records": records })),
        )
}

/// Query parameters for a filtered, paginated record read.
///
/// Mirrors the `request-details` filter the dashboard sends. Dates arrive as the
/// dashboard wrote them, and are parsed here.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailsQuery {
    #[serde(default)]
    page: Option<u32>,
    #[serde(default)]
    page_size: Option<u32>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    connection_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    start_date: Option<String>,
    #[serde(default)]
    end_date: Option<String>,
}

/// Non-empty parameter value, so `?provider=` filters nothing.
fn present(value: Option<&String>) -> Option<String> {
    value
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Filtered, paginated request records for the dashboard's request-details tab.
///
/// A `startDate`/`endDate` that is not a date is rejected: silently dropping the
/// bound would report a wider result set than the caller asked for.
async fn usage_details(
    store: web::Data<StateStore>,
    query: web::Query<DetailsQuery>,
) -> HttpResponse {
    let mut filter = usage_query::DetailFilter {
        provider: present(query.provider.as_ref()),
        model: present(query.model.as_ref()),
        connection_id: present(query.connection_id.as_ref()),
        status: present(query.status.as_ref()),
        ..usage_query::DetailFilter::default()
    };
    for (raw, bound) in [
        (present(query.start_date.as_ref()), &mut filter.start_ms),
        (present(query.end_date.as_ref()), &mut filter.end_ms),
    ] {
        if let Some(text) = raw {
            let Some(millis) = usage_query::parse_date_ms(&text) else {
                return responses::json(StatusCode::BAD_REQUEST, &responses::error("invalid date"));
            };
            *bound = Some(millis);
        }
    }

    // The whole retained ring, so filtering and paging see every record.
    let Ok(records) = store.usage_records(0, MAX_REQUEST_LOG) else {
        return internal_error();
    };
    let page = usage_query::page_details(
        &records,
        &filter,
        query.page.unwrap_or(1),
        query.page_size.unwrap_or(20),
    );
    responses::json(StatusCode::OK, &page.to_value())
}

/// Distinct providers seen in recorded usage, with their aggregates.
///
/// Names resolve through configured provider nodes, as upstream's
/// `/api/usage/providers` does.
async fn usage_providers(store: web::Data<StateStore>) -> HttpResponse {
    let Ok(records) = store.usage_records(0, MAX_REQUEST_LOG) else {
        return internal_error();
    };
    let names: BTreeMap<String, String> = store
        .list_provider_nodes()
        .unwrap_or_default()
        .into_iter()
        .filter(|node| !node.id.is_empty() && !node.name.is_empty())
        .map(|node| (node.id, node.name))
        .collect();

    responses::json(
        StatusCode::OK,
        &json!({ "providers": usage_query::providers(&records, &names) }),
    )
}

/// The live half of the dashboard's stats payload.
async fn usage_live(store: web::Data<StateStore>) -> HttpResponse {
    let Ok(records) = store.usage_records(0, MAX_REQUEST_LOG) else {
        return internal_error();
    };
    responses::json(
        StatusCode::OK,
        &usage_query::live_snapshot(&records, now_millis()),
    )
}

/// Window-scoped aggregate stats, in the dashboard's table shape.
///
/// Separate from `/internal/v1/usage/stats`, which reports untrimmed lifetime
/// counters: this one sums exactly the records in the requested window, which is
/// what makes the dashboard's period selector mean something. Callers wanting
/// lifetime totals should keep using `/usage/stats`.
async fn usage_aggregate(
    store: web::Data<StateStore>,
    query: web::Query<RecordsQuery>,
) -> HttpResponse {
    let Ok(records) = store.usage_records(query.since_ms.unwrap_or(0), MAX_REQUEST_LOG) else {
        return internal_error();
    };
    let names = usage_query::DisplayNames {
        providers: store
            .list_provider_nodes()
            .unwrap_or_default()
            .into_iter()
            .filter(|node| !node.id.is_empty() && !node.name.is_empty())
            .map(|node| (node.id, node.name))
            .collect(),
        connections: store
            .list_connections()
            .unwrap_or_default()
            .into_iter()
            .map(|connection| {
                let label = connection
                    .email
                    .filter(|email| !email.is_empty())
                    .or_else(|| Some(connection.name).filter(|name| !name.is_empty()))
                    .unwrap_or_else(|| connection.id.clone());
                (connection.id, label)
            })
            .collect(),
        api_keys: key_names(&store),
    };

    responses::json(
        StatusCode::OK,
        &usage_query::aggregate(&records, &names, now_millis()),
    )
}

/// API key id → name, for the dashboard's per-key table.
///
/// Reads the public key listing, which never exposes a secret.
fn key_names(store: &StateStore) -> BTreeMap<String, String> {
    let Ok(keys) = store.list_keys() else {
        return BTreeMap::new();
    };
    keys.into_iter()
        .filter_map(|key| serde_json::to_value(key).ok())
        .filter_map(|key| {
            let id = key.get("id")?.as_str()?.to_owned();
            let name = key.get("name")?.as_str()?.to_owned();
            Some((id, name))
        })
        .collect()
}

/// Recorded usage for one connection, plus the metadata the dashboard's quota
/// surface needs to decide what it can show.
///
/// 404 when the connection does not exist, so a caller can tell "no such
/// connection" from "no usage yet".
async fn usage_connection(store: web::Data<StateStore>, path: web::Path<String>) -> HttpResponse {
    let connection_id = path.into_inner();
    let Ok(found) = store.get_connection(&connection_id) else {
        return internal_error();
    };
    let Some(connection) = found else {
        return responses::json(
            StatusCode::NOT_FOUND,
            &responses::error("connection not found"),
        );
    };
    let Ok(records) = store.usage_records(0, MAX_REQUEST_LOG) else {
        return internal_error();
    };

    responses::json(
        StatusCode::OK,
        &json!({
            "connectionId": connection.id,
            "provider": connection.provider,
            "authType": connection.auth_type,
            "name": connection.name,
            "isActive": connection.is_active,
            "usage": usage_query::connection_totals(&records, &connection_id),
        }),
    )
}

/// Current wall-clock time in epoch millis.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Everything the runtime needs to resolve a model string, in one call:
/// combos and the settings that affect routing.
async fn routing_context(store: web::Data<StateStore>) -> HttpResponse {
    let Ok(settings) = store.settings() else {
        return internal_error();
    };
    let combos: Vec<serde_json::Value> = store
        .list_combos()
        .unwrap_or_default()
        .into_iter()
        .map(|combo| {
            json!({
                "id": combo.id,
                "name": combo.name,
                "kind": combo.kind,
                "models": combo.models,
            })
        })
        .collect();

    // Only active connections, and only their non-secret routing fields.
    let connections: Vec<serde_json::Value> = store
        .list_connections()
        .unwrap_or_default()
        .into_iter()
        .filter(|connection| connection.is_active)
        .map(|connection| {
            let settings = connection.provider_specific_data.unwrap_or_default();
            json!({
                "provider": connection.provider,
                "prefix": settings.get("prefix").and_then(serde_json::Value::as_str),
                "enabledModels": settings
                    .get("enabledModels")
                    .and_then(serde_json::Value::as_array)
                    .map(|models| {
                        models
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect();

    responses::json(
        StatusCode::OK,
        &json!({
            "combos": combos,
            "connections": connections,
            "settings": {
                "requireApiKey": settings.require_api_key,
                "fallbackStrategy": settings.fallback_strategy,
                "comboStrategy": settings.combo_strategy,
                "comboStickyRoundRobinLimit": settings.combo_sticky_round_robin_limit,
            },
        }),
    )
}

/// The dashboard SSO configuration, **including secrets**, for
/// `nullrouter-auth`.
///
/// `nullrouter-auth` runs the OIDC and SAML flows and so needs the OIDC client
/// secret and the IdP signing certificate in the clear; the public
/// `GET /api/settings` reports only whether each is set. Like every other
/// `/internal/*` route this is loopback-only — the gateway refuses `/internal/*`
/// from outside, pinned by `internal_paths_are_not_publicly_routable`.
async fn auth_settings(store: web::Data<StateStore>) -> HttpResponse {
    let Ok(settings) = store.settings() else {
        return internal_error();
    };
    responses::json(
        StatusCode::OK,
        &json!({
            "oidcIssuerUrl": settings.oidc_issuer_url,
            "oidcClientId": settings.oidc_client_id,
            "oidcClientSecret": settings.oidc_client_secret,
            "oidcScopes": settings.oidc_scopes,
            "oidcLoginLabel": settings.oidc_login_label,
            "samlEntryPoint": settings.saml_entry_point,
            "samlIssuer": settings.saml_issuer,
            "samlCert": settings.saml_cert,
            "samlAttributeEmail": settings.saml_attribute_email,
            "samlAttributeName": settings.saml_attribute_name,
        }),
    )
}

/// Request body for a 9Router import.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrateRequest {
    /// Explicit 9Router data directory; discovered when absent.
    #[serde(default)]
    data_dir: Option<String>,
    /// Report what would be imported without writing anything.
    #[serde(default)]
    dry_run: bool,
}

/// Import an existing 9Router installation.
///
/// Additive and non-destructive: existing records are kept and duplicates
/// skipped, so this is safe to run more than once.
async fn migrate_from_9router(
    store: web::Data<StateStore>,
    request: Option<web::Json<MigrateRequest>>,
) -> HttpResponse {
    let request = request.map(web::Json::into_inner).unwrap_or_default();
    match crate::migrate::import(&store, request.data_dir.as_deref(), request.dry_run) {
        Ok(report) => responses::json(
            StatusCode::OK,
            &json!({ "ok": true, "dryRun": request.dry_run, "report": report }),
        ),
        Err(crate::migrate::ImportError::NotFound { searched }) => responses::json(
            StatusCode::NOT_FOUND,
            &json!({
                "ok": false,
                "error": "no_9router_installation",
                "message": format!("No 9Router installation found. Searched: {searched}"),
            }),
        ),
        Err(error) => responses::json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({ "ok": false, "error": "import_failed", "message": error.to_string() }),
        ),
    }
}

fn internal_error() -> HttpResponse {
    responses::json(
        StatusCode::INTERNAL_SERVER_ERROR,
        &responses::error("state unavailable"),
    )
}
