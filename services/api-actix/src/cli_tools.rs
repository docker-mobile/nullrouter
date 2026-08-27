use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use crate::{json_body, responses};

mod mitm;

const TOOL_UNSUPPORTED: &str = "CLI tool configuration is not supported by nullrouter-api";
const MCP_UNSUPPORTED: &str = "Cowork MCP discovery is not supported by nullrouter-api";

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolStatus {
    installed: bool,
    has_9_router: bool,
    config: Option<&'static str>,
    settings: Option<&'static str>,
    config_path: Option<&'static str>,
    message: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct AllStatuses {
    claude: ToolStatus,
    codex: ToolStatus,
    opencode: ToolStatus,
    droid: ToolStatus,
    openclaw: ToolStatus,
    hermes: ToolStatus,
    cowork: ToolStatus,
    copilot: ToolStatus,
    cline: ToolStatus,
    kilo: ToolStatus,
    #[serde(rename = "deepseek-tui")]
    deepseek_tui: ToolStatus,
    jcode: ToolStatus,
}

#[derive(Debug, Deserialize)]
struct ToolMutationRequest {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpToolsRequest {
    url: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct UnsupportedMutation {
    success: bool,
    unsupported: bool,
    message: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct McpRegistry {
    cached: bool,
    servers: [(); 0],
    total: u32,
    unsupported: bool,
    message: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpTools {
    success: bool,
    tools: [(); 0],
    requires_auth: bool,
    unsupported: bool,
    message: &'static str,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(web::resource("/api/cli-tools/all-statuses").route(web::get().to(all_statuses)))
        .service(
            web::resource("/api/cli-tools/cowork-mcp-registry")
                .route(web::get().to(cowork_mcp_registry))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/api/cli-tools/cowork-mcp-tools")
                .route(web::post().to(cowork_mcp_tools))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        );
    mitm::configure(config);
    config.service(
        web::resource("/api/cli-tools/{tool}")
            .route(web::get().to(tool_status))
            .route(web::post().to(mutate_tool))
            .route(web::patch().to(mutate_tool))
            .route(web::delete().to(mutate_tool)),
    );
}

async fn no_content() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}

async fn all_statuses() -> HttpResponse {
    let status = default_tool_status();
    responses::json(
        StatusCode::OK,
        &AllStatuses {
            claude: status,
            codex: status,
            opencode: status,
            droid: status,
            openclaw: status,
            hermes: status,
            cowork: status,
            copilot: status,
            cline: status,
            kilo: status,
            deepseek_tui: status,
            jcode: status,
        },
    )
}

async fn tool_status(path: web::Path<String>) -> HttpResponse {
    let tool = path.into_inner();
    if is_settings_tool(&tool) {
        return responses::json(StatusCode::OK, &default_tool_status());
    }
    responses::json(
        StatusCode::NOT_FOUND,
        &responses::error("CLI tool route not found"),
    )
}

async fn cowork_mcp_registry() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &McpRegistry {
            cached: true,
            servers: [],
            total: 0,
            unsupported: true,
            message: MCP_UNSUPPORTED,
        },
    )
}

async fn cowork_mcp_tools(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<McpToolsRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.url.trim().is_empty() {
        return responses::json(StatusCode::BAD_REQUEST, &responses::error("url required"));
    }
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &McpTools {
            success: false,
            tools: [],
            requires_auth: false,
            unsupported: true,
            message: MCP_UNSUPPORTED,
        },
    )
}

async fn mutate_tool(body: web::Bytes) -> HttpResponse {
    if !body.is_empty() {
        let request = match json_body::parse::<ToolMutationRequest>(&body) {
            Ok(request) => request,
            Err(response) => return response,
        };
        let _ = request;
    }
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &UnsupportedMutation {
            success: false,
            unsupported: true,
            message: TOOL_UNSUPPORTED,
        },
    )
}

const fn default_tool_status() -> ToolStatus {
    ToolStatus {
        installed: false,
        has_9_router: false,
        config: None,
        settings: None,
        config_path: None,
        message: TOOL_UNSUPPORTED,
    }
}

fn is_settings_tool(tool: &str) -> bool {
    matches!(
        tool,
        "codex-settings"
            | "claude-settings"
            | "cline-settings"
            | "opencode-settings"
            | "copilot-settings"
            | "droid-settings"
            | "openclaw-settings"
            | "hermes-settings"
            | "cowork-settings"
            | "kilo-settings"
            | "deepseek-tui-settings"
            | "jcode-settings"
    )
}
