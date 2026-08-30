use std::collections::BTreeMap;

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    StoreError, responses,
    store::{
        ComboStrategyOverride, ComboUpdate, DeleteProxyPoolResult, ProviderConnectionInput,
        ProviderConnectionUpdate, ProxyPoolInput, ProxyPoolUpdate, SettingsUpdate, SettingsView,
        StateStore,
    },
};

pub(crate) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/health")
                .route(web::get().to(health))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/providers")
                .route(web::get().to(list_providers))
                .route(web::post().to(create_provider))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/providers/{id}")
                .route(web::get().to(get_provider))
                .route(web::put().to(update_provider))
                .route(web::delete().to(delete_provider))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/combos")
                .route(web::get().to(list_combos))
                .route(web::post().to(create_combo))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/combos/{id}")
                .route(web::get().to(get_combo))
                .route(web::put().to(update_combo))
                .route(web::delete().to(delete_combo))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/proxy-pools")
                .route(web::get().to(list_proxy_pools))
                .route(web::post().to(create_proxy_pool))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/proxy-pools/{id}")
                .route(web::get().to(get_proxy_pool))
                .route(web::put().to(update_proxy_pool))
                .route(web::delete().to(delete_proxy_pool))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        // `GET /api/settings/require-login` used to live here. It is gone:
        // dashboard login is unconditional in nullrouter, so there is no
        // per-install answer left for it to report.
        .service(
            web::resource("/api/settings")
                .route(web::get().to(get_settings))
                .route(web::put().to(update_settings))
                .route(web::post().to(update_settings))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        );
}

async fn options() -> HttpResponse {
    responses::no_content()
}

async fn health(store: web::Data<StateStore>) -> HttpResponse {
    store_json(StatusCode::OK, store.health())
}

#[derive(Debug, Serialize)]
struct ProvidersResponse<T> {
    connections: T,
}

#[derive(Debug, Serialize)]
struct ProviderResponse<T> {
    connection: T,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderRequest {
    provider: Option<String>,
    api_key: Option<String>,
    name: Option<String>,
    display_name: Option<String>,
    priority: Option<u32>,
    global_priority: Option<u32>,
    default_model: Option<String>,
    test_status: Option<String>,
    provider_specific_data: Option<BTreeMap<String, Value>>,
    proxy_pool_id: Option<Value>,
    connection_proxy_enabled: Option<bool>,
    connection_proxy_url: Option<String>,
    connection_no_proxy: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderUpdateRequest {
    name: Option<String>,
    api_key: Option<String>,
    priority: Option<u32>,
    global_priority: Option<u32>,
    default_model: Option<String>,
    is_active: Option<bool>,
    test_status: Option<String>,
    last_error: Option<String>,
    last_error_at: Option<String>,
    provider_specific_data: Option<BTreeMap<String, Value>>,
    proxy_pool_id: Option<Value>,
    connection_proxy_enabled: Option<bool>,
    connection_proxy_url: Option<String>,
    connection_no_proxy: Option<String>,
}

async fn list_providers(store: web::Data<StateStore>) -> HttpResponse {
    store_json(
        StatusCode::OK,
        store
            .list_connections()
            .map(|connections| ProvidersResponse { connections }),
    )
}

async fn create_provider(store: web::Data<StateStore>, body: web::Bytes) -> HttpResponse {
    let request = match parse_json::<ProviderRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(provider) = trim_optional(request.provider) else {
        return bad_request("Invalid provider");
    };
    let api_key = trim_optional(request.api_key);
    if provider != "ollama-local" && api_key.is_none() {
        return bad_request("API Key is required");
    }
    let name = trim_optional(request.name)
        .or_else(|| trim_optional(request.display_name))
        .unwrap_or_else(|| provider.clone());
    let provider_specific_data = match build_provider_specific_data(
        &store,
        request.provider_specific_data,
        request.proxy_pool_id.as_ref(),
        request.connection_proxy_enabled,
        request.connection_proxy_url.as_deref(),
        request.connection_no_proxy.as_deref(),
    ) {
        Ok(data) => data,
        Err(response) => return response,
    };
    let input = ProviderConnectionInput {
        provider,
        auth_type: Some("apikey".to_owned()),
        name,
        api_key,
        priority: request.priority,
        global_priority: request.global_priority,
        default_model: trim_optional(request.default_model),
        is_active: Some(true),
        test_status: trim_optional(request.test_status),
        email: None,
        last_error: None,
        last_error_at: None,
        provider_specific_data,
        // The public create endpoint is API-key only; OAuth secrets arrive
        // through the internal refresh endpoint.
        access_token: None,
        refresh_token: None,
        expires_at: None,
    };
    store_json(
        StatusCode::CREATED,
        store
            .create_connection(input)
            .map(|connection| ProviderResponse { connection }),
    )
}

async fn get_provider(store: web::Data<StateStore>, path: web::Path<String>) -> HttpResponse {
    match store.get_connection(&path) {
        Ok(Some(connection)) => responses::json(StatusCode::OK, &ProviderResponse { connection }),
        Ok(None) => not_found("Connection not found"),
        Err(_) => internal_error(),
    }
}

async fn update_provider(
    store: web::Data<StateStore>,
    path: web::Path<String>,
    body: web::Bytes,
) -> HttpResponse {
    let request = match parse_json::<ProviderUpdateRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.connection_proxy_enabled == Some(true)
        && request
            .connection_proxy_url
            .as_deref()
            .is_none_or(|url| url.trim().is_empty())
    {
        return bad_request("Connection proxy URL is required when connection proxy is enabled");
    }
    let provider_specific_data = match build_provider_specific_data(
        &store,
        request.provider_specific_data,
        request.proxy_pool_id.as_ref(),
        request.connection_proxy_enabled,
        request.connection_proxy_url.as_deref(),
        request.connection_no_proxy.as_deref(),
    ) {
        Ok(data) => data,
        Err(response) => return response,
    };
    let input = ProviderConnectionUpdate {
        name: trim_optional(request.name),
        api_key: trim_optional(request.api_key),
        priority: request.priority,
        global_priority: request.global_priority,
        default_model: trim_optional(request.default_model),
        is_active: request.is_active,
        test_status: trim_optional(request.test_status),
        last_error: trim_optional(request.last_error),
        last_error_at: trim_optional(request.last_error_at),
        provider_specific_data,
    };
    match store.update_connection(&path, input) {
        Ok(Some(connection)) => responses::json(StatusCode::OK, &ProviderResponse { connection }),
        Ok(None) => not_found("Connection not found"),
        Err(_) => internal_error(),
    }
}

async fn delete_provider(store: web::Data<StateStore>, path: web::Path<String>) -> HttpResponse {
    match store.delete_connection(&path) {
        Ok(true) => responses::json(
            StatusCode::OK,
            &serde_json::json!({ "message": "Connection deleted successfully" }),
        ),
        Ok(false) => not_found("Connection not found"),
        Err(_) => internal_error(),
    }
}

#[derive(Debug, Serialize)]
struct CombosResponse<T> {
    combos: T,
}

#[derive(Debug, Deserialize)]
struct ComboRequest {
    name: Option<String>,
    kind: Option<String>,
    models: Option<Vec<String>>,
}

async fn list_combos(store: web::Data<StateStore>) -> HttpResponse {
    store_json(
        StatusCode::OK,
        store.list_combos().map(|combos| CombosResponse { combos }),
    )
}

async fn create_combo(store: web::Data<StateStore>, body: web::Bytes) -> HttpResponse {
    let request = match parse_json::<ComboRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(name) = trim_optional(request.name) else {
        return bad_request("Name is required");
    };
    if !is_valid_combo_name(&name) {
        return bad_request("Name can only contain letters, numbers, -, _ and .");
    }
    match store.combo_name_exists(&name, None) {
        Ok(true) => return bad_request("Combo name already exists"),
        Ok(false) => {}
        Err(_) => return internal_error(),
    }
    store_json(
        StatusCode::CREATED,
        store.create_combo(name, request.kind, request.models.unwrap_or_default()),
    )
}

async fn get_combo(store: web::Data<StateStore>, path: web::Path<String>) -> HttpResponse {
    match store.get_combo(&path) {
        Ok(Some(combo)) => responses::json(StatusCode::OK, &combo),
        Ok(None) => not_found("Combo not found"),
        Err(_) => internal_error(),
    }
}

async fn update_combo(
    store: web::Data<StateStore>,
    path: web::Path<String>,
    body: web::Bytes,
) -> HttpResponse {
    let value = match parse_json::<Value>(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let name = value.get("name").and_then(Value::as_str).map(str::to_owned);
    if let Some(name) = name.as_deref() {
        if name.trim().is_empty() {
            return bad_request("Name is required");
        }
        if !is_valid_combo_name(name) {
            return bad_request("Name can only contain letters, numbers, -, _ and .");
        }
        match store.combo_name_exists(name, Some(&path)) {
            Ok(true) => return bad_request("Combo name already exists"),
            Ok(false) => {}
            Err(_) => return internal_error(),
        }
    }
    let models = value
        .get("models")
        .and_then(|models| serde_json::from_value::<Vec<String>>(models.clone()).ok());
    let kind_set = value
        .as_object()
        .is_some_and(|object| object.contains_key("kind"));
    let kind = value.get("kind").and_then(Value::as_str).map(str::to_owned);
    let update = ComboUpdate {
        name: name.map(|name| name.trim().to_owned()),
        kind,
        kind_set,
        models,
    };
    match store.update_combo(&path, update) {
        Ok(Some(combo)) => responses::json(StatusCode::OK, &combo),
        Ok(None) => not_found("Combo not found"),
        Err(_) => internal_error(),
    }
}

async fn delete_combo(store: web::Data<StateStore>, path: web::Path<String>) -> HttpResponse {
    match store.delete_combo(&path) {
        Ok(true) => responses::json(StatusCode::OK, &serde_json::json!({ "success": true })),
        Ok(false) => not_found("Combo not found"),
        Err(_) => internal_error(),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyPoolsResponse<T> {
    proxy_pools: T,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyPoolResponse<T> {
    proxy_pool: T,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProxyPoolsQuery {
    is_active: Option<String>,
    include_usage: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProxyPoolRequest {
    name: Option<String>,
    proxy_url: Option<String>,
    no_proxy: Option<String>,
    is_active: Option<bool>,
    strict_proxy: Option<bool>,
    #[serde(rename = "type")]
    proxy_type: Option<String>,
    test_status: Option<String>,
}

async fn list_proxy_pools(
    store: web::Data<StateStore>,
    query: web::Query<ProxyPoolsQuery>,
) -> HttpResponse {
    let is_active = query.is_active.as_deref().and_then(to_bool);
    let include_usage = query.include_usage.as_deref() == Some("true");
    store_json(
        StatusCode::OK,
        store
            .list_proxy_pools(is_active, include_usage)
            .map(|proxy_pools| ProxyPoolsResponse { proxy_pools }),
    )
}

async fn create_proxy_pool(store: web::Data<StateStore>, body: web::Bytes) -> HttpResponse {
    let input = match parse_proxy_pool_input(&body) {
        Ok(input) => input,
        Err(response) => return response,
    };
    store_json(
        StatusCode::CREATED,
        store
            .create_proxy_pool(input)
            .map(|proxy_pool| ProxyPoolResponse { proxy_pool }),
    )
}

async fn get_proxy_pool(store: web::Data<StateStore>, path: web::Path<String>) -> HttpResponse {
    match store.get_proxy_pool(&path) {
        Ok(Some(proxy_pool)) => responses::json(StatusCode::OK, &ProxyPoolResponse { proxy_pool }),
        Ok(None) => not_found("Proxy pool not found"),
        Err(_) => internal_error(),
    }
}

async fn update_proxy_pool(
    store: web::Data<StateStore>,
    path: web::Path<String>,
    body: web::Bytes,
) -> HttpResponse {
    let request = match parse_json::<ProxyPoolRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let update = match proxy_pool_update_from_request(request) {
        Ok(update) => update,
        Err(response) => return response,
    };
    match store.update_proxy_pool(&path, update) {
        Ok(Some(proxy_pool)) => responses::json(StatusCode::OK, &ProxyPoolResponse { proxy_pool }),
        Ok(None) => not_found("Proxy pool not found"),
        Err(_) => internal_error(),
    }
}

async fn delete_proxy_pool(store: web::Data<StateStore>, path: web::Path<String>) -> HttpResponse {
    match store.delete_proxy_pool(&path) {
        Ok(DeleteProxyPoolResult::Deleted) => {
            responses::json(StatusCode::OK, &serde_json::json!({ "success": true }))
        }
        Ok(DeleteProxyPoolResult::NotFound) => not_found("Proxy pool not found"),
        Ok(DeleteProxyPoolResult::InUse {
            bound_connection_count,
        }) => responses::json(
            StatusCode::CONFLICT,
            &serde_json::json!({
                "error": "Proxy pool is currently in use",
                "boundConnectionCount": bound_connection_count,
            }),
        ),
        Err(_) => internal_error(),
    }
}

/// A `PUT`/`POST /api/settings` body.
///
/// Every field is optional so a single-key write leaves the rest untouched. An
/// unknown key — `requireLogin`, for instance, which no longer exists — is
/// ignored rather than rejected, so an older dashboard build does not break.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsRequest {
    require_api_key: Option<bool>,
    tunnel_dashboard_access: Option<bool>,
    tunnel_url: Option<String>,
    tailscale_url: Option<String>,
    outbound_proxy_enabled: Option<bool>,
    outbound_proxy_url: Option<String>,
    outbound_no_proxy: Option<String>,
    oidc_issuer_url: Option<String>,
    oidc_client_id: Option<String>,
    oidc_client_secret: Option<String>,
    oidc_scopes: Option<String>,
    oidc_login_label: Option<String>,
    saml_entry_point: Option<String>,
    saml_issuer: Option<String>,
    saml_cert: Option<String>,
    saml_attribute_email: Option<String>,
    saml_attribute_name: Option<String>,
    pxpipe_enabled: Option<bool>,
    pxpipe_auto_install: Option<bool>,
    pxpipe_min_chars: Option<u64>,
    pxpipe_timeout_ms: Option<u64>,
    combo_strategies: Option<std::collections::BTreeMap<String, ComboStrategyOverride>>,
}

async fn get_settings(store: web::Data<StateStore>) -> HttpResponse {
    store_json(StatusCode::OK, store.settings().map(SettingsView::from))
}

async fn update_settings(store: web::Data<StateStore>, body: web::Bytes) -> HttpResponse {
    let request = match parse_json::<SettingsRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let update = SettingsUpdate {
        require_api_key: request.require_api_key,
        tunnel_dashboard_access: request.tunnel_dashboard_access,
        tunnel_url: request.tunnel_url,
        tailscale_url: request.tailscale_url,
        outbound_proxy_enabled: request.outbound_proxy_enabled,
        outbound_proxy_url: request.outbound_proxy_url,
        outbound_no_proxy: request.outbound_no_proxy,
        oidc_issuer_url: request.oidc_issuer_url,
        oidc_client_id: request.oidc_client_id,
        oidc_client_secret: request.oidc_client_secret,
        oidc_scopes: request.oidc_scopes,
        oidc_login_label: request.oidc_login_label,
        saml_entry_point: request.saml_entry_point,
        saml_issuer: request.saml_issuer,
        saml_cert: request.saml_cert,
        saml_attribute_email: request.saml_attribute_email,
        saml_attribute_name: request.saml_attribute_name,
        pxpipe_enabled: request.pxpipe_enabled,
        pxpipe_auto_install: request.pxpipe_auto_install,
        pxpipe_min_chars: request.pxpipe_min_chars,
        pxpipe_timeout_ms: request.pxpipe_timeout_ms,
        combo_strategies: request.combo_strategies,
    };
    store_json(
        StatusCode::OK,
        store.update_settings(update).map(SettingsView::from),
    )
}

fn parse_json<T>(body: &[u8]) -> Result<T, HttpResponse>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_slice(body).map_err(|_| bad_request("Invalid JSON body"))
}

fn bad_request(message: &'static str) -> HttpResponse {
    responses::json(StatusCode::BAD_REQUEST, &responses::error(message))
}

fn not_found(message: &'static str) -> HttpResponse {
    responses::json(StatusCode::NOT_FOUND, &responses::error(message))
}

fn internal_error() -> HttpResponse {
    responses::json(
        StatusCode::INTERNAL_SERVER_ERROR,
        &responses::error("State service error"),
    )
}

fn store_json<T>(status: StatusCode, result: Result<T, StoreError>) -> HttpResponse
where
    T: Serialize,
{
    result.map_or_else(|_| internal_error(), |body| responses::json(status, &body))
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn is_valid_combo_name(name: &str) -> bool {
    name.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn build_provider_specific_data(
    store: &StateStore,
    mut provider_specific_data: Option<BTreeMap<String, Value>>,
    proxy_pool_id: Option<&Value>,
    connection_proxy_enabled: Option<bool>,
    connection_proxy_url: Option<&str>,
    connection_no_proxy: Option<&str>,
) -> Result<Option<BTreeMap<String, Value>>, HttpResponse> {
    let mut has_data = provider_specific_data.is_some();
    let data = provider_specific_data.get_or_insert_with(BTreeMap::new);

    if let Some(proxy_pool_id) = proxy_pool_id.and_then(Value::as_str).map(str::trim)
        && !proxy_pool_id.is_empty()
        && proxy_pool_id != "__none__"
    {
        match store.proxy_pool_exists(proxy_pool_id) {
            Ok(true) => {
                data.insert("proxyPoolId".to_owned(), Value::from(proxy_pool_id));
                has_data = true;
            }
            Ok(false) => return Err(bad_request("Proxy pool not found")),
            Err(_) => return Err(internal_error()),
        }
    }

    if let Some(enabled) = connection_proxy_enabled {
        let proxy_url = connection_proxy_url.unwrap_or_default().trim();
        if enabled && proxy_url.is_empty() {
            return Err(bad_request(
                "Connection proxy URL is required when connection proxy is enabled",
            ));
        }
        data.insert("connectionProxyEnabled".to_owned(), Value::from(enabled));
        data.insert("connectionProxyUrl".to_owned(), Value::from(proxy_url));
        data.insert(
            "connectionNoProxy".to_owned(),
            Value::from(connection_no_proxy.unwrap_or_default().trim()),
        );
        has_data = true;
    }

    if has_data {
        Ok(provider_specific_data)
    } else {
        Ok(None)
    }
}

fn parse_proxy_pool_input(body: &[u8]) -> Result<ProxyPoolInput, HttpResponse> {
    let request = parse_json::<ProxyPoolRequest>(body)?;
    let Some(name) = trim_optional(request.name) else {
        return Err(bad_request("Name is required"));
    };
    let Some(proxy_url) = trim_optional(request.proxy_url) else {
        return Err(bad_request("Proxy URL is required"));
    };
    Ok(ProxyPoolInput {
        name,
        proxy_url,
        no_proxy: request.no_proxy.map(|value| value.trim().to_owned()),
        proxy_type: request.proxy_type.map(normalize_proxy_type),
        is_active: request.is_active,
        strict_proxy: request.strict_proxy,
        test_status: request.test_status,
    })
}

fn proxy_pool_update_from_request(
    request: ProxyPoolRequest,
) -> Result<ProxyPoolUpdate, HttpResponse> {
    let name = match request.name {
        Some(name) => {
            let Some(name) = trim_optional(Some(name)) else {
                return Err(bad_request("Name is required"));
            };
            Some(name)
        }
        None => None,
    };
    let proxy_url = match request.proxy_url {
        Some(proxy_url) => {
            let Some(proxy_url) = trim_optional(Some(proxy_url)) else {
                return Err(bad_request("Proxy URL is required"));
            };
            Some(proxy_url)
        }
        None => None,
    };
    Ok(ProxyPoolUpdate {
        name,
        proxy_url,
        no_proxy: request.no_proxy.map(|value| value.trim().to_owned()),
        proxy_type: request.proxy_type.map(|value| {
            if matches!(value.as_str(), "http" | "vercel" | "cloudflare") {
                value
            } else {
                "http".to_owned()
            }
        }),
        is_active: request.is_active,
        strict_proxy: request.strict_proxy,
    })
}

fn normalize_proxy_type(value: String) -> String {
    if matches!(value.as_str(), "http" | "vercel" | "cloudflare" | "deno") {
        value
    } else {
        "http".to_owned()
    }
}

fn to_bool(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}
