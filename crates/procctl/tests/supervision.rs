//! The supervisor driving real child processes.
//!
//! Every case here spawns an actual process, because the properties being checked are
//! properties of process handling — a fake would only re-test the fake. `/bin/sh` stands in
//! for `cloudflared`: it can print on either stream, exit with a chosen code, ignore
//! `SIGTERM`, or stay up indefinitely, which covers every behaviour the real daemons have.
#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "free helpers in an integration test are not covered by clippy.toml's \
              allow-expect-in-tests, which only reaches #[test] functions"
)]

use std::time::Duration;

use nullrouter_procctl::StopOutcome;
use nullrouter_procctl::binary::{BinarySpec, Executable};
use nullrouter_procctl::secret::Secret;
use nullrouter_procctl::supervise::{
    ChildSpec, ReadyRule, RestartPolicy, StartError, State, Supervisor,
};

/// Resolve `/bin/sh`, the stand-in for a supervised daemon.
fn shell() -> Executable {
    BinarySpec {
        name: "sh",
        candidates: &["/bin/sh"],
        env_override: "NULLROUTER_SUPERVISION_TEST_SH",
        search_dirs: &[],
    }
    .resolve(None)
    .expect("/bin/sh must exist")
}

/// A spec running one shell script.
fn spec(script: &str, ready: ReadyRule) -> ChildSpec {
    ChildSpec {
        program: shell(),
        args: vec!["-c".to_owned(), script.to_owned()],
        env: Vec::new(),
        secrets: Vec::new(),
        ready,
        startup_timeout: Duration::from_secs(10),
        graceful_timeout: Duration::from_secs(2),
        restart: RestartPolicy::never(),
        log_capacity: 50,
    }
}

/// Poll until `predicate` holds, or give up. Returns whether it held.
async fn settles<F>(supervisor: &Supervisor, mut predicate: F) -> bool
where
    F: FnMut(&nullrouter_procctl::supervise::Snapshot) -> bool,
{
    for _ in 0..300_u32 {
        if predicate(&supervisor.snapshot()) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn a_child_that_announces_itself_becomes_ready() {
    let supervisor = Supervisor::spawn("ready-child", 50);

    let value = supervisor
        .start(spec(
            "echo listening on port; sleep 60",
            ReadyRule::Occurrences {
                needle: "listening",
                times: 1,
            },
        ))
        .await
        .expect("a child that prints the needle must become ready");

    assert!(value.is_none(), "Occurrences captures no value");
    let snapshot = supervisor.snapshot();
    assert_eq!(snapshot.state, State::Running);
    assert!(snapshot.is_running());
    assert!(snapshot.pid.is_some(), "a running child has a pid");
    assert_eq!(snapshot.restarts, 0);
    assert!(snapshot.last_error.is_none());
}

#[tokio::test]
async fn readiness_waits_for_the_required_number_of_occurrences() {
    let supervisor = Supervisor::spawn("counted-child", 50);

    // This is the `cloudflared tunnel run` shape: one line per edge connection, and the
    // tunnel is only up once four have registered.
    let start = std::time::Instant::now();
    supervisor
        .start(spec(
            "for i in 1 2 3 4; do echo Registered tunnel connection $i; sleep 0.1; done; sleep 60",
            ReadyRule::Occurrences {
                needle: "Registered tunnel connection",
                times: 4,
            },
        ))
        .await
        .expect("four occurrences must satisfy the rule");

    // Readiness cannot have been declared before the fourth line was printed.
    assert!(
        start.elapsed() >= Duration::from_millis(300),
        "returned after {:?}, too early to have seen four lines",
        start.elapsed()
    );
    assert!(supervisor.snapshot().is_running());
}

#[tokio::test]
async fn a_partial_count_is_not_ready_and_times_out() {
    let mut spec = spec(
        // Only two of the four the rule wants.
        "echo Registered tunnel connection 1; echo Registered tunnel connection 2; sleep 60",
        ReadyRule::Occurrences {
            needle: "Registered tunnel connection",
            times: 4,
        },
    );
    spec.startup_timeout = Duration::from_millis(700);
    let supervisor = Supervisor::spawn("partial-child", 50);

    let error = supervisor
        .start(spec)
        .await
        .expect_err("two of four is not ready");

    match error {
        StartError::NotReady { tail, .. } => {
            // The tail has to carry what the child did say, or the operator has nothing.
            assert!(tail.contains("Registered tunnel connection 2"), "{tail}");
        }
        other => panic!("expected NotReady, got {other:?}"),
    }
    // And the child must be gone, not left running unsupervised.
    assert!(
        settles(&supervisor, |snapshot| snapshot.pid.is_none()).await,
        "a timed-out start must leave no child behind"
    );
}

#[tokio::test]
async fn readiness_can_extract_a_value_from_a_log_line() {
    /// The quick-tunnel hostname is only ever printed, exactly like this.
    fn quick_tunnel_url(line: &str) -> Option<String> {
        let start = line.find("https://")?;
        let rest = line.get(start..)?;
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let url = rest.get(..end)?;
        url.ends_with(".trycloudflare.com").then(|| url.to_owned())
    }

    let supervisor = Supervisor::spawn("extract-child", 50);

    let value = supervisor
        .start(spec(
            "echo 'INF |  https://sunny-mode-cats.trycloudflare.com  |'; sleep 60",
            ReadyRule::Extract(quick_tunnel_url),
        ))
        .await
        .expect("the extractor must match");

    assert_eq!(
        value.as_deref(),
        Some("https://sunny-mode-cats.trycloudflare.com")
    );
    assert_eq!(
        supervisor.snapshot().ready_value.as_deref(),
        Some("https://sunny-mode-cats.trycloudflare.com"),
        "the captured value stays visible while the child runs"
    );
}

#[tokio::test]
async fn readiness_is_detected_on_stderr_too() {
    // `cloudflared` prints its quick-tunnel URL to stderr, so a stdout-only watcher would
    // never see it.
    let supervisor = Supervisor::spawn("stderr-child", 50);

    supervisor
        .start(spec(
            "echo on-stderr 1>&2; sleep 60",
            ReadyRule::Occurrences {
                needle: "on-stderr",
                times: 1,
            },
        ))
        .await
        .expect("a needle on stderr must count");

    assert!(supervisor.snapshot().is_running());
}

#[tokio::test]
async fn a_child_that_exits_during_startup_reports_its_output() {
    let supervisor = Supervisor::spawn("early-exit-child", 50);

    let error = supervisor
        .start(spec(
            "echo failed to connect to edge 1>&2; exit 1",
            ReadyRule::Occurrences {
                needle: "never printed",
                times: 1,
            },
        ))
        .await
        .expect_err("an exit before readiness is a failed start");

    match error {
        StartError::ExitedEarly { status, tail, .. } => {
            assert_eq!(status, "exit code 1");
            assert!(tail.contains("failed to connect to edge"), "{tail}");
        }
        other => panic!("expected ExitedEarly, got {other:?}"),
    }
    assert_eq!(supervisor.snapshot().state, State::Failed);
}

#[tokio::test]
async fn a_one_shot_child_is_ready_when_it_exits_zero() {
    // The `tailscale funnel --bg` shape: it configures and returns.
    let supervisor = Supervisor::spawn("one-shot-child", 50);

    let value = supervisor
        .start(spec("echo configured; exit 0", ReadyRule::CompletesSuccessfully))
        .await
        .expect("a zero exit is success for this rule");

    assert!(value.is_none());
    assert!(
        settles(&supervisor, |snapshot| snapshot.state == State::Stopped).await,
        "a finished one-shot is stopped, not running: {:?}",
        supervisor.snapshot().state
    );
}

#[tokio::test]
async fn a_one_shot_child_that_fails_is_a_failed_start() {
    let supervisor = Supervisor::spawn("one-shot-failure", 50);

    let error = supervisor
        .start(spec(
            "echo Funnel is not enabled 1>&2; exit 1",
            ReadyRule::CompletesSuccessfully,
        ))
        .await
        .expect_err("a non-zero exit must fail the start");

    match error {
        StartError::ExitedEarly { tail, status, .. } => {
            assert_eq!(status, "exit code 1");
            assert!(tail.contains("Funnel is not enabled"), "{tail}");
        }
        other => panic!("expected ExitedEarly, got {other:?}"),
    }
}

#[tokio::test]
async fn a_spawned_rule_is_ready_without_any_output() {
    // `tailscaled` prints nothing useful at startup; readiness is its socket answering,
    // which the caller probes separately.
    let supervisor = Supervisor::spawn("silent-daemon", 50);

    supervisor
        .start(spec("sleep 60", ReadyRule::Spawned))
        .await
        .expect("a silent child must still be ready under this rule");

    assert!(supervisor.snapshot().is_running());
}

#[tokio::test]
async fn survival_alone_counts_as_ready_when_the_needle_never_appears() {
    // The rule that mirrors upstream's headroom check: it waits eight seconds and accepts a
    // process that is still alive. A child whose startup wording changed must not be reported as
    // a failed start when the original would have accepted it.
    let supervisor = Supervisor::spawn("survivor", 50);
    let start = std::time::Instant::now();

    supervisor
        .start(spec(
            "echo something else entirely; sleep 60",
            ReadyRule::SurvivesOr {
                needle: "never printed",
                grace: Duration::from_millis(400),
            },
        ))
        .await
        .expect("surviving the grace period must be enough");

    assert!(
        start.elapsed() >= Duration::from_millis(400),
        "returned after {:?}, before the grace period elapsed",
        start.elapsed()
    );
    assert!(supervisor.snapshot().is_running());
    supervisor.stop().await;
}

#[tokio::test]
async fn the_needle_ends_the_wait_early_when_it_does_appear() {
    // The stronger signal is preferred: a child that announces itself should not be held for the
    // whole grace period, or every start pays a delay it does not need.
    let supervisor = Supervisor::spawn("announcer", 50);
    let start = std::time::Instant::now();

    supervisor
        .start(spec(
            "echo Uvicorn running on port 8787; sleep 60",
            ReadyRule::SurvivesOr {
                needle: "Uvicorn running",
                grace: Duration::from_secs(20),
            },
        ))
        .await
        .expect("the needle must satisfy the rule");

    assert!(
        start.elapsed() < Duration::from_secs(5),
        "waited {:?}; the needle should have ended the wait",
        start.elapsed()
    );
    supervisor.stop().await;
}

#[tokio::test]
async fn a_child_that_dies_inside_the_grace_period_still_fails() {
    // The half of upstream's check that carries the weight: an early exit is a failed start, and
    // accepting survival must not weaken that.
    let supervisor = Supervisor::spawn("early-death", 50);

    let error = supervisor
        .start(spec(
            "echo binding to port 8787 1>&2; sleep 0.1; exit 1",
            ReadyRule::SurvivesOr {
                needle: "never printed",
                grace: Duration::from_secs(5),
            },
        ))
        .await
        .expect_err("a child that dies inside the grace period has not started");

    match error {
        StartError::ExitedEarly { tail, .. } => {
            assert!(tail.contains("binding to port 8787"), "{tail}");
        }
        other => panic!("expected ExitedEarly, got {other:?}"),
    }
}

#[tokio::test]
async fn a_grace_longer_than_the_startup_timeout_is_clamped() {
    // Otherwise the rule would be harsher than the one it mirrors: survival would be unreachable
    // and the deadline would fire first, failing every start.
    let mut spec = spec(
        "sleep 60",
        ReadyRule::SurvivesOr {
            needle: "never printed",
            grace: Duration::from_secs(600),
        },
    );
    spec.startup_timeout = Duration::from_millis(500);
    let supervisor = Supervisor::spawn("clamped", 50);

    supervisor
        .start(spec)
        .await
        .expect("the grace must be clamped to the startup timeout, not exceed it");

    assert!(supervisor.snapshot().is_running());
    supervisor.stop().await;
}

#[tokio::test]
async fn stop_terminates_the_child_gracefully() {
    let supervisor = Supervisor::spawn("graceful-child", 50);
    supervisor
        .start(spec(
            "echo up; sleep 60",
            ReadyRule::Occurrences {
                needle: "up",
                times: 1,
            },
        ))
        .await
        .expect("must start");
    let pid = supervisor.snapshot().pid.expect("a running child has a pid");

    let outcome = supervisor.stop().await;

    assert_eq!(outcome, StopOutcome::Graceful);
    assert_eq!(supervisor.snapshot().state, State::Stopped);
    assert!(supervisor.snapshot().pid.is_none());
    // And the process is really gone, not merely forgotten.
    assert!(!process_exists(pid), "pid {pid} outlived the stop");
}

#[tokio::test]
async fn a_child_that_ignores_sigterm_is_killed() {
    let supervisor = Supervisor::spawn("stubborn-child", 50);
    // `trap '' TERM` makes the shell ignore SIGTERM entirely, which is the case the
    // escalation exists for.
    let mut spec = spec(
        "trap '' TERM; echo trapped; while true; do sleep 0.2; done",
        ReadyRule::Occurrences {
            needle: "trapped",
            times: 1,
        },
    );
    spec.graceful_timeout = Duration::from_millis(400);
    supervisor.start(spec).await.expect("must start");
    let pid = supervisor.snapshot().pid.expect("a pid");

    let outcome = supervisor.stop().await;

    assert_eq!(
        outcome,
        StopOutcome::Forced,
        "a child that ignores SIGTERM must be reported as forced"
    );
    assert!(!process_exists(pid), "pid {pid} survived the forced kill");
}

#[tokio::test]
async fn stop_is_idempotent() {
    let supervisor = Supervisor::spawn("idempotent-stop", 50);

    assert_eq!(
        supervisor.stop().await,
        StopOutcome::NotRunning,
        "stopping nothing is not an error"
    );

    supervisor
        .start(spec("sleep 60", ReadyRule::Spawned))
        .await
        .expect("must start");
    assert_eq!(supervisor.stop().await, StopOutcome::Graceful);
    assert_eq!(supervisor.stop().await, StopOutcome::NotRunning);
}

#[tokio::test]
async fn a_second_start_replaces_the_first_child() {
    let supervisor = Supervisor::spawn("replacing-child", 50);
    supervisor
        .start(spec(
            "echo first; sleep 60",
            ReadyRule::Occurrences {
                needle: "first",
                times: 1,
            },
        ))
        .await
        .expect("must start");
    let first = supervisor.snapshot().pid.expect("a pid");

    supervisor
        .start(spec(
            "echo second; sleep 60",
            ReadyRule::Occurrences {
                needle: "second",
                times: 1,
            },
        ))
        .await
        .expect("the second start must succeed");
    let second = supervisor.snapshot().pid.expect("a pid");

    assert_ne!(first, second, "a new child must really be a new process");
    assert!(
        !process_exists(first),
        "the superseded child {first} is still running"
    );
    assert!(process_exists(second));
}

#[tokio::test]
async fn only_one_child_runs_per_supervisor() {
    // The invariant behind "no room for things going wrong": two cloudflared processes on
    // the same tunnel fight over it, and upstream's port-pattern `pkill` exists precisely
    // because it loses track of them.
    let supervisor = Supervisor::spawn("single-child", 50);
    let mut pids = Vec::new();

    for _ in 0..4_u32 {
        supervisor
            .start(spec("sleep 60", ReadyRule::Spawned))
            .await
            .expect("must start");
        pids.push(supervisor.snapshot().pid.expect("a pid"));
    }

    let live: Vec<u32> = pids
        .iter()
        .copied()
        .filter(|pid| process_exists(*pid))
        .collect();
    assert_eq!(live.len(), 1, "expected exactly one live child, got {live:?}");
    assert_eq!(live.first().copied(), pids.last().copied());
    supervisor.stop().await;
}

#[tokio::test]
async fn an_unexpected_exit_is_restarted_within_the_policy() {
    let mut spec = spec(
        "echo up; sleep 0.3; exit 3",
        ReadyRule::Occurrences {
            needle: "up",
            times: 1,
        },
    );
    spec.restart = RestartPolicy {
        max_attempts: 2,
        backoff: Duration::from_millis(80),
        max_backoff: Duration::from_millis(200),
        reset_after: Duration::from_secs(60),
    };
    let supervisor = Supervisor::spawn("restarting-child", 50);

    supervisor.start(spec).await.expect("the first start works");

    // It dies, is restarted twice, then must land in Failed rather than looping forever.
    assert!(
        settles(&supervisor, |snapshot| snapshot.state == State::Failed).await,
        "expected Failed, got {:?}",
        supervisor.snapshot()
    );
    let snapshot = supervisor.snapshot();
    assert_eq!(snapshot.restarts, 2, "the policy allowed exactly two");
    assert!(snapshot.pid.is_none());
    assert!(
        snapshot
            .last_error
            .as_ref()
            .is_some_and(|error| error.contains("exit") || error.contains("restart")),
        "{:?}",
        snapshot.last_error
    );
}

#[tokio::test]
async fn a_never_restart_policy_leaves_the_child_down() {
    let supervisor = Supervisor::spawn("no-restart-child", 50);
    supervisor
        .start(spec(
            "echo up; sleep 0.2; exit 1",
            ReadyRule::Occurrences {
                needle: "up",
                times: 1,
            },
        ))
        .await
        .expect("must start");

    assert!(
        settles(&supervisor, |snapshot| {
            matches!(snapshot.state, State::Failed)
        })
        .await,
        "expected Failed, got {:?}",
        supervisor.snapshot().state
    );
    assert_eq!(supervisor.snapshot().restarts, 0, "none were attempted");
}

#[tokio::test]
async fn a_stop_during_a_pending_start_answers_the_caller() {
    // The deadlock this guards against: a start holding the engine for its whole timeout
    // while a stop waits behind it.
    let mut spec = spec(
        "sleep 60",
        ReadyRule::Occurrences {
            needle: "never printed",
            times: 1,
        },
    );
    spec.startup_timeout = Duration::from_secs(30);
    let supervisor = Supervisor::spawn("interrupted-start", 50);

    let starting = {
        let supervisor = supervisor.clone();
        tokio::spawn(async move { supervisor.start(spec).await })
    };
    assert!(
        settles(&supervisor, |snapshot| snapshot.pid.is_some()).await,
        "the child should have spawned"
    );

    // Well inside the 30s startup timeout: if the stop had to queue behind the start, this
    // would not return for half a minute.
    let stopped = tokio::time::timeout(Duration::from_secs(5), supervisor.stop())
        .await
        .expect("a stop must not wait out the startup deadline");
    assert_eq!(stopped, StopOutcome::Graceful);

    let outcome = tokio::time::timeout(Duration::from_secs(5), starting)
        .await
        .expect("the pending start must be answered promptly")
        .expect("the task must not panic");
    assert!(
        matches!(outcome, Err(StartError::NotReady { .. })),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_secret_never_reaches_the_log_ring() {
    let mut spec = spec(
        // The child echoing its own credential is exactly how these leak: `cloudflared`
        // quotes parts of its configuration back when a connection fails.
        "echo \"connecting with token $TUNNEL_TOKEN\"; echo ready; sleep 60",
        ReadyRule::Occurrences {
            needle: "ready",
            times: 1,
        },
    );
    let token = Secret::new("tunnel-token-abcdef123456");
    spec.env = vec![(
        "TUNNEL_TOKEN".to_owned(),
        token.expose_for_child_env().to_owned(),
    )];
    spec.secrets = vec![token];
    let supervisor = Supervisor::spawn("secret-child", 50);

    supervisor.start(spec).await.expect("must start");

    let logs = supervisor.snapshot().logs.join("\n");
    assert!(
        logs.contains("connecting with token"),
        "the line itself should be kept: {logs}"
    );
    assert!(
        !logs.contains("tunnel-token-abcdef123456"),
        "the credential leaked into the log ring: {logs}"
    );
    assert!(logs.contains("<redacted>"), "{logs}");
    supervisor.stop().await;
}

#[tokio::test]
async fn the_child_environment_is_not_inherited() {
    // The service's own environment holds provider API keys. A tunnel binary has no use
    // for them, and a child that crashes can dump its environment into a log.
    // SAFETY: this variable is set once here and read only by the child below; no other
    // case in this binary touches it.
    unsafe { std::env::set_var("NULLROUTER_MUST_NOT_LEAK", "sensitive-value") };
    let supervisor = Supervisor::spawn("clean-env-child", 50);

    supervisor
        .start(spec(
            "echo \"leaked=[${NULLROUTER_MUST_NOT_LEAK:-}]\"; echo done; sleep 30",
            ReadyRule::Occurrences {
                needle: "done",
                times: 1,
            },
        ))
        .await
        .expect("must start");

    let logs = supervisor.snapshot().logs.join("\n");
    assert!(logs.contains("leaked=[]"), "the child inherited it: {logs}");
    assert!(!logs.contains("sensitive-value"), "{logs}");
    supervisor.stop().await;
}

#[tokio::test]
async fn the_log_ring_stays_bounded_under_a_chatty_child() {
    let mut spec = spec(
        "i=0; while [ $i -lt 400 ]; do echo line $i; i=$((i+1)); done; echo marker; sleep 30",
        ReadyRule::Occurrences {
            needle: "marker",
            times: 1,
        },
    );
    spec.log_capacity = 25;
    let supervisor = Supervisor::spawn("chatty-child", 25);

    supervisor.start(spec).await.expect("must start");

    let snapshot = supervisor.snapshot();
    assert!(
        snapshot.logs.len() <= 25,
        "the ring grew to {} lines",
        snapshot.logs.len()
    );
    assert!(
        snapshot.dropped_logs > 0,
        "400 lines into a 25-line ring must report drops"
    );
    supervisor.stop().await;
}

#[tokio::test]
async fn dropping_the_last_handle_stops_the_child() {
    // Otherwise a restarted service leaves a live tunnel behind that nothing can address.
    let supervisor = Supervisor::spawn("orphan-check", 50);
    supervisor
        .start(spec("sleep 60", ReadyRule::Spawned))
        .await
        .expect("must start");
    let pid = supervisor.snapshot().pid.expect("a pid");
    assert!(process_exists(pid));

    drop(supervisor);

    for _ in 0..100_u32 {
        if !process_exists(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("child {pid} outlived the last supervisor handle");
}

#[tokio::test]
async fn a_start_that_cannot_spawn_reports_it() {
    let supervisor = Supervisor::spawn("unspawnable", 50);
    let mut spec = spec("true", ReadyRule::Spawned);
    // A resolved-then-removed binary: the shape of an operator uninstalling mid-session.
    let dir = tempfile::tempdir().expect("tempdir");
    let gone = dir.path().join("vanished");
    std::fs::write(&gone, "#!/bin/sh\nexit 0\n").expect("write");
    #[allow(
        clippy::unwrap_used,
        reason = "setting a mode on a file just written cannot fail here"
    )]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&gone, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // SAFETY: only this case reads or writes this variable in this binary.
    unsafe { std::env::set_var("NULLROUTER_VANISHING_BIN", &gone) };
    let resolved = BinarySpec {
        name: "vanished",
        candidates: &[],
        env_override: "NULLROUTER_VANISHING_BIN",
        search_dirs: &[],
    }
    .resolve(None)
    .expect("it exists at resolve time");
    // SAFETY: as above.
    unsafe { std::env::remove_var("NULLROUTER_VANISHING_BIN") };
    drop(dir);
    spec.program = resolved;

    let error = supervisor
        .start(spec)
        .await
        .expect_err("a removed binary cannot be spawned");

    assert!(matches!(error, StartError::Spawn { .. }), "{error:?}");
    assert_eq!(supervisor.snapshot().state, State::Failed);
}

#[tokio::test]
async fn a_snapshot_is_readable_while_a_start_is_pending() {
    // A status endpoint polls several times a second; it must not block behind a start.
    let mut spec = spec(
        "sleep 60",
        ReadyRule::Occurrences {
            needle: "never",
            times: 1,
        },
    );
    spec.startup_timeout = Duration::from_secs(20);
    let supervisor = Supervisor::spawn("pollable", 50);
    let starting = {
        let supervisor = supervisor.clone();
        tokio::spawn(async move { supervisor.start(spec).await })
    };

    assert!(
        settles(&supervisor, |snapshot| snapshot.state == State::Starting).await,
        "expected Starting"
    );
    for _ in 0..20_u32 {
        let snapshot = tokio::time::timeout(Duration::from_millis(200), async {
            supervisor.snapshot()
        })
        .await
        .expect("a snapshot must never block");
        assert_eq!(snapshot.state.as_str(), "starting");
    }

    supervisor.stop().await;
    let _outcome = starting.await;
}

/// Whether a pid is still live, without reaping anything.
///
/// `kill -0` is the check: it reports deliverability rather than sending a signal.
fn process_exists(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
