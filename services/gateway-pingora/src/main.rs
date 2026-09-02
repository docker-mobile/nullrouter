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

/// Stderr logging plus the shipper that feeds the dashboard's console pane.
///
/// Installed through `nullrouter_logship` like every service, which is possible here only because
/// the shipper runs on its own thread: this is called before Pingora starts its runtime, so there is
/// none to spawn onto.
fn init_tracing() {
    let _ = EnvFilter::try_from_default_env();
    nullrouter_logship::install_with_default_filter(
        "nullrouter-gateway",
        "nullrouter_gateway=info",
    );
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
    // Configurable, and left at Pingora's default of 1 unless asked otherwise. The reasoning is
    // measured, and it went against the change:
    //
    // Against a *zero-latency* mock the single worker saturates a core at 98% and plateaus at
    // 7013 req/s, where the core count raises that to 8788 (+25%). That looked worth having until
    // the same load was run against a 250 ms mock — a realistic provider — at c=64: the gateway
    // then sits at **3% of one core**. The ceiling is never reached in real use, because provider
    // latency dominates by two orders of magnitude.
    //
    // Meanwhile the extra threads cost latency on every request: S1 and S2 at c=8 went from 1.61
    // and 1.66 ms to 1.83 and 1.81 ms, trial ranges not overlapping, so real rather than noise.
    //
    // So the default stays 1: a measured latency cost on every request against a throughput
    // ceiling that only exists when the provider is infinitely fast. `NULLROUTER_GATEWAY_THREADS`
    // is there for anyone whose upstream really is that quick — a local llama.cpp on the same box,
    // say — where the tradeoff inverts.
    let worker_threads = gateway_threads();
    if let Some(configuration) = std::sync::Arc::get_mut(&mut server.configuration) {
        configuration.threads = worker_threads;
    } else {
        // Nothing else holds the Arc at this point, so this is unreachable in practice. Reported
        // rather than ignored: silently running single-threaded is the thing being fixed.
        tracing::warn!("could not set worker threads; the gateway will run with Pingora's default");
    }
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

/// Worker threads for the proxy service.
///
/// `NULLROUTER_GATEWAY_THREADS` when set and parseable as a non-zero number, otherwise 1 — see the
/// measurement in `main`. Capped at 32, so a mistyped five-digit value does not spawn five digits of
/// threads.
///
/// `"cores"` is accepted as a value for the machine's core count, since the number that makes sense
/// depends on the host and hardcoding it in a deployment script is worse than naming it.
fn gateway_threads() -> usize {
    const MAX_THREADS: usize = 32;
    const DEFAULT_THREADS: usize = 1;

    let Ok(configured) = std::env::var("NULLROUTER_GATEWAY_THREADS") else {
        return DEFAULT_THREADS;
    };
    let configured = configured.trim();
    if configured.eq_ignore_ascii_case("cores") {
        return std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(DEFAULT_THREADS)
            .min(MAX_THREADS);
    }
    configured
        .parse::<usize>()
        .ok()
        .filter(|threads| *threads > 0)
        .map_or(DEFAULT_THREADS, |threads| threads.min(MAX_THREADS))
}

#[cfg(test)]
mod threads_tests {
    use super::gateway_threads;

    /// `NULLROUTER_GATEWAY_THREADS` is process-wide, so these must not run concurrently.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Sets the variable for the guard's lifetime and clears it after.
    struct Threads {
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Threads {
        fn set(value: Option<&str>) -> Self {
            let lock = env_lock();
            // SAFETY: the lock serialises every test in this module that touches the variable.
            match value {
                // SAFETY: the lock above is held, so no other test in this process reads or
                // writes this variable here.
                Some(value) => unsafe { std::env::set_var("NULLROUTER_GATEWAY_THREADS", value) },
                // SAFETY: as above.
                None => unsafe { std::env::remove_var("NULLROUTER_GATEWAY_THREADS") },
            }
            Self { _lock: lock }
        }
    }

    impl Drop for Threads {
        fn drop(&mut self) {
            // SAFETY: still under the lock until this guard finishes dropping.
            unsafe { std::env::remove_var("NULLROUTER_GATEWAY_THREADS") };
        }
    }

    #[test]
    fn the_default_is_one() {
        // Pingora's own default, kept deliberately: the extra threads cost measurable latency on
        // every request and buy a throughput ceiling that is never reached against a real provider.
        // See the measurement in `main`.
        let _threads = Threads::set(None);
        assert_eq!(gateway_threads(), 1);
    }

    #[test]
    fn cores_asks_for_the_machines_core_count() {
        // Named rather than requiring a deployment script to hardcode a number for the host.
        let expected = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1)
            .min(32);
        // One guard at a time: `Threads::set` takes the env lock, so holding one while creating
        // another deadlocks against itself. Scoped rather than shadowed for that reason.
        {
            let _threads = Threads::set(Some("cores"));
            assert_eq!(gateway_threads(), expected);
        }
        {
            let _threads = Threads::set(Some("CORES"));
            assert_eq!(gateway_threads(), expected, "the value is case-insensitive");
        }
    }

    #[test]
    fn an_explicit_value_wins() {
        let _threads = Threads::set(Some("4"));
        assert_eq!(gateway_threads(), 4);
    }

    #[test]
    fn a_useless_value_falls_back_rather_than_breaking_the_gateway() {
        // Zero threads would mean a router that accepts nothing; a non-number is a typo. Both fall
        // back to the default instead of failing to start, because the gateway is the only way in.
        for value in ["0", "", "  ", "many", "-4", "3.5"] {
            // Dropped at the end of each iteration, releasing the env lock before the next `set`
            // takes it. Two live guards at once deadlock against each other.
            let _threads = Threads::set(Some(value));
            assert_eq!(
                gateway_threads(),
                1,
                "{value:?} should fall back to the default"
            );
        }
    }

    #[test]
    fn an_absurd_value_is_capped() {
        let _threads = Threads::set(Some("100000"));
        assert_eq!(gateway_threads(), 32);
    }

    #[test]
    fn whitespace_around_a_number_is_tolerated() {
        let _threads = Threads::set(Some("  6  "));
        assert_eq!(gateway_threads(), 6);
    }
}
