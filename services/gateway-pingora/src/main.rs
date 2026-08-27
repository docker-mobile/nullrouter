use std::net::SocketAddr;

use clap::Parser;
use nullrouter_gateway::{GatewayConfig, GatewayConfigError, GatewayProxy, GatewayUpstreamAddrs};
use pingora_core::Result as PingoraResult;
use pingora_core::server::Server;
use pingora_error::{Error, ErrorType};
use pingora_proxy::http_proxy_service;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[arg(
        long,
        env = "NULLROUTER_GATEWAY_LISTEN",
        default_value = "127.0.0.1:20128"
    )]
    listen: SocketAddr,

    #[arg(
        long,
        env = "NULLROUTER_API_UPSTREAM",
        default_value = "127.0.0.1:20129"
    )]
    api_upstream: SocketAddr,

    #[arg(
        long,
        env = "NULLROUTER_DASHBOARD_UPSTREAM",
        default_value = "127.0.0.1:20130"
    )]
    dashboard_upstream: SocketAddr,

    #[arg(
        long,
        env = "NULLROUTER_CATALOG_UPSTREAM",
        default_value = "127.0.0.1:20131"
    )]
    catalog_upstream: SocketAddr,

    #[arg(
        long,
        env = "NULLROUTER_RUNTIME_UPSTREAM",
        default_value = "127.0.0.1:20132"
    )]
    runtime_upstream: SocketAddr,

    #[arg(
        long,
        env = "NULLROUTER_EVENTS_UPSTREAM",
        default_value = "127.0.0.1:20133"
    )]
    events_upstream: SocketAddr,

    #[arg(
        long,
        env = "NULLROUTER_STATE_UPSTREAM",
        default_value = "127.0.0.1:20134"
    )]
    state_upstream: SocketAddr,

    #[arg(
        long,
        env = "NULLROUTER_AUTH_UPSTREAM",
        default_value = "127.0.0.1:20135"
    )]
    auth_upstream: SocketAddr,

    #[arg(long, env = "NULLROUTER_REQUIRE_API_KEY", default_value_t = false)]
    require_api_key: bool,
}

impl TryFrom<Cli> for GatewayConfig {
    type Error = GatewayConfigError;

    fn try_from(cli: Cli) -> Result<Self, Self::Error> {
        Self::new(
            cli.listen,
            GatewayUpstreamAddrs {
                api: cli.api_upstream,
                dashboard: cli.dashboard_upstream,
                catalog: cli.catalog_upstream,
                runtime: cli.runtime_upstream,
                events: cli.events_upstream,
                state: cli.state_upstream,
                auth: cli.auth_upstream,
            },
        )
        .map(|config| config.with_managed_api_key_enforcement(cli.require_api_key))
    }
}

fn main() -> PingoraResult<()> {
    init_tracing();
    let config = GatewayConfig::try_from(Cli::parse()).map_err(|error| {
        Error::because(
            ErrorType::InternalError,
            "invalid gateway configuration",
            error,
        )
    })?;
    run(config)
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("nullrouter_gateway=info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .try_init();
}

fn run(config: GatewayConfig) -> PingoraResult<()> {
    let listen_addr = config.listen_addr().to_string();
    let api_addr = config.api_upstream().authority().to_owned();
    let dashboard_addr = config.dashboard_upstream().authority().to_owned();
    let catalog_addr = config.catalog_upstream().authority().to_owned();
    let runtime_addr = config.runtime_upstream().authority().to_owned();
    let events_addr = config.events_upstream().authority().to_owned();
    let state_addr = config.state_upstream().authority().to_owned();
    let auth_addr = config.auth_upstream().authority().to_owned();

    let mut server = Server::new(None)?;
    server.bootstrap();

    let proxy = GatewayProxy::new(config).map_err(|error| {
        Error::because(
            ErrorType::InternalError,
            "invalid Auth client configuration",
            error,
        )
    })?;
    let mut service = http_proxy_service(&server.configuration, proxy);
    service.add_tcp(&listen_addr);
    server.add_service(service);

    tracing::info!(
        listen_addr,
        api_upstream = api_addr,
        dashboard_upstream = dashboard_addr,
        catalog_upstream = catalog_addr,
        runtime_upstream = runtime_addr,
        events_upstream = events_addr,
        state_upstream = state_addr,
        auth_upstream = auth_addr,
        "starting nullrouter-gateway"
    );
    server.run_forever()
}
