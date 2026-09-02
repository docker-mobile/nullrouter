//! Live console-log SSE, read from the buffer the state service holds.
//!
//! The buffer is not here for a reason worth stating: the gateway sends this stream to this service
//! but `GET`/`DELETE /api/translator/console-logs` to the API service. A buffer in either process
//! would make the list and the stream show different lines — each holding one process's output —
//! and both would look like they worked. So the state service holds one buffer, every service ships
//! its lines to it, and this route polls it.
//!
//! Polling rather than a push subscription, matching how `/api/usage/stream` already reads state.
//! What that costs is up to one interval of latency; what it buys is that a dropped connection
//! between the two services heals on the next tick instead of leaving a stream that is open and
//! permanently silent.

use std::time::Duration;

use serde_json::Value;

/// Default loopback address of `nullrouter-state`.
const DEFAULT_STATE_ADDR: &str = "127.0.0.1:20134";

/// How often the buffer is re-read.
///
/// Upstream flushes its in-process emitter every 100ms. This is a cross-process poll, so it is
/// slower on purpose: 100ms would be ten requests a second per connected dashboard for output a
/// human is reading.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// A single read must not stall the stream.
const READ_TIMEOUT: Duration = Duration::from_secs(3);

/// Keepalive cadence, matching upstream's 25s.
///
/// Sent as an SSE comment, which every client ignores. Without it an idle stream is indistinguishable
/// from a dead one to whatever proxy sits in between, and it gets closed.
pub(crate) const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(25);

/// Reads the shared buffer.
#[derive(Debug, Clone)]
pub(crate) struct LogReader {
    client: reqwest::Client,
    endpoint: String,
}

impl Default for LogReader {
    fn default() -> Self {
        Self::new(&state_addr())
    }
}

impl LogReader {
    pub(crate) fn new(state_addr: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(READ_TIMEOUT)
                .build()
                .unwrap_or_default(),
            endpoint: format!(
                "http://{state_addr}{}",
                nullrouter_contracts::INTERNAL_CONSOLE_LOGS_PATH
            ),
        }
    }

    /// Everything after `cursor`, or the whole buffer when `cursor` is `None`.
    ///
    /// `None` on failure, which the caller reports as `liveCapture: false` rather than as an empty
    /// buffer: a router whose log store is unreachable is not a quiet router, and the pane should
    /// not imply it is.
    pub(crate) async fn poll(&self, cursor: Option<u64>) -> Option<Page> {
        let url = match cursor {
            Some(cursor) => format!("{}?cursor={cursor}", self.endpoint),
            None => self.endpoint.clone(),
        };
        let response = self.client.get(&url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let body: Value = response.json().await.ok()?;
        Some(Page {
            logs: body
                .get("logs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            cursor: body.get("cursor").and_then(Value::as_u64).unwrap_or(0),
            generation: body.get("generation").and_then(Value::as_u64).unwrap_or(0),
            dropped: body
                .get("dropped")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
}

/// One read of the buffer.
#[derive(Debug, Clone)]
pub(crate) struct Page {
    pub(crate) logs: Vec<Value>,
    pub(crate) cursor: u64,
    pub(crate) generation: u64,
    pub(crate) dropped: bool,
}

/// What the stream has sent so far, carried between ticks.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamState {
    /// `None` until the first successful read, which is what makes that read an `init` frame.
    pub(crate) cursor: Option<u64>,
    pub(crate) generation: u64,
    /// Ticks since the last frame of any kind, for the keepalive.
    pub(crate) idle_ticks: u32,
    /// Whether the last read succeeded, so a recovery is reported rather than silently resumed.
    pub(crate) live: bool,
}

impl Default for StreamState {
    fn default() -> Self {
        Self {
            cursor: None,
            generation: 0,
            idle_ticks: 0,
            live: true,
        }
    }
}

impl StreamState {
    /// How many idle ticks make up the keepalive interval.
    fn keepalive_ticks() -> u32 {
        let ticks = KEEPALIVE_INTERVAL.as_millis() / POLL_INTERVAL.as_millis().max(1);
        u32::try_from(ticks).unwrap_or(u32::MAX).max(1)
    }
}

pub(crate) fn poll_interval() -> Duration {
    POLL_INTERVAL
}

/// The state service's address, overridable the same way the usage stream's is.
fn state_addr() -> String {
    std::env::var("NULLROUTER_STATE_ADDR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_STATE_ADDR.to_owned())
}

/// The next frame to send, given a poll result and what has already been sent.
///
/// Pure, so the frame sequence is testable without a state service or a live stream — which is the
/// part worth testing: the rules about when `init` is sent instead of `lines`, and when a clear has
/// to be announced, are where this can silently duplicate or lose a client's lines.
pub(crate) fn next_frame(state: &mut StreamState, page: Option<&Page>) -> Option<String> {
    let Some(page) = page else {
        state.idle_ticks = 0;
        // Reported once per outage rather than every tick, so a state service that is down does not
        // fill the pane with its own error.
        if std::mem::replace(&mut state.live, false) {
            return Some(frame(&serde_json::json!({
                "type": "lines",
                "liveCapture": false,
                "lines": ["[events] warn the console-log buffer in nullrouter-state is unreachable"],
            })));
        }
        return None;
    };

    // A generation change means the buffer was cleared. Announced before anything else, because a
    // client that appended the new lines first would show them below stale ones it should have
    // dropped.
    let cleared = page.generation != state.generation && state.cursor.is_some();
    state.generation = page.generation;

    let recovered = !std::mem::replace(&mut state.live, true);
    // The first read of a connection replaces whatever the client holds, as upstream's `init` does.
    // A clear, a recovery, or an eviction that outran this reader all mean the same thing: what the
    // client has can no longer be appended to safely, so it is replaced rather than added to.
    let replace = state.cursor.is_none() || cleared || recovered || page.dropped;

    if cleared && page.logs.is_empty() {
        state.cursor = Some(page.cursor);
        state.idle_ticks = 0;
        return Some(frame(
            &serde_json::json!({ "type": "clear", "liveCapture": true }),
        ));
    }

    if page.logs.is_empty() {
        state.idle_ticks = state.idle_ticks.saturating_add(1);
        if state.idle_ticks >= StreamState::keepalive_ticks() {
            state.idle_ticks = 0;
            // An SSE comment, not an event: it keeps the connection alive without the client having
            // to know about it.
            return Some(": ping\n\n".to_owned());
        }
        return None;
    }

    state.cursor = Some(page.cursor);
    state.idle_ticks = 0;
    Some(frame(&serde_json::json!({
        "type": if replace { "init" } else { "lines" },
        "liveCapture": true,
        "logs": page.logs,
        "lines": page.logs,
    })))
}

/// One frame, named `console_logs` like the opening one this route already sent.
///
/// Upstream's frames are unnamed, so its client picks them up with `onmessage`. This port's frames
/// carry the name because the dashboard that reads them subscribes to it — `CONSOLE_LOGS_EVENT` in
/// `dashboard::console_log_live` — and because the `connected` and `console_logs` frames this route
/// opened with were already named. An unnamed frame here would be delivered to neither listener.
///
/// The payload keeps upstream's `type` discriminator, and both `logs` and `lines` carry the same
/// array: upstream's `init` frame names it `logs` and its incremental frame names it `lines`, so a
/// reader looking for either finds it.
fn frame(payload: &Value) -> String {
    format!("event: {EVENT_NAME}\ndata: {payload}\n\n")
}

/// The SSE event name, matching the dashboard's `CONSOLE_LOGS_EVENT`.
const EVENT_NAME: &str = "console_logs";

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "indexing a serde_json::Value is the assertion: a frame with the wrong shape is a test \
              failure, which is what the panic reports"
)]
mod tests {
    use super::{Page, StreamState, next_frame};
    use serde_json::{Value, json};

    fn page(logs: &[&str], cursor: u64, generation: u64, dropped: bool) -> Page {
        Page {
            logs: logs.iter().map(|line| json!(line)).collect(),
            cursor,
            generation,
            dropped,
        }
    }

    fn parsed(frame: &str) -> Value {
        // Named frames, so the payload is on the second line. Asserted rather than skipped: an
        // unnamed frame would reach no listener in this port's dashboard.
        let payload = frame
            .strip_prefix("event: console_logs\ndata: ")
            .expect("a named console_logs frame")
            .trim_end();
        serde_json::from_str(payload).expect("valid JSON")
    }

    #[test]
    fn the_first_read_is_an_init_and_the_next_is_incremental() {
        // The distinction the client depends on: `init` replaces what it holds, `lines` appends.
        // Sending `init` twice would be harmless; sending `lines` first would append to nothing and
        // silently lose the buffered history.
        let mut state = StreamState::default();
        let first = next_frame(&mut state, Some(&page(&["one", "two"], 2, 0, false)))
            .expect("a first frame");
        assert_eq!(parsed(&first)["type"], "init");
        assert_eq!(parsed(&first)["logs"][0], "one");
        assert_eq!(parsed(&first)["liveCapture"], true);

        let second =
            next_frame(&mut state, Some(&page(&["three"], 3, 0, false))).expect("a second frame");
        assert_eq!(parsed(&second)["type"], "lines");
        assert_eq!(parsed(&second)["lines"][0], "three");
    }

    #[test]
    fn a_quiet_tick_sends_nothing_until_the_keepalive_is_due() {
        let mut state = StreamState::default();
        next_frame(&mut state, Some(&page(&["one"], 1, 0, false)));

        // Nothing to say, so nothing is said — the alternative is an empty frame every tick.
        assert!(next_frame(&mut state, Some(&page(&[], 1, 0, false))).is_none());

        // Until enough idle ticks accumulate, at which point a comment goes out so an intermediate
        // proxy does not close a stream it thinks is dead.
        let mut ping = None;
        for _tick in 0..200 {
            if let Some(frame) = next_frame(&mut state, Some(&page(&[], 1, 0, false))) {
                ping = Some(frame);
                break;
            }
        }
        assert_eq!(ping.as_deref(), Some(": ping\n\n"));
    }

    #[test]
    fn a_clear_is_announced_before_anything_that_followed_it() {
        // A client that appended the post-clear lines without being told about the clear would show
        // them below stale ones it should have dropped.
        let mut state = StreamState::default();
        next_frame(&mut state, Some(&page(&["old"], 1, 0, false)));

        let cleared = next_frame(&mut state, Some(&page(&[], 1, 1, false))).expect("a clear frame");
        assert_eq!(parsed(&cleared)["type"], "clear");

        // And a clear that already has new lines behind it replaces rather than appends.
        let mut state = StreamState::default();
        next_frame(&mut state, Some(&page(&["old"], 1, 0, false)));
        let after = next_frame(&mut state, Some(&page(&["new"], 2, 1, false))).expect("a frame");
        assert_eq!(parsed(&after)["type"], "init");
        assert_eq!(parsed(&after)["logs"][0], "new");
    }

    #[test]
    fn an_eviction_that_outran_this_reader_replaces_rather_than_appends() {
        // Appending after a gap would present a discontinuous log as continuous.
        let mut state = StreamState::default();
        next_frame(&mut state, Some(&page(&["one"], 1, 0, false)));
        let after = next_frame(&mut state, Some(&page(&["later"], 400, 0, true))).expect("a frame");
        assert_eq!(parsed(&after)["type"], "init");
    }

    #[test]
    fn an_unreachable_buffer_is_reported_once_and_recovery_replaces() {
        // Reported, because an empty pane would read as a quiet router — the opposite of what
        // someone checking their logs needs to know. Once, because a frame per tick would fill the
        // pane with the outage itself.
        let mut state = StreamState::default();
        next_frame(&mut state, Some(&page(&["one"], 1, 0, false)));

        let outage = next_frame(&mut state, None).expect("an outage frame");
        assert_eq!(parsed(&outage)["liveCapture"], false);
        assert!(
            parsed(&outage)["lines"][0]
                .as_str()
                .is_some_and(|line| line.contains("unreachable")),
            "{outage}"
        );
        assert!(
            next_frame(&mut state, None).is_none(),
            "silent while still down"
        );

        // On recovery the client's contents are replaced, since lines may have been missed.
        let back = next_frame(&mut state, Some(&page(&["two"], 2, 0, false))).expect("a frame");
        assert_eq!(parsed(&back)["type"], "init");
        assert_eq!(parsed(&back)["liveCapture"], true);
    }

    #[test]
    fn an_outage_before_any_successful_read_still_reports() {
        // A dashboard opened while state is down must be told, not left with a silent stream.
        let mut state = StreamState::default();
        let outage = next_frame(&mut state, None).expect("an outage frame");
        assert_eq!(parsed(&outage)["liveCapture"], false);
    }
}
