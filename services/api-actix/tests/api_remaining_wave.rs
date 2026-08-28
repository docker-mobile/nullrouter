#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::Value;

use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A closed loopback port: usage reads fall back to the zeroed shape,
/// so these parity tests need no state service.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

async fn request_json(method: Method, uri: &str, body: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
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
    let body = to_bytes(res.into_body()).await?;
    let json = serde_json::from_slice(&body)?;
    Ok((status, json))
}

async fn get_json(uri: &str) -> TestResult<(StatusCode, Value)> {
    request_json(Method::GET, uri, "").await
}

/// The status of a `GET`, without requiring a JSON body.
///
/// Used to assert a route is *absent*: a 404 from Actix carries an empty body,
/// so [`get_json`] would fail to parse it before the status could be checked.
async fn status_of(uri: &str) -> TestResult<StatusCode> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default().uri(uri).to_request();
    Ok(test::call_service(&app, req).await.status())
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

#[actix_rt::test]
async fn cli_tool_routes_return_default_status_json() -> TestResult {
    // Given: the Actix API does not inspect or modify host CLI configuration.

    // When: CLI tool status endpoints are requested.
    let (all_status, all) = get_json("/api/cli-tools/all-statuses").await?;
    let setting_routes = [
        "/api/cli-tools/codex-settings",
        "/api/cli-tools/claude-settings",
        "/api/cli-tools/cline-settings",
        "/api/cli-tools/opencode-settings",
        "/api/cli-tools/copilot-settings",
    ];
    let (mitm_status, mitm) = get_json("/api/cli-tools/antigravity-mitm").await?;

    // Then: each tool is represented as unconfigured JSON instead of a route miss.
    assert_eq!(all_status, StatusCode::OK);
    for tool in ["codex", "claude", "cline", "opencode", "copilot"] {
        let status = field(&all, tool)?;
        assert_eq!(field(status, "installed")?, false, "{tool}");
        assert_eq!(field(status, "has9Router")?, false, "{tool}");
    }
    for uri in setting_routes {
        let (status, json) = get_json(uri).await?;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(field(&json, "installed")?, false, "{uri}");
        assert_eq!(field(&json, "has9Router")?, false, "{uri}");
    }
    assert_eq!(mitm_status, StatusCode::OK);
    assert_eq!(field(&mitm, "running")?, false);
    assert_eq!(field(&mitm, "certTrusted")?, false);
    Ok(())
}

#[actix_rt::test]
async fn headroom_and_tunnel_routes_return_safe_defaults() -> TestResult {
    // Given: this service cannot start local Headroom, Cloudflare, or Tailscale processes.

    // When: status and mutation endpoints are requested.
    let (headroom_status, headroom) = get_json("/api/headroom/status").await?;
    let (headroom_start_status, headroom_start) =
        request_json(Method::POST, "/api/headroom/start", "").await?;
    let (headroom_stop_status, headroom_stop) =
        request_json(Method::POST, "/api/headroom/stop", "").await?;
    let (proxy_status, proxy) = get_json("/api/headroom/proxy/v1/models").await?;
    let (tunnel_status, tunnel) = get_json("/api/tunnel/status").await?;
    let (tunnel_enable_status, tunnel_enable) =
        request_json(Method::POST, "/api/tunnel/enable", "").await?;
    let (tailscale_check_status, tailscale_check) = get_json("/api/tunnel/tailscale-check").await?;
    let (tailscale_install_status, tailscale_install) =
        request_json(Method::POST, "/api/tunnel/tailscale-install", "").await?;

    // Then: process-changing routes are explicit no-ops and status routes stay structured.
    assert_eq!(headroom_status, StatusCode::OK);
    assert_eq!(field(&headroom, "running")?, false);
    assert_eq!(headroom_start_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&headroom_start, "success")?, false);
    assert_eq!(headroom_stop_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&headroom_stop, "success")?, false);
    assert_eq!(proxy_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&proxy, "unsupported")?, true);
    assert_eq!(tunnel_status, StatusCode::OK);
    assert_eq!(field(field(&tunnel, "tunnel")?, "enabled")?, false);
    assert_eq!(tunnel_enable_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&tunnel_enable, "success")?, false);
    assert_eq!(tailscale_check_status, StatusCode::OK);
    assert_eq!(field(&tailscale_check, "installed")?, false);
    assert_eq!(tailscale_install_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&tailscale_install, "success")?, false);
    Ok(())
}

#[actix_rt::test]
async fn translator_routes_return_defaults_and_validate_json() -> TestResult {
    // Given: translator logs and provider execution are unavailable in this deterministic slice.

    // When: translator endpoints are requested.
    let (load_status, load) = get_json("/api/translator/load?file=1_req_client.json").await?;
    let (logs_status, logs) = get_json("/api/translator/console-logs").await?;
    let (save_status, save) = request_json(
        Method::POST,
        "/api/translator/save",
        r#"{"file":"1_req_client.json","content":"{}"}"#,
    )
    .await?;
    let (send_status, send) = request_json(
        Method::POST,
        "/api/translator/send",
        r#"{"provider":"openai","model":"gpt-5","body":{}}"#,
    )
    .await?;
    let (translate_status, translate) = request_json(
        Method::POST,
        "/api/translator/translate",
        r#"{"step":1,"body":{"model":"openai/gpt-5"}}"#,
    )
    .await?;
    let (malformed_status, malformed) =
        request_json(Method::POST, "/api/translator/translate", "{").await?;

    // Then: they return JSON defaults, and malformed POST bodies fail at the boundary.
    assert_eq!(load_status, StatusCode::OK);
    assert_eq!(field(&load, "success")?, false);
    assert_eq!(logs_status, StatusCode::OK);
    assert_eq!(field(&logs, "logs")?, &serde_json::json!([]));
    assert_eq!(save_status, StatusCode::OK);
    assert_eq!(field(&save, "success")?, false);
    assert_eq!(send_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&send, "success")?, false);
    assert_eq!(translate_status, StatusCode::OK);
    assert_eq!(field(&translate, "success")?, true);
    assert_eq!(malformed_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&malformed, "error")?, "Invalid JSON body");
    Ok(())
}

#[actix_rt::test]
async fn media_and_settings_routes_return_default_json() -> TestResult {
    // Given: no media provider accounts or persisted settings are present.

    // When: media voice and settings endpoints are requested.
    let (voices_status, voices) = get_json("/api/media-providers/tts/voices").await?;
    let (provider_voices_status, provider_voices) =
        get_json("/api/media-providers/tts/elevenlabs/voices").await?;
    let (database_status, database) = get_json("/api/settings/database").await?;
    let require_login_status = status_of("/api/settings/require-login").await?;
    let (proxy_test_status, proxy_test) = request_json(
        Method::POST,
        "/api/settings/proxy-test",
        r#"{"proxyUrl":"http://127.0.0.1:8080","testUrl":"https://example.com","timeoutMs":100}"#,
    )
    .await?;
    let (malformed_status, malformed) =
        request_json(Method::POST, "/api/settings/proxy-test", "{").await?;

    // Then: defaults remain structured and settings mutations parse JSON at the edge.
    assert_eq!(voices_status, StatusCode::OK);
    assert_eq!(field(&voices, "voices")?, &serde_json::json!([]));
    assert_eq!(field(&voices, "languages")?, &serde_json::json!([]));
    assert_eq!(provider_voices_status, StatusCode::OK);
    assert_eq!(field(&provider_voices, "voices")?, &serde_json::json!([]));
    assert_eq!(database_status, StatusCode::OK);
    assert_eq!(field(&database, "success")?, true);
    // `GET /api/settings/require-login` is deliberately absent: dashboard login is
    // unconditional in nullrouter, so there is no value to report. A 200 here
    // would mean the route came back and is answering with an invented flag.
    assert_eq!(require_login_status, StatusCode::NOT_FOUND);
    assert_eq!(proxy_test_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&proxy_test, "ok")?, false);
    assert_eq!(malformed_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&malformed, "error")?, "Invalid JSON body");
    Ok(())
}

#[actix_rt::test]
async fn provider_node_and_shutdown_routes_return_structured_outcomes() -> TestResult {
    // Given: provider nodes and app lifecycle operations are not persisted by this slice.

    // When: provider node and lifecycle endpoints are requested.
    let (nodes_status, nodes) = get_json("/api/provider-nodes").await?;
    let (validate_status, validate) =
        request_json(Method::POST, "/api/provider-nodes/validate", "{}").await?;
    let (malformed_status, malformed) =
        request_json(Method::POST, "/api/provider-nodes/validate", "{").await?;
    let (node_status, node) = get_json("/api/provider-nodes/missing").await?;
    let (shutdown_status, shutdown) = request_json(Method::POST, "/api/shutdown", "").await?;
    let (update_status, update) = request_json(Method::POST, "/api/version/update", "").await?;
    let (version_shutdown_status, version_shutdown) =
        request_json(Method::POST, "/api/version/shutdown", "").await?;

    // Then: every route returns explicit JSON instead of default HTML/404 behavior.
    assert_eq!(nodes_status, StatusCode::OK);
    assert_eq!(field(&nodes, "nodes")?, &serde_json::json!([]));
    assert_eq!(validate_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&validate, "error")?, "Base URL and API key required");
    assert_eq!(malformed_status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&malformed, "error")?, "Invalid JSON body");
    assert_eq!(node_status, StatusCode::NOT_FOUND);
    assert_eq!(field(&node, "error")?, "Provider node not found");
    assert_eq!(shutdown_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&shutdown, "success")?, false);
    assert_eq!(update_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&update, "success")?, false);
    assert_eq!(version_shutdown_status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&version_shutdown, "success")?, false);
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
