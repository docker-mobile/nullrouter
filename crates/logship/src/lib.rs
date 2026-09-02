//! Sends this service's log output to the state service, so the dashboard can show all of it.
//!
//! Upstream patches `console.log` in one Node process and keeps the lines in a module-level array.
//! Here there are eight processes, and the dashboard pane that reads them is served by two more, so
//! "the router's console" has to be assembled rather than merely held. Each service installs the
//! [`Layer`] from this crate; it batches what the service logs and posts it to the state service's
//! buffer, which is the one place every service already talks to.
//!
//! # The deadlock this is shaped to avoid
//!
//! The shipper makes HTTP requests, and `reqwest` and `hyper` log. A layer that posted inline, from
//! inside the tracing callback, would log while logging — either recursing until the stack ran out
//! or deadlocking on the subscriber's own lock. Two things prevent it:
//!
//! - The layer only pushes onto a channel. It never awaits, never allocates a client, never logs.
//!   A background task does the posting, so the request path is a channel send.
//! - The background task's own client traffic is filtered out by target, so the shipper cannot feed
//!   itself. This is belt and braces given the point above, but a self-feeding log shipper is a
//!   runaway rather than a bug, and it costs one string comparison to make impossible.
//!
//! # What is dropped, and why that is the right failure
//!
//! The channel is bounded. When it is full — the state service is down, or a burst outran the
//! flush — lines are dropped and a counter records how many. Logging must never block a request or
//! grow without bound to preserve debug output: the request is the thing that matters, and the
//! operator can see in the pane that lines were lost.

pub mod scrub;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

/// How long a partial batch waits before being sent. Upstream's `FLUSH_INTERVAL_MS`.
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// Lines that force an immediate send. Upstream's `MAX_BATCH_LINES`.
const MAX_BATCH_LINES: usize = 50;

/// The channel's depth.
///
/// Twenty flushes' worth at the batch limit. Deep enough that an ordinary burst rides through,
/// shallow enough that a state service that has been down for a minute is not holding a megabyte of
/// stale debug output nobody will read.
const CHANNEL_CAPACITY: usize = MAX_BATCH_LINES * 20;

/// One post must not pile up behind another.
const POST_TIMEOUT: Duration = Duration::from_secs(3);

/// Log targets never shipped, because shipping them is what produces them.
///
/// `reqwest` and `hyper` are the client this crate posts with; `nullrouter_logship` is this crate's
/// own diagnostics.
const EXCLUDED_TARGET_PREFIXES: &[&str] = &["reqwest", "hyper", "h2", "nullrouter_logship"];

/// Default loopback address of the state service that holds the buffer.
const DEFAULT_STATE_ADDR: &str = "127.0.0.1:20134";

/// Install the usual stderr subscriber plus the shipper, in one call.
///
/// Every service calls this from `main` instead of building its own subscriber. Two of the eight had
/// one before this; the other six emitted `tracing` events into a process with no subscriber
/// installed, so their logs went nowhere at all — which is the first thing a console-log pane needs
/// fixed, since a capture backend with nothing to capture is not a feature.
///
/// Callable from anywhere, including before any runtime exists: the shipper runs on its own thread.
///
/// Stderr output is unchanged: whatever collects a service's logs today keeps getting them, and the
/// dashboard pane is an addition rather than a redirect.
pub fn install(service: &'static str) {
    install_with_default_filter(service, "info");
}

/// As [`install`], with a different filter for when `RUST_LOG` is unset.
///
/// The gateway wants `nullrouter_gateway=info` rather than a bare `info`, because at `info` across
/// every crate a Pingora process logs its own internals on the request path.
pub fn install_with_default_filter(service: &'static str, default_filter: &str) {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));
    let shipper = Layer::new(service, &state_addr());

    // `try_init`, not `init`: a second call — a test binary, or a service that also sets one up —
    // must not abort the process over its log plumbing.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(shipper)
        .try_init();
}

/// The state service's address, from the same variable the other services read.
fn state_addr() -> String {
    std::env::var("NULLROUTER_STATE_ADDR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_STATE_ADDR.to_owned())
}

/// A line on its way out.
#[derive(Debug, serde::Serialize)]
struct Line {
    level: String,
    message: String,
}

#[derive(Debug, serde::Serialize)]
struct Batch<'a> {
    service: &'a str,
    lines: Vec<Line>,
}

/// Installed alongside a service's normal formatting layer.
///
/// Does not replace stderr output: a service's logs should still reach whatever is collecting them,
/// and a dashboard pane is an addition rather than a destination.
#[derive(Debug)]
pub struct Layer {
    sender: mpsc::Sender<Line>,
    dropped: Arc<AtomicU64>,
}

impl Layer {
    /// Build the layer and the thread that drains it.
    ///
    /// The drain gets its own OS thread with its own single-threaded runtime rather than being
    /// spawned onto an ambient one. That is what makes this installable from any process: the
    /// gateway is a Pingora binary that owns its runtime and has none running at the point logging
    /// is set up, so a `tokio::spawn` here would panic on the process that handles every request —
    /// the one whose logs are worth the most.
    ///
    /// The thread runs until the process exits. There is nothing to shut down, because a shipper
    /// that stopped early would lose the shutdown logs, which are usually the interesting ones.
    #[must_use]
    pub fn new(service: &'static str, state_addr: &str) -> Self {
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let endpoint = format!(
            "http://{state_addr}{}",
            nullrouter_contracts::INTERNAL_CONSOLE_LOGS_PATH
        );
        let counter = Arc::clone(&dropped);
        let spawned = std::thread::Builder::new()
            .name("logship".to_owned())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    // Nothing to report this through that would not go back through the layer whose
                    // drain just failed to start, so the thread ends. Lines then fill the channel
                    // and are counted as dropped, which is visible in the pane.
                    return;
                };
                runtime.block_on(drain(service, endpoint, receiver, counter));
            });
        if spawned.is_err() {
            // Same reasoning: the layer still works, every line is counted as dropped.
        }
        Self { sender, dropped }
    }

    /// How many lines have been dropped for want of channel space.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl<S> tracing_subscriber::Layer<S> for Layer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
        let target = event.metadata().target();
        if EXCLUDED_TARGET_PREFIXES
            .iter()
            .any(|prefix| target.starts_with(prefix))
        {
            return;
        }

        let mut message = Message::default();
        event.record(&mut message);
        if message.text.is_empty() {
            return;
        }

        let line = Line {
            level: event.metadata().level().as_str().to_ascii_lowercase(),
            // Scrubbed here, at the source, so a credential never reaches the channel — let alone the
            // loopback socket to the state service or the browser tab beyond it. The state service
            // scrubs again at ingest, because anything may post to that endpoint.
            message: scrub::scrub(&strip_ansi(&message.text)),
        };
        // `try_send`, never `send`: this runs inside the tracing callback, on whatever thread
        // logged. Awaiting here would block a request behind the state service's availability, and
        // that is exactly backwards — a router that stops serving because its log viewer is down is
        // worse than one with a gap in its log viewer.
        if self.sender.try_send(line).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Collects an event's message and fields into one line.
///
/// `message` is the event's own text; other fields are appended as `key=value`, which is how the
/// structured fields a `tracing::info!("...", key = value)` carries reach a reader who only has a
/// string to look at.
#[derive(Default)]
struct Message {
    text: String,
}

impl Message {
    fn push(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        // `Debug` on a `&str` field quotes it; the message field reads better unquoted.
        let rendered = rendered
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(&rendered)
            .to_owned();
        if field.name() == "message" {
            if self.text.is_empty() {
                self.text = rendered;
            } else {
                self.text = format!("{rendered} {}", self.text);
            }
            return;
        }
        if !self.text.is_empty() {
            self.text.push(' ');
        }
        self.text.push_str(&format!("{}={rendered}", field.name()));
    }
}

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.push(field, value);
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.push(field, &value);
    }
}

/// Batch and post, forever.
async fn drain(
    service: &'static str,
    endpoint: String,
    mut receiver: mpsc::Receiver<Line>,
    dropped: Arc<AtomicU64>,
) {
    let client = match reqwest::Client::builder().timeout(POST_TIMEOUT).build() {
        Ok(client) => client,
        // Nothing to log this to that would not go through the layer that just failed to get a
        // client, so the task ends rather than spinning.
        Err(_) => return,
    };
    let mut batch: Vec<Line> = Vec::with_capacity(MAX_BATCH_LINES);

    loop {
        // Block until there is something, so an idle service does not wake on a timer.
        let Some(first) = receiver.recv().await else {
            break;
        };
        batch.push(first);

        // Then fill the batch for up to one flush interval.
        let deadline = tokio::time::Instant::now() + FLUSH_INTERVAL;
        while batch.len() < MAX_BATCH_LINES {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, receiver.recv()).await {
                Ok(Some(line)) => batch.push(line),
                // Sender gone: send what is in hand rather than discarding it.
                Ok(None) => break,
                Err(_elapsed) => break,
            }
        }

        // Reported as a line of its own so the gap is visible in the pane rather than being a
        // silent discontinuity. Taken before the post, so the count cannot be lost by a failed one.
        let missed = dropped.swap(0, Ordering::Relaxed);
        if missed > 0 {
            batch.push(Line {
                level: "warn".to_owned(),
                message: format!(
                    "{missed} log line(s) from {service} were dropped: the shipping queue was full."
                ),
            });
        }

        let payload = Batch {
            service,
            lines: std::mem::take(&mut batch),
        };
        // A failed post is discarded, not retried. A retry queue would hold debug output while the
        // state service was down and then deliver a flood of stale lines when it came back, which
        // is less useful than the gap.
        let _ = client.post(&endpoint).json(&payload).send().await;
        batch = Vec::with_capacity(MAX_BATCH_LINES);
    }
}

/// Remove ANSI colour escapes, so terminal formatting does not reach the browser as noise.
///
/// Upstream's `ANSI_RE`, matching `ESC [ ... m`. Written as a scan rather than a regex: it is one
/// pattern, and this runs on every line.
fn strip_ansi(text: &str) -> String {
    if !text.contains('\u{1b}') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character != '\u{1b}' {
            out.push(character);
            continue;
        }
        // `ESC` not followed by `[` is not a colour code, so it is kept rather than silently
        // swallowing the rest of the line.
        let mut lookahead = chars.clone();
        if lookahead.next() != Some('[') {
            out.push(character);
            continue;
        }
        // Consume through the terminating letter.
        chars.next();
        for inner in chars.by_ref() {
            if inner.is_ascii_alphabetic() {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test assertions read clearer with expect than with error plumbing"
)]
mod tests {
    use super::strip_ansi;

    #[test]
    fn ansi_colour_codes_are_removed() {
        assert_eq!(strip_ansi("\u{1b}[31merror\u{1b}[0m: nope"), "error: nope");
        assert_eq!(strip_ansi("\u{1b}[1;32mok\u{1b}[m"), "ok");
        // Text with no escapes is returned unchanged, and cheaply.
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn a_lone_escape_does_not_swallow_the_line() {
        // `ESC` without `[` is not a colour code. Treating it as one would drop everything after it,
        // which is the half of a log line that usually says what went wrong.
        assert_eq!(strip_ansi("before \u{1b} after"), "before \u{1b} after");
        assert_eq!(strip_ansi("tail\u{1b}"), "tail\u{1b}");
    }

    #[test]
    fn an_unterminated_escape_consumes_only_to_the_end() {
        // Malformed input must not panic; the remainder is dropped because there is no terminator
        // to resume at.
        assert_eq!(strip_ansi("a\u{1b}[31"), "a");
    }
}
