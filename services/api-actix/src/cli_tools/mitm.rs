use std::collections::BTreeMap;

use actix_web::{
    HttpResponse,
    http::{Method, StatusCode, Uri, header},
    web,
};
use serde::{Deserialize, Serialize, de::IgnoredAny};

use crate::{json_body, responses};

const MITM_UNSUPPORTED: &str = "Antigravity MITM control is not supported by nullrouter-api";
const INVALID_ROUTER_URL: &str = "Invalid MITM router URL";
const INVALID_ROUTER_PROTOCOL: &str = "MITM router URL must use http or https";
const MITM_ALLOW: &str = "GET, POST, DELETE, PATCH, OPTIONS";
const ALIAS_ALLOW: &str = "GET, PUT, OPTIONS";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MitmStatus {
    running: bool,
    pid: Option<u32>,
    cert_exists: bool,
    cert_trusted: bool,
    dns_status: BTreeMap<String, bool>,
    has_cached_password: bool,
    is_win: bool,
    needs_sudo_password: bool,
    is_admin: bool,
    mitm_router_base_url: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct AliasStatus {
    aliases: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct UnsupportedMutation {
    success: bool,
    unsupported: bool,
    message: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct OwnedError {
    error: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartRequest {
    api_key: Option<String>,
    mitm_router_base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PatchRequest {
    tool: Option<String>,
    action: Option<String>,
}

#[derive(Deserialize)]
struct AliasPutRequest {
    tool: Option<String>,
    mappings: Option<AliasMappings>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AliasMappings {
    Object(BTreeMap<String, Option<IgnoredAny>>),
    Other(IgnoredAny),
}

impl AliasMappings {
    fn into_object_flag(self) -> bool {
        match self {
            Self::Object(_entries) => true,
            Self::Other(_value) => false,
        }
    }
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/cli-tools/antigravity-mitm")
                .route(web::get().to(status))
                .route(web::post().to(start))
                .route(web::delete().to(stop))
                .route(web::patch().to(patch))
                .route(web::method(Method::OPTIONS).to(mitm_options))
                .route(web::route().to(mitm_method_not_allowed)),
        )
        .service(
            web::resource("/api/cli-tools/antigravity-mitm/alias")
                .route(web::get().to(alias_status))
                .route(web::put().to(update_alias))
                .route(web::method(Method::OPTIONS).to(alias_options))
                .route(web::route().to(alias_method_not_allowed)),
        );
}

async fn status() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &MitmStatus {
            running: false,
            pid: None,
            cert_exists: false,
            cert_trusted: false,
            dns_status: BTreeMap::from([
                ("antigravity".to_owned(), false),
                ("copilot".to_owned(), false),
                ("cursor".to_owned(), false),
                ("kiro".to_owned(), false),
            ]),
            has_cached_password: false,
            is_win: cfg!(windows),
            needs_sudo_password: false,
            is_admin: false,
            mitm_router_base_url: "http://localhost:20128",
        },
    )
}

async fn start(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<StartRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request
        .api_key
        .as_deref()
        .is_none_or(|api_key| api_key.trim().is_empty())
    {
        return bad_request("Missing apiKey");
    }
    if let Some(router_url) = request.mitm_router_base_url.as_deref()
        && !router_url.trim().is_empty()
        && let Err(error) = validate_router_url(router_url)
    {
        return bad_request(error);
    }
    unsupported()
}

async fn stop(_body: web::Bytes) -> HttpResponse {
    unsupported()
}

async fn patch(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<PatchRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let tool = request.tool.as_deref().map(str::trim);
    let action = request.action.as_deref().map(str::trim);
    if action.is_none_or(str::is_empty) {
        return bad_request("tool and action required");
    }
    match action {
        Some("trust-cert") => unsupported(),
        Some("enable" | "disable") if tool.is_some_and(|tool| !tool.is_empty()) => unsupported(),
        Some("enable" | "disable") | None => bad_request("tool and action required"),
        Some(_) if tool.is_none_or(str::is_empty) => bad_request("tool and action required"),
        Some(_) => bad_request("action must be enable, disable, or trust-cert"),
    }
}

async fn alias_status() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &AliasStatus {
            aliases: BTreeMap::new(),
        },
    )
}

async fn update_alias(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<AliasPutRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let tool = match request.tool.as_deref().map(str::trim) {
        Some(tool) if !tool.is_empty() => tool,
        Some(_) | None => return bad_request("tool and mappings required"),
    };
    if request
        .mappings
        .is_none_or(|mappings| !mappings.into_object_flag())
    {
        return bad_request("tool and mappings required");
    }
    responses::json(
        StatusCode::FORBIDDEN,
        &OwnedError {
            error: format!("DNS must be enabled for {tool} before editing model mappings"),
        },
    )
}

async fn mitm_options() -> HttpResponse {
    with_allow(responses::empty(StatusCode::NO_CONTENT), MITM_ALLOW)
}

async fn alias_options() -> HttpResponse {
    with_allow(responses::empty(StatusCode::NO_CONTENT), ALIAS_ALLOW)
}

async fn mitm_method_not_allowed() -> HttpResponse {
    method_not_allowed(MITM_ALLOW)
}

async fn alias_method_not_allowed() -> HttpResponse {
    method_not_allowed(ALIAS_ALLOW)
}

fn method_not_allowed(allow: &'static str) -> HttpResponse {
    with_allow(
        responses::json(
            StatusCode::METHOD_NOT_ALLOWED,
            &responses::error("Method not allowed"),
        ),
        allow,
    )
}

fn with_allow(mut response: HttpResponse, allow: &'static str) -> HttpResponse {
    response
        .headers_mut()
        .insert(header::ALLOW, header::HeaderValue::from_static(allow));
    response
}

fn bad_request(error: &'static str) -> HttpResponse {
    responses::json(StatusCode::BAD_REQUEST, &responses::error(error))
}

fn unsupported() -> HttpResponse {
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &UnsupportedMutation {
            success: false,
            unsupported: true,
            message: MITM_UNSUPPORTED,
        },
    )
}

fn validate_router_url(value: &str) -> Result<(), &'static str> {
    let uri = value
        .trim()
        .parse::<Uri>()
        .map_err(|_| INVALID_ROUTER_URL)?;
    let scheme = uri.scheme_str().ok_or(INVALID_ROUTER_URL)?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(INVALID_ROUTER_PROTOCOL);
    }
    if uri.authority().is_none() {
        return Err(INVALID_ROUTER_URL);
    }
    Ok(())
}
