//! Headroom process control against interpreters and binaries that are actually present.
//!
//! `headroom.rs` covers what this machine has: a Python with no `pip` module and no `headroom`
//! binary. That leaves two states untested, and one of them is an acceptance criterion — a
//! PEP 668 interpreter has to be reported as such rather than attempted and failed.
//!
//! So these cases install stand-in executables at the paths the resolver reads from environment
//! overrides. The stand-ins print what the real tools print: `pip` refusing on an
//! externally-managed environment, `pip` succeeding, `headroom proxy` logging its uvicorn line.
//! What is being tested is the classification and the sequence, and both are ours.
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

const UNREACHABLE_STATE_ADDR: &str = "http://127.0.0.1:1";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Serialises the cases that redirect the interpreter, since they share the environment.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Every variable these cases touch.
const MANAGED_VARS: &[&str] = &[
    "NULLROUTER_PYTHON",
    "PATH",
    "HEADROOM_URL",
    "HEADROOM_CODE_AWARE",
    "HEADROOM_KOMPRESS",
];

/// Stand-in executables and the environment pointing at them.
///
/// Holds the lock as a field, the shape `cli_tool_writes.rs` established, so no `MutexGuard`
/// sits in a local across an await.
struct Fixture {
    _lock: MutexGuard<'static, ()>,
    home: tempfile::TempDir,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl Fixture {
    /// Start with an empty directory and the managed variables cleared.
    fn new() -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = MANAGED_VARS
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();
        Self {
            _lock: lock,
            home: tempfile::tempdir().expect("tempdir"),
            previous,
        }
    }

    /// Write an executable stand-in and return its path.
    fn script(&self, name: &str, body: &str) -> std::path::PathBuf {
        let path = self.home.path().join(name);
        std::fs::write(&path, body).expect("write the stand-in");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make it executable");
        path
    }

    /// Point `NULLROUTER_PYTHON` at a stand-in interpreter.
    fn python(&self, body: &str) -> &Self {
        let path = self.script("python3", body);
        // SAFETY: the lock is held, so no other case in this binary reads this variable while it
        // is being set.
        unsafe { std::env::set_var("NULLROUTER_PYTHON", path) };
        self
    }

    /// Put a stand-in `headroom` on a `PATH` containing only this directory.
    ///
    /// Replacing `PATH` rather than prepending, so a real `headroom` on the host — if this ever
    /// runs somewhere that has one — cannot be picked up instead.
    fn headroom(&self, body: &str) -> &Self {
        let _path = self.script("headroom", body);
        // SAFETY: as above.
        unsafe { std::env::set_var("PATH", self.home.path()) };
        self
    }

    /// Set one of the option variables.
    fn env(&self, name: &str, value: &str) -> &Self {
        // SAFETY: as above.
        unsafe { std::env::set_var(name, value) };
        self
    }
}

impl Drop for Fixture {
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

/// A stand-in interpreter that reports a supported version and refuses to install, PEP 668 style.
///
/// The wording is `pip`'s own, and the exit code is `1` — the same code it uses for a missing
/// package and for a network failure, which is why the classification cannot key on the code.
const PYTHON_EXTERNALLY_MANAGED: &str = r#"#!/bin/sh
case "$*" in
  *--version*) echo "Python 3.12.1"; exit 0 ;;
  *"-m pip install"*)
    echo "error: externally-managed-environment" 1>&2
    echo "" 1>&2
    echo "x This environment is externally managed" 1>&2
    exit 1 ;;
  *"-m pip list"*) echo "Package Version"; exit 0 ;;
esac
exit 0
"#;

/// A stand-in interpreter whose install succeeds.
const PYTHON_INSTALLS: &str = r#"#!/bin/sh
case "$*" in
  *--version*) echo "Python 3.12.1"; exit 0 ;;
  *"-m pip install"*)
    echo "Collecting headroom-ai"
    echo "Successfully installed headroom-ai-1.4.0"
    exit 0 ;;
  *"-m pip uninstall"*) echo "Successfully uninstalled torch-2.4.0"; exit 0 ;;
  *"-m pip list"*) echo "Package Version"; exit 0 ;;
esac
exit 0
"#;

/// A stand-in interpreter whose install fails for an ordinary reason.
const PYTHON_INSTALL_FAILS: &str = r#"#!/bin/sh
case "$*" in
  *--version*) echo "Python 3.12.1"; exit 0 ;;
  *"-m pip install"*)
    echo "ERROR: Could not find a version that satisfies the requirement headroom-ai" 1>&2
    exit 1 ;;
  *"-m pip list"*) echo "Package Version"; exit 0 ;;
esac
exit 0
"#;

/// A stand-in interpreter that is too old.
const PYTHON_TOO_OLD: &str = r#"#!/bin/sh
case "$*" in
  *--version*) echo "Python 3.8.10"; exit 0 ;;
esac
exit 0
"#;

#[actix_rt::test]
async fn an_externally_managed_interpreter_is_reported_as_such() -> TestResult {
    // Given: a distribution-managed Python, which pip refuses to install into. This is the
    // acceptance criterion: it must be reported as its own case, because the fix is a virtualenv
    // and no number of retries will work.
    let fixture = Fixture::new();
    fixture.python(PYTHON_EXTERNALLY_MANAGED);

    // When: an install is requested.
    let (status, body) = call(Method::POST, "/api/headroom/extras", r#"{"extras":["code"]}"#).await?;

    // Then: 409, not a 502 retryable failure, with a code a panel can branch on.
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(false)));
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("EXTERNALLY_MANAGED"),
        "{body}"
    );
    // And the message names the way out, plus what pip actually said.
    let error = body.get("error").and_then(Value::as_str).ok_or("no error")?;
    assert!(error.contains("virtual environment"), "{error}");
    assert!(error.contains("NULLROUTER_PYTHON"), "{error}");
    assert!(error.contains("externally-managed-environment"), "{error}");
    // The requirement is still reported, so the operator can run it themselves.
    assert_eq!(
        body.get("spec").and_then(Value::as_str),
        Some("headroom-ai[proxy,code]"),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_successful_install_reports_what_pip_did_and_re_detects() -> TestResult {
    // Given: an interpreter whose install succeeds.
    let fixture = Fixture::new();
    fixture.python(PYTHON_INSTALLS);

    // When: both extras are requested.
    let (status, body) = call(
        Method::POST,
        "/api/headroom/extras",
        r#"{"extras":["code","ml"]}"#,
    )
    .await?;

    // Then: success, with the requirement that was installed and pip's own closing lines.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(true)));
    assert_eq!(
        body.get("spec").and_then(Value::as_str),
        Some("headroom-ai[proxy,code,ml]"),
        "{body}"
    );
    assert!(
        body.get("output")
            .and_then(Value::as_str)
            .is_some_and(|output| output.contains("Successfully installed")),
        "{body}"
    );
    // And the state is re-detected rather than assumed: pip can succeed by resolving an
    // already-satisfied requirement, so what the panel needs is what is installed now.
    assert!(body.get("extras").is_some(), "{body}");
    Ok(())
}

#[actix_rt::test]
async fn an_ordinary_pip_failure_is_not_mistaken_for_pep_668() -> TestResult {
    // Given: pip failing for a reason a retry might fix. It exits 1, the same as the PEP 668
    // refusal, so keying on the code rather than the text would give both the same wrong advice.
    let fixture = Fixture::new();
    fixture.python(PYTHON_INSTALL_FAILS);

    // When: an install is requested.
    let (status, body) = call(Method::POST, "/api/headroom/extras", r#"{"extras":["ml"]}"#).await?;

    // Then: PIP_FAILED at 502, and the message is pip's, not a virtualenv suggestion.
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("PIP_FAILED"),
        "{body}"
    );
    let error = body.get("error").and_then(Value::as_str).ok_or("no error")?;
    assert!(error.contains("Could not find a version"), "{error}");
    assert!(
        !error.contains("virtual environment"),
        "an ordinary failure must not be given the PEP 668 advice: {error}"
    );
    Ok(())
}

#[actix_rt::test]
async fn an_interpreter_below_the_minimum_is_not_the_one_chosen() -> TestResult {
    // Given: the override points at a Python 3.8, where headroom-ai needs 3.10. The override
    // wins the *search order*, not the version check — otherwise naming an interpreter would be
    // a way to bypass the requirement and get a confusing failure from inside pip instead.
    let fixture = Fixture::new();
    let stub = fixture.home.path().join("python3");
    fixture.python(PYTHON_TOO_OLD);

    // When: detection is asked which interpreter it would use.
    let (status, body) = call(Method::GET, "/api/headroom/extras", "").await?;

    // Then: not that one. The search falls through to whatever else the host has, so the
    // assertion is about the rejection rather than about what replaced it.
    assert_eq!(status, StatusCode::OK, "{body}");
    let chosen = body.get("python").and_then(Value::as_str);
    assert_ne!(
        chosen,
        Some(stub.to_string_lossy().as_ref()),
        "a 3.8 interpreter was chosen despite the 3.10 minimum: {body}"
    );
    // And the reported minimum is the one being enforced.
    assert_eq!(
        body.get("pythonMinVersion").and_then(Value::as_str),
        Some("3.10"),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_named_interpreter_is_used_when_it_meets_the_minimum() -> TestResult {
    // The other half: an override that is new enough must actually be used, or pointing at a
    // virtualenv would not be the way out of a PEP 668 refusal that the error message promises.
    let fixture = Fixture::new();
    let stub = fixture.home.path().join("python3");
    fixture.python(PYTHON_INSTALLS);

    let (status, body) = call(Method::GET, "/api/headroom/extras", "").await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body.get("python").and_then(Value::as_str),
        Some(stub.to_string_lossy().as_ref()),
        "the named interpreter was not used: {body}"
    );
    // Reported as major.minor, which is what the minimum is compared against.
    assert_eq!(
        body.get("pythonVersion").and_then(Value::as_str),
        Some("3.12"),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn uninstall_removes_the_marker_packages_the_extra_pulled_in() -> TestResult {
    // Given: an interpreter whose uninstall succeeds.
    let fixture = Fixture::new();
    fixture.python(PYTHON_INSTALLS);

    // When: `ml` is removed.
    let (status, body) = call(
        Method::DELETE,
        "/api/headroom/extras",
        r#"{"extras":["ml"]}"#,
    )
    .await?;

    // Then: the packages named are the marker packages for that extra, so removing exactly
    // reverses what installing added.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(true)));
    assert!(
        body.get("spec")
            .and_then(Value::as_str)
            .is_some_and(|spec| !spec.is_empty()),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn uninstall_with_no_recognised_extra_is_refused_rather_than_running_pip() -> TestResult {
    let fixture = Fixture::new();
    fixture.python(PYTHON_INSTALLS);

    // When: nothing recognisable is named. An empty package list reaching `pip uninstall -y`
    // would be a command with no operand, and the answer has to come before that.
    let (status, body) = call(
        Method::DELETE,
        "/api/headroom/extras",
        r#"{"extras":["nonsense"]}"#,
    )
    .await?;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("NO_EXTRAS"),
        "{body}"
    );
    assert_eq!(
        body.get("ignored"),
        Some(&serde_json::json!(["nonsense"])),
        "{body}"
    );
    Ok(())
}

/// A stand-in `headroom` that logs the uvicorn line the readiness rule waits for.
const HEADROOM_STARTS: &str = r#"#!/bin/sh
echo "argv: $*" > "$(dirname "$0")/argv.txt"
echo "INFO:     Started server process [1]"
echo "INFO:     Uvicorn running on http://127.0.0.1:8787 (Press CTRL+C to quit)"
while true; do sleep 1; done
"#;

/// A stand-in `headroom` that starts but never prints a recognisable startup line.
///
/// The shape of a future uvicorn that reworded its banner, or a headroom that stopped using
/// uvicorn. Upstream would accept this — its check is only that the process is still alive — so
/// this port must too.
const HEADROOM_SILENT: &str = r#"#!/bin/sh
echo "argv: $*" > "$(dirname "$0")/argv.txt"
echo "starting up, in some other wording entirely"
while true; do sleep 1; done
"#;

/// A stand-in `headroom` that exits before announcing anything.
const HEADROOM_FAILS: &str = r#"#!/bin/sh
echo "Error: port 8787 is already in use" 1>&2
exit 1
"#;

#[actix_rt::test]
async fn starting_the_proxy_reports_the_pid_this_service_owns() -> TestResult {
    // Given: a headroom binary that comes up and logs its uvicorn line.
    let fixture = Fixture::new();
    fixture.headroom(HEADROOM_STARTS);

    // When: start is called, then status is read, then stop.
    let (start_status, start) = call(Method::POST, "/api/headroom/start", "").await?;
    let (status_status, status_body) = call(Method::GET, "/api/headroom/status", "").await?;
    let (stop_status, stop) = call(Method::POST, "/api/headroom/stop", "").await?;

    // Then: the start succeeds with a pid, and the pid is this service's own rather than one
    // read back from a file that might describe a reused pid.
    assert_eq!(start_status, StatusCode::OK, "{start}");
    assert_eq!(start.get("success"), Some(&Value::Bool(true)));
    assert_eq!(start.get("running"), Some(&Value::Bool(true)), "{start}");
    assert_eq!(start.get("state").and_then(Value::as_str), Some("running"));
    let pid = start.get("pid").and_then(Value::as_u64);
    assert!(pid.is_some_and(|pid| pid > 1), "{start}");

    // And status agrees while it runs.
    assert_eq!(status_status, StatusCode::OK);
    assert_eq!(status_body.get("running"), Some(&Value::Bool(true)), "{status_body}");
    assert_eq!(status_body.get("healthy"), Some(&Value::Bool(true)), "{status_body}");
    assert_eq!(status_body.get("managedPid").and_then(Value::as_u64), pid);

    // And the stop reports it down.
    assert_eq!(stop_status, StatusCode::OK, "{stop}");
    assert_eq!(stop.get("running"), Some(&Value::Bool(false)));
    Ok(())
}

#[actix_rt::test]
async fn the_proxy_argv_matches_the_configured_options() -> TestResult {
    // Given: kompress off and code-aware on, which is the combination that produces both flags.
    // Upstream's default for kompress is *on* — it pushes `--disable-kompress` only when the
    // setting is false — so getting this backwards would silently disable compression.
    let fixture = Fixture::new();
    fixture
        .headroom(HEADROOM_STARTS)
        .env("HEADROOM_CODE_AWARE", "true")
        .env("HEADROOM_KOMPRESS", "false");

    // When: the proxy is started.
    let (status, body) = call(Method::POST, "/api/headroom/start", "").await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Then: the child received the port and both flags.
    let argv = std::fs::read_to_string(fixture.home.path().join("argv.txt"))?;
    assert!(argv.contains("proxy"), "{argv}");
    assert!(argv.contains("--port 8787"), "{argv}");
    assert!(argv.contains("--code-aware"), "{argv}");
    assert!(argv.contains("--disable-kompress"), "{argv}");

    let (_stop_status, _stop) = call(Method::POST, "/api/headroom/stop", "").await?;
    Ok(())
}

#[actix_rt::test]
async fn kompress_stays_on_when_nothing_says_otherwise() -> TestResult {
    // The default that matters: upstream's `settings.headroomKompress !== false`.
    let fixture = Fixture::new();
    fixture.headroom(HEADROOM_STARTS);

    let (status, body) = call(Method::POST, "/api/headroom/start", "").await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    let argv = std::fs::read_to_string(fixture.home.path().join("argv.txt"))?;
    assert!(
        !argv.contains("--disable-kompress"),
        "kompress must default to on: {argv}"
    );
    assert!(
        !argv.contains("--code-aware"),
        "code-aware must default to off: {argv}"
    );

    let (_stop_status, _stop) = call(Method::POST, "/api/headroom/stop", "").await?;
    Ok(())
}

#[actix_rt::test]
async fn a_proxy_that_never_announces_itself_still_starts() -> TestResult {
    // Given: a headroom whose startup wording this port does not recognise. Upstream's own check
    // is "still alive after eight seconds" and never reads the log, so requiring a particular
    // line would fail a start that upstream accepts — strictly worse, not stricter.
    let fixture = Fixture::new();
    fixture.headroom(HEADROOM_SILENT);

    // When: start is called.
    let (status, body) = call(Method::POST, "/api/headroom/start", "").await?;

    // Then: it succeeds on survival alone, with a pid.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(true)), "{body}");
    assert!(
        body.get("pid").and_then(Value::as_u64).is_some_and(|pid| pid > 1),
        "{body}"
    );

    let (_stop_status, _stop) = call(Method::POST, "/api/headroom/stop", "").await?;
    Ok(())
}

#[actix_rt::test]
async fn a_proxy_that_will_not_start_reports_what_it_printed() -> TestResult {
    // Given: a headroom that exits immediately with a reason.
    let fixture = Fixture::new();
    fixture.headroom(HEADROOM_FAILS);

    // When: start is called.
    let (status, body) = call(Method::POST, "/api/headroom/start", "").await?;

    // Then: the failure carries the daemon's own message, which is where the cause is. "exit
    // code 1" alone would leave an operator guessing at a port conflict.
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(false)));
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("START_FAILED"),
        "{body}"
    );
    assert!(
        body.get("error")
            .and_then(Value::as_str)
            .is_some_and(|error| error.contains("already in use")),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_proxy_on_another_host_is_not_ours_to_start() -> TestResult {
    // Given: HEADROOM_URL pointing somewhere else. Upstream refuses this with a 400 and so does
    // this, and the check has to run before anything is spawned.
    let fixture = Fixture::new();
    fixture
        .headroom(HEADROOM_STARTS)
        .env("HEADROOM_URL", "http://headroom.example.com:8787");

    for route in ["/api/headroom/start", "/api/headroom/restart"] {
        // When: a start or restart is requested.
        let (status, body) = call(Method::POST, route, "").await?;

        // Then: 400 EXTERNAL_PROXY, naming the URL that was judged.
        assert_eq!(status, StatusCode::BAD_REQUEST, "{route}: {body}");
        assert_eq!(
            body.get("code").and_then(Value::as_str),
            Some("EXTERNAL_PROXY"),
            "{route}: {body}"
        );
        assert_eq!(
            body.get("url").and_then(Value::as_str),
            Some("http://headroom.example.com:8787"),
            "{route}: {body}"
        );
    }

    // And nothing was started, so no argv file exists.
    assert!(
        !fixture.home.path().join("argv.txt").exists(),
        "a refused external proxy must not have spawned anything"
    );
    Ok(())
}

#[actix_rt::test]
async fn restart_replaces_the_running_child() -> TestResult {
    // Given: a running proxy.
    let fixture = Fixture::new();
    fixture.headroom(HEADROOM_STARTS);
    let (first_status, first) = call(Method::POST, "/api/headroom/start", "").await?;
    assert_eq!(first_status, StatusCode::OK, "{first}");
    let first_pid = first.get("pid").and_then(Value::as_u64).ok_or("no pid")?;

    // When: restart is called.
    let (status, body) = call(Method::POST, "/api/headroom/restart", "").await?;

    // Then: it is a new process, and the old one is gone. One proxy at a time is what stops two
    // of them fighting over the port.
    assert_eq!(status, StatusCode::OK, "{body}");
    let second_pid = body.get("pid").and_then(Value::as_u64).ok_or("no pid")?;
    assert_ne!(first_pid, second_pid, "restart reused the pid: {body}");

    let (_stop_status, _stop) = call(Method::POST, "/api/headroom/stop", "").await?;
    Ok(())
}
