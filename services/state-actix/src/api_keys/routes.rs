use actix_web::{
    HttpResponse,
    dev::PeerAddr,
    http::{StatusCode, header},
    web,
};
use nullrouter_contracts::{
    ApiKeyGateRequest, ApiKeyGateResponse, INTERNAL_API_KEY_GATE_PATH,
    INTERNAL_API_KEY_VALIDATE_PATH, ValidateApiKeyRequest,
};
use serde::{Deserialize, Serialize};

use crate::{StateStore, StoreError, responses};

pub(crate) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/keys")
                .route(web::get().to(list_keys))
                .route(web::post().to(create_key))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/keys/{id}")
                .route(web::get().to(get_key))
                .route(web::put().to(update_key))
                .route(web::delete().to(delete_key))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource(INTERNAL_API_KEY_VALIDATE_PATH)
                .route(web::post().to(validate_key))
                .route(web::route().to(method_not_allowed)),
        )
        .service(
            web::resource(INTERNAL_API_KEY_GATE_PATH)
                .route(web::post().to(gate))
                .route(web::route().to(method_not_allowed)),
        );
}

#[derive(Debug, Serialize)]
struct KeysResponse<T> {
    keys: T,
}

#[derive(Debug, Serialize)]
struct KeyResponse<T> {
    key: T,
}

#[derive(Debug, Deserialize)]
struct CreateKeyRequest {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateKeyRequest {
    is_active: Option<bool>,
}

async fn options() -> HttpResponse {
    responses::no_content()
}

async fn list_keys(store: web::Data<StateStore>) -> HttpResponse {
    public_store_json(
        StatusCode::OK,
        store.list_keys().map(|keys| KeysResponse { keys }),
    )
}

async fn create_key(store: web::Data<StateStore>, body: web::Bytes) -> HttpResponse {
    let request = match parse_public_json::<CreateKeyRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(name) = request
        .name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
    else {
        return public_error(StatusCode::BAD_REQUEST, "Name is required");
    };
    public_store_json(StatusCode::CREATED, store.create_key(name))
}

async fn get_key(store: web::Data<StateStore>, path: web::Path<String>) -> HttpResponse {
    match store.get_key(&path) {
        Ok(Some(key)) => responses::json(StatusCode::OK, &KeyResponse { key }),
        Ok(None) => public_error(StatusCode::NOT_FOUND, "Key not found"),
        Err(_) => public_error(StatusCode::INTERNAL_SERVER_ERROR, "State service error"),
    }
}

async fn update_key(
    store: web::Data<StateStore>,
    path: web::Path<String>,
    body: web::Bytes,
) -> HttpResponse {
    let request = match parse_public_json::<UpdateKeyRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    match store.update_key(&path, request.is_active) {
        Ok(Some(key)) => responses::json(StatusCode::OK, &KeyResponse { key }),
        Ok(None) => public_error(StatusCode::NOT_FOUND, "Key not found"),
        Err(_) => public_error(StatusCode::INTERNAL_SERVER_ERROR, "State service error"),
    }
}

async fn delete_key(store: web::Data<StateStore>, path: web::Path<String>) -> HttpResponse {
    match store.delete_key(&path) {
        Ok(true) => responses::json(
            StatusCode::OK,
            &serde_json::json!({ "message": "Key deleted successfully" }),
        ),
        Ok(false) => public_error(StatusCode::NOT_FOUND, "Key not found"),
        Err(_) => public_error(StatusCode::INTERNAL_SERVER_ERROR, "State service error"),
    }
}

async fn validate_key(
    peer_addr: Option<PeerAddr>,
    content_type: Option<web::Header<header::ContentType>>,
    store: web::Data<StateStore>,
    body: web::Bytes,
) -> HttpResponse {
    if !is_loopback(peer_addr) {
        return internal_error(
            StatusCode::FORBIDDEN,
            "Internal route requires loopback peer",
        );
    }
    if !is_json(content_type.as_ref()) {
        return internal_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/json",
        );
    }
    let Ok(request) = serde_json::from_slice::<ValidateApiKeyRequest>(&body) else {
        return internal_error(StatusCode::BAD_REQUEST, "Invalid JSON body");
    };
    store
        .validate_managed_key(request.api_key.expose_secret())
        .map_or_else(
            |_| internal_error(StatusCode::INTERNAL_SERVER_ERROR, "State service error"),
            |response| internal_json(StatusCode::OK, &response),
        )
}

/// Read the live gate setting and validate the presented key under one snapshot lock.
async fn gate(
    peer_addr: Option<PeerAddr>,
    content_type: Option<web::Header<header::ContentType>>,
    store: web::Data<StateStore>,
    body: web::Bytes,
) -> HttpResponse {
    if !is_loopback(peer_addr) {
        return internal_error(
            StatusCode::FORBIDDEN,
            "Internal route requires loopback peer",
        );
    }
    if !is_json(content_type.as_ref()) {
        return internal_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/json",
        );
    }
    let Ok(request) = serde_json::from_slice::<ApiKeyGateRequest>(&body) else {
        return internal_error(StatusCode::BAD_REQUEST, "Invalid JSON body");
    };
    let result = store.with_snapshot(|snapshot| {
        let candidate = request
            .api_key
            .as_ref()
            .map(|key| crate::api_keys::digest_secret(key.expose_secret()));
        let mut matched = None;
        if let Some(candidate) = candidate.as_ref() {
            for key in &snapshot.api_keys {
                if key.matches_digest(candidate) {
                    matched = Some((key.id.clone(), key.is_active));
                }
            }
        }
        ApiKeyGateResponse {
            require_api_key: snapshot.settings.require_api_key,
            valid: matched.is_some(),
            active: matched.as_ref().is_some_and(|(_, active)| *active),
            key_id: matched.map(|(id, _)| id),
        }
    });
    result.map_or_else(
        |_| internal_error(StatusCode::INTERNAL_SERVER_ERROR, "State service error"),
        |response| internal_json(StatusCode::OK, &response),
    )
}

async fn method_not_allowed(peer_addr: Option<PeerAddr>) -> HttpResponse {
    if !is_loopback(peer_addr) {
        return internal_error(
            StatusCode::FORBIDDEN,
            "Internal route requires loopback peer",
        );
    }
    internal_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed")
}

fn is_loopback(peer_addr: Option<PeerAddr>) -> bool {
    peer_addr.is_some_and(|PeerAddr(address)| address.ip().is_loopback())
}

fn is_json(content_type: Option<&web::Header<header::ContentType>>) -> bool {
    content_type
        .map(ToString::to_string)
        .as_deref()
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

fn parse_public_json<T>(body: &[u8]) -> Result<T, HttpResponse>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_slice(body)
        .map_err(|_| public_error(StatusCode::BAD_REQUEST, "Invalid JSON body"))
}

fn public_store_json<T>(status: StatusCode, result: Result<T, StoreError>) -> HttpResponse
where
    T: Serialize,
{
    result.map_or_else(
        |_| public_error(StatusCode::INTERNAL_SERVER_ERROR, "State service error"),
        |body| responses::json(status, &body),
    )
}

fn public_error(status: StatusCode, message: &'static str) -> HttpResponse {
    responses::json(status, &responses::error(message))
}

fn internal_json<T>(status: StatusCode, body: &T) -> HttpResponse
where
    T: Serialize,
{
    HttpResponse::build(status).json(body)
}

fn internal_error(status: StatusCode, message: &'static str) -> HttpResponse {
    internal_json(status, &responses::error(message))
}
