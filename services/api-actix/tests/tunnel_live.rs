//! The tunnel routes driving a binary that is actually present.
//!
//! `tunnel_control.rs` covers the absent-binary path, which is what this sandbox has. These
//! cases install a stand-in binary at the path the resolver reads from an environment
//! override, so the enable flows run end to end: argv is built, a process is spawned, its
//! output is parsed, and the response is assembled from what it said.
//!
//! The stand-in is a shell script that answers like `tailscale` or `cloudflared`. That is the
//! point rather than a shortcut: it lets the *sequence* be exercised — daemon check, login
//! check, funnel start, certificate, read back the real hostname — including the two states
//! that are otherwise impossible to reach here, present-but-not-authenticated and enabled.
#![cfg(unix)]
#![allow(clippy::future_not_send)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "free helpers in an integration test are not covered by clippy.toml's \
              allow-expect-in-tests, which only reaches #[test] functions"
)]

use std::os::unix::fs::PermissionsExt as _;
use std::sync::{Mutex, MutexGuard, OnceLock};

use actix_web::http::{Method, StatusCode, header};
use actix_web::{App, test, web};
use nullrouter_api::{AppConfig, RuntimeClient, StateClient, TunnelManager, configure};
use serde_json::Value;

/// A port nothing listens on.
const UNREACHABLE_STATE_ADDR: &str = "http://127.0.0.1:1";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Serialises the cases that install a stand-in binary, since they share the environment.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Every variable these cases set, so each is restored even when a case sets only some.
const MANAGED_VARS: &[&str] = &[
    "NULLROUTER_CLOUDFLARED_BIN",
    "NULLROUTER_TAILSCALE_BIN",
    "NULLROUTER_TAILSCALED_BIN",
    "NULLROUTER_TAILSCALE_STATE_DIR",
];

/// A stand-in binary and the environment pointing at it.
///
/// Holds the lock as a field rather than leaving it in a local binding, the same shape
/// `cli_tool_writes.rs` uses: a `MutexGuard` in a local would be held across the request's
/// await points, which `clippy::await_holding_lock` refuses on sight even where — as here,
/// with one current-thread runtime per case — it cannot deadlock.
struct Installed {
    _lock: MutexGuard<'static, ()>,
    _home: tempfile::TempDir,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl Installed {
    /// Write each script as an executable stand-in and point the resolver at it.
    fn new(scripts: &[(&str, &str, &str)]) -> Self {
        Self::with_mode(scripts, 0o755)
    }

    /// As [`Installed::new`], with a chosen permission mode.
    ///
    /// A mode is a parameter because the mode is the subject of one case: a binary the whole
    /// machine can write to must be refused, and that case has to go through the same locked
    /// path as the others rather than setting the variable behind their backs.
    fn with_mode(scripts: &[(&str, &str, &str)], mode: u32) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let home = tempfile::tempdir().expect("tempdir");
        let state = home.path().join("state");
        std::fs::create_dir_all(&state).expect("create the state directory");

        let previous = MANAGED_VARS
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();

        for (variable, name, script) in scripts {
            let path = home.path().join(name);
            std::fs::write(&path, script).expect("write the stand-in");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                .expect("set the stand-in's mode");
            // SAFETY: the lock above is held, so no other case in this binary reads or writes
            // these variables while they are being set.
            unsafe { std::env::set_var(variable, &path) };
        }
        // SAFETY: as above.
        unsafe { std::env::set_var("NULLROUTER_TAILSCALE_STATE_DIR", &state) };

        Self {
            _lock: lock,
            _home: home,
            previous,
        }
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        for (name, value) in &self.previous {
            match value {
                // SAFETY: the lock is still held until this guard finishes dropping.
                Some(previous) => unsafe { std::env::set_var(name, previous) },
                // SAFETY: as above.
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}

/// Send one request through the full route table.
async fn call(method: Method, uri: &str, body: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppConfig::new("0.5.20")))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(TunnelManager::new()))
            .configure(configure),
    )
    .await;
    let request = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
        .to_request();
    let response = test::call_service(&app, request).await;
    let status = response.status();
    let bytes = test::read_body(response).await;
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)?
    };
    Ok((status, json))
}

/// A `tailscale` stand-in whose `status --json` reports a logged-in, online device.
///
/// The `--socket <path>` this service always passes is consumed by the `shift`s, which is
/// incidentally a check that the flag is really being sent: without it the case statement
/// would see `--socket` as the subcommand and fall through to the failure branch.
const TAILSCALE_LOGGED_IN: &str = r#"#!/bin/sh
[ "$1" = "--socket" ] && shift 2
case "$1 $2" in
  "status --json")
    echo '{"BackendState":"Running","Self":{"DNSName":"r4nd0m.tail1a2b3c.ts.net.","Online":true}}'
    exit 0 ;;
  "funnel status")
    echo '{"AllowFunnel":{"r4nd0m.tail1a2b3c.ts.net:443":true}}'
    exit 0 ;;
esac
case "$1" in
  funnel) echo "Available on the internet:"; exit 0 ;;
  cert)   echo "wrote certificate"; exit 0 ;;
  ip)     echo "100.64.0.1"; exit 0 ;;
  logout) exit 0 ;;
  --version) echo "1.99.0"; exit 0 ;;
esac
echo "unexpected invocation: $*" 1>&2
exit 64
"#;

/// A `tailscale` stand-in whose daemon answers but is not logged in.
const TAILSCALE_NEEDS_LOGIN: &str = r#"#!/bin/sh
[ "$1" = "--socket" ] && shift 2
case "$1 $2" in
  "status --json")
    echo '{"BackendState":"NeedsLogin","AuthURL":"https://login.tailscale.com/a/deadbeef","Self":{"DNSName":"","Online":false}}'
    exit 0 ;;
esac
case "$1" in
  up) echo "To authenticate, visit: https://login.tailscale.com/a/deadbeef"; exit 0 ;;
  --version) echo "1.99.0"; exit 0 ;;
esac
echo "unexpected invocation: $*" 1>&2
exit 64
"#;

/// A `tailscale` stand-in on a tailnet where Funnel is switched off.
const TAILSCALE_FUNNEL_DISABLED: &str = r#"#!/bin/sh
[ "$1" = "--socket" ] && shift 2
case "$1 $2" in
  "status --json")
    echo '{"BackendState":"Running","Self":{"DNSName":"r4nd0m.tail1a2b3c.ts.net.","Online":true}}'
    exit 0 ;;
esac
case "$1" in
  funnel)
    echo "Funnel is not enabled on your tailnet." 1>&2
    echo "To enable: https://login.tailscale.com/f/funnel?node=abc123" 1>&2
    exit 1 ;;
esac
echo "unexpected invocation: $*" 1>&2
exit 64
"#;

/// A `tailscaled` stand-in that stays up.
const TAILSCALED: &str = "#!/bin/sh\nwhile true; do sleep 1; done\n";

#[actix_rt::test]
async fn an_installed_but_unauthenticated_tailscale_asks_for_a_login() -> TestResult {
    // Given: tailscale is installed and its daemon answers, but the device is not logged in.
    // This is the state that cannot be reached with no binary present, and the one where a
    // naive implementation reports a generic failure instead of an actionable step.
    let _installed = Installed::new(&[
        ("NULLROUTER_TAILSCALE_BIN", "tailscale", TAILSCALE_NEEDS_LOGIN),
        ("NULLROUTER_TAILSCALED_BIN", "tailscaled", TAILSCALED),
    ]);

    // When: enabling is attempted.
    let (status, body) = call(Method::POST, "/api/tunnel/tailscale-enable", "").await?;

    // Then: it is not an error — it is a login, with the URL to complete it in a browser.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(false)));
    assert_eq!(body.get("needsLogin"), Some(&Value::Bool(true)));
    assert_eq!(
        body.get("authUrl").and_then(Value::as_str),
        Some("https://login.tailscale.com/a/deadbeef")
    );
    Ok(())
}

#[actix_rt::test]
async fn an_installed_unauthenticated_tailscale_is_reported_distinctly_from_an_absent_one()
-> TestResult {
    // Given: installed, daemon up, not logged in.
    let _installed = Installed::new(&[
        ("NULLROUTER_TAILSCALE_BIN", "tailscale", TAILSCALE_NEEDS_LOGIN),
        ("NULLROUTER_TAILSCALED_BIN", "tailscaled", TAILSCALED),
    ]);

    // When: the check route is read.
    let (status, body) = call(Method::GET, "/api/tunnel/tailscale-check", "").await?;

    // Then: installed and daemon-running are true while logged-in is false. Collapsing these
    // into one flag is what makes a panel offer an install to someone who only needs a login.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.get("installed"), Some(&Value::Bool(true)), "{body}");
    assert_eq!(body.get("daemonInstalled"), Some(&Value::Bool(true)));
    assert_eq!(body.get("loggedIn"), Some(&Value::Bool(false)), "{body}");
    Ok(())
}

#[actix_rt::test]
async fn a_logged_in_tailscale_enables_funnel_and_reports_its_real_hostname() -> TestResult {
    // Given: installed, daemon up, logged in, Funnel permitted.
    let _installed = Installed::new(&[
        ("NULLROUTER_TAILSCALE_BIN", "tailscale", TAILSCALE_LOGGED_IN),
        ("NULLROUTER_TAILSCALED_BIN", "tailscaled", TAILSCALED),
    ]);

    // When: enabling runs the whole sequence.
    let (status, body) = call(Method::POST, "/api/tunnel/tailscale-enable", "").await?;

    // Then: it succeeds with the URL built from `Self.DNSName`, trailing dot stripped. Built
    // from the requested hostname instead, a name collision would produce a URL that 404s.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(true)), "{body}");
    assert_eq!(
        body.get("tunnelUrl").and_then(Value::as_str),
        Some("https://r4nd0m.tail1a2b3c.ts.net"),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_tailnet_with_funnel_switched_off_reports_the_url_that_switches_it_on() -> TestResult {
    // Given: logged in, but the tailnet's policy forbids Funnel. Only an admin can change
    // that, and only through the URL tailscale prints.
    let _installed = Installed::new(&[
        (
            "NULLROUTER_TAILSCALE_BIN",
            "tailscale",
            TAILSCALE_FUNNEL_DISABLED,
        ),
        ("NULLROUTER_TAILSCALED_BIN", "tailscaled", TAILSCALED),
    ]);

    // When: enabling is attempted.
    let (status, body) = call(Method::POST, "/api/tunnel/tailscale-enable", "").await?;

    // Then: the failure carries that URL rather than the raw CLI error.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(false)));
    assert_eq!(
        body.get("enableUrl").and_then(Value::as_str),
        Some("https://login.tailscale.com/f/funnel?node=abc123"),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn the_status_route_reports_a_live_funnel() -> TestResult {
    // Given: logged in with a Funnel mapping in place.
    let _installed = Installed::new(&[
        ("NULLROUTER_TAILSCALE_BIN", "tailscale", TAILSCALE_LOGGED_IN),
        ("NULLROUTER_TAILSCALED_BIN", "tailscaled", TAILSCALED),
    ]);

    // When: status is read.
    let (status, body) = call(Method::GET, "/api/tunnel/status", "").await?;

    // Then: the Tailscale half reports the URL, and reports it only because Funnel is serving.
    // A device has a name as soon as it is logged in, and publishing that as the tunnel URL
    // would advertise an address that answers nothing.
    assert_eq!(status, StatusCode::OK);
    let tailscale = body.get("tailscale").ok_or("no tailscale section")?;
    assert_eq!(tailscale.get("installed"), Some(&Value::Bool(true)));
    assert_eq!(tailscale.get("loggedIn"), Some(&Value::Bool(true)), "{body}");
    assert_eq!(tailscale.get("funnelActive"), Some(&Value::Bool(true)), "{body}");
    assert_eq!(
        tailscale.get("url").and_then(Value::as_str),
        Some("https://r4nd0m.tail1a2b3c.ts.net"),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_read_operation_returns_the_binary_output_it_produced() -> TestResult {
    // Given: an installed tailscale.
    let _installed = Installed::new(&[
        ("NULLROUTER_TAILSCALE_BIN", "tailscale", TAILSCALE_LOGGED_IN),
        ("NULLROUTER_TAILSCALED_BIN", "tailscaled", TAILSCALED),
    ]);

    // When: a catalog read operation is run through the generic surface.
    let (status, body) = call(Method::POST, "/api/tunnel/operations/tailscale.ip", "{}").await?;

    // Then: the process output comes back, which is what makes the surface useful for an
    // operation this repository has not written a bespoke handler for.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(true)));
    assert_eq!(body.get("code"), Some(&Value::from(0)));
    assert!(
        body.get("stdout")
            .and_then(Value::as_str)
            .is_some_and(|out| out.contains("100.64.0.1")),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn disabling_withdraws_funnel_and_leaves_the_daemon_alone() -> TestResult {
    // Given: a live funnel.
    let _installed = Installed::new(&[
        ("NULLROUTER_TAILSCALE_BIN", "tailscale", TAILSCALE_LOGGED_IN),
        ("NULLROUTER_TAILSCALED_BIN", "tailscaled", TAILSCALED),
    ]);

    // When: disable is called.
    let (status, body) = call(Method::POST, "/api/tunnel/tailscale-disable", "").await?;

    // Then: it succeeds, and says the daemon was left up. Killing it would drop tailnet
    // traffic the operator may be relying on for reasons unrelated to this router.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(true)));
    assert!(
        body.get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("left running")),
        "{body}"
    );
    Ok(())
}

/// A `cloudflared` stand-in that announces a quick tunnel URL the way the real one does.
const CLOUDFLARED_QUICK: &str = r#"#!/bin/sh
echo "INF Requesting new quick Tunnel on trycloudflare.com..." 1>&2
echo "INF |  https://sunny-mode-cats-tv.trycloudflare.com  |" 1>&2
echo "INF Registered tunnel connection connIndex=0" 1>&2
while true; do sleep 1; done
"#;

/// A `cloudflared` stand-in that fails the way a bad token does.
const CLOUDFLARED_BAD_TOKEN: &str = r#"#!/bin/sh
echo "ERR Couldn't connect: Unauthorized: Invalid tunnel token" 1>&2
exit 1
"#;

/// A `cloudflared` stand-in that echoes its token, to prove the scrubbing.
const CLOUDFLARED_ECHOES_TOKEN: &str = r#"#!/bin/sh
echo "INF using token ${TUNNEL_TOKEN:-none} and argv: $*" 1>&2
exit 1
"#;

#[actix_rt::test]
async fn a_quick_tunnel_reports_the_hostname_cloudflared_printed() -> TestResult {
    // Given: an installed cloudflared. A quick tunnel's hostname is assigned per run and
    // exists only in its log output, so parsing that is the only way to learn it.
    let _installed = Installed::new(&[(
        "NULLROUTER_CLOUDFLARED_BIN",
        "cloudflared",
        CLOUDFLARED_QUICK,
    )]);

    // When: enable runs.
    let (status, body) = call(Method::POST, "/api/tunnel/enable", "").await?;

    // Then: the URL comes back, and it is the tunnel host rather than the `api.` control-plane
    // host that appears in the same output.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(true)), "{body}");
    assert_eq!(
        body.get("tunnelUrl").and_then(Value::as_str),
        Some("https://sunny-mode-cats-tv.trycloudflare.com"),
        "{body}"
    );

    // And status agrees, with a pid: the process is owned, not fired and forgotten.
    let (_status, live) = call(Method::GET, "/api/tunnel/status", "").await?;
    let tunnel = live.get("tunnel").ok_or("no tunnel section")?;
    assert_eq!(tunnel.get("installed"), Some(&Value::Bool(true)));

    // Stop it, so the case does not leave a child behind.
    let (stop_status, _stopped) = call(Method::POST, "/api/tunnel/disable", "").await?;
    assert_eq!(stop_status, StatusCode::OK);
    Ok(())
}

#[actix_rt::test]
async fn a_named_tunnel_that_is_refused_reports_the_reason_from_the_log() -> TestResult {
    // Given: a cloudflared that rejects the token, which is the commonest real failure.
    let _installed = Installed::new(&[(
        "NULLROUTER_CLOUDFLARED_BIN",
        "cloudflared",
        CLOUDFLARED_BAD_TOKEN,
    )]);

    // When: a named tunnel is started.
    let (status, body) = call(
        Method::POST,
        "/api/tunnel/named/enable",
        r#"{"token":"an-invalid-token-value"}"#,
    )
    .await?;

    // Then: the failure carries what cloudflared actually said, so the operator can act on it
    // rather than on "exit code 1".
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(false)));
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .ok_or("no message")?;
    assert!(message.contains("Invalid tunnel token"), "{message}");
    Ok(())
}

#[actix_rt::test]
async fn a_token_reaches_the_child_by_environment_and_is_scrubbed_from_its_output() -> TestResult {
    // Given: a cloudflared that prints both its environment token and its whole argv. This is
    // the case that pins the two properties together: the token has to arrive, and it must not
    // come back out.
    let _installed = Installed::new(&[(
        "NULLROUTER_CLOUDFLARED_BIN",
        "cloudflared",
        CLOUDFLARED_ECHOES_TOKEN,
    )]);

    // When: a named tunnel is started with a recognisable token.
    let (status, body) = call(
        Method::POST,
        "/api/tunnel/named/enable",
        r#"{"token":"TOKEN-SENTINEL-9f8e7d6c"}"#,
    )
    .await?;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .ok_or("no message")?;

    // Then: the child received it — it printed a token rather than "none" — proving the
    // environment channel works and `--token` is not needed.
    assert!(
        !message.contains("using token none"),
        "the child never received the token: {message}"
    );
    // And it is scrubbed out of the captured log, so it reaches neither the response nor the
    // console-log buffer. This is the check that upstream's `--token <value>` fails.
    assert!(
        !message.contains("TOKEN-SENTINEL-9f8e7d6c"),
        "the token leaked into the response: {message}"
    );
    assert!(message.contains("<redacted>"), "{message}");
    // And it was never an argument in the first place: the child echoed its own argv.
    assert!(
        !message.contains("--token"),
        "the token was passed as an argument: {message}"
    );
    Ok(())
}

#[actix_rt::test]
async fn the_resolver_refuses_a_world_writable_binary() -> TestResult {
    // Given: a cloudflared that any local account can overwrite. Upstream's download path
    // produces exactly this shape by `chmod`ing what it fetched, and running it means running
    // whatever the last writer put there.
    let _installed = Installed::with_mode(
        &[(
            "NULLROUTER_CLOUDFLARED_BIN",
            "cloudflared",
            CLOUDFLARED_QUICK,
        )],
        0o777,
    );

    // When: a tunnel is started.
    let (status, body) = call(Method::POST, "/api/tunnel/enable", "").await?;

    // Then: it is refused, and the reason names the risk rather than reporting a spawn failure.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .ok_or("no message")?;
    assert!(
        message.contains("writable by group or others"),
        "{message}"
    );
    Ok(())
}

#[actix_rt::test]
async fn the_daemon_is_started_with_userspace_networking_and_no_privilege() -> TestResult {
    // Given: a tailscaled stand-in that records the arguments it was given, and a tailscale
    // whose status only succeeds once that file exists — so the daemon really has to have been
    // started for the flow to proceed.
    let home = tempfile::tempdir()?;
    let record = home.path().join("daemon-argv");
    let daemon = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" > {}\nwhile true; do sleep 1; done\n",
        record.display()
    );
    let cli = format!(
        r#"#!/bin/sh
[ "$1" = "--socket" ] && shift 2
if [ ! -f {record} ]; then echo "no daemon" 1>&2; exit 1; fi
case "$1 $2" in
  "status --json")
    echo '{{"BackendState":"Running","Self":{{"DNSName":"a.b.ts.net.","Online":true}}}}'
    exit 0 ;;
esac
case "$1" in
  funnel) exit 0 ;;
  cert) exit 0 ;;
esac
exit 64
"#,
        record = record.display()
    );
    let _installed = Installed::new(&[
        ("NULLROUTER_TAILSCALE_BIN", "tailscale", &cli),
        ("NULLROUTER_TAILSCALED_BIN", "tailscaled", &daemon),
    ]);

    // When: enabling runs, which has to start the daemon first.
    let (status, body) = call(Method::POST, "/api/tunnel/tailscale-enable", "").await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Then: the daemon was started in userspace-networking mode against our own socket and
    // state directory. That flag is what removes the need for root, and therefore the need for
    // upstream's cached sudo password.
    let argv = std::fs::read_to_string(&record)?;
    assert!(argv.contains("--tun=userspace-networking"), "{argv}");
    assert!(argv.contains("--socket="), "{argv}");
    assert!(argv.contains("--statedir="), "{argv}");
    assert!(!argv.contains("sudo"), "{argv}");
    Ok(())
}
