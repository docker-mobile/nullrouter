use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use crate::{json_body, responses};

#[derive(Debug, Deserialize)]
struct ProxyPoolRequest {
    name: Option<String>,
    #[serde(rename = "proxyUrl")]
    proxy_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProxyPoolsResponse {
    #[serde(rename = "proxyPools")]
    proxy_pools: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ProxyPoolResponse {
    #[serde(rename = "proxyPool")]
    proxy_pool: ProxyPool,
}

#[derive(Debug, Serialize)]
struct ProxyPool {
    id: String,
    name: String,
    #[serde(rename = "proxyUrl")]
    proxy_url: String,
    #[serde(rename = "noProxy")]
    no_proxy: &'static str,
    #[serde(rename = "isActive")]
    is_active: bool,
    #[serde(rename = "strictProxy")]
    strict_proxy: bool,
    #[serde(rename = "type")]
    proxy_type: &'static str,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/proxy-pools")
                .route(web::get().to(list))
                .route(web::post().to(create)),
        )
        .service(
            web::resource("/api/proxy-pools/{id}")
                .route(web::get().to(unknown))
                .route(web::put().to(update_unknown))
                .route(web::delete().to(unknown)),
        );
}

async fn list() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &ProxyPoolsResponse {
            proxy_pools: Vec::new(),
        },
    )
}

async fn create(body: web::Bytes) -> HttpResponse {
    let request = match parse_request(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    responses::json(
        StatusCode::CREATED,
        &ProxyPoolResponse {
            proxy_pool: request,
        },
    )
}

async fn update_unknown(body: web::Bytes) -> HttpResponse {
    let _ = match parse_request(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    unknown().await
}

async fn unknown() -> HttpResponse {
    responses::json(
        StatusCode::NOT_FOUND,
        &responses::error("Proxy pool not found"),
    )
}

fn parse_request(body: &[u8]) -> Result<ProxyPool, HttpResponse> {
    let request = json_body::parse::<ProxyPoolRequest>(body)?;
    let Some(name) = request
        .name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
    else {
        return Err(responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Name is required"),
        ));
    };
    let Some(proxy_url) = request
        .proxy_url
        .map(|proxy_url| proxy_url.trim().to_owned())
        .filter(|proxy_url| !proxy_url.is_empty())
    else {
        return Err(responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Proxy URL is required"),
        ));
    };

    Ok(ProxyPool {
        id: stable_proxy_pool_id(&name),
        name,
        proxy_url,
        no_proxy: "",
        is_active: true,
        strict_proxy: false,
        proxy_type: "http",
    })
}

fn stable_proxy_pool_id(name: &str) -> String {
    format!("proxy_pool_{name}")
}
