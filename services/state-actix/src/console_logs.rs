//! The router's own log output, kept in a ring buffer for the dashboard to read.
//!
//! # Why this lives in the state service
//!
//! Upstream is one Node process, so its console buffer is a module-level array and every route sees
//! the same one. Here the two halves of the same feature are served by two different processes: the
//! gateway sends `GET`/`DELETE /api/translator/console-logs` to the API service and
//! `/api/translator/console-logs/stream` to the events service. A buffer inside either one would
//! make the list and the stream disagree — the list showing one process's logs and the stream
//! another's — which is worse than not having the feature, because both look like they work.
//!
//! So the buffer sits in the state service, which is where everything else that more than one
//! service needs already lives, and the other services ship their lines to it. That is also what
//! makes the pane show the *router's* logs rather than one service's: eight processes write here.
//!
//! # Deliberately not persisted
//!
//! [`Buffer`] is its own structure rather than a field on the state snapshot. The snapshot is
//! serialised to disk whenever it changes, so log lines in it would mean writing the whole state
//! file every time anything logged — and a crash-safe copy of debug output is worth nothing anyway.
//! Losing the buffer on restart is correct: it describes a process that is no longer running.

use std::collections::VecDeque;
use std::sync::RwLock;

use actix_web::{HttpResponse, http::StatusCode, web};

/// How many lines are kept. Upstream's `CONSOLE_LOG_CONFIG.maxLines`.
const MAX_LINES: usize = 200;

/// The longest single line kept, in bytes.
///
/// Not upstream's — it has no limit. A tracing event carrying a whole request body would otherwise
/// let one line evict the entire buffer's worth of useful context, and the pane renders it into a
/// browser besides. Truncation is marked so a reader knows the line is not the whole story.
const MAX_LINE_BYTES: usize = 8 * 1024;

const TRUNCATION_MARK: &str = "… [truncated]";

/// One captured line, with enough about its origin to be worth reading.
///
/// Upstream's buffer holds bare strings, formed by joining the arguments of a `console.*` call. That
/// works when there is one process; with eight, a line with no service name on it is not traceable
/// to anything. The `service` and `level` fields are this port's, and the wire shape keeps
/// upstream's `logs: string[]` alongside them so an unmodified dashboard still renders.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Line {
    /// Which service emitted it.
    pub(crate) service: String,
    /// `error`, `warn`, `info`, `debug` or `trace`.
    pub(crate) level: String,
    /// The rendered message, with ANSI escapes already stripped by the shipper.
    pub(crate) message: String,
    /// Milliseconds since the Unix epoch, assigned on arrival here rather than at the source, so
    /// lines from services whose clocks differ still sort into one coherent order.
    pub(crate) at_ms: u64,
    /// Monotonic within this process, so a reader can ask for "everything after N" without
    /// depending on timestamps being distinct — a batch of 50 lines usually shares a millisecond.
    pub(crate) seq: u64,
}

impl Line {
    /// The single-string form upstream's dashboard expects.
    fn rendered(&self) -> String {
        format!("[{}] {} {}", self.service, self.level, self.message)
    }
}

/// A bounded ring of recent lines.
#[derive(Debug)]
pub(crate) struct Buffer {
    inner: RwLock<Inner>,
}

#[derive(Debug)]
struct Inner {
    lines: VecDeque<Line>,
    /// The sequence number the next line gets. Never reset by a clear: a reader holding a cursor
    /// from before the clear must not be handed lines it has already seen because numbering
    /// restarted.
    next_seq: u64,
    /// Bumped by a clear, so a poller can tell "nothing new" from "the buffer was emptied".
    generation: u64,
}

impl Default for Buffer {
    fn default() -> Self {
        Self {
            inner: RwLock::new(Inner {
                lines: VecDeque::with_capacity(MAX_LINES),
                next_seq: 1,
                generation: 0,
            }),
        }
    }
}

/// What a poll found.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Page {
    /// Lines after the requested cursor, oldest first.
    pub(crate) lines: Vec<Line>,
    /// The single-string form, for the dashboard field upstream sends.
    pub(crate) logs: Vec<String>,
    /// The cursor to pass next time.
    pub(crate) cursor: u64,
    /// The buffer's clear-count. A change means the client should drop what it has.
    pub(crate) generation: u64,
    /// True when the cursor was older than anything still held, so lines were missed.
    pub(crate) dropped: bool,
}

impl Buffer {
    /// Append a batch, evicting the oldest beyond [`MAX_LINES`].
    ///
    /// Takes a batch rather than a line because the shippers batch: one lock acquisition for fifty
    /// lines instead of fifty, on a lock that a poller also wants.
    pub(crate) fn append(&self, service: &str, lines: Vec<IncomingLine>) -> usize {
        let Ok(mut inner) = self.inner.write() else {
            // A poisoned lock means a previous holder panicked while writing. Dropping the batch is
            // the right failure for debug output: it must not take a request down with it.
            return 0;
        };
        let at_ms = now_ms();
        let mut accepted = 0_usize;
        for line in lines {
            let seq = inner.next_seq;
            inner.next_seq = inner.next_seq.saturating_add(1);
            inner.lines.push_back(Line {
                service: service.to_owned(),
                level: line.level,
                // Scrubbed again here, though the shipper already scrubbed at the source. This endpoint
                // accepts a post from anything on loopback, so a line can arrive without having passed
                // through that layer — and one leaked credential in a pane the operator screenshots is
                // worse than the cost of a second pass over a 200-line ring.
                message: truncate(nullrouter_logship::scrub::scrub(&line.message)),
                at_ms,
                seq,
            });
            accepted += 1;
            while inner.lines.len() > MAX_LINES {
                inner.lines.pop_front();
            }
        }
        accepted
    }

    /// Everything after `cursor`, or the whole buffer when `cursor` is `None`.
    pub(crate) fn since(&self, cursor: Option<u64>) -> Page {
        let Ok(inner) = self.inner.read() else {
            return Page {
                lines: Vec::new(),
                logs: Vec::new(),
                cursor: 0,
                generation: 0,
                dropped: false,
            };
        };
        let oldest = inner.lines.front().map(|line| line.seq);
        let lines: Vec<Line> = match cursor {
            Some(cursor) => inner
                .lines
                .iter()
                .filter(|line| line.seq > cursor)
                .cloned()
                .collect(),
            None => inner.lines.iter().cloned().collect(),
        };
        // A cursor older than the oldest line held means eviction outran this reader. Said out loud
        // rather than papered over, so the pane can show a gap instead of implying continuity.
        let dropped = cursor.is_some_and(|cursor| oldest.is_some_and(|oldest| cursor + 1 < oldest));
        Page {
            logs: lines.iter().map(Line::rendered).collect(),
            cursor: lines
                .last()
                .map_or_else(|| cursor.unwrap_or(0), |line| line.seq),
            generation: inner.generation,
            dropped,
            lines,
        }
    }

    /// Drop everything, bumping the generation so pollers notice.
    pub(crate) fn clear(&self) {
        if let Ok(mut inner) = self.inner.write() {
            inner.lines.clear();
            inner.generation = inner.generation.saturating_add(1);
        }
    }
}

/// One line as a shipper sends it, before this service stamps it.
#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct IncomingLine {
    pub(crate) level: String,
    pub(crate) message: String,
}

fn truncate(mut message: String) -> String {
    if message.len() <= MAX_LINE_BYTES {
        return message;
    }
    // Cut back to a character boundary: slicing a multi-byte codepoint down the middle panics, and
    // a log line is exactly where arbitrary bytes turn up.
    let mut end = MAX_LINE_BYTES;
    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }
    message.truncate(end);
    message.push_str(TRUNCATION_MARK);
    message
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// What a shipper posts.
#[derive(Debug, serde::Deserialize)]
struct Batch {
    /// The sending service's name, so the pane can say where a line came from.
    service: String,
    lines: Vec<IncomingLine>,
}

#[derive(Debug, serde::Deserialize)]
struct SinceQuery {
    /// The last sequence number the caller has. Absent means "give me the whole buffer".
    #[serde(default)]
    cursor: Option<u64>,
}

pub(crate) fn configure(config: &mut actix_web::web::ServiceConfig) {
    use actix_web::web;

    config.app_data(web::Data::new(Buffer::default())).service(
        // Internal, so the gateway refuses it from outside: anything that can write here can
        // put arbitrary text in front of an operator reading their own logs.
        web::resource(nullrouter_contracts::INTERNAL_CONSOLE_LOGS_PATH)
            .route(web::post().to(ingest))
            .route(web::get().to(read))
            .route(web::delete().to(clear)),
    );
}

/// Accept a batch from a service's log shipper.
async fn ingest(buffer: web::Data<Buffer>, body: web::Bytes) -> HttpResponse {
    let Ok(batch) = serde_json::from_slice::<Batch>(&body) else {
        return crate::responses::json(
            StatusCode::BAD_REQUEST,
            &serde_json::json!({ "error": "Invalid console log batch" }),
        );
    };
    let accepted = buffer.append(&batch.service, batch.lines);
    crate::responses::json(
        StatusCode::OK,
        &serde_json::json!({ "success": true, "accepted": accepted }),
    )
}

/// Everything after `?cursor=`, for the events service's stream and the API service's list.
async fn read(buffer: web::Data<Buffer>, query: web::Query<SinceQuery>) -> HttpResponse {
    crate::responses::json(StatusCode::OK, &buffer.since(query.cursor))
}

async fn clear(buffer: web::Data<Buffer>) -> HttpResponse {
    buffer.clear();
    crate::responses::json(StatusCode::OK, &serde_json::json!({ "success": true }))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "indexing the buffer is the assertion; a shorter buffer is a test failure"
)]
mod tests {
    use super::{Buffer, IncomingLine, MAX_LINE_BYTES, MAX_LINES};

    fn lines(messages: &[&str]) -> Vec<IncomingLine> {
        messages
            .iter()
            .map(|message| IncomingLine {
                level: "info".to_owned(),
                message: (*message).to_owned(),
            })
            .collect()
    }

    #[test]
    fn a_cursor_returns_only_what_is_new() {
        // The property the stream depends on: polling with the last cursor must not re-send lines
        // the client already has, or the pane duplicates every line on every tick.
        let buffer = Buffer::default();
        buffer.append("api", lines(&["one", "two"]));
        let first = buffer.since(None);
        assert_eq!(first.lines.len(), 2);

        buffer.append("state", lines(&["three"]));
        let second = buffer.since(Some(first.cursor));
        assert_eq!(second.lines.len(), 1);
        assert_eq!(second.lines[0].message, "three");
        assert_eq!(second.lines[0].service, "state");

        // And polling again with nothing new returns nothing, keeping the cursor usable.
        let third = buffer.since(Some(second.cursor));
        assert!(third.lines.is_empty());
        assert_eq!(third.cursor, second.cursor);
    }

    #[test]
    fn the_ring_evicts_the_oldest_and_says_when_a_reader_missed_lines() {
        let buffer = Buffer::default();
        let many: Vec<&str> = (0..MAX_LINES + 50).map(|_| "line").collect();
        buffer.append("api", lines(&many));

        let page = buffer.since(None);
        assert_eq!(page.lines.len(), MAX_LINES, "the ring must be bounded");
        // A cursor from before the eviction is reported as having missed lines, rather than
        // silently returning a partial tail that reads as continuous.
        let stale = buffer.since(Some(1));
        assert!(
            stale.dropped,
            "eviction outran this reader and must be said"
        );
    }

    #[test]
    fn a_clear_bumps_the_generation_without_reusing_sequence_numbers() {
        // Reusing numbers after a clear would hand a client holding an old cursor lines it had
        // already seen, because the new ones would be numbered below its cursor.
        let buffer = Buffer::default();
        buffer.append("api", lines(&["before"]));
        let before = buffer.since(None);

        buffer.clear();
        let cleared = buffer.since(None);
        assert!(cleared.lines.is_empty());
        assert_eq!(
            cleared.generation,
            before.generation + 1,
            "a poller tells a clear from a quiet tick by the generation"
        );

        buffer.append("api", lines(&["after"]));
        let after = buffer.since(Some(before.cursor));
        assert_eq!(after.lines.len(), 1);
        assert!(
            after.lines[0].seq > before.cursor,
            "numbering must keep going past the clear"
        );
    }

    #[test]
    fn an_enormous_line_is_truncated_at_a_character_boundary() {
        // A tracing event carrying a request body would otherwise evict the whole buffer, and
        // slicing a multi-byte codepoint down the middle panics — which a log line, of all places,
        // is where arbitrary bytes turn up.
        let buffer = Buffer::default();
        let huge = "é".repeat(MAX_LINE_BYTES);
        buffer.append("api", lines(&[&huge]));
        let page = buffer.since(None);
        let message = &page.lines[0].message;
        assert!(message.len() <= MAX_LINE_BYTES + 32, "{}", message.len());
        assert!(
            message.ends_with("… [truncated]"),
            "truncation must be visible"
        );
        // The kept part is whole characters, with none split at the cut. `MAX_LINE_BYTES` is even
        // and `é` is two bytes, so an off-by-one cut would land mid-character and show up here as a
        // replacement character or a short count rather than as a panic.
        let kept = message.trim_end_matches("… [truncated]");
        assert!(
            kept.chars().all(|character| character == 'é'),
            "the cut split a character: {:?}",
            kept.chars().rev().take(3).collect::<String>()
        );
        assert_eq!(kept.len() % 2, 0, "a two-byte character was cut in half");
    }

    #[test]
    fn the_rendered_form_names_the_service_that_logged() {
        // Eight processes write here, so a bare message is not traceable to anything.
        let buffer = Buffer::default();
        buffer.append("runtime", lines(&["upstream returned 503"]));
        let page = buffer.since(None);
        assert_eq!(page.logs[0], "[runtime] info upstream returned 503");
    }
}
