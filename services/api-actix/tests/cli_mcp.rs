#![allow(clippy::future_not_send)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "free helpers here are not #[test] fns, so clippy.toml's allow-expect-in-tests does \
              not cover them"
)]

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::Value;

use nullrouter_api::{AppConfig, RuntimeClient, StateClient, TunnelManager, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A closed loopback port: usage reads fall back to the zeroed shape,
/// so these parity tests need no state service.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

struct ApiResponse {
    status: StatusCode,
    body: String,
}

async fn request(method: Method, uri: &str, body: &str) -> TestResult<ApiResponse> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(TunnelManager::new()))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok(ApiResponse { status, body })
}

async fn request_json(method: Method, uri: &str, body: &str) -> TestResult<(StatusCode, Value)> {
    let response = request(method, uri, body).await?;
    Ok((response.status, serde_json::from_str(&response.body)?))
}

async fn get_json(uri: &str) -> TestResult<(StatusCode, Value)> {
    request_json(Method::GET, uri, "").await
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

/// A stub registry serving two pages, exercised through the real route.
///
/// Pointed at with `NULLROUTER_MCP_REGISTRY_URL` rather than calling the real registry: a test that
/// reached out to Anthropic would depend on the network and on whatever the registry happens to be
/// listing that day.
async fn stub_registry(pages: Vec<String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();
    let served = Arc::new(Mutex::new(0_usize));

    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let pages = pages.clone();
            let served = Arc::clone(&served);
            tokio::spawn(async move {
                let mut discard = [0_u8; 4096];
                let _ = stream.read(&mut discard).await;
                let index = {
                    let mut count = served
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let index = *count;
                    *count += 1;
                    index
                };
                let body = pages.get(index).cloned().unwrap_or_else(|| "{}".to_owned());
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    format!("http://{addr}/servers")
}

#[actix_rt::test]
async fn the_registry_lists_what_a_client_here_can_connect_to_directly() -> TestResult {
    // Given: a registry serving two pages, holding one usable entry, one duplicate of it, and
    // three that must each be filtered out for a different reason.
    let first = serde_json::json!({
        "servers": [
            {
                "server": {
                    "name": "com.example/mcp",
                    "title": "Example",
                    "description": "An example server",
                    "remotes": [{"url": "https://mcp.example.com/mcp", "type": "sse"}],
                },
                "_meta": {"com.anthropic.api/mcp-registry": {
                    "slug": "example", "isAuthless": true,
                    "toolNames": ["one", "two"], "iconUrl": "https://example.com/icon.png",
                }},
            },
            {
                // claude.ai-mediated, which does not work in this mode.
                "server": {"name": "a", "remotes": [{"url": "https://mcp.claude.com/x"}]},
                "_meta": {},
            },
            {
                // An unfilled template.
                "server": {"name": "b", "remotes": [{"url": "https://{tenant}.example.com/mcp"}]},
                "_meta": {},
            },
            {
                // Needs tenant-specific fields, which there is nowhere to supply from this pane.
                "server": {"name": "c", "remotes": [{"url": "https://ok.example.com/mcp"}]},
                "_meta": {"com.anthropic.api/mcp-registry": {"requiredFields": ["workspace"]}},
            },
        ],
        "metadata": {"nextCursor": "page-2"},
    })
    .to_string();
    let second = serde_json::json!({
        "servers": [{
            // The same URL again, under a different name.
            "server": {"name": "com.example/mirror", "remotes": [{"url": "https://mcp.example.com/mcp"}]},
            "_meta": {},
        }],
    })
    .to_string();

    let url = stub_registry(vec![first, second]).await;
    // SAFETY: no other thread in this binary reads this variable, and it is removed again
    // below before any other case runs.
    unsafe { std::env::set_var("NULLROUTER_MCP_REGISTRY_URL", &url) };

    // When: the pane loads, bypassing the cache so the fetch actually happens.
    let (status, registry) = get_json("/api/cli-tools/cowork-mcp-registry?refresh=1").await?;

    // SAFETY: still the only thread touching this variable, and it is cleared before any
    // other case in this binary reads it.
    unsafe { std::env::remove_var("NULLROUTER_MCP_REGISTRY_URL") };

    // Then: one entry survives, with its fields flattened the way the pane reads them.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(field(&registry, "total")?, 1, "{registry}");
    let servers = field(&registry, "servers")?
        .as_array()
        .ok_or_else(|| test_error("servers is not an array"))?;
    let entry = servers
        .first()
        .ok_or_else(|| test_error("no entry survived the filter"))?;
    assert_eq!(entry["name"], "com.example/mcp");
    assert_eq!(entry["slug"], "example");
    assert_eq!(entry["title"], "Example");
    assert_eq!(entry["url"], "https://mcp.example.com/mcp");
    assert_eq!(entry["transport"], "sse");
    // `isAuthless: true` inverts to no OAuth step.
    assert_eq!(entry["oauth"], false);
    assert_eq!(entry["toolCount"], 2);
    assert_eq!(entry["iconUrl"], "https://example.com/icon.png");
    Ok(())
}

#[actix_rt::test]
async fn the_tool_probe_refuses_a_target_only_this_service_can_reach() -> TestResult {
    // The probe has this service fetch a URL the caller supplies, so it must not be usable to
    // reach the loopback services on 20129-20135 or anything else on the host's networks.
    for url in [
        "http://127.0.0.1:20134/internal/v1/keys",
        "https://169.254.169.254/latest/meta-data/",
        "http://mcp.example.com/mcp",
    ] {
        let (status, body) = request_json(
            Method::POST,
            "/api/cli-tools/cowork-mcp-tools",
            &serde_json::json!({"url": url}).to_string(),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{url} was not refused: {body}"
        );
        assert_eq!(field(&body, "tools")?, &serde_json::json!([]), "{url}");
    }
    Ok(())
}

#[actix_rt::test]
async fn cowork_mcp_routes_validate_json_and_options_boundaries() -> TestResult {
    // Given: Cowork MCP tool probing accepts only JSON with a non-empty url.

    // When: malformed, missing, and preflight requests are sent.
    let malformed = request_json(Method::POST, "/api/cli-tools/cowork-mcp-tools", "{").await?;
    let missing = request_json(
        Method::POST,
        "/api/cli-tools/cowork-mcp-tools",
        r#"{"url":""}"#,
    )
    .await?;
    let tools_options = request(Method::OPTIONS, "/api/cli-tools/cowork-mcp-tools", "").await?;
    let registry_options =
        request(Method::OPTIONS, "/api/cli-tools/cowork-mcp-registry", "").await?;

    // Then: boundaries are explicit structured JSON or CORS no-content.
    assert_eq!(malformed.0, StatusCode::BAD_REQUEST);
    assert_eq!(field(&malformed.1, "error")?, "Invalid JSON body");
    assert_eq!(missing.0, StatusCode::BAD_REQUEST);
    assert_eq!(field(&missing.1, "error")?, "url required");
    assert_eq!(tools_options.status, StatusCode::NO_CONTENT);
    assert_eq!(registry_options.status, StatusCode::NO_CONTENT);
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
