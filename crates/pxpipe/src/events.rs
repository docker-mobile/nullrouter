//! The per-request event log and the aggregates the dashboard reads.
//!
//! Ports `inspire/src/lib/pxpipe/events.js`.
//!
//! One JSON object per line, appended per request and rotated at 5 MB. A file
//! rather than the state database because the request path writes it: a token saver
//! must not add a loopback round trip to every request, and losing a stats line is
//! not worth failing one.

use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::install::Paths;

/// Rotate once the live file passes this.
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// Days of daily totals in the timeline.
const TIMELINE_DAYS: u64 = 30;

/// One transform attempt.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// Epoch milliseconds.
    pub ts: u64,
    /// Whether the body was actually replaced.
    #[serde(default)]
    pub applied: bool,
    /// Machine-readable outcome: `applied`, `below_threshold`, `timeout`,
    /// `transform_error`, `not_installed`, `disabled`, ...
    #[serde(default)]
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default)]
    pub original_chars: u64,
    #[serde(default)]
    pub tokens_before_est: u64,
    #[serde(default)]
    pub tokens_after_est: u64,
    #[serde(default)]
    pub tokens_saved_est: u64,
    #[serde(default)]
    pub image_count: u64,
    #[serde(default)]
    pub duration_ms: u64,
}

/// Aggregated counters over one window.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub requests: u64,
    pub compressed: u64,
    pub bypassed: u64,
    pub errors: u64,
    pub tokens_before_est: u64,
    pub tokens_after_est: u64,
    pub tokens_saved_est: u64,
    /// Percentage saved, two decimal places.
    pub saved_pct: f64,
    pub images_generated: u64,
    pub compression_time_ms: u64,
    pub avg_compression_ms: u64,
}

impl Totals {
    fn accumulate(&mut self, event: &Event) {
        self.requests += 1;
        if event.applied {
            self.compressed += 1;
            self.tokens_before_est += event.tokens_before_est;
            self.tokens_after_est += event.tokens_after_est;
            self.tokens_saved_est += event.tokens_saved_est;
            self.images_generated += event.image_count;
            self.compression_time_ms += event.duration_ms;
        } else if matches!(
            event.reason.as_str(),
            "transform_error" | "timeout" | "parse_error" | "worker_gone" | "load_error"
        ) {
            // A failure is counted apart from a deliberate bypass: one means the
            // saver is broken, the other means it decided not to act. `parse_error`
            // is on this side because a body the package cannot read is a fault
            // somewhere, not a decision.
            self.errors += 1;
        } else {
            self.bypassed += 1;
        }
    }

    fn finalize(&mut self) {
        self.saved_pct = percentage(self.tokens_saved_est, self.tokens_before_est);
        self.avg_compression_ms = self
            .compression_time_ms
            .checked_div(self.compressed)
            .unwrap_or(0);
    }
}

/// One day of the timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayTotals {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub tokens_saved_est: u64,
    pub compressed: u64,
    pub requests: u64,
}

/// Every window the dashboard shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Windows {
    pub all: Totals,
    pub today: Totals,
    pub yesterday: Totals,
    pub last7d: Totals,
    pub last30d: Totals,
}

/// The stats payload.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub windows: Windows,
    pub timeline: Vec<DayTotals>,
    /// Most recent first.
    pub recent: Vec<Event>,
}

/// `numerator / denominator` as a percentage to two places, 0 when there is no
/// denominator.
///
/// Shared with [`crate::compress`] so a summary and the aggregate that counts it
/// cannot round differently.
pub(crate) fn percentage(numerator: u64, denominator: u64) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "token counts stay far below 2^53; the result is a display percentage"
    )]
    let ratio = numerator as f64 / denominator as f64 * 100.0;
    if denominator == 0 {
        return 0.0;
    }
    (ratio * 100.0).round() / 100.0
}

/// Milliseconds since the epoch.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(0))
}

/// The current time as RFC3339 UTC, for log lines.
pub fn now_iso() -> String {
    iso_from_millis(now_millis())
}

/// `YYYY-MM-DDTHH:MM:SSZ` for epoch milliseconds.
fn iso_from_millis(millis: u64) -> String {
    let (year, month, day) = civil_from_millis(millis);
    let seconds = millis / 1000 % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

/// `YYYY-MM-DD` for epoch milliseconds.
fn date_from_millis(millis: u64) -> String {
    let (year, month, day) = civil_from_millis(millis);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Civil date from epoch milliseconds, via Howard Hinnant's `civil_from_days`.
fn civil_from_millis(millis: u64) -> (i64, u64, u64) {
    let days = i64::try_from(millis / 1000 / 86_400).unwrap_or(0);
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_shifted + 2) / 5 + 1;
    let month = if month_shifted < 10 {
        month_shifted + 3
    } else {
        month_shifted - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (
        year,
        u64::try_from(month).unwrap_or(1),
        u64::try_from(day).unwrap_or(1),
    )
}

/// Midnight UTC on the day containing `millis`.
const fn start_of_day(millis: u64) -> u64 {
    millis - millis % DAY_MS
}

/// Append one event.
///
/// Best-effort by design: this runs in the request path, and a stats line is not
/// worth failing a request over.
pub fn append(paths: &Paths, event: &Event) {
    if let Err(error) = try_append(paths, event) {
        tracing::debug!(%error, "could not record a pxpipe event");
    }
}

fn try_append(paths: &Paths, event: &Event) -> std::io::Result<()> {
    std::fs::create_dir_all(&paths.root)?;
    let live = paths.events();
    // Rotated before the write, so one oversized file cannot grow without bound.
    if std::fs::metadata(&live).is_ok_and(|meta| meta.len() > MAX_FILE_BYTES) {
        let _ = std::fs::rename(&live, paths.rotated_events());
    }
    let mut frame = serde_json::to_string(event)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    frame.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&live)?
        .write_all(frame.as_bytes())
}

/// Read events oldest-first, optionally bounded to the most recent `limit`.
///
/// A corrupt line is skipped rather than failing the read: a torn append from a
/// killed process must not hide every event around it.
pub fn read(paths: &Paths, since_ms: Option<u64>, limit: Option<usize>) -> Vec<Event> {
    let mut events = Vec::new();
    for file in [paths.rotated_events(), paths.events()] {
        read_file(&file, since_ms, &mut events);
    }
    events.sort_by_key(|event| event.ts);
    if let Some(limit) = limit {
        let start = events.len().saturating_sub(limit);
        return events.split_off(start);
    }
    events
}

fn read_file(path: &Path, since_ms: Option<u64>, into: &mut Vec<Event>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        if let Ok(event) = serde_json::from_str::<Event>(line)
            && since_ms.is_none_or(|since| event.ts >= since)
        {
            into.push(event);
        }
    }
}

/// Aggregate the event log.
///
/// `now_ms` is a parameter so the windowing is testable without waiting a day.
pub fn stats(paths: &Paths, now_ms: u64, recent_limit: usize) -> Stats {
    let events = read(paths, None, None);
    let today = start_of_day(now_ms);

    let mut windows = Windows::default();
    let mut timeline: Vec<DayTotals> = (0..TIMELINE_DAYS)
        .rev()
        .map(|offset| DayTotals {
            date: date_from_millis(today.saturating_sub(offset * DAY_MS)),
            tokens_saved_est: 0,
            compressed: 0,
            requests: 0,
        })
        .collect();

    for event in &events {
        windows.all.accumulate(event);
        if event.ts >= today {
            windows.today.accumulate(event);
        } else if event.ts >= today.saturating_sub(DAY_MS) {
            windows.yesterday.accumulate(event);
        }
        if event.ts >= now_ms.saturating_sub(7 * DAY_MS) {
            windows.last7d.accumulate(event);
        }
        if event.ts >= now_ms.saturating_sub(30 * DAY_MS) {
            windows.last30d.accumulate(event);
        }

        let date = date_from_millis(event.ts);
        if let Some(bucket) = timeline.iter_mut().find(|bucket| bucket.date == date) {
            bucket.requests += 1;
            if event.applied {
                bucket.compressed += 1;
                bucket.tokens_saved_est += event.tokens_saved_est;
            }
        }
    }

    for window in [
        &mut windows.all,
        &mut windows.today,
        &mut windows.yesterday,
        &mut windows.last7d,
        &mut windows.last30d,
    ] {
        window.finalize();
    }

    let start = events.len().saturating_sub(recent_limit);
    let mut recent = events.get(start..).unwrap_or_default().to_vec();
    // Newest first: that is the order a log panel reads in.
    recent.reverse();

    Stats {
        windows,
        timeline,
        recent,
    }
}

#[cfg(test)]
mod tests {
    use super::{DAY_MS, Event, append, date_from_millis, iso_from_millis, read, stats};
    use crate::install::Paths;

    /// 2024-06-01T12:00:00Z.
    const NOW: u64 = 1_717_243_200_000;

    fn applied(ts: u64, saved: u64) -> Event {
        Event {
            ts,
            applied: true,
            reason: "applied".to_owned(),
            original_chars: 40_000,
            tokens_before_est: 10_000,
            tokens_after_est: 10_000 - saved,
            tokens_saved_est: saved,
            image_count: 2,
            duration_ms: 300,
            detail: None,
        }
    }

    fn bypassed(ts: u64, reason: &str) -> Event {
        Event {
            ts,
            applied: false,
            reason: reason.to_owned(),
            ..Event::default()
        }
    }

    #[test]
    fn dates_are_rendered_from_epoch_millis() {
        assert_eq!(date_from_millis(NOW), "2024-06-01");
        assert_eq!(iso_from_millis(NOW), "2024-06-01T12:00:00Z");
        // A leap day, and a year boundary.
        assert_eq!(date_from_millis(1_709_208_000_000), "2024-02-29");
        assert_eq!(date_from_millis(1_735_689_600_000), "2025-01-01");
    }

    #[test]
    fn events_round_trip_through_the_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        append(&paths, &applied(NOW, 4000));
        append(&paths, &bypassed(NOW + 1, "below_threshold"));

        let events = read(&paths, None, None);
        assert_eq!(events.len(), 2);
        // Oldest first.
        assert!(events.first().is_some_and(|event| event.applied));
        assert_eq!(
            events.get(1).map(|event| event.reason.as_str()),
            Some("below_threshold")
        );
    }

    #[test]
    fn a_corrupt_line_is_skipped_rather_than_hiding_the_file() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        append(&paths, &applied(NOW, 1000));
        // A torn append, as a killed process leaves.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(paths.events())
            .expect("open");
        writeln!(file, "{{\"ts\":123,\"appl").expect("write torn line");
        drop(file);
        append(&paths, &applied(NOW + 1, 2000));

        let events = read(&paths, None, None);
        assert_eq!(events.len(), 2, "a torn line must not hide its neighbours");
    }

    #[test]
    fn the_rotated_file_is_read_alongside_the_live_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(&paths.root).expect("create root");
        // An older event in the rotated file, a newer one live. Reading only the
        // live file would silently reset every all-time total on rotation.
        std::fs::write(
            paths.rotated_events(),
            format!(
                "{}\n",
                serde_json::to_string(&applied(NOW - DAY_MS, 500)).expect("json")
            ),
        )
        .expect("write rotated");
        append(&paths, &applied(NOW, 700));

        let events = read(&paths, None, None);
        assert_eq!(events.len(), 2);
        // Sorted across both files, oldest first.
        assert_eq!(events.first().map(|event| event.ts), Some(NOW - DAY_MS));
    }

    #[test]
    fn reads_can_be_bounded_and_filtered_by_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        for index in 0..10 {
            append(&paths, &applied(NOW + index, 100));
        }
        // A bound keeps the *newest*, which is what a log panel wants.
        let bounded = read(&paths, None, Some(3));
        assert_eq!(bounded.len(), 3);
        assert_eq!(bounded.first().map(|event| event.ts), Some(NOW + 7));

        let since = read(&paths, Some(NOW + 5), None);
        assert_eq!(since.len(), 5);
    }

    #[test]
    fn totals_separate_a_failure_from_a_deliberate_bypass() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        append(&paths, &applied(NOW, 4000));
        append(&paths, &bypassed(NOW, "below_threshold"));
        append(&paths, &bypassed(NOW, "disabled"));
        append(&paths, &bypassed(NOW, "transform_error"));
        append(&paths, &bypassed(NOW, "timeout"));

        let stats = stats(&paths, NOW, 100);
        let all = &stats.windows.all;
        assert_eq!(all.requests, 5);
        assert_eq!(all.compressed, 1);
        // A saver that decided not to act is not a saver that broke.
        assert_eq!(all.bypassed, 2);
        assert_eq!(all.errors, 2);
    }

    #[test]
    fn savings_are_reported_as_a_percentage_of_the_before_estimate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        append(&paths, &applied(NOW, 2500));

        let stats = stats(&paths, NOW, 100);
        assert_eq!(stats.windows.all.tokens_before_est, 10_000);
        assert_eq!(stats.windows.all.tokens_saved_est, 2500);
        assert!(
            (stats.windows.all.saved_pct - 25.0).abs() < f64::EPSILON,
            "got {}",
            stats.windows.all.saved_pct
        );
        assert_eq!(stats.windows.all.avg_compression_ms, 300);
    }

    #[test]
    fn an_empty_log_reports_zeroes_rather_than_dividing_by_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        let stats = stats(&paths, NOW, 100);
        assert_eq!(stats.windows.all.requests, 0);
        assert!(stats.windows.all.saved_pct.abs() < f64::EPSILON);
        assert_eq!(stats.windows.all.avg_compression_ms, 0);
        // The timeline is still a full month of zeroes, so a chart has an axis.
        assert_eq!(stats.timeline.len(), 30);
        assert!(stats.recent.is_empty());
    }

    #[test]
    fn windows_place_events_in_the_right_buckets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        append(&paths, &applied(NOW, 100)); // today
        append(&paths, &applied(NOW - DAY_MS, 200)); // yesterday
        append(&paths, &applied(NOW - 5 * DAY_MS, 300)); // this week
        append(&paths, &applied(NOW - 20 * DAY_MS, 400)); // this month
        append(&paths, &applied(NOW - 200 * DAY_MS, 500)); // all-time only

        let stats = stats(&paths, NOW, 100);
        assert_eq!(stats.windows.all.requests, 5);
        assert_eq!(stats.windows.today.requests, 1);
        assert_eq!(stats.windows.yesterday.requests, 1);
        assert_eq!(stats.windows.last7d.requests, 3);
        assert_eq!(stats.windows.last30d.requests, 4);
        // Today's numbers are today's, not a running total.
        assert_eq!(stats.windows.today.tokens_saved_est, 100);
    }

    #[test]
    fn the_timeline_has_one_bucket_per_day_ending_today() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        append(&paths, &applied(NOW, 111));
        append(&paths, &applied(NOW - 2 * DAY_MS, 222));

        let stats = stats(&paths, NOW, 100);
        assert_eq!(stats.timeline.len(), 30);
        assert_eq!(
            stats.timeline.last().map(|day| day.date.as_str()),
            Some("2024-06-01"),
            "the last bucket is today"
        );
        let today = stats.timeline.last().expect("today");
        assert_eq!(today.tokens_saved_est, 111);
        let two_days_ago = stats
            .timeline
            .iter()
            .find(|day| day.date == "2024-05-30")
            .expect("that day");
        assert_eq!(two_days_ago.tokens_saved_est, 222);
    }

    #[test]
    fn recent_events_are_newest_first_and_bounded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        for index in 0..5 {
            append(&paths, &applied(NOW + index, index * 10));
        }
        let stats = stats(&paths, NOW, 3);
        assert_eq!(stats.recent.len(), 3);
        assert_eq!(stats.recent.first().map(|event| event.ts), Some(NOW + 4));
        assert_eq!(stats.recent.last().map(|event| event.ts), Some(NOW + 2));
    }
}
