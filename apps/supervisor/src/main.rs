#![allow(
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::unnecessary_wraps,
    clippy::unused_self,
    clippy::map_unwrap_or
)]
//! Single-binary supervisor: forks all nullrouter microservices as child
//! processes, using /tmp/nullrouter.{random6} as the runtime directory.
//!
//! Usage: `nullrouter` (release) or `nullrouter --debug`
//!
//! The supervisor:
//! 1. Generates a random 6-digit runtime directory under /tmp.
//! 2. Spawns each service binary with the correct env vars (ports, upstreams).
//! 3. Forwards SIGTERM/SIGINT to all children and waits for them to exit.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Parser;

#[derive(Debug, Parser)]
#[command(author, version, about = "NullRouter single-binary supervisor")]
struct Cli {
    /// Use debug binaries instead of release
    #[arg(long)]
    debug: bool,

    /// Base port for the gateway (all other ports are derived from this)
    #[arg(long, env = "NULLROUTER_BASE_PORT", default_value_t = 20128)]
    base_port: u16,

    /// Path to the state file
    #[arg(long, env = "NULLROUTER_STATE_FILE")]
    state_file: Option<PathBuf>,

    /// Verbose logging
    #[arg(long, env = "NULLROUTER_VERBOSE", default_value_t = false)]
    verbose: bool,
}

/// One service to spawn.
struct ServiceSpec {
    name: &'static str,
    binary: &'static str,
    env_prefix: &'static str,
}

const SERVICES: &[ServiceSpec] = &[
    ServiceSpec {
        name: "state",
        binary: "nullrouter-state",
        env_prefix: "NULLROUTER_STATE",
    },
    ServiceSpec {
        name: "runtime",
        binary: "nullrouter-runtime",
        env_prefix: "NULLROUTER_RUNTIME",
    },
    ServiceSpec {
        name: "api",
        binary: "nullrouter-api",
        env_prefix: "NULLROUTER_API",
    },
    ServiceSpec {
        name: "events",
        binary: "nullrouter-events",
        env_prefix: "NULLROUTER_EVENTS",
    },
    ServiceSpec {
        name: "catalog",
        binary: "nullrouter-catalog",
        env_prefix: "NULLROUTER_CATALOG",
    },
    ServiceSpec {
        name: "auth",
        binary: "nullrouter-auth",
        env_prefix: "NULLROUTER_AUTH",
    },
    ServiceSpec {
        name: "dashboard-host",
        binary: "nullrouter-dashboard-host",
        env_prefix: "NULLROUTER_DASHBOARD",
    },
];

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();

    if cli.verbose {
        tracing_subscriber::fmt().with_env_filter("info").init();
    } else {
        tracing_subscriber::fmt().with_env_filter("warn").init();
    }

    // Generate runtime directory: /tmp/nullrouter.{random6}
    let rand6: u32 = rand_simple();
    let runtime_dir = PathBuf::from(format!("/tmp/nullrouter.{rand6:06}"));
    std::fs::create_dir_all(&runtime_dir)?;
    eprintln!("==> Runtime directory: {}", runtime_dir.display());

    // Derive ports from base_port (20128 = gateway, 20129 = api, etc.)
    let base = cli.base_port;
    let ports: HashMap<&str, u16> = [
        ("gateway", base),
        ("api", base + 1),
        ("dashboard-host", base + 2),
        ("catalog", base + 3),
        ("runtime", base + 4),
        ("events", base + 5),
        ("state", base + 6),
        ("auth", base + 7),
    ]
    .into_iter()
    .collect();

    // Build the binary path
    let profile_dir = if cli.debug { "debug" } else { "release" };
    let bin_dir = PathBuf::from(format!("target/{profile_dir}"));

    // Verify binaries exist
    for spec in SERVICES {
        let bin_path = bin_dir.join(spec.binary);
        if !bin_path.exists() {
            eprintln!("ERROR: binary not found: {}", bin_path.display());
            eprintln!(
                "Run `cargo build{}` first",
                if cli.debug { "" } else { " --release" }
            );
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("missing binary: {}", spec.binary),
            ));
        }
    }

    // Also check gateway
    let gateway_bin = bin_dir.join("nullrouter-gateway");
    if !gateway_bin.exists() {
        eprintln!("ERROR: gateway binary not found: {}", gateway_bin.display());
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing gateway binary",
        ));
    }

    // Build env vars for all services
    let state_file = cli
        .state_file
        .clone()
        .unwrap_or_else(|| runtime_dir.join("nullrouter-state.json"));

    eprintln!("==> State file: {}", state_file.display());
    eprintln!("==> Gateway: 127.0.0.1:{base}");

    // Spawn services in dependency order
    let mut children: Vec<(String, Child)> = Vec::new();

    // SIGINT/SIGTERM is delivered to the entire process group on Ctrl-C,
    // so all child processes receive it too. The supervisor just waits
    // for them to exit and cleans up.
    let shutdown = Arc::new(AtomicBool::new(false));

    for spec in SERVICES {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        let port = ports[spec.name];
        let bin_path = bin_dir.join(spec.binary);

        eprintln!("==> Starting {} on 127.0.0.1:{port}", spec.name);

        let mut cmd = Command::new(&bin_path);
        cmd.env("NULLROUTER_STATE_FILE", &state_file)
            .env(format!("{}_HOST", spec.env_prefix), "127.0.0.1")
            .env(format!("{}_PORT", spec.env_prefix), port.to_string())
            .env("RUST_LOG", if cli.verbose { "info" } else { "warn" })
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        // Unset PORT so it doesn't interfere (services check PORT first)
        cmd.env_remove("PORT");

        // Gateway upstream env vars for the api service
        if spec.name == "api" {
            cmd.env("NULLROUTER_STATE_HOST", "127.0.0.1")
                .env("NULLROUTER_STATE_PORT", ports["state"].to_string())
                .env("NULLROUTER_RUNTIME_HOST", "127.0.0.1")
                .env("NULLROUTER_RUNTIME_PORT", ports["runtime"].to_string());
        }

        match cmd.spawn() {
            Ok(child) => {
                children.push((spec.name.to_string(), child));
                // Brief delay to let the service bind
                std::thread::sleep(std::time::Duration::from_millis(1500));
            }
            Err(e) => {
                eprintln!("ERROR: failed to start {}: {e}", spec.name);
                // Kill already-started children
                for (_, mut child) in children {
                    let _ = child.kill();
                }
                return Err(e);
            }
        }
    }

    // Start gateway last (it needs all upstreams)
    if !shutdown.load(Ordering::SeqCst) {
        eprintln!("==> Starting gateway on 127.0.0.1:{base}");
        let gateway_port = ports["gateway"];
        let mut cmd = Command::new(&gateway_bin);
        cmd.env(
            "NULLROUTER_GATEWAY_LISTEN",
            format!("127.0.0.1:{gateway_port}"),
        )
        .env(
            "NULLROUTER_API_UPSTREAM",
            format!("127.0.0.1:{}", ports["api"]),
        )
        .env(
            "NULLROUTER_DASHBOARD_UPSTREAM",
            format!("127.0.0.1:{}", ports["dashboard-host"]),
        )
        .env(
            "NULLROUTER_CATALOG_UPSTREAM",
            format!("127.0.0.1:{}", ports["catalog"]),
        )
        .env(
            "NULLROUTER_RUNTIME_UPSTREAM",
            format!("127.0.0.1:{}", ports["runtime"]),
        )
        .env(
            "NULLROUTER_EVENTS_UPSTREAM",
            format!("127.0.0.1:{}", ports["events"]),
        )
        .env(
            "NULLROUTER_STATE_UPSTREAM",
            format!("127.0.0.1:{}", ports["state"]),
        )
        .env(
            "NULLROUTER_AUTH_UPSTREAM",
            format!("127.0.0.1:{}", ports["auth"]),
        )
        .env("RUST_LOG", if cli.verbose { "info" } else { "warn" })
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

        match cmd.spawn() {
            Ok(child) => {
                children.push(("gateway".to_string(), child));
            }
            Err(e) => {
                eprintln!("ERROR: failed to start gateway: {e}");
                for (_, mut child) in children {
                    let _ = child.kill();
                }
                return Err(e);
            }
        }
    }

    eprintln!("==> All services started. Dashboard: http://localhost:{base}/dashboard");
    eprintln!("==> Press Ctrl-C to stop all services.");

    // Wait for all children
    let mut exit_code = 0;
    for (name, mut child) in children {
        if shutdown.load(Ordering::SeqCst) {
            eprintln!("==> Sending SIGTERM to {name}");
            let _ = child.kill();
        }
        match child.wait() {
            Ok(status) => {
                if !status.success() {
                    eprintln!("==> {name} exited with {status}");
                    exit_code = 1;
                }
            }
            Err(e) => {
                eprintln!("==> Error waiting for {name}: {e}");
                exit_code = 1;
            }
        }
    }

    // Cleanup runtime directory
    if runtime_dir.exists() {
        let _ = std::fs::remove_dir_all(&runtime_dir);
        eprintln!("==> Cleaned up {}", runtime_dir.display());
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

/// Simple PRNG for a 6-digit random number (no external dependency).
fn rand_simple() -> u32 {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(42);
    // xorshift
    let mut x = seed as u64 ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x as u32) % 1_000_000
}
