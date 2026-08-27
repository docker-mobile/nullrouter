use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use nullrouter_gateway::{GatewayConfig, GatewayUpstreamAddrs, RouteKind};

const LISTEN: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20128);
const API: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20129);
const DASHBOARD: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20130);
const CATALOG: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20131);
const RUNTIME: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20132);
const EVENTS: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20133);
const STATE: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20134);
const AUTH: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20135);
const EXTERNAL_API: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)), 20129);
const EXTERNAL_DASHBOARD: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 6)), 20130);
const EXTERNAL_CATALOG: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)), 20131);
const EXTERNAL_RUNTIME: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 8)), 20132);
const EXTERNAL_EVENTS: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9)), 20133);
const EXTERNAL_STATE: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10)), 20134);

#[test]
fn package_metadata_uses_nullrouter_gateway_names() {
    // Given: the gateway crate is built as a Rust microservice.
    let package_name = env!("CARGO_PKG_NAME");
    let gateway_binary = option_env!("CARGO_BIN_EXE_nullrouter-gateway");

    // When: Cargo exposes package and binary metadata to integration tests.

    // Then: both public names follow the nullrouter-* service naming convention.
    assert_eq!(package_name, "nullrouter-gateway");
    assert!(gateway_binary.is_some());
}

#[test]
fn default_config_uses_local_service_ports() {
    // Given: no explicit gateway configuration is supplied.
    let config = GatewayConfig::default();

    // When: the default config is inspected.

    // Then: every internal service upstream is mapped to its loopback port.
    assert_eq!(config.listen_addr(), LISTEN);
    assert_eq!(config.api_upstream().addr(), API);
    assert_eq!(config.dashboard_upstream().addr(), DASHBOARD);
    assert_eq!(config.catalog_upstream().addr(), CATALOG);
    assert_eq!(config.runtime_upstream().addr(), RUNTIME);
    assert_eq!(config.events_upstream().addr(), EVENTS);
    assert_eq!(config.state_upstream().addr(), STATE);
    assert_eq!(config.auth_upstream().addr(), AUTH);
}

#[test]
fn config_rejects_non_loopback_internal_upstreams() {
    // Given: the public gateway is configured with internal upstream addresses.
    let upstream_cases = [
        GatewayUpstreamAddrs {
            api: EXTERNAL_API,
            dashboard: DASHBOARD,
            catalog: CATALOG,
            runtime: RUNTIME,
            events: EVENTS,
            state: STATE,
            auth: AUTH,
        },
        GatewayUpstreamAddrs {
            api: API,
            dashboard: EXTERNAL_DASHBOARD,
            catalog: CATALOG,
            runtime: RUNTIME,
            events: EVENTS,
            state: STATE,
            auth: AUTH,
        },
        GatewayUpstreamAddrs {
            api: API,
            dashboard: DASHBOARD,
            catalog: EXTERNAL_CATALOG,
            runtime: RUNTIME,
            events: EVENTS,
            state: STATE,
            auth: AUTH,
        },
        GatewayUpstreamAddrs {
            api: API,
            dashboard: DASHBOARD,
            catalog: CATALOG,
            runtime: EXTERNAL_RUNTIME,
            events: EVENTS,
            state: STATE,
            auth: AUTH,
        },
        GatewayUpstreamAddrs {
            api: API,
            dashboard: DASHBOARD,
            catalog: CATALOG,
            runtime: RUNTIME,
            events: EXTERNAL_EVENTS,
            state: STATE,
            auth: AUTH,
        },
        GatewayUpstreamAddrs {
            api: API,
            dashboard: DASHBOARD,
            catalog: CATALOG,
            runtime: RUNTIME,
            events: EVENTS,
            state: EXTERNAL_STATE,
            auth: AUTH,
        },
    ];

    for upstreams in upstream_cases {
        // When: any upstream address is outside loopback.
        let config = GatewayConfig::new(LISTEN, upstreams);

        // Then: the configuration is rejected before Pingora starts proxying.
        assert!(config.is_err());
    }
}

#[test]
fn target_for_route_returns_matching_upstream() {
    // Given: all configured upstreams are loopback services.
    let config = GatewayConfig::new(
        LISTEN,
        GatewayUpstreamAddrs {
            api: API,
            dashboard: DASHBOARD,
            catalog: CATALOG,
            runtime: RUNTIME,
            events: EVENTS,
            state: STATE,
            auth: AUTH,
        },
    )
    .expect("loopback upstreams are valid");

    // When: route kinds are mapped to proxy targets.

    // Then: each route resolves to its own internal service port.
    assert_eq!(config.target_for(RouteKind::Api).addr(), API);
    assert_eq!(config.target_for(RouteKind::Dashboard).addr(), DASHBOARD);
    assert_eq!(config.target_for(RouteKind::Catalog).addr(), CATALOG);
    assert_eq!(config.target_for(RouteKind::Runtime).addr(), RUNTIME);
    assert_eq!(config.target_for(RouteKind::Events).addr(), EVENTS);
    assert_eq!(config.target_for(RouteKind::State).addr(), STATE);
    assert_eq!(config.target_for(RouteKind::Auth).addr(), AUTH);
}
