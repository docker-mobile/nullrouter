use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Deserialize;

use crate::{json_body, responses};

#[derive(Debug, Deserialize)]
struct CloudflareDeployRequest {
    #[serde(rename = "accountId")]
    account_id: Option<String>,
    #[serde(rename = "apiToken")]
    api_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DenoDeployRequest {
    #[serde(rename = "orgDomain")]
    org_domain: Option<String>,
    #[serde(rename = "denoToken")]
    deno_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VercelDeployRequest {
    #[serde(rename = "vercelToken")]
    vercel_token: Option<String>,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/proxy-pools/{id}/test")
                .route(web::post().to(test_pool))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/proxy-pools/cloudflare-deploy")
                .route(web::post().to(cloudflare_deploy))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/proxy-pools/deno-deploy")
                .route(web::post().to(deno_deploy))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/proxy-pools/vercel-deploy")
                .route(web::post().to(vercel_deploy))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        );
}

async fn test_pool(path: web::Path<String>, body: web::Bytes) -> HttpResponse {
    match json_body::parse_optional::<serde_json::Value>(&body) {
        Ok(_) => responses::json(
            StatusCode::NOT_IMPLEMENTED,
            &serde_json::json!({
                "id": path.into_inner(),
                "ok": false,
                "status": null,
                "statusText": null,
                "error": "Proxy pool testing is not supported by nullrouter-api",
                "elapsedMs": 0,
                "unsupported": true,
            }),
        ),
        Err(response) => response,
    }
}

async fn cloudflare_deploy(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<CloudflareDeployRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.account_id.as_deref().is_none_or(str::is_empty)
        || request.api_token.as_deref().is_none_or(str::is_empty)
    {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Cloudflare Account ID and API Token are required"),
        );
    }
    deploy_unsupported("cloudflare")
}

async fn deno_deploy(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<DenoDeployRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.org_domain.as_deref().is_none_or(str::is_empty) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Organization domain is required"),
        );
    }
    if request.deno_token.as_deref().is_none_or(str::is_empty) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Deno Deploy API token is required"),
        );
    }
    deploy_unsupported("deno")
}

async fn vercel_deploy(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<VercelDeployRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.vercel_token.as_deref().is_none_or(str::is_empty) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Vercel API token is required"),
        );
    }
    deploy_unsupported("vercel")
}

fn deploy_unsupported(target: &'static str) -> HttpResponse {
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &serde_json::json!({
            "success": false,
            "target": target,
            "unsupported": true,
            "error": "Proxy relay deployment is not supported by nullrouter-api",
        }),
    )
}

async fn options() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}
