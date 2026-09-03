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
    let grace_period = gateway_grace_period();
    if let Some(configuration) = std::sync::Arc::get_mut(&mut server.configuration) {
        configuration.threads = worker_threads;
        configuration.grace_period_seconds = Some(grace_period);
        // Pingora waits this out *after* the grace period, once per runtime, so it is added to every
        // graceful stop rather than being a ceiling on it. Its default is 5s, which would put the
        // floor for any stop at 5s and the default total at 15s. Two seconds is enough for tokio to
        // wind down runtimes that are already idle by this point.
        configuration.graceful_shutdown_timeout_seconds = Some(GRACEFUL_RUNTIME_EXIT_SECONDS);
    } else {
        // Nothing else holds the Arc at this point, so this is unreachable in practice. Reported
        // rather than ignored: silently running single-threaded, or waiting five minutes to stop,
        // are the things being fixed here.
        tracing::warn!(
            "could not set worker threads or grace period; the gateway will run with Pingora's defaults"
        );
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
            .map_or(DEFAULT_THREADS, std::num::NonZeroUsize::get)
            .min(MAX_THREADS);
    }
    configured
        .parse::<usize>()
        .ok()
        .filter(|threads| *threads > 0)
        .map_or(DEFAULT_THREADS, |threads| threads.min(MAX_THREADS))
}

/// Pingora's post-grace wait for its runtimes to exit. See the call site for why it is not its 5s
/// default.
const GRACEFUL_RUNTIME_EXIT_SECONDS: u64 = 2;

/// How long a `SIGTERM` waits for in-flight requests before the process exits.
///
/// Pingora's own default is `EXIT_TIMEOUT`, five minutes of unconditional `thread::sleep` after the
/// listener closes — even with zero connections open. Every process supervisor sends `SIGTERM`, so
/// left alone the gateway looks hung for five minutes on every stop, and systemd or Docker reaches
/// its own patience first and `SIGKILL`s it, which drops exactly the connections the grace period
/// was supposed to protect.
///
/// Five seconds instead, which with `GRACEFUL_RUNTIME_EXIT_SECONDS` puts a whole graceful stop at
/// about seven — inside Docker's 10s default before it escalates to `SIGKILL`, so a drain that is
/// meant to be clean actually completes. Long enough for a non-streaming request to finish, short
/// enough that a restart is a restart.
///
/// Streaming responses are the reason this is configurable rather than fixed: a long generation can
/// outlive any bounded period, and whoever runs one knows better than this default does. `SIGINT` is
/// unaffected and still exits immediately.
fn gateway_grace_period() -> u64 {
    const DEFAULT_GRACE_SECONDS: u64 = 5;

    std::env::var("NULLROUTER_GATEWAY_GRACE_SECONDS")
        .ok()
        .map_or(DEFAULT_GRACE_SECONDS, |configured| {
            // An unparseable value falls back rather than failing: refusing to start over a
            // malformed tuning knob is worse than starting with the documented default.
            configured
                .trim()
                .parse::<u64>()
                .unwrap_or(DEFAULT_GRACE_SECONDS)
        })
}

#[cfg(test)]
mod grace_period_tests {
    use super::gateway_grace_period;

    /// `NULLROUTER_GATEWAY_GRACE_SECONDS` is process-wide, so these must not run concurrently.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn with_var<T>(value: Option<&str>, body: impl FnOnce() -> T) -> T {
        let _guard = env_lock();
        match value {
            // SAFETY: the lock above serialises every mutation of this variable in this process, and
            // no other thread in these tests reads it.
            Some(value) => unsafe {
                std::env::set_var("NULLROUTER_GATEWAY_GRACE_SECONDS", value);
            },
            // SAFETY: as above.
            None => unsafe {
                std::env::remove_var("NULLROUTER_GATEWAY_GRACE_SECONDS");
            },
        }
        let result = body();
        // SAFETY: as above.
        unsafe {
            std::env::remove_var("NULLROUTER_GATEWAY_GRACE_SECONDS");
        }
        result
    }

    #[test]
    fn defaults_to_five_seconds() {
        assert_eq!(with_var(None, gateway_grace_period), 5);
    }

    /// A whole graceful stop is the grace period plus Pingora's runtime-exit wait, and it has to land
    /// inside Docker's 10s patience or the drain this exists for is cut short by a `SIGKILL`.
    #[test]
    fn a_whole_graceful_stop_fits_within_dockers_default_timeout() {
        let total = with_var(None, gateway_grace_period) + super::GRACEFUL_RUNTIME_EXIT_SECONDS;
        assert!(
            total < 10,
            "a graceful stop takes {total}s, which Docker would cut short"
        );
    }

    /// The whole point of the setting: not Pingora's 300.
    #[test]
    fn default_is_not_pingoras_five_minutes() {
        assert_ne!(with_var(None, gateway_grace_period), 60 * 5);
    }

    #[test]
    fn honours_a_configured_value() {
        assert_eq!(with_var(Some("120"), gateway_grace_period), 120);
    }

    /// Zero is meaningful — "drop in-flight requests, stop now" — so it must not be treated as unset.
    #[test]
    fn zero_is_honoured() {
        assert_eq!(with_var(Some("0"), gateway_grace_period), 0);
    }

    #[test]
    fn tolerates_surrounding_whitespace() {
        assert_eq!(with_var(Some("  30\n"), gateway_grace_period), 30);
    }

    #[test]
    fn falls_back_on_a_malformed_value() {
        assert_eq!(with_var(Some("soon"), gateway_grace_period), 5);
        assert_eq!(with_var(Some("-5"), gateway_grace_period), 5);
        assert_eq!(with_var(Some(""), gateway_grace_period), 5);
    }
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
            .map_or(1, std::num::NonZeroUsize::get)
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
