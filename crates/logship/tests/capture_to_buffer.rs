//! The console-log capture path end to end: a `tracing` event reaches the state service's buffer,
//! scrubbed.
//!
//! The unit tests in `scrub.rs` prove the scrubber and the suite in `state-actix` proves the buffer.
//! What neither covers is the join between them — that the layer actually ships, that it scrubs before
//! shipping rather than after, and that a line carrying a credential arrives with the credential already
//! gone. That is the property the console pane's safety rests on, and it spans two crates, so it is
//! asserted here against a stub standing in for the state service.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "a failed setup step in a test is a test failure, and the panic names which step"
)]

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

/// A stub that accepts one POST and returns its body.
///
/// Written against `TcpListener` rather than a framework: this test runs inside a `tracing` subscriber
/// that the shipper feeds, and adding an HTTP server with its own logging would put the shipper's own
/// traffic back into the stream it is being measured on.
fn stub_state() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = listener
        .local_addr()
        .expect("the bound address")
        .to_string();
    let (sender, receiver) = mpsc::channel();

    std::thread::spawn(move || {
        for stream in listener.incoming().take(8) {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().expect("a clone"));
            let mut length = 0_usize;
            // Read the request head, keeping only the content length.
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line
                    .to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(str::trim)
                    .and_then(|value| value.parse::<usize>().ok())
                {
                    length = value;
                }
            }
            let mut body = vec![0_u8; length];
            if reader.read_exact(&mut body).is_ok() {
                let _ignored = sender.send(String::from_utf8_lossy(&body).into_owned());
            }
            let _ignored = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 16\r\n\
                  Connection: close\r\n\r\n{\"accepted\":  1}",
            );
        }
    });

    (addr, receiver)
}

/// Collect posted batches until one satisfies `wanted`, or the deadline passes.
fn wait_for(receiver: &mpsc::Receiver<String>, wanted: impl Fn(&str) -> bool) -> Option<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    while std::time::Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(500)) {
            Ok(body) => {
                if wanted(&body) {
                    return Some(body);
                }
                seen.push(body);
            }
            // A timeout is not the end: the deadline above decides that, so this just loops again.
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    if !seen.is_empty() {
        eprintln!("batches seen but none matched: {seen:?}");
    }
    None
}

#[test]
fn a_traffic_line_reaches_the_buffer_with_its_credential_already_gone() {
    let (addr, batches) = stub_state();
    // SAFETY: single-threaded test setup, before the shipper thread reads it. `set_var` is unsafe in
    // edition 2024 because another thread reading the environment concurrently is a data race; nothing
    // else in this process is running yet.
    unsafe {
        std::env::set_var("NULLROUTER_STATE_ADDR", &addr);
    }

    nullrouter_logship::install("nullrouter-runtime-test");

    // The per-request line the runtime emits on every completed call, and a failure line carrying the
    // credential that failure paths tend to print.
    tracing::info!(
        endpoint = "/v1/chat/completions",
        provider = "anthropic",
        model = "claude-sonnet-4-5",
        status_code = 200,
        latency_ms = 412,
        "request complete"
    );
    tracing::warn!("dispatch failed: Authorization: Bearer ya29.a0AfB_alongaccesstokenvalue");

    let body = wait_for(&batches, |body| body.contains("request complete"))
        .expect("the traffic line should reach the buffer");

    // The traffic line arrives with the fields that make the pane a traffic view.
    assert!(body.contains("/v1/chat/completions"), "{body}");
    assert!(body.contains("anthropic"), "{body}");
    assert!(body.contains("latency_ms=412"), "{body}");
    assert!(
        body.contains("nullrouter-runtime-test"),
        "the batch must name its service: {body}"
    );

    // Both lines usually arrive in one batch, since the flush interval is longer than the gap between
    // them. Check the batch in hand before waiting for another, or this spends the full deadline
    // waiting for a post that has already happened.
    let failure = if body.contains("dispatch failed") {
        body
    } else {
        wait_for(&batches, |body| body.contains("dispatch failed"))
            .expect("the failure line should reach the buffer")
    };

    // And the credential in it is gone before it ever left this process.
    assert!(
        !failure.contains("alongaccesstokenvalue"),
        "a credential crossed the wire: {failure}"
    );
    assert!(
        failure.contains("[redacted]"),
        "the redaction marker should be there: {failure}"
    );
    assert!(
        failure.contains("dispatch failed"),
        "the diagnostic text should survive: {failure}"
    );
}

/// Note on what this test does *not* assert.
///
/// The line above still appears unscrubbed on this process's own stderr, and that is deliberate. Stderr
/// goes to whoever runs the binary — the operator, their journal, their container logs — which is the
/// same trust boundary the credential already lives in: that process is holding the token in memory to
/// use it. The console pane is a different boundary: it is served over HTTP to anyone with dashboard
/// access, rendered in a browser, and screenshotted into bug reports. Scrubbing the shipped copy
/// protects the second without blinding the first, which is what an operator debugging an auth failure
/// actually needs to see.
#[test]
fn stderr_is_deliberately_not_scrubbed() {
    // Asserted as a property of the scrubber's placement rather than by capturing stderr: the layer
    // scrubs the line it ships, and the stderr layer formats the original event. Nothing in this crate
    // rewrites the event itself, so a change that scrubbed at the event level — blinding the operator's
    // own terminal — would have to touch `install`, and this test states why that would be wrong.
    let line = "Authorization: Bearer ya29.averylongtokenvalue";
    let shipped = nullrouter_logship::scrub::scrub(line);
    assert_ne!(shipped, line, "the shipped copy must be scrubbed");
    assert!(
        !nullrouter_logship::scrub::looks_clean(line),
        "the original is what stderr shows, and it is left intact on purpose"
    );
}
