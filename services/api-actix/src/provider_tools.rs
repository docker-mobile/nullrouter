use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Deserialize;

use crate::{
    json_body, provider_probe, responses,
    state_client::{ConnectionLookup, RuntimeClient, StateClient},
};

/// The router cannot read its own connection store.
///
/// Reported as 503 rather than 404 so a down state service is not indistinguishable
/// from a connection the user never created.
fn state_unavailable(connection_id: &str) -> HttpResponse {
    responses::json(
        StatusCode::SERVICE_UNAVAILABLE,
        &serde_json::json!({
            "success": false,
            "connectionId": connection_id,
            "results": [],
            "error": "The state service is unreachable, so this connection could not be read",
        }),
    )
}

#[derive(Debug, Deserialize)]
struct BatchRequest {
    mode: Option<String>,
    #[serde(rename = "providerId")]
    provider_id: Option<String>,
}

/// Body of `POST /api/providers/{id}/test-models`.
#[derive(Debug, Default, Deserialize)]
struct ModelsRequest {
    /// Explicit models to test. Empty means "pick some from the registry".
    #[serde(default)]
    models: Vec<String>,
}

/// Cap on models tested in one request.
///
/// Every entry is a real billable call, so an unbounded list would turn one
/// dashboard click into a bill.
const MAX_TESTED_MODELS: usize = 5;

/// Cap on connections tested in one batch, for the same reason.
const MAX_BATCH_CONNECTIONS: usize = 20;

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(web::resource("/api/providers/{id}/models").route(web::get().to(models)))
        .service(
            web::resource("/api/providers/{id}/test")
                .route(web::post().to(test_provider))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/providers/{id}/test-models")
                .route(web::post().to(test_models))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(web::resource("/api/providers/kilo/free-models").route(web::get().to(kilo_models)))
        .service(
            web::resource("/api/providers/test-batch")
                .route(web::post().to(test_batch))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        );
}

async fn models(path: web::Path<String>) -> HttpResponse {
    let provider = path.into_inner();
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "provider": provider,
            "models": [],
            "cached": true,
            "warning": "Live provider model discovery is not configured",
        }),
    )
}

/// Test one connection by making a real, minimal upstream call.
///
/// The connection is read from state to learn its provider and model, then a
/// one-token chat request goes through the runtime. Only a 2xx passes; a failure
/// relays the provider's own scrubbed message, because "invalid key" and "model not
/// found" send the user to different places.
async fn test_provider(
    path: web::Path<String>,
    body: web::Bytes,
    state: web::Data<StateClient>,
    runtime: web::Data<RuntimeClient>,
) -> HttpResponse {
    if let Err(response) = json_body::parse_optional::<serde_json::Value>(&body) {
        return response;
    }
    let connection_id = path.into_inner();

    let connection = match state.connection(&connection_id).await {
        ConnectionLookup::Found(connection) => connection,
        ConnectionLookup::Missing => {
            return responses::json(
                StatusCode::NOT_FOUND,
                &serde_json::json!({
                    "success": false,
                    "connectionId": connection_id,
                    "error": "No such provider connection",
                }),
            );
        }
        ConnectionLookup::Unavailable => return state_unavailable(&connection_id),
    };
    let Some(model) = provider_probe::probe_model(&connection) else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &serde_json::json!({
                "success": false,
                "connectionId": connection_id,
                "error": "This connection names no model to test: set a default model first",
            }),
        );
    };

    let result = probe_once(&runtime, &model).await;
    let status = if result.success {
        StatusCode::OK
    } else {
        // The test itself ran; it is the provider that refused. 200 with
        // `success: false` would make a broken connection look like a working call.
        StatusCode::BAD_GATEWAY
    };
    let mut payload = provider_probe::to_object(&result);
    payload.insert("connectionId".to_owned(), connection_id.into());
    payload.insert(
        "provider".to_owned(),
        connection.get("provider").cloned().unwrap_or_default(),
    );
    // `valid` is the field the dashboard's connection list reads; it means the same
    // thing as `success` here and is kept for that surface.
    payload.insert("valid".to_owned(), result.success.into());
    responses::json(status, &serde_json::Value::Object(payload))
}

/// Run one probe through the runtime.
async fn probe_once(runtime: &RuntimeClient, model: &str) -> provider_probe::ProbeResult {
    let started = std::time::Instant::now();
    let payload = provider_probe::probe_body(model).to_string();
    let reply = runtime
        .forward_chat(payload.as_bytes())
        .await
        .map(|forwarded| (forwarded.status, forwarded.body));
    provider_probe::settle(model, started, reply)
}

/// Test several of a connection's models, one real call each.
///
/// Bounded to [`MAX_TESTED_MODELS`]: each entry is a billable request, so a provider
/// with 80 models must not turn one dashboard click into 80 calls.
async fn test_models(
    path: web::Path<String>,
    body: web::Bytes,
    state: web::Data<StateClient>,
    runtime: web::Data<RuntimeClient>,
) -> HttpResponse {
    let request = match json_body::parse_optional::<ModelsRequest>(&body) {
        Ok(request) => request.unwrap_or_default(),
        Err(response) => return response,
    };
    let connection_id = path.into_inner();

    let connection = match state.connection(&connection_id).await {
        ConnectionLookup::Found(connection) => connection,
        ConnectionLookup::Missing => {
            return responses::json(
                StatusCode::NOT_FOUND,
                &serde_json::json!({
                    "connectionId": connection_id,
                    "results": [],
                    "error": "No such provider connection",
                }),
            );
        }
        ConnectionLookup::Unavailable => return state_unavailable(&connection_id),
    };
    let provider = connection
        .get("provider")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();

    // Explicit models win; otherwise the registry's first few for this provider.
    let requested: Vec<String> = if request.models.is_empty() {
        nullrouter_providers::models_for_provider(&provider)
            .iter()
            .take(MAX_TESTED_MODELS)
            .map(|model| format!("{provider}/{}", model.id))
            .collect()
    } else {
        request
            .models
            .into_iter()
            .take(MAX_TESTED_MODELS)
            .map(|model| {
                if model.contains('/') {
                    model
                } else {
                    format!("{provider}/{model}")
                }
            })
            .collect()
    };

    let mut results = Vec::with_capacity(requested.len());
    let mut passed = 0_usize;
    for model in requested {
        let result = probe_once(&runtime, &model).await;
        if result.success {
            passed += 1;
        }
        results.push(result);
    }

    let total = results.len();
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "connectionId": connection_id,
            "provider": provider,
            "results": results,
            "summary": { "total": total, "passed": passed, "failed": total - passed },
        }),
    )
}

async fn kilo_models() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "models": [],
            "cached": true,
            "warning": "Live Kilo model discovery is not configured",
        }),
    )
}

async fn test_batch(
    body: web::Bytes,
    state: web::Data<StateClient>,
    runtime: web::Data<RuntimeClient>,
) -> HttpResponse {
    let request = match json_body::parse::<BatchRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(mode) = request
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
    else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("mode is required"),
        );
    };
    if !matches!(
        mode,
        "provider" | "oauth" | "free" | "apikey" | "compatible" | "all"
    ) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Invalid mode. Use: provider, oauth, free, apikey, compatible, all"),
        );
    }
    // Which connections this mode covers. `provider` needs an explicit id; the
    // others filter the configured set.
    let Some(connections) = state.connections().await else {
        // Not an empty result: nothing was tested, and saying `failed: 0` here would
        // report a green batch for a router that cannot see its own connections.
        return responses::json(
            StatusCode::SERVICE_UNAVAILABLE,
            &serde_json::json!({
                "mode": mode,
                "results": [],
                "error": "The state service is unreachable, so no connection could be tested",
            }),
        );
    };
    let selected: Vec<serde_json::Value> = connections
        .into_iter()
        .filter(|connection| batch_selects(mode, request.provider_id.as_deref(), connection))
        .take(MAX_BATCH_CONNECTIONS)
        .collect();

    let mut results = Vec::with_capacity(selected.len());
    let mut passed = 0_usize;
    for connection in &selected {
        let id = connection
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let provider = connection
            .get("provider")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let Some(model) = provider_probe::probe_model(connection) else {
            results.push(serde_json::json!({
                "connectionId": id,
                "provider": provider,
                "success": false,
                "error": "This connection names no model to test",
            }));
            continue;
        };
        let result = probe_once(&runtime, &model).await;
        if result.success {
            passed += 1;
        }
        let mut entry = provider_probe::to_object(&result);
        entry.insert("connectionId".to_owned(), id.into());
        entry.insert("provider".to_owned(), provider.into());
        results.push(serde_json::Value::Object(entry));
    }

    let total = results.len();
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "mode": mode,
            "providerId": request.provider_id.unwrap_or_default(),
            "results": results,
            "summary": { "total": total, "passed": passed, "failed": total - passed },
        }),
    )
}

/// Whether a batch mode covers this connection.
fn batch_selects(mode: &str, provider_id: Option<&str>, connection: &serde_json::Value) -> bool {
    let provider = connection
        .get("provider")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let has_key = connection
        .get("hasApiKey")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let has_oauth = connection
        .get("hasAccessToken")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    match mode {
        "all" => true,
        // A single named provider, matched on the connection's provider id.
        "provider" => provider_id.is_some_and(|wanted| wanted == provider),
        "oauth" => has_oauth,
        "apikey" => has_key,
        "compatible" => {
            nullrouter_providers::is_openai_compatible(provider)
                || nullrouter_providers::is_anthropic_compatible(provider)
        }
        // `free` has no marker in the stored record, so it selects nothing rather
        // than guessing which providers are free and billing the user to find out.
        _ => false,
    }
}

async fn options() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}
