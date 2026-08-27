//! Live console log state: a bounded buffer, and the SSE frames that fill it.
//!
//! The panel this backs rendered `console_log_dashboard_state()` — an empty
//! fixture whose own labels admitted the stream was unwired, next to a Clear
//! button that did nothing. Both endpoints existed the whole time.
//!
//! Three properties shape this module.
//!
//! * [`LogBuffer`] is bounded at [`MAX_LINES`]. A console stream is unbounded by
//!   nature; a browser tab left open for a day must not grow without limit, so
//!   the oldest line is dropped and the count of dropped lines is kept and
//!   shown.
//! * A frame that does not carry the shape the events service promises decodes
//!   to `None` and is ignored, rather than appending a blank line that would look
//!   like output.
//! * [`StreamState`] distinguishes connecting, live, disconnected, and "no
//!   browser to subscribe from". Entries already received are never presented as
//!   live once the connection drops.

use crate::api::ApiError;
use serde_json::Value;

/// `GET`/`DELETE` history endpoint.
pub const HISTORY_PATH: &str = "/api/translator/console-logs";

/// The SSE endpoint.
pub const STREAM_PATH: &str = "/api/translator/console-logs/stream";

/// The named event the events service emits for log frames.
pub const CONSOLE_LOGS_EVENT: &str = "console_logs";

/// The named event the events service emits once on connect.
pub const CONNECTED_EVENT: &str = "connected";

/// Newest lines retained, matching `CONSOLE_LOG_CONFIG.maxLines` upstream.
///
/// The server trims to the same number, so the browser holding more would only
/// ever be holding lines the server has already forgotten.
pub const MAX_LINES: usize = 200;

/// Severity of one line, read from its `[LEVEL]` tag.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LogLevel {
    #[default]
    Log,
    Info,
    Warn,
    Error,
    Debug,
}

impl LogLevel {
    /// Every level, in the legend's order.
    pub const ALL: [Self; 5] = [Self::Log, Self::Info, Self::Warn, Self::Error, Self::Debug];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Log => "LOG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Debug => "DEBUG",
        }
    }

    /// Colour class, matching the level vocabulary the stylesheet defines.
    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Log => "nr-console-level-log",
            Self::Info => "nr-console-level-info",
            Self::Warn => "nr-console-level-warn",
            Self::Error => "nr-console-level-error",
            Self::Debug => "nr-console-level-debug",
        }
    }

    /// Read the level from a line's bracketed tags.
    ///
    /// Upstream colours by the *second* bracketed token
    /// (`ConsoleLogClient.js`: `line.match(/\[(\w+)\]/g)` then `match[1]`),
    /// because the first is the subsystem: `[9Router] [WARN] …`. That is
    /// reproduced here rather than corrected, so a line is coloured the same in
    /// both dashboards. A line with no recognised tag is [`Self::Log`], which is
    /// also upstream's default.
    pub fn from_line(line: &str) -> Self {
        bracketed_tags(line)
            .nth(1)
            .and_then(Self::from_tag)
            .unwrap_or_default()
    }

    fn from_tag(tag: &str) -> Option<Self> {
        match tag.to_ascii_uppercase().as_str() {
            "LOG" => Some(Self::Log),
            "INFO" => Some(Self::Info),
            "WARN" => Some(Self::Warn),
            "ERROR" => Some(Self::Error),
            "DEBUG" => Some(Self::Debug),
            _ => None,
        }
    }
}

/// Bracketed `[word]` tokens in a line, in order.
///
/// Only tokens made entirely of word characters count, matching the `\[(\w+)\]`
/// upstream uses; `[2024-01-01]` is not a level tag.
fn bracketed_tags(line: &str) -> impl Iterator<Item = &str> {
    line.split('[').skip(1).filter_map(|rest| {
        rest.split_once(']').map(|(tag, _)| tag).filter(|tag| {
            !tag.is_empty() && tag.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
        })
    })
}

/// One retained line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogLine {
    /// Monotonic sequence number, used as the render key so an identical line
    /// arriving twice is still two rows.
    pub sequence: u64,
    pub level: LogLevel,
    pub text: String,
    /// `true` while this line is new enough to pulse.
    pub fresh: bool,
}

impl LogLine {
    /// Class list for the row.
    pub fn class_name(&self) -> String {
        let base = self.level.class_name();
        if self.fresh {
            format!("nr-console-line {base} nr-tick")
        } else {
            format!("nr-console-line {base}")
        }
    }
}

/// The retained lines, oldest first, capped at [`MAX_LINES`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LogBuffer {
    lines: Vec<LogLine>,
    /// Total lines ever appended, which is also the next sequence number.
    received: u64,
    /// Lines dropped to stay within the cap.
    dropped: u64,
}

impl LogBuffer {
    pub fn lines(&self) -> &[LogLine] {
        &self.lines
    }

    pub const fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub const fn len(&self) -> usize {
        self.lines.len()
    }

    /// How many lines have been dropped to respect the cap.
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }

    /// How many lines this buffer has ever been given.
    pub const fn received(&self) -> u64 {
        self.received
    }

    /// Append one line, trimming the oldest if the cap is reached.
    ///
    /// Blank lines are kept: a console genuinely emits them, and dropping them
    /// would misrepresent the output's shape.
    pub fn push(&mut self, text: String) {
        self.received = self.received.saturating_add(1);
        for line in &mut self.lines {
            line.fresh = false;
        }
        self.lines.push(LogLine {
            sequence: self.received,
            level: LogLevel::from_line(&text),
            text,
            fresh: true,
        });
        self.trim();
    }

    /// Append several lines, marking only the batch as fresh.
    pub fn extend(&mut self, lines: impl IntoIterator<Item = String>) {
        for line in &mut self.lines {
            line.fresh = false;
        }
        for text in lines {
            self.received = self.received.saturating_add(1);
            self.lines.push(LogLine {
                sequence: self.received,
                level: LogLevel::from_line(&text),
                text,
                fresh: true,
            });
        }
        self.trim();
    }

    /// Replace the contents wholesale, as an `init` frame or a history fetch does.
    ///
    /// Nothing is marked fresh: a snapshot of what already existed is not new
    /// output, and pulsing 200 rows at once would be noise.
    pub fn replace(&mut self, lines: impl IntoIterator<Item = String>) {
        self.lines.clear();
        for text in lines {
            self.received = self.received.saturating_add(1);
            self.lines.push(LogLine {
                sequence: self.received,
                level: LogLevel::from_line(&text),
                text,
                fresh: false,
            });
        }
        self.trim();
    }

    /// Drop every retained line, as a `clear` frame or a successful DELETE does.
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// Enforce the cap, counting what was discarded.
    fn trim(&mut self) {
        let excess = self.lines.len().saturating_sub(MAX_LINES);
        if excess == 0 {
            return;
        }
        self.lines.drain(0..excess);
        self.dropped = self
            .dropped
            .saturating_add(u64::try_from(excess).unwrap_or(u64::MAX));
    }

    /// Retention line for the panel, stated from what is actually held.
    pub fn retained_label(&self) -> String {
        format!("{} retained", self.len())
    }

    /// The cap, as a label.
    pub fn max_label() -> String {
        format!("{MAX_LINES} max")
    }

    /// How trimming is described, including whether any has happened.
    pub fn trim_label(&self) -> String {
        if self.dropped == 0 {
            format!("Newest {MAX_LINES} lines retained")
        } else {
            format!(
                "Newest {MAX_LINES} lines retained, {} older dropped",
                self.dropped
            )
        }
    }
}

/// What one `console_logs` frame asks the buffer to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameKind {
    /// `type: "init"` — the buffered history, replacing whatever is held.
    Init,
    /// `type: "line"` or `"lines"` — new output to append.
    Append,
    /// `type: "clear"` — the server's buffer was emptied.
    Clear,
}

/// One decoded `console_logs` frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsoleFrame {
    pub kind: FrameKind,
    /// Lines the frame carried. Empty for `clear`, and possibly empty for
    /// `init` — a server with nothing buffered still sends one.
    pub lines: Vec<String>,
    /// Whether the events service says it is capturing console output.
    ///
    /// `None` when the frame omitted the flag, which is not the same as `false`.
    pub live_capture: Option<bool>,
}

/// Decode the `data:` payload of a `console_logs` frame.
///
/// `None` when the payload is not a JSON object or names no recognised `type`.
/// Ignoring an unrecognised frame is deliberate: the alternative is appending a
/// line the server never sent.
pub fn parse_console_frame(data: &str) -> Option<ConsoleFrame> {
    let value: Value = serde_json::from_str(data).ok()?;
    if !value.is_object() {
        return None;
    }
    let kind = match value.get("type").and_then(Value::as_str)?.trim() {
        "init" | "lines_init" => FrameKind::Init,
        "line" | "lines" => FrameKind::Append,
        "clear" => FrameKind::Clear,
        _ => return None,
    };

    let mut lines = Vec::new();
    if kind != FrameKind::Clear {
        // `init`/`lines` carry an array; `line` carries a single value. Both
        // shapes are read, and a frame missing its payload yields no lines
        // rather than a blank one.
        if let Some(array) = value
            .get("logs")
            .or_else(|| value.get("lines"))
            .and_then(Value::as_array)
        {
            lines.extend(array.iter().filter_map(log_text));
        }
        if let Some(single) = value.get("line").and_then(log_text) {
            lines.push(single);
        }
    }

    Some(ConsoleFrame {
        kind,
        lines,
        live_capture: value.get("liveCapture").and_then(Value::as_bool),
    })
}

/// One line's text, from a string or from an object that wraps one.
///
/// `null` entries yield `None`. The events service currently serialises its
/// (always empty) `logs` array from a unit struct, so a future non-empty frame
/// could carry either shape; neither may become a blank row.
fn log_text(entry: &Value) -> Option<String> {
    match entry {
        Value::String(text) => Some(text.clone()),
        Value::Object(_) => ["line", "text", "message"]
            .into_iter()
            .find_map(|key| entry.get(key).and_then(Value::as_str))
            .map(ToOwned::to_owned),
        _ => None,
    }
}

/// Decode the `connected` frame the events service sends first.
///
/// Only used to confirm the stream is the console-log stream; `None` when the
/// payload is not an object.
pub fn parse_connected_frame(data: &str) -> Option<bool> {
    let value: Value = serde_json::from_str(data).ok()?;
    if !value.is_object() {
        return None;
    }
    Some(value.get("connected").and_then(Value::as_bool) == Some(true))
}

/// Parse `GET /api/translator/console-logs`.
///
/// `None` when the body is not an object carrying a `logs` array, so a shape
/// change is a visible failure rather than an empty console.
pub fn parse_history(body: &str) -> Option<Vec<String>> {
    let value: Value = serde_json::from_str(body).ok()?;
    let logs = value.get("logs")?.as_array()?;
    Some(logs.iter().filter_map(log_text).collect())
}

/// State of the `/api/translator/console-logs/stream` subscription.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StreamState {
    #[default]
    Connecting,
    /// Connected, and the events service reports it is capturing output.
    Live,
    /// Connected, but the service says it is not capturing console output, so
    /// nothing new will arrive.
    NotCapturing,
    /// The browser lost the connection and is retrying.
    Interrupted,
    /// No browser to subscribe from, or the subscription could not be created.
    Unavailable,
}

impl StreamState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting…",
            Self::Live => "Connected",
            Self::NotCapturing => "No live capture",
            Self::Interrupted => "Disconnected",
            Self::Unavailable => "Stream offline",
        }
    }

    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Live => "is-connected",
            Self::NotCapturing | Self::Interrupted => "is-degraded",
            Self::Connecting | Self::Unavailable => "is-idle",
        }
    }

    /// What the state means, in a sentence.
    pub const fn detail(self) -> &'static str {
        match self {
            Self::Connecting => "Waiting for the first frame from the console log stream.",
            Self::Live => "New console output appears here as the router emits it.",
            Self::NotCapturing => {
                "The stream is connected but the router is not capturing console output, so no new lines will arrive."
            }
            Self::Interrupted => {
                "The live feed is disconnected and the browser is retrying. Lines below are from before the drop and are not current."
            }
            Self::Unavailable => {
                "This build has no browser event source, so the live feed cannot be opened."
            }
        }
    }

    /// Whether lines arriving now can be called live.
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Live)
    }

    /// Fold a frame's `liveCapture` flag into the connected state.
    pub const fn from_capture(capture: Option<bool>) -> Self {
        match capture {
            Some(false) => Self::NotCapturing,
            // A frame arrived, so the stream is connected. An omitted flag is
            // not read as "not capturing".
            Some(true) | None => Self::Live,
        }
    }
}

/// How a `DELETE /api/translator/console-logs` ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClearOutcome {
    /// The router cleared its buffer.
    Cleared,
    /// The router answered, but not with a success.
    Refused,
    Rejected(ApiError),
}

impl ClearOutcome {
    pub fn message(&self) -> String {
        match self {
            Self::Cleared => String::from("The router cleared its console buffer."),
            Self::Refused => {
                String::from("The router did not confirm the clear, so lines may remain.")
            }
            Self::Rejected(error) => format!("Not cleared. {}", error.message()),
        }
    }

    pub const fn succeeded(&self) -> bool {
        matches!(self, Self::Cleared)
    }
}

/// Interpret a clear response.
pub fn settle_clear(response: Result<&str, ApiError>) -> ClearOutcome {
    match response {
        Ok(body) => match serde_json::from_str::<Value>(body) {
            Ok(value) if value.get("success").and_then(Value::as_bool) == Some(true) => {
                ClearOutcome::Cleared
            }
            Ok(_value) => ClearOutcome::Refused,
            Err(_error) => ClearOutcome::Rejected(ApiError::Body),
        },
        Err(error) => ClearOutcome::Rejected(error),
    }
}

// ── requests ────────────────────────────────────────────────────────────────

/// `GET /api/translator/console-logs`, the fallback for the initial fill.
pub async fn load_history() -> Result<Vec<String>, ApiError> {
    let body = crate::api::get(HISTORY_PATH).await?;
    parse_history(&body).ok_or(ApiError::Body)
}

/// `DELETE /api/translator/console-logs`.
pub async fn clear_history() -> ClearOutcome {
    let response = crate::api::delete(HISTORY_PATH).await;
    settle_clear(response.as_deref().map_err(|error| *error))
}

#[cfg(test)]
mod tests {
    use super::{LogBuffer, LogLevel, MAX_LINES, parse_console_frame};

    #[test]
    fn the_second_bracketed_tag_sets_the_level() {
        assert_eq!(LogLevel::from_line("[9Router] [WARN] slow"), LogLevel::Warn);
        assert_eq!(LogLevel::from_line("no tags here"), LogLevel::Log);
    }

    #[test]
    fn the_buffer_cannot_grow_past_the_cap() {
        let mut buffer = LogBuffer::default();
        for index in 0..(MAX_LINES + 50) {
            buffer.push(format!("line {index}"));
        }
        assert_eq!(buffer.len(), MAX_LINES);
        assert_eq!(buffer.dropped(), 50);
    }

    #[test]
    fn an_unknown_frame_type_is_ignored_rather_than_appended() {
        assert!(parse_console_frame(r#"{"type":"heartbeat"}"#).is_none());
        assert!(parse_console_frame("{}").is_none());
    }
}
