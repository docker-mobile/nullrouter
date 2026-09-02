//! `cloudflared`: where it lives, how it is invoked, and how its output is read.
//!
//! Two tunnel shapes, both from `src/lib/tunnel/cloudflare/cloudflared.js`:
//!
//! * a **named** tunnel, `tunnel run`, authenticated by a token and configured in the
//!   Cloudflare dashboard. Upstream passes the token as `--token <value>`; this passes it as
//!   `TUNNEL_TOKEN` in the child's environment, which `cloudflared` documents as equivalent
//!   and which keeps it out of `/proc/<pid>/cmdline`;
//! * a **quick** tunnel, `tunnel --url`, needing no account, whose `*.trycloudflare.com`
//!   hostname exists only in the child's log output.

use std::time::Duration;

use nullrouter_procctl::argv::{ArgError, Argv};
use nullrouter_procctl::binary::{BinarySpec, SYSTEM_BIN_DIRS};
use nullrouter_procctl::supervise::{ChildSpec, ReadyRule, RestartPolicy};

use super::catalog::Args;

/// Where `cloudflared` may be found.
///
/// No download path: see [`nullrouter_procctl::binary`]. An operator installs it, or the
/// operation reports that it is not installed.
pub(crate) const CLOUDFLARED: BinarySpec = BinarySpec {
    name: "cloudflared",
    candidates: &[],
    env_override: "NULLROUTER_CLOUDFLARED_BIN",
    search_dirs: SYSTEM_BIN_DIRS,
};

/// The gateway's port, the only thing a tunnel here ever exposes.
const GATEWAY_PORT: u16 = 20128;

/// Environment variable `cloudflared` reads a tunnel token from.
///
/// Documented as equivalent to `--token`, which is why the credential never needs to be an
/// argument.
const TOKEN_VAR: &str = "TUNNEL_TOKEN";

/// Place the tunnel token in the child's environment.
pub(crate) fn token_env(args: &Args) -> Vec<(String, String)> {
    args.get("token")
        .map(|token| vec![(TOKEN_VAR.to_owned(), token.to_owned())])
        .unwrap_or_default()
}

/// `tunnel run`, for a named remotely-managed tunnel.
///
/// `--dns-resolver-addrs 1.1.1.1:53` matches upstream. The token is absent by design: it
/// arrives through [`token_env`].
#[expect(
    clippy::unnecessary_wraps,
    reason = "the catalog's build field is one function pointer type for every row, and a \
              row that cannot fail still has to have that signature"
)]
pub(crate) fn named_tunnel_argv(_args: &Args) -> Result<Argv, ArgError> {
    Ok(Argv::new()
        .word("tunnel")
        .flag("--no-autoupdate")
        .flag("--dns-resolver-addrs")
        .word("1.1.1.1:53")
        .word("run"))
}

/// `tunnel --url http://127.0.0.1:<port>`, for a quick tunnel.
///
/// The host is fixed to loopback by [`Argv::loopback_origin`], so no caller can point a
/// tunnel at another machine. `--retries 99` and `--no-autoupdate` match upstream.
pub(crate) fn quick_tunnel_argv(args: &Args) -> Result<Argv, ArgError> {
    let port = match args.get("port") {
        Some(_present) => args.require_port("port")?,
        None => GATEWAY_PORT,
    };
    Ok(Argv::new()
        .word("tunnel")
        .flag("--no-autoupdate")
        .flag("--retries")
        .word("99")
        .flag("--url")
        .loopback_origin(port))
}

/// Pull a quick tunnel's hostname out of a log line.
///
/// The only channel: `cloudflared` prints it and nothing else reports it. `api` is skipped
/// because `cloudflared` also logs `api.trycloudflare.com`, its own control plane, which is
/// not the tunnel.
pub(crate) fn quick_tunnel_url(line: &str) -> Option<String> {
    /// What the hostname ends with.
    const SUFFIX: &str = ".trycloudflare.com";
    /// The control-plane host, which is not a tunnel.
    const CONTROL_PLANE: &str = "api.trycloudflare.com";

    let mut found = None;
    let mut rest = line;
    while let Some(at) = rest.find("https://") {
        let tail = rest.get(at..)?;
        let end = tail
            .find(|character: char| {
                !(character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '/' | ':'))
            })
            .unwrap_or(tail.len());
        let candidate = tail.get(..end)?.trim_end_matches('/');
        if candidate.ends_with(SUFFIX) && !candidate.ends_with(CONTROL_PLANE) {
            // Upstream keeps the last match in a chunk, because a reconnect logs a new
            // hostname after the old one.
            found = Some(candidate.to_owned());
        }
        rest = tail.get(end..).unwrap_or("");
    }
    found
}

/// How long a tunnel gets to establish itself.
///
/// Upstream's 90 seconds, which is generous but is what a cold QUIC handshake behind a
/// restrictive network actually takes.
const TUNNEL_STARTUP: Duration = Duration::from_secs(90);

/// How long `cloudflared` gets to unregister its connections after `SIGTERM`.
const TUNNEL_SHUTDOWN: Duration = Duration::from_secs(10);

/// Retained log lines per tunnel.
pub(crate) const LOG_LINES: usize = 200;

/// The readiness rule for a named tunnel: four registered edge connections.
///
/// `cloudflared` opens four by design, and fewer than four means a partly-established
/// tunnel that can drop without warning.
const REGISTERED_CONNECTIONS: usize = 4;

/// Build the supervised child for a named tunnel.
pub(crate) const fn named_tunnel_child(
    program: nullrouter_procctl::binary::Executable,
    args: Vec<String>,
    env: Vec<(String, String)>,
    secrets: Vec<nullrouter_procctl::secret::Secret>,
) -> ChildSpec {
    ChildSpec {
        program,
        args,
        env,
        secrets,
        ready: ReadyRule::Occurrences {
            needle: "Registered tunnel connection",
            times: REGISTERED_CONNECTIONS,
        },
        startup_timeout: TUNNEL_STARTUP,
        graceful_timeout: TUNNEL_SHUTDOWN,
        restart: RestartPolicy::resilient(),
        log_capacity: LOG_LINES,
    }
}

/// Build the supervised child for a quick tunnel.
pub(crate) const fn quick_tunnel_child(
    program: nullrouter_procctl::binary::Executable,
    args: Vec<String>,
) -> ChildSpec {
    ChildSpec {
        program,
        args,
        env: Vec::new(),
        secrets: Vec::new(),
        // The hostname is assigned by Cloudflare per run, so readiness and the result are
        // the same event.
        ready: ReadyRule::Extract(quick_tunnel_url),
        startup_timeout: TUNNEL_STARTUP,
        graceful_timeout: TUNNEL_SHUTDOWN,
        restart: RestartPolicy::resilient(),
        log_capacity: LOG_LINES,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Args, GATEWAY_PORT, named_tunnel_argv, quick_tunnel_argv, quick_tunnel_url, token_env,
    };

    #[test]
    fn a_named_tunnel_never_carries_its_token_in_argv() {
        let args = Args::from_pairs(vec![("token".to_owned(), "eyJhIjoi-secret".to_owned())]);

        let built = named_tunnel_argv(&args).expect("builds");
        let rendered = built.as_slice().join(" ");

        // Upstream: `tunnel run --dns-resolver-addrs 1.1.1.1:53 --token <value>`.
        assert!(!rendered.contains("eyJhIjoi-secret"), "{rendered}");
        assert!(!rendered.contains("--token"), "{rendered}");
        assert_eq!(
            built.as_slice(),
            [
                "tunnel",
                "--no-autoupdate",
                "--dns-resolver-addrs",
                "1.1.1.1:53",
                "run"
            ]
        );
    }

    #[test]
    fn the_token_reaches_the_child_through_the_environment() {
        let args = Args::from_pairs(vec![("token".to_owned(), "the-token".to_owned())]);

        assert_eq!(
            token_env(&args),
            vec![("TUNNEL_TOKEN".to_owned(), "the-token".to_owned())]
        );
        assert!(
            token_env(&Args::default()).is_empty(),
            "no token, no variable"
        );
    }

    #[test]
    fn a_quick_tunnel_defaults_to_the_gateway_port() {
        let built = quick_tunnel_argv(&Args::default()).expect("builds with no arguments");

        assert_eq!(
            built.as_slice().last().map(String::as_str),
            Some("http://127.0.0.1:20128")
        );
        assert_eq!(GATEWAY_PORT, 20128);
    }

    #[test]
    fn a_quick_tunnel_accepts_another_loopback_port_but_not_another_host() {
        let args = Args::from_pairs(vec![("port".to_owned(), "20131".to_owned())]);

        let built = quick_tunnel_argv(&args).expect("builds");

        assert_eq!(
            built.as_slice().last().map(String::as_str),
            Some("http://127.0.0.1:20131")
        );
        // The host is not a parameter at all, so there is nothing to point elsewhere.
        assert!(!built.as_slice().join(" ").contains("evil.example.com"));
    }

    #[test]
    fn a_port_that_is_not_a_number_is_refused() {
        for bad in ["evil.example.com", "20128; rm -rf /", "-1", "99999", ""] {
            let args = Args::from_pairs(vec![("port".to_owned(), bad.to_owned())]);
            let built = quick_tunnel_argv(&args);
            if bad.is_empty() {
                // An empty value is absent, so the default applies.
                assert!(built.is_ok());
                continue;
            }
            assert!(built.is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn the_quick_tunnel_hostname_is_read_out_of_a_real_log_line() {
        // The exact shape cloudflared prints, box drawing and all.
        let line = "2026-09-01T10:00:00Z INF |  https://sunny-mode-cats-tv.trycloudflare.com   |";

        assert_eq!(
            quick_tunnel_url(line).as_deref(),
            Some("https://sunny-mode-cats-tv.trycloudflare.com")
        );
    }

    #[test]
    fn the_control_plane_host_is_not_mistaken_for_a_tunnel() {
        // cloudflared logs this on startup; treating it as the tunnel would report a URL
        // that serves nothing.
        let line = "INF Requesting new quick Tunnel on trycloudflare.com... url=https://api.trycloudflare.com/tunnel";

        assert_eq!(quick_tunnel_url(line), None, "{line}");
    }

    #[test]
    fn a_later_hostname_in_one_line_wins() {
        // A reconnect logs the new hostname after the old one.
        let line =
            "old https://first-one.trycloudflare.com new https://second-one.trycloudflare.com";

        assert_eq!(
            quick_tunnel_url(line).as_deref(),
            Some("https://second-one.trycloudflare.com")
        );
    }

    #[test]
    fn unrelated_lines_yield_nothing() {
        for line in [
            "INF Registered tunnel connection connIndex=0",
            "ERR failed to connect to https://example.com",
            "https://trycloudflare.com.evil.example.com/",
            "",
            "http://plain-http.trycloudflare.com",
        ] {
            assert_eq!(quick_tunnel_url(line), None, "{line:?}");
        }
    }
}
