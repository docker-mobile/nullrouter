//! Cowork's MCP registry and tool discovery.
//!
//! Two routes the dashboard uses to fill its "which MCP servers can I add" pane: one lists what
//! Anthropic's public registry offers, the other asks a specific server what tools it has.
//!
//! # The registry fetch
//!
//! An outbound `GET` to a fixed public URL, sending nothing about the user or their config. Paged,
//! with both a page cap and a deadline, and cached for an hour — the dashboard polls this pane, and
//! a registry that has gone slow should not make the pane hang.
//!
//! # Why the probe is restricted and upstream's is not
//!
//! `POST /api/cli-tools/cowork-mcp-tools` takes a URL from the caller and has this service fetch
//! it. Upstream accepts any URL at all, which makes it a server-side request forgery pivot: this
//! process can reach the loopback services on 20129-20135 and any address on the host's networks,
//! none of which the caller can reach directly. So the target here must be `https://` and must not
//! resolve to a loopback, private, link-local or unspecified address.
//!
//! That is a real restriction rather than a theoretical one, and it costs nothing a user wants: the
//! registry this pane is populated from only ever yields `https://` entries, because upstream's own
//! `isDirectConnect` requires it.

use std::time::Duration;

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{json_body, responses};

/// Anthropic's public MCP registry.
const REGISTRY_URL: &str = "https://api.anthropic.com/mcp-registry/v0/servers";

/// Overrides [`REGISTRY_URL`], so the paging and dedup here can be tested against a local mock.
///
/// Read from the process environment, which only whoever starts the service controls — it is not
/// reachable from a request, and so is not a way to redirect this fetch from outside. Without it the
/// only way to cover the paging loop is to call the real registry from a test, which makes the suite
/// depend on the network and on what the registry happens to be listing that day.
const REGISTRY_URL_VAR: &str = "NULLROUTER_MCP_REGISTRY_URL";

fn registry_url() -> String {
    std::env::var(REGISTRY_URL_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| REGISTRY_URL.to_owned())
}

/// Upstream's visibility filter, passed through verbatim.
const VISIBILITY: &str = "commercial,gsuite,gsuite-google";

/// How long a fetched listing is served from memory.
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);

/// Page cap. Upstream's is 20 pages of 500; the same bound is kept so a registry that paged forever
/// could not hold a worker.
const MAX_PAGES: usize = 20;
const PAGE_SIZE: usize = 500;

/// Whole-fetch deadline, across every page.
const REGISTRY_TIMEOUT: Duration = Duration::from_secs(20);

/// Per-request timeout for a tool probe. Upstream's 8 seconds.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// The MCP revision the probe announces, matching upstream.
const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolsRequest {
    url: String,
}

#[derive(Debug, Deserialize)]
struct RefreshQuery {
    #[serde(default)]
    refresh: Option<String>,
}

/// The cached listing.
///
/// A plain mutex rather than a lock-free cache: it is read once per pane load, and the work it
/// guards is a network fetch.
static CACHE: std::sync::Mutex<Option<(std::time::Instant, Value)>> = std::sync::Mutex::new(None);

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/cli-tools/cowork-mcp-registry")
                .route(web::get().to(registry))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/api/cli-tools/cowork-mcp-tools")
                .route(web::post().to(tools))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        );
}

async fn no_content() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}

/// The registry listing, from cache unless `?refresh=1`.
async fn registry(query: web::Query<RefreshQuery>) -> HttpResponse {
    let forced = query.refresh.as_deref() == Some("1");
    if !forced
        && let Some(cached) = cached_listing()
    {
        let mut body = cached;
        super::mutations::insert_key(&mut body, "cached", Value::Bool(true));
        return responses::json(StatusCode::OK, &body);
    }

    match fetch_registry().await {
        Ok(servers) => {
            let body = serde_json::json!({
                "cached": false,
                "servers": servers,
                "total": servers.len(),
            });
            store_listing(&body);
            responses::json(StatusCode::OK, &body)
        }
        // Served stale rather than failed: a listing an hour old is more use to the pane than an
        // error, and the entries are descriptions of public servers rather than anything that goes
        // wrong for being out of date.
        Err(error) => match stale_listing() {
            Some(mut body) => {
                super::mutations::insert_key(&mut body, "cached", Value::Bool(true));
                super::mutations::insert_key(&mut body, "stale", Value::Bool(true));
                super::mutations::insert_key(&mut body, "error", Value::String(error));
                responses::json(StatusCode::OK, &body)
            }
            None => responses::json(
                StatusCode::BAD_GATEWAY,
                &serde_json::json!({
                    "servers": [],
                    "total": 0,
                    "error": format!("Could not read the MCP registry: {error}"),
                }),
            ),
        },
    }
}

fn cached_listing() -> Option<Value> {
    let guard = CACHE.lock().ok()?;
    let (stored, body) = guard.as_ref()?;
    (stored.elapsed() < CACHE_TTL).then(|| body.clone())
}

/// The cached listing regardless of age, for the case where a refetch failed.
fn stale_listing() -> Option<Value> {
    let guard = CACHE.lock().ok()?;
    guard.as_ref().map(|(_, body)| body.clone())
}

fn store_listing(body: &Value) {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((std::time::Instant::now(), body.clone()));
    }
}

/// Walk the registry's pages and flatten what is directly connectable.
async fn fetch_registry() -> Result<Vec<Value>, String> {
    let client = reqwest::Client::builder()
        .timeout(REGISTRY_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let deadline = std::time::Instant::now() + REGISTRY_TIMEOUT;

    let mut servers: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut cursor = String::new();

    for _page in 0..MAX_PAGES {
        if std::time::Instant::now() >= deadline {
            break;
        }
        let mut url = format!("{}?limit={PAGE_SIZE}&visibility={VISIBILITY}", registry_url());
        if !cursor.is_empty() {
            url.push_str("&cursor=");
            url.push_str(&urlencoding(&cursor));
        }
        let response = client
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            // Upstream breaks out of the loop rather than failing, keeping whatever it already has.
            break;
        }
        let page: Value = response.json().await.map_err(|error| error.to_string())?;

        for item in page.get("servers").and_then(Value::as_array).into_iter().flatten() {
            if let Some(entry) = registry_entry(item)
                && let Some(url) = entry.get("url").and_then(Value::as_str)
            {
                // Deduped by URL, since the same server appears under several visibilities.
                if !seen.iter().any(|existing| existing == url) {
                    seen.push(url.to_owned());
                    servers.push(entry);
                }
            }
        }

        cursor = page
            .get("metadata")
            .and_then(|metadata| metadata.get("nextCursor"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if cursor.is_empty() {
            break;
        }
    }
    Ok(servers)
}

/// One registry item flattened to what the pane shows, or `None` if it is not usable.
fn registry_entry(item: &Value) -> Option<Value> {
    let server = item.get("server")?;
    let meta = item
        .get("_meta")
        .and_then(|meta| meta.get("com.anthropic.api/mcp-registry"))
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));

    // Entries needing tenant-specific fields are skipped: there is nothing for a user to fill in
    // from this pane, so listing one offers a server that cannot be added.
    if meta
        .get("requiredFields")
        .and_then(Value::as_array)
        .is_some_and(|fields| !fields.is_empty())
    {
        return None;
    }

    let remote = server.get("remotes").and_then(Value::as_array)?.first()?;
    let url = remote.get("url").and_then(Value::as_str)?;
    if !is_direct_connect(url) {
        return None;
    }

    let name = server.get("name").and_then(Value::as_str).unwrap_or_default();
    let text = |value: Option<&Value>| value.and_then(Value::as_str).unwrap_or_default().to_owned();
    let tool_names = meta
        .get("toolNames")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    Some(serde_json::json!({
        "name": name,
        "slug": first_non_empty(&[text(meta.get("slug")), name.to_owned()]),
        "title": first_non_empty(&[
            text(server.get("title")),
            text(meta.get("displayName")),
            name.to_owned(),
        ]),
        "description": first_non_empty(&[
            text(server.get("description")),
            text(meta.get("oneLiner")),
        ]),
        "url": url,
        "transport": if remote.get("type").and_then(Value::as_str) == Some("sse") { "sse" } else { "http" },
        // `isAuthless` inverted, per upstream: an entry that does not say it is authless needs
        // OAuth, so the pane shows a connect step rather than silently failing later.
        "oauth": !meta.get("isAuthless").and_then(Value::as_bool).unwrap_or(false),
        "toolNames": tool_names,
        "toolCount": tool_names.len(),
        "iconUrl": meta.get("iconUrl").cloned().unwrap_or(Value::Null),
    }))
}

fn first_non_empty(candidates: &[String]) -> String {
    candidates
        .iter()
        .find(|candidate| !candidate.is_empty())
        .cloned()
        .unwrap_or_default()
}

/// Whether a registry URL is one a client here can connect to directly.
///
/// Upstream's `isDirectConnect`, and each of its four rejections is load-bearing: `mcp.claude.com`
/// and `api.anthropic.com/mcp` are mediated by claude.ai and do not work in this mode, a URL with
/// `<` or `{` in it is an unfilled template, and anything not `https://` is not offered at all.
fn is_direct_connect(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("https://") {
        return false;
    }
    if url.contains('<') || url.contains('{') {
        return false;
    }
    let host_and_path = lower.trim_start_matches("https://");
    let host = host_and_path
        .split('/')
        .next()
        .unwrap_or_default()
        .split('@')
        .next_back()
        .unwrap_or_default();
    if host == "mcp.claude.com" || host.ends_with(".mcp.claude.com") {
        return false;
    }
    if host == "api.anthropic.com" && host_and_path.starts_with("api.anthropic.com/mcp") {
        return false;
    }
    true
}

/// Ask one MCP server what tools it exposes.
async fn tools(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<ToolsRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let url = request.url.trim();
    if url.is_empty() {
        return responses::json(StatusCode::BAD_REQUEST, &responses::error("url required"));
    }
    if let Err(refusal) = probe_target_is_allowed(url) {
        // A 400 naming the reason rather than a probe. This is the difference between this route and
        // upstream's, which fetches whatever it is handed.
        return responses::json(
            StatusCode::BAD_REQUEST,
            &serde_json::json!({ "error": refusal, "tools": [] }),
        );
    }

    match probe(url).await {
        Ok(result) => responses::json(StatusCode::OK, &result),
        Err(error) => responses::json(
            StatusCode::OK,
            &serde_json::json!({ "error": error, "tools": [] }),
        ),
    }
}

/// Whether this service will fetch `url` on a caller's behalf.
///
/// The check upstream does not do. Without it the route is a server-side request forgery pivot:
/// this process can reach the loopback services on 20129-20135 and every address on the host's
/// networks, none of which the caller can reach directly.
fn probe_target_is_allowed(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|error| format!("Invalid url: {error}"))?;
    if parsed.scheme() != "https" {
        return Err(format!(
            "Only https MCP endpoints can be probed, not {:?}. Every entry the registry offers is \
             https, so this is not a limit on what can be added.",
            parsed.scheme()
        ));
    }
    let host = parsed.host_str().ok_or_else(|| "Invalid url: no host".to_owned())?;
    if crate::proxy_test::is_local_target(host) {
        return Err(format!(
            "Refusing to probe {host}: it is a loopback or private address, which this service can \
             reach and the caller cannot."
        ));
    }
    Ok(())
}

/// `initialize`, then `notifications/initialized`, then `tools/list`.
///
/// The middle step is not optional decoration: the MCP specification requires the notification
/// before any other request, and a compliant server answers `tools/list` with an error without it.
async fn probe(url: &str) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;

    let initialise = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "nullrouter", "version": "1"},
        },
    });
    let response = send(&client, url, None, &initialise)
        .await
        .map_err(|error| error.to_string())?;
    if matches!(response.status().as_u16(), 401 | 403) {
        // Not an error: an OAuth server refusing an unauthenticated probe is the expected answer,
        // and the pane shows a connect step instead of a tool list.
        return Ok(serde_json::json!({"requiresAuth": true, "tools": []}));
    }
    if !response.status().is_success() {
        return Ok(serde_json::json!({
            "error": format!("init {}", response.status().as_u16()),
            "tools": [],
        }));
    }
    // Carried on every later request when present: a session-based server rejects requests without
    // it, and losing it turns a working probe into an empty tool list.
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    // Drained so the connection can be reused.
    let _ = response.text().await;

    let initialised = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {},
    });
    // Failure is ignored, as upstream does: a server that does not want the notification still
    // answers the list.
    let _ = send(&client, url, session.as_deref(), &initialised).await;

    let list = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    let response = send(&client, url, session.as_deref(), &list)
        .await
        .map_err(|error| error.to_string())?;
    if matches!(response.status().as_u16(), 401 | 403) {
        return Ok(serde_json::json!({"requiresAuth": true, "tools": []}));
    }
    let streamed = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/event-stream"));
    let text = response.text().await.map_err(|error| error.to_string())?;

    let parsed = if streamed {
        parse_sse_result(&text)
    } else {
        serde_json::from_str::<Value>(&text).ok()
    };
    let tools = parsed
        .as_ref()
        .and_then(|message| message.get("result"))
        .and_then(|result| result.get("tools"))
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .map(|tool| {
                    serde_json::json!({
                        "name": tool.get("name").and_then(Value::as_str).unwrap_or_default(),
                        "description": tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    })
                })
                .collect::<Vec<Value>>()
        })
        .unwrap_or_default();
    Ok(serde_json::json!({"tools": tools}))
}

async fn send(
    client: &reqwest::Client,
    url: &str,
    session: Option<&str>,
    body: &Value,
) -> reqwest::Result<reqwest::Response> {
    let mut request = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ACCEPT, "application/json, text/event-stream")
        .header("MCP-Protocol-Version", PROTOCOL_VERSION);
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    request.json(body).send().await
}

/// The `tools/list` reply out of an SSE body.
///
/// A streaming transport delivers the answer as `data:` lines, and there may be other messages
/// before it — so the one whose `id` is the request's is picked rather than the first that parses.
fn parse_sse_result(text: &str) -> Option<Value> {
    text.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|payload| serde_json::from_str::<Value>(payload.trim()).ok())
        .find(|message| {
            message.get("id").and_then(Value::as_u64) == Some(2) && message.get("result").is_some()
        })
}

/// Minimal percent-encoding for a cursor going into a query string.
fn urlencoding(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
