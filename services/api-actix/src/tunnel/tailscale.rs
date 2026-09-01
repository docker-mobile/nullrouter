//! `tailscale` and `tailscaled`: the socket, the daemon, and Funnel.
//!
//! From `src/lib/tunnel/tailscale/tailscale.js`. Two things about upstream's arrangement are
//! worth keeping and one is not.
//!
//! Worth keeping: it runs `tailscaled` with `--tun=userspace-networking` against a socket in
//! its own data directory, so the daemon needs no root and cannot disturb a system Tailscale
//! the operator already runs. Every CLI call then carries `--socket <that path>`. That is a
//! genuinely good design and this reproduces it.
//!
//! Also worth keeping: it reads the *system* socket separately, read-only, so a machine
//! where Tailscale is already installed and logged in is detected rather than fought with.
//!
//! Not kept: its install path. Upstream will `curl | sudo sh`, run `sudo installer`, and
//! `spawn("sudo", ["-S", ...])` with the user's password on stdin, to install Tailscale for
//! them. Nothing here installs anything or asks for a password. `tailscale.install` is not an
//! operation in the catalog, and the status endpoint reports "not installed" with the
//! command the operator can run themselves.

use std::path::{Path, PathBuf};
use std::time::Duration;

use nullrouter_procctl::argv::{ArgError, Argv, StateDir};
use nullrouter_procctl::binary::{BinarySpec, SYSTEM_BIN_DIRS};
use nullrouter_procctl::supervise::{ChildSpec, ReadyRule, RestartPolicy};

use super::catalog::Args;

/// The `tailscale` CLI.
pub(crate) const TAILSCALE: BinarySpec = BinarySpec {
    name: "tailscale",
    candidates: &[],
    env_override: "NULLROUTER_TAILSCALE_BIN",
    search_dirs: SYSTEM_BIN_DIRS,
};

/// The `tailscaled` daemon.
pub(crate) const TAILSCALED: BinarySpec = BinarySpec {
    name: "tailscaled",
    candidates: &[],
    env_override: "NULLROUTER_TAILSCALED_BIN",
    search_dirs: SYSTEM_BIN_DIRS,
};

/// The gateway's port, the only thing Funnel here ever exposes.
const GATEWAY_PORT: u16 = 20128;

/// Where this service keeps its own tailscale state.
///
/// Overridable so a deployment can put it on durable storage; the default is under
/// `/var/lib` when writable, falling back to a per-user directory.
fn state_root() -> PathBuf {
    if let Some(configured) = std::env::var_os("NULLROUTER_TAILSCALE_STATE_DIR") {
        return PathBuf::from(configured);
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/var/lib"));
    base.join("nullrouter/tailscale")
}

/// The directory holding the socket and the daemon's state.
pub(crate) fn state_dir() -> Result<StateDir, ArgError> {
    StateDir::new(state_root())
}

/// Path of the socket this service's own daemon listens on.
pub(crate) fn socket_path() -> Result<PathBuf, ArgError> {
    state_dir()?.join("tailscaled.sock")
}

/// The socket a system-installed Tailscale uses.
///
/// Read only, to notice a daemon the operator already runs. Nothing here writes to it.
pub(crate) const SYSTEM_SOCKET: &str = "/var/run/tailscale/tailscaled.sock";

/// Start an argv addressed at this service's own daemon socket.
///
/// Every CLI operation begins here, which is what keeps them off a system daemon.
pub(crate) fn socket_argv() -> Result<Argv, ArgError> {
    Argv::new().flag("--socket").abs_path("tailscale socket", &socket_path()?)
}

/// `funnel --bg <port>`: expose a loopback port publicly.
pub(crate) fn funnel_start_argv(args: &Args) -> Result<Argv, ArgError> {
    let port = match args.get("port") {
        Some(_present) => args.require_port("port")?,
        None => GATEWAY_PORT,
    };
    Ok(socket_argv()?.word("funnel").flag("--bg").port(port))
}

/// `cert --cert-file … --key-file … <hostname>`: provision TLS for the tailnet name.
///
/// Funnel needs a certificate before it can serve HTTPS. The files land in this service's
/// own state directory, and the hostname is validated as an identifier.
pub(crate) fn cert_argv(args: &Args) -> Result<Argv, ArgError> {
    let hostname = args.require("hostname")?;
    let dir = state_dir()?;
    let certificate = dir.join("funnel-cert")?;
    let key = dir.join("funnel-key")?;
    socket_argv()?
        .word("cert")
        .flag("--cert-file")
        .abs_path("certificate file", &certificate)?
        .flag("--key-file")
        .abs_path("key file", &key)?
        .token("hostname", hostname)
}

/// `up --accept-routes [--hostname=…]`: begin a login.
///
/// Returns the browser URL rather than completing anything: the operator finishes the login
/// in a browser, which is the only place it can be finished.
pub(crate) fn up_argv(args: &Args) -> Result<Argv, ArgError> {
    let base = socket_argv()?.word("up").flag("--accept-routes");
    match args.get("hostname") {
        Some(hostname) => base.token_eq("--hostname", "hostname", hostname),
        None => Ok(base),
    }
}

/// The daemon's own arguments.
///
/// `--tun=userspace-networking` is what removes the need for root: no TUN device is created,
/// so no privileged operation is attempted. This is why nothing here needs a sudo password.
pub(crate) fn daemon_argv() -> Result<Argv, ArgError> {
    let dir = state_dir()?;
    Argv::new()
        .abs_path_eq("--socket", "tailscale socket", &socket_path()?)?
        .abs_path_eq("--statedir", "tailscale state directory", dir.path())
        .map(|argv| argv.flag("--tun=userspace-networking"))
}

/// Pull a login URL out of a line of `tailscale up` output.
pub(crate) fn login_url(line: &str) -> Option<String> {
    /// Where a Tailscale login always begins.
    const PREFIX: &str = "https://login.tailscale.com/";

    let at = line.find(PREFIX)?;
    let tail = line.get(at..)?;
    let end = tail
        .find(|character: char| character.is_whitespace() || matches!(character, '"' | '\''))
        .unwrap_or(tail.len());
    let url = tail.get(..end)?.trim_end_matches(['.', ',']);
    (url.len() > PREFIX.len()).then(|| url.to_owned())
}

/// How long `tailscaled` gets to come up before the start is abandoned.
const DAEMON_STARTUP: Duration = Duration::from_secs(20);

/// How long it gets to write its state after `SIGTERM`.
const DAEMON_SHUTDOWN: Duration = Duration::from_secs(8);

/// Retained log lines for the daemon.
pub(crate) const LOG_LINES: usize = 200;

/// Build the supervised `tailscaled` child.
///
/// [`ReadyRule::Spawned`], because the daemon's readiness is its socket answering rather
/// than anything it prints; the caller probes that with `status --json` and its own deadline.
pub(crate) const fn daemon_child(
    program: nullrouter_procctl::binary::Executable,
    args: Vec<String>,
) -> ChildSpec {
    ChildSpec {
        program,
        args,
        env: Vec::new(),
        secrets: Vec::new(),
        ready: ReadyRule::Spawned,
        startup_timeout: DAEMON_STARTUP,
        graceful_timeout: DAEMON_SHUTDOWN,
        restart: RestartPolicy::resilient(),
        log_capacity: LOG_LINES,
    }
}

/// Whether a system Tailscale daemon's socket is present.
pub(crate) fn system_daemon_present() -> bool {
    Path::new(SYSTEM_SOCKET).exists()
}

#[cfg(test)]
mod tests {
    use super::{
        Args, cert_argv, daemon_argv, funnel_start_argv, login_url, socket_argv, up_argv,
    };

    #[test]
    fn every_cli_operation_is_addressed_at_our_own_socket() {
        // Without this, a call would reach a system Tailscale the operator runs for their
        // own reasons, and `funnel reset` there would withdraw their mappings.
        let built = socket_argv().expect("builds");

        assert_eq!(
            built.as_slice().first().map(String::as_str),
            Some("--socket")
        );
        assert!(
            built.as_slice()
                .get(1)
                .is_some_and(|path| path.ends_with("tailscaled.sock")),
            "{:?}",
            built.as_slice()
        );
    }

    #[test]
    fn funnel_defaults_to_the_gateway_port() {
        let built = funnel_start_argv(&Args::default()).expect("builds with no arguments");

        let rendered = built.as_slice().join(" ");
        assert!(rendered.ends_with("funnel --bg 20128"), "{rendered}");
    }

    #[test]
    fn a_hostile_funnel_port_is_refused() {
        for bad in ["8080; rm -rf /", "--help", "abc"] {
            let args = Args::from_pairs(vec![("port".to_owned(), bad.to_owned())]);
            assert!(funnel_start_argv(&args).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn the_daemon_runs_in_userspace_so_no_privilege_is_ever_needed() {
        // This is the line that makes upstream's `sudo -S` with a password on stdin
        // unnecessary. If it were dropped, tailscaled would try to create a TUN device.
        let built = daemon_argv().expect("builds");
        let rendered = built.as_slice().join(" ");

        assert!(rendered.contains("--tun=userspace-networking"), "{rendered}");
        assert!(rendered.contains("--socket="), "{rendered}");
        assert!(rendered.contains("--statedir="), "{rendered}");
        assert!(!rendered.contains("sudo"), "{rendered}");
    }

    #[test]
    fn a_certificate_is_written_into_our_own_state_directory() {
        let args = Args::from_pairs(vec![(
            "hostname".to_owned(),
            "device.tail1234.ts.net".to_owned(),
        )]);

        let built = cert_argv(&args).expect("builds");
        let rendered = built.as_slice().join(" ");

        assert!(rendered.contains("--cert-file"), "{rendered}");
        assert!(rendered.contains("funnel-cert"), "{rendered}");
        assert!(rendered.ends_with("device.tail1234.ts.net"), "{rendered}");
    }

    #[test]
    fn a_hostile_certificate_hostname_cannot_reach_the_file_paths() {
        for bad in ["../../etc/passwd", "$(id)", "a b", "--cert-file"] {
            let args = Args::from_pairs(vec![("hostname".to_owned(), bad.to_owned())]);
            assert!(cert_argv(&args).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn a_login_can_claim_a_device_name() {
        let args = Args::from_pairs(vec![("hostname".to_owned(), "r4nd0m".to_owned())]);

        let rendered = up_argv(&args).expect("builds").as_slice().join(" ");

        assert!(rendered.contains("up --accept-routes"), "{rendered}");
        assert!(rendered.contains("--hostname=r4nd0m"), "{rendered}");
    }

    #[test]
    fn a_login_without_a_name_omits_the_flag() {
        let rendered = up_argv(&Args::default())
            .expect("builds")
            .as_slice()
            .join(" ");

        assert!(rendered.ends_with("up --accept-routes"), "{rendered}");
    }

    #[test]
    fn the_login_url_is_read_out_of_the_daemon_output() {
        for (line, expected) in [
            (
                "To authenticate, visit:\n\thttps://login.tailscale.com/a/1234abcd",
                Some("https://login.tailscale.com/a/1234abcd"),
            ),
            (
                "Funnel is not enabled: https://login.tailscale.com/f/funnel?node=abc",
                Some("https://login.tailscale.com/f/funnel?node=abc"),
            ),
            ("visit https://login.tailscale.com/a/xyz.", Some("https://login.tailscale.com/a/xyz")),
            ("Success.", None),
            ("https://login.tailscale.com/", None),
            ("https://evil.example.com/login.tailscale.com/a/1", None),
        ] {
            assert_eq!(login_url(line).as_deref(), expected, "{line:?}");
        }
    }
}
