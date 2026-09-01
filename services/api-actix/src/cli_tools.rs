use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Serialize;

use crate::{json_body, responses};

mod cowork_mcp;
mod detect;
mod mitm;
mod mutations;
mod spec;
mod toml_text;
mod write;
mod yaml_block;

const TOOL_UNSUPPORTED: &str = "CLI tool configuration is not supported by nullrouter-api";

// `ToolStatus` and `AllStatuses` lived here as fixed structs full of `&'static str`, which is what
// a hardcoded answer looks like in the type system: there was nowhere to put a real path or a real
// parsed config. Statuses are now built as `serde_json::Value` by `tool_status_body`, keyed by the
// same short ids `AllStatuses` had as fields.

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct UnsupportedMutation {
    success: bool,
    unsupported: bool,
    message: &'static str,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config.service(web::resource("/api/cli-tools/all-statuses").route(web::get().to(all_statuses)));
    cowork_mcp::configure(config);
    mitm::configure(config);
    config.service(
        web::resource("/api/cli-tools/{tool}")
            .route(web::get().to(tool_status))
            .route(web::post().to(mutate_tool))
            .route(web::patch().to(mutate_tool))
            .route(web::delete().to(mutate_tool)),
    );
}

/// Every tool at once, keyed by short id, as upstream's `all-statuses` does.
///
/// Each entry costs a `PATH` walk and one small file read. Run concurrently on the blocking pool:
/// fourteen synchronous file reads on an actix worker would stall every other request on that
/// worker, and the dashboard polls this.
async fn all_statuses() -> HttpResponse {
    let statuses = actix_web::rt::task::spawn_blocking(|| {
        let mut map = serde_json::Map::new();
        for tool in spec::TOOLS {
            map.insert(tool.short_id().to_owned(), tool_status_body(tool));
        }
        map
    })
    .await;

    match statuses {
        Ok(map) => responses::json(StatusCode::OK, &serde_json::Value::Object(map)),
        Err(error) => responses::json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &serde_json::json!({ "error": format!("Could not read CLI tool statuses: {error}") }),
        ),
    }
}

async fn tool_status(path: web::Path<String>) -> HttpResponse {
    let segment = path.into_inner();
    let Some(tool) = spec::Tool::parse(&segment) else {
        return responses::json(
            StatusCode::NOT_FOUND,
            &responses::error("CLI tool route not found"),
        );
    };
    match actix_web::rt::task::spawn_blocking(move || tool_status_body(tool)).await {
        Ok(body) => responses::json(StatusCode::OK, &body),
        Err(error) => responses::json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &serde_json::json!({
                "error": format!("Could not read {} status: {error}", tool.id),
            }),
        ),
    }
}

/// `POST`/`PATCH` applies a config; `DELETE` revokes it.
///
/// The work is a handful of synchronous file reads and writes, so it runs on the blocking pool. On
/// an actix worker it would stall every other request that worker holds, and the dashboard polls
/// `all-statuses` while this runs.
async fn mutate_tool(
    path: web::Path<String>,
    method: actix_web::http::Method,
    body: web::Bytes,
) -> HttpResponse {
    let segment = path.into_inner();
    let Some(tool) = spec::Tool::parse(&segment) else {
        return responses::json(
            StatusCode::NOT_FOUND,
            &responses::error("CLI tool route not found"),
        );
    };

    let Some(writer) = mutations::writer_for(tool.id) else {
        // Not a gap: upstream exports no mutation for this tool either. `spec`'s `writable` flag
        // says the same thing, and a test holds the two together.
        return responses::json(
            StatusCode::NOT_IMPLEMENTED,
            &UnsupportedMutation {
                success: false,
                unsupported: true,
                message: TOOL_UNSUPPORTED,
            },
        );
    };

    let direction = if method == actix_web::http::Method::DELETE {
        mutations::Direction::Revoke
    } else {
        mutations::Direction::Apply
    };

    // A revoke carries no body upstream, and an apply's body is required. Parsed before the
    // blocking hop so a malformed body costs nothing.
    let payload = if direction == mutations::Direction::Revoke && body.is_empty() {
        mutations::Payload::default()
    } else {
        match json_body::parse::<mutations::Payload>(&body) {
            Ok(payload) => payload,
            Err(response) => return response,
        }
    };

    if direction == mutations::Direction::Apply
        && let Err(error) = (writer.validate)(&payload)
    {
        // Before any file is opened, so a rejected request leaves the disk untouched.
        return responses::json(StatusCode::BAD_REQUEST, &responses::error(error));
    }

    match actix_web::rt::task::spawn_blocking(move || writer.run(direction, &payload)).await {
        Ok(Ok(outcome)) => responses::json(StatusCode::OK, &mutation_body(tool, direction, &outcome)),
        Ok(Err(error)) => responses::json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &serde_json::json!({ "error": error.message() }),
        ),
        Err(error) => responses::json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &serde_json::json!({
                "error": format!("Could not write {} settings: {error}", tool.id),
            }),
        ),
    }
}

/// The response body for a completed mutation.
///
/// `success` and `message` are upstream's, so the dashboard needs no changes. The rest are
/// additions this port makes because it has the information: which files were touched, where the
/// backups went, and which best-effort targets failed. Upstream discards that last one silently,
/// which leaves a user whose VS Code settings are read-only with nothing to go on.
fn mutation_body(
    tool: &spec::Tool,
    direction: mutations::Direction,
    outcome: &mutations::Outcome,
) -> serde_json::Value {
    let message = match (direction, outcome.nothing_to_do) {
        (mutations::Direction::Apply, _) => {
            format!("{} settings applied successfully!", tool.display_name)
        }
        (mutations::Direction::Revoke, false) => {
            format!("9Router settings removed from {}", tool.display_name)
        }
        (mutations::Direction::Revoke, true) => "No settings file to reset".to_owned(),
    };
    let paths = |items: &[std::path::PathBuf]| -> serde_json::Value {
        items
            .iter()
            .map(|path| serde_json::Value::String(path.display().to_string()))
            .collect()
    };
    let mut body = serde_json::json!({
        "success": true,
        "message": message,
        "written": paths(&outcome.written),
        "backedUp": paths(&outcome.backed_up),
    });
    if !outcome.warnings.is_empty() {
        let warnings = outcome
            .warnings
            .iter()
            .map(|warning| serde_json::Value::String(warning.clone()))
            .collect();
        responses::insert_key(&mut body, "warnings", warnings);
    }
    // Upstream names the written path per tool, so the dashboard's success pane finds whichever it
    // reads.
    if let Some(first) = outcome.written.first().map(|path| path.display().to_string()) {
        for key in ["settingsPath", "configPath", "authPath", "globalStatePath"] {
            responses::insert_key(&mut body, key, serde_json::Value::String(first.clone()));
        }
    }
    body
}

/// One tool's status as the dashboard reads it.
///
/// Built from a real filesystem look, not a constant. The field names follow upstream's response
/// so the dashboard needs no changes; the two extra fields (`source`, `configError`) are additions
/// this port makes because it has the information and hiding it helps nobody.
fn tool_status_body(tool: &spec::Tool) -> serde_json::Value {
    let status = detect::status(tool);
    let mut body = serde_json::json!({
        "installed": status.installed,
        "has9Router": status.has_router,
        "settings": status.settings,
        "displayName": tool.display_name,
        "writable": status.installed && tool.writable == spec::Writable::Yes,
    });

    // Upstream names this field differently per tool: `settingsPath` for claude and droid,
    // `configPath` for codex and copilot, `authPath` for kilo, `globalStatePath` for cline. All of
    // them are sent so whichever the dashboard reads is present, rather than picking one and
    // leaving a pane blank.
    if let Some(path) = status
        .config_path
        .as_ref()
        .map(|path| path.display().to_string())
    {
        for key in ["settingsPath", "configPath", "authPath", "globalStatePath"] {
            responses::insert_key(&mut body, key, serde_json::Value::String(path.clone()));
        }
    }
    if let Some(source) = status.source {
        responses::insert_key(&mut body, "source", serde_json::Value::String(source));
    }
    if let Some(error) = status.parse_error {
        // Reported alongside `settings: null` rather than as a 500: upstream swallows the error so
        // the UI does not read it as "not installed", but swallowing it silently means a user with
        // a stray comma sees "not configured" and no reason.
        responses::insert_key(&mut body, "configError", serde_json::Value::String(error));
    }
    if !status.installed {
        responses::insert_key(
            &mut body,
            "message",
            serde_json::Value::String(format!(
                "{} is not installed: no binary on PATH and no config file.",
                tool.display_name
            )),
        );
    }
    body
}
