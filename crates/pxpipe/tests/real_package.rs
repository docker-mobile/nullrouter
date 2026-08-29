//! The bridge against the actual `pxpipe-proxy` from npm.
//!
//! Every other test here uses a stub, which proves the pipe works and proves nothing
//! about whether this code reads the *real* package correctly. That distinction is
//! not academic: a first draft of the reason mapping in `bridge.rs` invented three
//! reason names the package never emits, and every stub test passed. Only installing
//! it showed that its `classifyReason` returns `unsupported_model`, `below_min_chars`,
//! `not_profitable` and so on — meaning every real refusal would have been filed as a
//! generic passthrough.
//!
//! Off by default, because it needs the network and a registry: run with
//! `PXPIPE_TEST_INSTALL=1 cargo test -p nullrouter-pxpipe --test real_package`.
//! It fails rather than skips once asked for, so a broken environment is reported.
//!
//! It also does not require the transform to *succeed*. The package needs Node
//! ≥20.19; below that it imports and then fails on a missing global. What is asserted
//! is that whatever it answers is read correctly and reported honestly.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test assertions read clearer with direct expect than with error plumbing"
)]

use nullrouter_pxpipe::bridge::StartError;
use nullrouter_pxpipe::install::{InstallOutcome, Paths, find_node, node_satisfies};
use nullrouter_pxpipe::service::TokenSaver;

fn requested() -> bool {
    std::env::var("PXPIPE_TEST_INSTALL").is_ok_and(|value| value == "1")
}

/// A Claude-format body with a closed history prefix, which is the shape the package
/// looks for. Large enough to clear any reasonable threshold.
fn realistic_body() -> String {
    let filler = "The quick brown fox jumps over the lazy dog. ".repeat(1_500);
    serde_json::json!({
        "model": "claude-fable-5",
        "max_tokens": 128,
        "messages": [
            { "role": "user", "content": [{ "type": "text", "text": filler }] },
            { "role": "assistant", "content": [{ "type": "text", "text": "Understood." }] },
            { "role": "user", "content": [{ "type": "text", "text": "Now summarise it." }] },
        ],
    })
    .to_string()
}

#[tokio::test]
async fn the_real_package_installs_and_answers() {
    if !requested() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    let saver = TokenSaver::new(paths.clone());

    // The install is blocking, as npm is.
    let outcome = tokio::task::spawn_blocking(move || saver.install())
        .await
        .expect("the install task");
    let info = match outcome {
        InstallOutcome::Installed(info) => info,
        InstallOutcome::NpmMissing => panic!("PXPIPE_TEST_INSTALL was set but there is no npm"),
        InstallOutcome::Failed { message } => panic!("the install failed: {message}"),
    };
    assert!(info.installed);
    assert!(info.version.is_some(), "the version came from the manifest");
    // The requirement this code exists to check is really declared.
    assert!(
        info.requires_node.is_some(),
        "pxpipe-proxy declares engines.node; if that stopped being true the gate in \
         bridge.rs is now checking nothing"
    );

    let saver = TokenSaver::new(paths);
    let running = running_node();
    let requirement = info.requires_node.unwrap_or_default();
    let satisfied = node_satisfies(&requirement, &running);

    match saver.start().await {
        Ok(loaded) => {
            assert_eq!(
                satisfied,
                Some(true),
                "the worker started on node {running} against {requirement}"
            );
            assert!(loaded.loaded);
            assert_eq!(loaded.node_version.as_deref(), Some(running.as_str()));

            // The real export exists and the real reply frame parses. Whether it
            // applies depends on the package's own profitability maths, which is its
            // decision to make, so both are accepted — what is asserted is that the
            // answer is one this code understands.
            let outcome = saver
                .compress(&realistic_body(), "claude-fable-5", &enabled_gate())
                .await;
            let summary = &outcome.summary;
            assert!(
                !summary.reason.is_empty(),
                "every answer carries a reason: {summary:?}"
            );
            assert_ne!(
                summary.reason, "passthrough",
                "an unmapped reason means the package emits something this build files \
                 as generic; its own name is in detail: {:?}",
                summary.detail
            );
            if summary.applied {
                let body = outcome
                    .body
                    .expect("an applied transform replaces the body");
                serde_json::from_str::<serde_json::Value>(&body)
                    .expect("the replacement body is valid JSON");
                assert!(summary.image_count > 0, "applied means images: {summary:?}");
                assert!(summary.tokens_before_est > 0);
            } else {
                assert_eq!(outcome.body, None, "a refusal leaves the body alone");
            }
            saver.stop().await;
        }
        Err(StartError::UnsupportedNode(detail)) => {
            // The gate fired, which is the correct outcome on an old Node — and the
            // message names Node rather than leaving a user with the package's own
            // `crypto is not defined`.
            assert_eq!(
                satisfied,
                Some(false),
                "the node gate fired on a node that satisfies the requirement"
            );
            assert!(detail.contains("node"), "{detail}");
            assert!(detail.contains(&running), "{detail}");

            // And a request bypasses with that reason rather than failing.
            let outcome = saver
                .compress(&realistic_body(), "claude-fable-5", &enabled_gate())
                .await;
            assert_eq!(outcome.body, None);
            assert_eq!(outcome.summary.reason, "node_unsupported");
        }
        Err(other) => panic!("the real package would not load: {other}"),
    }
}

#[tokio::test]
async fn an_unsupported_model_is_reported_as_such_against_the_real_package() {
    if !requested() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    let installer = TokenSaver::new(paths.clone());
    let outcome = tokio::task::spawn_blocking(move || installer.install())
        .await
        .expect("the install task");
    let info = match outcome {
        InstallOutcome::Installed(info) => info,
        other => panic!("the install failed: {other:?}"),
    };
    if node_satisfies(&info.requires_node.unwrap_or_default(), &running_node()) != Some(true) {
        // Nothing to learn about model gating on a Node that cannot run the package.
        return;
    }

    let saver = TokenSaver::new(paths);
    // The package images three model families by default and refuses the rest. That
    // refusal is the most common reason a user sees no compression, so it has to
    // arrive as itself rather than as a generic passthrough.
    let outcome = saver
        .compress(&realistic_body(), "gpt-4o", &enabled_gate())
        .await;
    assert_eq!(
        outcome.summary.reason, "unsupported_model",
        "{:?}",
        outcome.summary
    );
    assert_eq!(outcome.body, None);
    saver.stop().await;
}

/// A body the package actually compresses: bulky tool documents and a large
/// `tool_result`, which is what its `origChars` counts and what its images replace.
///
/// [`realistic_body`] is refused as `below_min_chars` despite being 67 kB, because the
/// package measures its threshold against compressible content rather than body size.
/// That is worth having a test say out loud — it is the single most confusing thing
/// about the feature in practice.
fn compressible_body() -> String {
    let bulk = "def handler(event, context):\n    return {'ok': True}  # filler\n".repeat(600);
    serde_json::json!({
        "model": "claude-fable-5",
        "max_tokens": 128,
        "tools": [{
            "name": "read_file",
            "description": "Read a file. ".repeat(400),
            "input_schema": { "type": "object", "properties": {} },
        }],
        "messages": [
            { "role": "user", "content": [{ "type": "text", "text": "Inspect the repo." }] },
            { "role": "assistant", "content": [
                { "type": "tool_use", "id": "tu_1", "name": "read_file", "input": { "path": "a.py" } },
            ]},
            { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "tu_1",
                  "content": [{ "type": "text", "text": bulk }] },
            ]},
            { "role": "assistant", "content": [{ "type": "text", "text": "Read it." }] },
            { "role": "user", "content": [{ "type": "text", "text": "Now summarise." }] },
        ],
    })
    .to_string()
}

#[tokio::test]
async fn a_real_compression_is_read_and_measured_correctly() {
    if !requested() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    let installer = TokenSaver::new(paths.clone());
    let outcome = tokio::task::spawn_blocking(move || installer.install())
        .await
        .expect("the install task");
    let info = match outcome {
        InstallOutcome::Installed(info) => info,
        other => panic!("the install failed: {other:?}"),
    };
    if node_satisfies(&info.requires_node.unwrap_or_default(), &running_node()) != Some(true) {
        // The package cannot run at all below its required Node; the gate test above
        // covers that case, and there is no transform here to measure.
        return;
    }

    let saver = TokenSaver::new(paths);
    let original = compressible_body();
    let result = saver
        .compress(&original, "claude-fable-5", &enabled_gate())
        .await;
    let summary = &result.summary;
    assert!(
        summary.applied,
        "this body has bulky tool docs and a large tool_result, which is what the \
         package images; if it now refuses, the shape it looks for has changed: {summary:?}"
    );

    let body = result.body.expect("an applied transform replaces the body");
    let parsed: serde_json::Value =
        serde_json::from_str(&body).expect("the replacement body is valid JSON");
    // The replacement is a Claude request with images where the bulk was, not an
    // opaque blob: it has to be dispatchable as-is.
    assert!(parsed["messages"].is_array());
    assert!(
        body.contains("\"type\":\"image\"") || body.contains("\"image\""),
        "the replacement carries image blocks"
    );

    assert!(summary.image_count > 0, "{summary:?}");
    assert!(summary.image_bytes > 0, "{summary:?}");
    assert!(summary.imaged_chars > 0, "{summary:?}");
    // The estimate is a saving, and it is a saving because images bill by pixel — the
    // replacement body is *larger* in characters than the original.
    assert!(
        summary.tokens_after_est < summary.tokens_before_est,
        "an applied transform that does not reduce the estimate is not worth doing: \
         {summary:?}"
    );
    assert!(summary.tokens_saved_est > 0);
    assert!(summary.saved_pct > 0.0);
    assert_eq!(
        summary.tokens_saved_est,
        summary.tokens_before_est - summary.tokens_after_est
    );
    assert!(summary.log_line().is_some());

    // And the recorded event agrees with what was returned, so the dashboard's
    // numbers are the request's numbers.
    let stats = saver.stats(10);
    assert_eq!(stats.windows.all.compressed, 1);
    assert_eq!(stats.windows.all.errors, 0);
    assert_eq!(stats.windows.all.tokens_saved_est, summary.tokens_saved_est);
    assert_eq!(stats.windows.all.images_generated, summary.image_count);
    saver.stop().await;
}

#[tokio::test]
async fn a_large_body_with_nothing_compressible_is_refused_by_the_package() {
    if !requested() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(dir.path());
    let installer = TokenSaver::new(paths.clone());
    let outcome = tokio::task::spawn_blocking(move || installer.install())
        .await
        .expect("the install task");
    let info = match outcome {
        InstallOutcome::Installed(info) => info,
        other => panic!("the install failed: {other:?}"),
    };
    if node_satisfies(&info.requires_node.unwrap_or_default(), &running_node()) != Some(true) {
        return;
    }

    let saver = TokenSaver::new(paths);
    // 67 kB of plain prose. This router's gate passes it — it is far over the
    // threshold — and the package still refuses, because its own threshold counts
    // compressible content (static slab, tool documents) rather than body size.
    // Recorded here because "I sent a huge request and nothing happened" is the most
    // common confusion about this feature, and the answer is this refusal.
    let result = saver
        .compress(&realistic_body(), "claude-fable-5", &enabled_gate())
        .await;
    assert_eq!(
        result.summary.reason, "below_min_chars",
        "{:?}",
        result.summary
    );
    assert!(
        result
            .summary
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("below_min_chars")),
        "the package's own numbers reach the log: {:?}",
        result.summary.detail
    );
    assert!(
        result.summary.original_chars > 60_000,
        "the body really was large"
    );
    assert_eq!(result.body, None);
    saver.stop().await;
}

fn enabled_gate() -> nullrouter_pxpipe::compress::Gate {
    nullrouter_pxpipe::compress::Gate {
        enabled: true,
        claude_format: true,
        format: "claude".to_owned(),
        min_chars: 1_000,
        timeout_ms: 60_000,
    }
}

/// The `node` this machine would run the worker with.
fn running_node() -> String {
    let node = find_node().expect("node is required for these tests");
    let output = std::process::Command::new(node)
        .arg("--version")
        .output()
        .expect("node --version");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_start_matches('v')
        .to_owned()
}
