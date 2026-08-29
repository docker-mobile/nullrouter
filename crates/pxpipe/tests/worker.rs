//! The transform worker, against a real `node` process.
//!
//! The unit tests in `bridge.rs` cover how a reply frame is settled, which proves
//! nothing about the pipe: whether the shim parses a request, whether a reply is
//! framed as expected, and — the property that actually matters in production —
//! whether a worker stuck inside an uninterruptible transform is killed and replaced
//! rather than wedging the router for the rest of its life.
//!
//! These need `node` on the path, and fail rather than skip if it is absent. The
//! transform is a JavaScript library; a suite that quietly passed without it would
//! report the feature as tested when nothing had been exercised at all.
//!
//! The package is a stub written by each test, not the real `pxpipe-proxy`: the
//! subject here is the bridge, and depending on a live npm registry would make these
//! tests fail for reasons that have nothing to do with the code.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions read clearer with direct expect than with error plumbing"
)]

use std::path::Path;

use nullrouter_pxpipe::bridge::{Bridge, StartError, TransformOutcome, TransformRequest};
use nullrouter_pxpipe::compress::Gate;
use nullrouter_pxpipe::install::{Paths, find_node};
use nullrouter_pxpipe::service::TokenSaver;

fn require_node() {
    assert!(
        find_node().is_some(),
        "these tests exercise the Node transform worker and need `node` on the PATH"
    );
}

/// Write a stub `pxpipe-proxy` whose module body is `source`.
fn install_stub(paths: &Paths, version: &str, source: &str) {
    let core = paths.package_root().join("dist").join("core");
    std::fs::create_dir_all(&core).expect("create package tree");
    std::fs::write(
        paths.package_root().join("package.json"),
        format!("{{\"name\":\"pxpipe-proxy\",\"version\":\"{version}\",\"type\":\"module\"}}"),
    )
    .expect("write manifest");
    std::fs::write(core.join("library.js"), source).expect("write library");
}

/// A transform that reports what it was handed.
const ECHO: &str = r#"
export async function transformAnthropicMessages({ body, model, options }) {
  const text = new TextDecoder().decode(body);
  return {
    applied: true,
    reason: "applied",
    body: new TextEncoder().encode(JSON.stringify({
      chars: text.length, model, minCompressChars: options?.minCompressChars,
    })),
    info: { imageCount: 2, compressedChars: 30, imagePixels: 1500, baselineTokens: 40 },
    cache: { ownsCacheControl: true },
  };
}
"#;

fn request(body: &str) -> TransformRequest {
    TransformRequest {
        body: body.to_owned(),
        model: "claude-fable-5".to_owned(),
        min_chars: 25,
    }
}

fn bridge(dir: &Path) -> Bridge {
    Bridge::new(Paths::new(dir))
}

#[tokio::test]
async fn a_real_worker_transforms_a_body_and_reports_what_it_did() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    install_stub(&paths, "1.2.3", ECHO);
    let bridge = Bridge::new(paths);

    let loaded = bridge.start().await.expect("the worker starts");
    assert!(loaded.loaded);
    assert_eq!(loaded.version.as_deref(), Some("1.2.3"));

    let outcome = bridge
        .transform(&request("{\"messages\":[]}"), 10_000, true)
        .await;
    match outcome {
        TransformOutcome::Applied {
            body,
            info,
            cache_owns_control,
        } => {
            let echoed: serde_json::Value = serde_json::from_str(&body).expect("the body is json");
            // The body reached the package as the exact text we sent, and the gate's
            // threshold travelled with it rather than being decided twice.
            assert_eq!(echoed["chars"], 15);
            assert_eq!(echoed["model"], "claude-fable-5");
            assert_eq!(echoed["minCompressChars"], 25);
            assert_eq!(info.image_count, 2);
            assert_eq!(info.baseline_tokens, 40);
            assert!(cache_owns_control);
        }
        TransformOutcome::Bypassed { reason, detail } => {
            panic!("bypassed as {reason}: {detail:?}");
        }
    }
    bridge.stop().await;
}

#[tokio::test]
async fn one_worker_serves_many_requests() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    install_stub(&paths, "1.0.0", ECHO);
    let bridge = Bridge::new(paths);

    let first = bridge.transform(&request("{\"a\":1}"), 10_000, true).await;
    let started = bridge.loaded().await.loaded_at;
    for _ in 0..5 {
        let outcome = bridge.transform(&request("{\"a\":1}"), 10_000, true).await;
        assert!(outcome.applied(), "every request answers");
    }
    assert!(first.applied());
    // The same worker throughout: a process per request would pay Node's start-up
    // on every large body and eat the saving it exists to make.
    assert_eq!(bridge.loaded().await.loaded_at, started);
    bridge.stop().await;
}

#[tokio::test]
async fn a_hung_transform_is_killed_and_the_next_request_still_works() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    // A busy loop, not a sleep: real compression is CPU-bound and cannot be
    // interrupted from inside, which is exactly why the worker is a process.
    //
    // "Hang only the first time" is tracked on disk rather than in a variable,
    // because the kill under test destroys the process holding the variable — the
    // replacement worker would hang too, and the recovery would look broken.
    install_stub(
        &paths,
        "1.0.0",
        r#"
import fs from "node:fs";
// Beside the module itself, which is this test's own temporary directory. Not an
// environment variable: these tests share a process, and setting one would race
// every other test's spawn.
const marker = new URL("./hung.marker", import.meta.url);
export async function transformAnthropicMessages({ body }) {
  if (!fs.existsSync(marker)) {
    fs.writeFileSync(marker, "hung");
    const until = Date.now() + 30000;
    while (Date.now() < until) { /* uninterruptible */ }
  }
  return { applied: true, reason: "applied", body, info: { imageCount: 1 } };
}
"#,
    );
    let bridge = Bridge::new(paths);

    let started = std::time::Instant::now();
    let outcome = bridge.transform(&request("{\"a\":1}"), 300, true).await;
    let elapsed = started.elapsed();
    assert_eq!(
        outcome,
        TransformOutcome::Bypassed {
            reason: "timeout",
            detail: Some("no reply within 300ms".to_owned()),
        }
    );
    // The budget was honoured rather than waited out: 30 s of work, abandoned in
    // well under a second.
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the timeout did not fire: {elapsed:?}"
    );
    // And the worker is gone, not left spinning. Upstream races a timer and leaves
    // the work running inside its own process; here it stops with the process.
    assert!(!bridge.loaded().await.loaded, "the hung worker was dropped");

    // The router recovers: a fresh worker serves the next request. Without the kill
    // the stale answer would be read as the reply to this one.
    let outcome = bridge.transform(&request("{\"a\":1}"), 10_000, true).await;
    assert!(outcome.applied(), "got {outcome:?}");
    bridge.stop().await;
}

#[tokio::test]
async fn a_throwing_transform_is_a_bypass_and_the_worker_survives_it() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    install_stub(
        &paths,
        "1.0.0",
        r#"
let calls = 0;
export async function transformAnthropicMessages({ body }) {
  calls += 1;
  if (calls === 1) throw new Error("could not render the tile");
  return { applied: true, reason: "applied", body, info: { imageCount: 1 } };
}
"#,
    );
    let bridge = Bridge::new(paths);

    let outcome = bridge.transform(&request("{\"a\":1}"), 10_000, true).await;
    assert_eq!(
        outcome,
        TransformOutcome::Bypassed {
            reason: "transform_error",
            detail: Some("could not render the tile".to_owned()),
        },
        "the package's own message reaches the log"
    );
    let started = bridge.loaded().await.loaded_at;
    assert!(started.is_some(), "a thrown error is not a dead worker");

    let outcome = bridge.transform(&request("{\"a\":1}"), 10_000, true).await;
    assert!(outcome.applied());
    // Not restarted: the reply arrived, so the pipe is still in step. Restarting on
    // every failed transform would pay Node's start-up for a request that failed.
    assert_eq!(bridge.loaded().await.loaded_at, started);
    bridge.stop().await;
}

#[tokio::test]
async fn a_worker_that_dies_mid_transform_is_a_bypass_not_a_hang() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    install_stub(
        &paths,
        "1.0.0",
        r#"
export async function transformAnthropicMessages() {
  process.exit(3);
}
"#,
    );
    let bridge = Bridge::new(paths);
    let outcome = bridge.transform(&request("{\"a\":1}"), 10_000, true).await;
    match outcome {
        TransformOutcome::Bypassed { reason, .. } => {
            assert_eq!(reason, "transform_error", "the exit is read as a failure");
        }
        TransformOutcome::Applied { .. } => panic!("a dead worker cannot have applied anything"),
    }
    assert!(!bridge.loaded().await.loaded);
}

#[tokio::test]
async fn a_package_without_the_export_fails_to_start_and_says_why() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    install_stub(&paths, "1.0.0", "export const somethingElse = 1;\n");
    let bridge = Bridge::new(paths);

    match bridge.start().await {
        Err(StartError::Failed(detail)) => {
            assert!(
                detail.contains("transformAnthropicMessages"),
                "the message names the missing export: {detail}"
            );
        }
        other => panic!("expected a start failure, got {other:?}"),
    }
    // Reported, not papered over: a request now bypasses with a reason a user can
    // act on rather than looking like a transform that chose to do nothing.
    let outcome = bridge.transform(&request("{\"a\":1}"), 10_000, true).await;
    assert!(matches!(
        outcome,
        TransformOutcome::Bypassed {
            reason: "load_error",
            ..
        }
    ));
}

#[tokio::test]
async fn a_module_that_throws_on_import_is_reported_rather_than_retried_forever() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    install_stub(&paths, "1.0.0", "throw new Error('bad build');\n");
    let bridge = Bridge::new(paths);
    match bridge.start().await {
        Err(StartError::Failed(detail)) => assert!(detail.contains("bad build"), "{detail}"),
        other => panic!("expected a start failure, got {other:?}"),
    }
}

#[tokio::test]
async fn nothing_installed_is_distinguished_from_a_broken_install() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let bridge = bridge(dir.path());
    assert_eq!(bridge.start().await, Err(StartError::NotInstalled));
    assert_eq!(StartError::NotInstalled.code(), "NOT_INSTALLED");

    // A manifest with no code is an interrupted install, and must not report as
    // installed — it would fail on every request instead of once, here.
    let paths = Paths::new(dir.path());
    std::fs::create_dir_all(paths.package_root()).expect("create package root");
    std::fs::write(
        paths.package_root().join("package.json"),
        "{\"name\":\"pxpipe-proxy\",\"version\":\"1.0.0\"}",
    )
    .expect("write manifest");
    assert_eq!(bridge.start().await, Err(StartError::NotInstalled));
}

#[tokio::test]
async fn a_stopped_worker_bypasses_unless_the_request_may_warm_it() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    install_stub(&paths, "1.0.0", ECHO);
    let bridge = Bridge::new(paths);

    bridge.start().await.expect("start");
    assert!(bridge.stop().await, "a running worker was stopped");
    assert!(!bridge.stop().await, "stopping twice is not an error");

    // Upstream's `getTransform({ autoLoad: false })`: an explicit stop stays stopped
    // for callers that do not want to pay for a load.
    let outcome = bridge.transform(&request("{\"a\":1}"), 10_000, false).await;
    assert!(matches!(
        outcome,
        TransformOutcome::Bypassed {
            reason: "not_loaded",
            ..
        }
    ));
    // With auto-load, the first request warms it again.
    assert!(
        bridge
            .transform(&request("{\"a\":1}"), 10_000, true)
            .await
            .applied()
    );
    bridge.stop().await;
}

#[tokio::test]
async fn a_restart_picks_up_an_upgraded_install() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    install_stub(&paths, "1.0.0", ECHO);
    let bridge = Bridge::new(paths.clone());
    assert_eq!(
        bridge.start().await.expect("start").version.as_deref(),
        Some("1.0.0")
    );

    install_stub(&paths, "2.0.0", ECHO);
    // Still the old version: a running worker holds the module it imported, which is
    // the whole reason repair offers a restart.
    assert_eq!(bridge.loaded().await.version.as_deref(), Some("1.0.0"));
    assert_eq!(
        bridge.restart().await.expect("restart").version.as_deref(),
        Some("2.0.0")
    );
    bridge.stop().await;
}

#[tokio::test]
async fn worker_stderr_is_kept_for_the_logs_view() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    install_stub(
        &paths,
        "1.0.0",
        r#"
export async function transformAnthropicMessages({ body }) {
  console.error("tile cache miss");
  return { applied: false, reason: "not_profitable" };
}
"#,
    );
    let bridge = Bridge::new(paths);
    let outcome = bridge.transform(&request("{\"a\":1}"), 10_000, true).await;
    assert_eq!(
        outcome,
        TransformOutcome::Bypassed {
            reason: "not_profitable",
            detail: Some("not_profitable".to_owned()),
        }
    );
    // Drained rather than ignored: an unread stderr pipe fills and blocks the worker
    // inside its own write, which would be indistinguishable from a hung transform.
    let mut tail = bridge.stderr_tail().await;
    for _ in 0..20 {
        if tail
            .as_deref()
            .is_some_and(|text| text.contains("tile cache miss"))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        tail = bridge.stderr_tail().await;
    }
    assert!(
        tail.as_deref()
            .is_some_and(|text| text.contains("tile cache miss")),
        "got {tail:?}"
    );
    bridge.stop().await;
}

#[tokio::test]
async fn the_health_check_passes_end_to_end_against_a_working_package() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    install_stub(&paths, "3.1.4", ECHO);
    let saver = TokenSaver::new(paths);

    let health = saver.health().await;
    assert!(health.healthy, "{health:?}");
    assert_eq!(health.error, None);
    let ids: Vec<&str> = health.checks.iter().map(|step| step.id).collect();
    assert_eq!(ids, ["installed", "module", "transform"]);
    assert!(health.checks.iter().all(|step| step.ok));
    assert_eq!(
        health
            .checks
            .first()
            .and_then(|step| step.detail.as_deref()),
        Some("v3.1.4")
    );

    let status = saver.status().await;
    assert!(status.installed);
    assert!(status.running, "the health check left the worker warm");
    assert!(status.node_available);
    saver.stop().await;
}

#[tokio::test]
async fn a_package_that_imports_but_cannot_transform_fails_the_health_check() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    // The failure the install state cannot see: it is present, it imports, and it
    // is useless.
    install_stub(
        &paths,
        "1.0.0",
        r#"
export async function transformAnthropicMessages() {
  throw new Error("no renderer available");
}
"#,
    );
    let saver = TokenSaver::new(paths);
    let health = saver.health().await;
    assert!(!health.healthy);
    assert!(
        health
            .error
            .as_deref()
            .is_some_and(|error| error.contains("no renderer available")),
        "{health:?}"
    );
    let failing = health.checks.iter().find(|step| step.id == "transform");
    assert!(failing.is_some_and(|step| !step.ok), "{health:?}");
    // The two steps before it passed, so the checklist points at the transform
    // rather than at the install.
    assert!(
        health
            .checks
            .iter()
            .filter(|step| step.id != "transform")
            .all(|step| step.ok)
    );
    saver.stop().await;
}

#[tokio::test]
async fn a_refusal_is_a_healthy_answer() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    // The real package refuses a 16-token synthetic request. That is the module
    // working, so the self-test must not read it as a fault.
    install_stub(
        &paths,
        "1.0.0",
        r#"
export async function transformAnthropicMessages() {
  return { applied: false, reason: "below_min_chars" };
}
"#,
    );
    let saver = TokenSaver::new(paths);
    let health = saver.health().await;
    assert!(health.healthy, "{health:?}");
    assert!(
        health.checks.iter().any(|step| step
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("below_min_chars"))),
        "the reason is reported: {health:?}"
    );
    saver.stop().await;
}

#[tokio::test]
async fn a_compressed_request_replaces_the_body_and_is_counted() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    install_stub(
        &paths,
        "1.0.0",
        r#"
export async function transformAnthropicMessages({ body }) {
  const text = new TextDecoder().decode(body);
  return {
    applied: true,
    reason: "applied",
    body: new TextEncoder().encode(JSON.stringify({ imaged: true, was: text.length })),
    info: { imageCount: 3, compressedChars: 28000, imageTokens: 1200, baselineTokens: 7500 },
  };
}
"#,
    );
    let saver = TokenSaver::new(paths);
    let gate = Gate {
        enabled: true,
        claude_format: true,
        format: "claude".to_owned(),
        min_chars: 0,
        timeout_ms: 0,
    };
    let body = format!("{{\"messages\":[\"{}\"]}}", "x".repeat(30_000));
    let result = saver.compress(&body, "claude-fable-5", &gate).await;

    let replaced = result.body.expect("the body was replaced");
    let parsed: serde_json::Value = serde_json::from_str(&replaced).expect("json");
    assert_eq!(parsed["imaged"], true);
    assert_eq!(parsed["was"], 30_017);

    let summary = &result.summary;
    assert!(summary.applied);
    assert_eq!(summary.reason, "applied");
    assert_eq!(summary.tokens_before_est, 7_500, "the measured baseline");
    // 30 017 chars in, 28 000 imaged: 2 017 remaining → 504 tokens, plus the 1 200
    // the package measured for the images.
    assert_eq!(summary.tokens_after_est, 504 + 1_200);
    assert_eq!(summary.tokens_saved_est, 7_500 - 1_704);
    assert!(
        summary
            .log_line()
            .is_some_and(|line| line.contains("3 image(s)"))
    );

    // And it reached the event log, so the dashboard's numbers come from requests
    // that actually happened.
    let stats = saver.stats(10);
    assert_eq!(stats.windows.all.requests, 1);
    assert_eq!(stats.windows.all.compressed, 1);
    assert_eq!(stats.windows.all.errors, 0);
    assert_eq!(stats.windows.all.tokens_saved_est, 5_796);
    assert_eq!(stats.windows.today.compressed, 1);
    assert_eq!(stats.windows.all.images_generated, 3);
    saver.stop().await;
}

#[tokio::test]
async fn a_timed_out_request_is_counted_as_an_error_and_the_body_is_untouched() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    install_stub(
        &paths,
        "1.0.0",
        r#"
export async function transformAnthropicMessages() {
  const until = Date.now() + 30000;
  while (Date.now() < until) { /* uninterruptible */ }
}
"#,
    );
    let saver = TokenSaver::new(paths);
    let gate = Gate {
        enabled: true,
        claude_format: true,
        format: "claude".to_owned(),
        min_chars: 0,
        timeout_ms: 250,
    };
    let body = format!("{{\"messages\":[\"{}\"]}}", "x".repeat(30_000));
    let result = saver.compress(&body, "claude-fable-5", &gate).await;
    assert_eq!(result.body, None, "the original body is dispatched");
    assert_eq!(result.summary.reason, "timeout");

    let stats = saver.stats(10);
    // A timeout is an error, not a bypass: the saver failed rather than declining,
    // and the dashboard has to be able to tell those apart.
    assert_eq!(stats.windows.all.errors, 1);
    assert_eq!(stats.windows.all.bypassed, 0);
    assert_eq!(stats.windows.all.compressed, 0);
}
