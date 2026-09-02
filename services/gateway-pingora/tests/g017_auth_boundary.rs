use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use http::{Method, StatusCode, header};
use nullrouter_contracts::{AuthorizeRequest, SecretString};
use nullrouter_gateway::{
    AccessDecision, AccessRequirement, AuthorizationState, GatewayConfig, GatewayUpstreamAddrs,
    RouteKind, authorization_request, stamp_trusted_identity_headers,
};
use pingora_http::RequestHeader;

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const REMOTE: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));

#[test]
fn route_ownership_selects_auth_without_disturbing_precedence() {
    // Given: the default one-port gateway configuration.
    let config = GatewayConfig::default();

    for (path, expected) in [
        ("/api/auth/status", RouteKind::Auth),
        ("/api/auth/login", RouteKind::Auth),
        ("/api/usage/stream", RouteKind::Events),
        ("/v1/chat/completions", RouteKind::Runtime),
        ("/api/catalog/providers", RouteKind::Catalog),
        ("/api/keys/key_1", RouteKind::State),
        ("/api/translator/load", RouteKind::Api),
        ("/dashboard/translator", RouteKind::Dashboard),
    ] {
        // When: each path is classified.
        let actual = config.route_for_path(path);

        // Then: Auth owns only its family and existing precedence remains intact.
        assert_eq!(actual, expected, "{path}");
    }
    assert_eq!(
        config.target_for(RouteKind::Auth).addr(),
        SocketAddr::new(LOOPBACK, 20135)
    );
}

#[test]
fn auth_upstream_rejects_non_loopback_address() {
    // Given: an Auth upstream outside the loopback trust boundary.
    let upstreams = GatewayUpstreamAddrs {
        auth: SocketAddr::new(REMOTE, 20135),
        ..GatewayUpstreamAddrs::default()
    };

    // When: gateway configuration is constructed.
    let result = GatewayConfig::new(SocketAddr::new(LOOPBACK, 20128), upstreams);

    // Then: startup rejects the untrusted Auth target.
    assert!(result.is_err());
}

#[test]
fn public_paths_bypass_authorize() {
    // Given: the public browser, asset, health, and Auth routes.
    let config = GatewayConfig::default();

    for path in [
        "/",
        "/login",
        "/landing",
        "/callback",
        "/favicon.svg",
        "/pkg/dashboard_leptos.js",
        "/providers/openai.png",
        "/assets/dashboard.css",
        "/api/health",
        "/api/auth/status",
        "/api/auth/login",
    ] {
        // When: access policy is evaluated for a loopback client.
        let requirement = config.access_requirement(path, &Method::GET, Some(LOOPBACK));

        // Then: no authorization transport is required.
        assert_eq!(requirement, AccessRequirement::Public, "{path}");
    }
}

#[test]
fn dashboard_denial_redirects_to_login() {
    // Given: a protected dashboard route without a valid session.
    let requirement =
        GatewayConfig::default().access_requirement("/dashboard", &Method::GET, Some(LOOPBACK));

    // When: Auth denies the session.
    let decision = requirement.decision(AuthorizationState::Denied);

    // Then: browser navigation receives the source-faithful login redirect.
    assert_eq!(decision, AccessDecision::RedirectToLogin);
    assert_eq!(decision.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(decision.location(), Some("/login"));
    assert_eq!(decision.body(), None);
}

#[test]
fn protected_api_denial_returns_json_401() {
    // Given: a protected API route without a valid session.
    let requirement = GatewayConfig::default().access_requirement(
        "/api/providers/client",
        &Method::GET,
        Some(LOOPBACK),
    );

    // When: Auth denies the session.
    let decision = requirement.decision(AuthorizationState::Denied);

    // Then: API callers receive JSON instead of a browser redirect.
    assert_eq!(decision, AccessDecision::Unauthorized);
    assert_eq!(decision.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(decision.location(), None);
    assert_eq!(decision.body(), Some(r#"{"error":"Unauthorized"}"#));
}

#[test]
fn authorization_unavailable_fails_closed() {
    // Given: protected dashboard and API requirements.
    let config = GatewayConfig::default();
    let dashboard = config.access_requirement("/dashboard", &Method::GET, Some(LOOPBACK));
    let api = config.access_requirement("/api/settings", &Method::GET, Some(LOOPBACK));

    // When: the Auth transport is unavailable.
    let dashboard_decision = dashboard.decision(AuthorizationState::Unavailable);
    let api_decision = api.decision(AuthorizationState::Unavailable);

    // Then: neither protected request is allowed through.
    assert_eq!(dashboard_decision, AccessDecision::RedirectToLogin);
    assert_eq!(api_decision, AccessDecision::Unauthorized);
}

#[test]
fn public_internal_paths_are_denied() {
    // Given: public attempts to reach the internal namespace.
    let config = GatewayConfig::default();

    for path in ["/internal", "/internal/v1/authorize", "/internal/health"] {
        // When: policy is evaluated even for a loopback socket.
        let requirement = config.access_requirement(path, &Method::GET, Some(LOOPBACK));

        // Then: the public port rejects the request before routing.
        assert_eq!(requirement, AccessRequirement::Forbidden, "{path}");
        assert_eq!(
            requirement.decision(AuthorizationState::Authorized),
            AccessDecision::Forbidden,
            "{path}"
        );
    }
}

#[test]
fn host_only_route_rejects_non_loopback_peer() {
    // Given: a spawn-capable host-only API route.
    let config = GatewayConfig::default();

    // When: the actual socket peer is remote rather than loopback.
    let remote =
        config.access_requirement("/api/cli-tools/cowork-settings", &Method::GET, Some(REMOTE));
    let local = config.access_requirement(
        "/api/cli-tools/cowork-settings",
        &Method::GET,
        Some(LOOPBACK),
    );

    // Then: the remote peer is forbidden while the local peer still needs a session.
    assert_eq!(remote, AccessRequirement::Forbidden);
    assert_eq!(local, AccessRequirement::ApiSession);
}

#[test]
fn cli_tool_config_writes_are_host_only_while_reads_are_not() {
    // Given: the per-tool settings route, which writes files under the operator's home
    // directory — `~/.claude/settings.json`, `~/.codex/config.toml`, VS Code user settings.
    let config = GatewayConfig::default();

    for path in [
        "/api/cli-tools/claude-settings",
        "/api/cli-tools/codex-settings",
        "/api/cli-tools/copilot-settings",
        "/api/cli-tools/all-statuses",
    ] {
        for method in [Method::POST, Method::PATCH, Method::DELETE, Method::PUT] {
            // When: a mutating request arrives from another machine, session and all.
            let remote = config.access_requirement(path, &method, Some(REMOTE));

            // Then: it is refused before routing. A session cookie taken from a browser
            // elsewhere must not rewrite this host's dotfiles.
            assert_eq!(remote, AccessRequirement::Forbidden, "{method} {path}");
            assert_eq!(
                remote.decision(AuthorizationState::Authorized),
                AccessDecision::Forbidden,
                "{method} {path} allowed a remote peer holding a valid session"
            );
            // And the same write from this host still needs a session — host-only is not
            // a bypass.
            assert_eq!(
                config.access_requirement(path, &method, Some(LOOPBACK)),
                AccessRequirement::ApiSession,
                "{method} {path}"
            );
        }

        // And the read stays reachable: it reports which tools are installed, which is what
        // the dashboard's status pane is. Holding it to loopback would blank that pane for
        // every remote user while protecting nothing.
        for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert_eq!(
                config.access_requirement(path, &method, Some(REMOTE)),
                AccessRequirement::ApiSession,
                "{method} {path} should stay readable with a session"
            );
        }
    }
}

#[test]
fn headroom_process_control_is_host_only_and_its_extras_writes_are_too() {
    // Given: start, stop and restart signal a process on this host, and POSTing extras installs
    // packages into a Python interpreter on it. Reading extras is the panel's compression pane.
    let config = GatewayConfig::default();

    for path in [
        "/api/headroom/start",
        "/api/headroom/stop",
        "/api/headroom/restart",
        "/api/headroom/proxy/v1/models",
    ] {
        for method in [Method::GET, Method::POST, Method::DELETE] {
            // When: the request arrives from another machine with a valid session.
            let remote = config.access_requirement(path, &method, Some(REMOTE));

            // Then: refused before routing, and not a bypass from this host either.
            assert_eq!(remote, AccessRequirement::Forbidden, "{method} {path}");
            assert_eq!(
                remote.decision(AuthorizationState::Authorized),
                AccessDecision::Forbidden,
                "{method} {path} allowed a remote peer holding a valid session"
            );
            assert_eq!(
                config.access_requirement(path, &method, Some(LOOPBACK)),
                AccessRequirement::ApiSession,
                "{method} {path}"
            );
        }
    }

    // The extras route splits by method: installing is host-only, reading is not.
    for method in [Method::POST, Method::DELETE, Method::PATCH, Method::PUT] {
        assert_eq!(
            config.access_requirement("/api/headroom/extras", &method, Some(REMOTE)),
            AccessRequirement::Forbidden,
            "{method} /api/headroom/extras installs or removes packages on this host"
        );
    }
    for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
        assert_eq!(
            config.access_requirement("/api/headroom/extras", &method, Some(REMOTE)),
            AccessRequirement::ApiSession,
            "{method} /api/headroom/extras is the compression pane and should stay readable"
        );
    }
}

#[test]
fn every_tunnel_route_is_host_only_including_its_reads() {
    // Given: the tunnel routes drive `cloudflared` and `tailscaled`. Unlike the CLI-tool reads
    // above, the reads here are held to loopback too: the operation catalog is a list of ways
    // to publish this host to the internet, and enumerating it is worth withholding even when
    // running one is what actually opens the route.
    let config = GatewayConfig::default();

    for path in [
        "/api/tunnel/status",
        "/api/tunnel/enable",
        "/api/tunnel/disable",
        "/api/tunnel/named/enable",
        "/api/tunnel/tailscale-check",
        "/api/tunnel/tailscale-enable",
        "/api/tunnel/tailscale-disable",
        "/api/tunnel/tailscale-install",
        "/api/tunnel/operations",
        "/api/tunnel/operations/cloudflared.tunnel.quick",
        "/api/tunnel/operations/tailscale.funnel.start",
        // A route added later under this prefix is covered by the same rule, which is the
        // point of matching the prefix rather than each path.
        "/api/tunnel/something-added-later",
    ] {
        for method in [
            Method::GET,
            Method::HEAD,
            Method::OPTIONS,
            Method::POST,
            Method::DELETE,
        ] {
            // When: the request comes from another machine, valid session and all.
            let remote = config.access_requirement(path, &method, Some(REMOTE));

            // Then: it is refused before routing.
            assert_eq!(remote, AccessRequirement::Forbidden, "{method} {path}");
            assert_eq!(
                remote.decision(AuthorizationState::Authorized),
                AccessDecision::Forbidden,
                "{method} {path} allowed a remote peer holding a valid session"
            );

            // And from this host it still needs a session: host-only is not a bypass.
            assert_eq!(
                config.access_requirement(path, &method, Some(LOOPBACK)),
                AccessRequirement::ApiSession,
                "{method} {path}"
            );
        }
    }
}

#[test]
fn the_all_method_host_only_routes_are_not_relaxed_by_the_write_rule() {
    // The write rule is additive. `cowork-settings` and `antigravity-mitm` are host-only for
    // every method — cowork injects MCP bridge entries and the MITM route spawns a proxy — and
    // adding `/api/cli-tools` as a write-only prefix must not have made their reads remote.
    let config = GatewayConfig::default();
    for path in [
        "/api/cli-tools/cowork-settings",
        "/api/cli-tools/antigravity-mitm",
        "/api/cli-tools/antigravity-mitm/alias",
    ] {
        for method in [Method::GET, Method::OPTIONS, Method::POST] {
            assert_eq!(
                config.access_requirement(path, &method, Some(REMOTE)),
                AccessRequirement::Forbidden,
                "{method} {path}"
            );
        }
    }
}

#[test]
fn pxpipe_install_and_start_are_host_only() {
    // Given: the two PXPIPE routes that can run `npm install pxpipe-proxy@latest`,
    // whose lifecycle scripts execute as the API service.
    let config = GatewayConfig::default();

    for path in ["/api/pxpipe/install", "/api/pxpipe/start"] {
        // When: the socket peer is remote, even with a valid dashboard session.
        let remote = config.access_requirement(path, &Method::GET, Some(REMOTE));
        let local = config.access_requirement(path, &Method::GET, Some(LOOPBACK));

        // Then: it is refused before routing. Stricter than upstream, which allows
        // these from any authenticated session: a session cookie taken from a browser
        // on another machine must not install software on this host.
        assert_eq!(remote, AccessRequirement::Forbidden, "{path}");
        assert_eq!(
            remote.decision(AuthorizationState::Authorized),
            AccessDecision::Forbidden,
            "{path} allowed a remote peer holding a valid session"
        );
        // And a local caller still needs a session — host-only is not a bypass.
        assert_eq!(local, AccessRequirement::ApiSession, "{path}");
    }
}

#[test]
fn the_read_only_pxpipe_routes_stay_reachable_with_a_session() {
    // Given: the routes that only report. Holding these to host-only would break the
    // dashboard for every remote user without protecting anything: they install
    // nothing and run no process.
    let config = GatewayConfig::default();
    for path in [
        "/api/pxpipe/status",
        "/api/pxpipe/health",
        "/api/pxpipe/stats",
        "/api/pxpipe/logs",
        "/api/pxpipe/stop",
        "/api/pxpipe/restart",
    ] {
        assert_eq!(
            config.access_requirement(path, &Method::GET, Some(REMOTE)),
            AccessRequirement::ApiSession,
            "{path}"
        );
    }
}

#[test]
fn runtime_key_enforcement_allows_valid_key() {
    // Given: managed API-key enforcement is active for runtime routes.
    let config = GatewayConfig::default().with_managed_api_key_enforcement(true);
    let requirement = config.access_requirement("/v1/models", &Method::GET, Some(REMOTE));

    // When: Auth validates the managed key.
    let decision = requirement.decision(AuthorizationState::Authorized);

    // Then: the runtime request may proceed.
    assert_eq!(requirement, AccessRequirement::RuntimeApiKey);
    assert_eq!(decision, AccessDecision::Allow);
}

#[test]
fn runtime_key_enforcement_denies_invalid_key() {
    // Given: managed API-key enforcement is active for both runtime families.
    let config = GatewayConfig::default().with_managed_api_key_enforcement(true);

    for path in ["/v1/chat/completions", "/v1beta/models"] {
        // When: Auth denies or cannot validate the managed key.
        let requirement = config.access_requirement(path, &Method::GET, Some(REMOTE));

        // Then: the request fails closed as JSON 401.
        assert_eq!(requirement, AccessRequirement::RuntimeApiKey, "{path}");
        assert_eq!(
            requirement.decision(AuthorizationState::Denied),
            AccessDecision::Unauthorized,
            "{path}"
        );
        assert_eq!(
            requirement.decision(AuthorizationState::Unavailable),
            AccessDecision::Unauthorized,
            "{path}"
        );
    }
}

#[test]
fn runtime_key_enforcement_can_be_disabled() {
    // Given: the managed API-key setting is disabled.
    let config = GatewayConfig::default().with_managed_api_key_enforcement(false);

    for path in ["/v1/models", "/v1beta/models"] {
        // When: runtime policy is evaluated.
        let requirement = config.access_requirement(path, &Method::GET, Some(REMOTE));

        // Then: the gateway does not call Auth for a managed key.
        assert_eq!(requirement, AccessRequirement::Public, "{path}");
    }
}

#[test]
fn authorization_requests_use_canonical_shared_dtos() {
    // Given: session and managed-key credentials on inbound requests.
    let mut dashboard = RequestHeader::build(Method::GET, b"/dashboard", Some(1))
        .expect("dashboard request is valid");
    dashboard
        .insert_header(header::COOKIE, "theme=dark; auth_token=session-1")
        .expect("cookie header is valid");
    let mut runtime = RequestHeader::build(Method::POST, b"/v1/chat/completions", Some(1))
        .expect("runtime request is valid");
    runtime
        .insert_header(header::AUTHORIZATION, "Bearer valid-key")
        .expect("authorization header is valid");

    // When: gateway authorization payloads are built.
    let dashboard_request = authorization_request(&dashboard, AccessRequirement::DashboardSession);
    let runtime_request = authorization_request(&runtime, AccessRequirement::RuntimeApiKey);

    // Then: payloads are exactly the shared nullrouter-contracts variants.
    assert_eq!(
        dashboard_request,
        Some(AuthorizeRequest::Dashboard {
            session_token: Some(SecretString::new("session-1")),
        })
    );
    assert_eq!(
        runtime_request,
        Some(AuthorizeRequest::Runtime {
            api_key: Some(SecretString::new("valid-key")),
        })
    );
}

#[test]
fn spoofed_forwarding_headers_are_removed() {
    // Given: an inbound request carrying attacker-controlled forwarding identity.
    let mut request =
        RequestHeader::build(Method::GET, b"/api/health", Some(8)).expect("request is valid");
    for (name, value) in [
        ("forwarded", "for=203.0.113.9"),
        ("x-forwarded-for", "203.0.113.9"),
        ("x-forwarded-host", "evil.example"),
        ("x-forwarded-proto", "https"),
        ("x-forwarded-custom", "spoofed"),
        ("x-9r-real-ip", "203.0.113.9"),
        ("x-9r-via-proxy", "1"),
        ("x-request-id", "preserved"),
    ] {
        request
            .insert_header(name, value)
            .expect("test header is valid");
    }

    // When: the gateway stamps the actual socket peer.
    stamp_trusted_identity_headers(&mut request, LOOPBACK)
        .expect("trusted identity header is valid");

    // Then: spoofable identity is gone and unrelated metadata remains.
    for name in [
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-forwarded-custom",
        "x-9r-via-proxy",
    ] {
        assert!(request.headers.get(name).is_none(), "{name}");
    }
    assert_eq!(
        request
            .headers
            .get("x-request-id")
            .and_then(|value| value.to_str().ok()),
        Some("preserved")
    );
}

#[test]
fn trusted_identity_uses_socket_peer() {
    // Given: a spoofed trusted identity header and an actual remote socket peer.
    let mut request =
        RequestHeader::build(Method::GET, b"/api/health", Some(1)).expect("request is valid");
    request
        .insert_header("x-9r-real-ip", "127.0.0.1")
        .expect("test header is valid");

    // When: the gateway stamps identity from the socket peer.
    stamp_trusted_identity_headers(&mut request, REMOTE).expect("trusted identity header is valid");

    // Then: downstream sees only the socket-derived address.
    assert_eq!(
        request
            .headers
            .get("x-9r-real-ip")
            .and_then(|value| value.to_str().ok()),
        Some("203.0.113.9")
    );
}
