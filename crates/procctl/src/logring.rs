//! A bounded tail of a child's output.
//!
//! Every supervised child here is chatty and long-lived: `cloudflared` logs on every
//! reconnect, forever. Upstream keeps `logTail = (logTail + msg).slice(-4000)`, which is
//! a reasonable shape, but it also builds an unbounded intermediate string on every chunk.
//! This keeps a fixed number of already-split lines and drops the oldest, so memory is
//! bounded no matter how long the child runs or how much it says.

use std::collections::VecDeque;

/// The most recent lines of a child's output.
#[derive(Debug, Clone)]
pub struct LogRing {
    lines: VecDeque<String>,
    capacity: usize,
    max_line: usize,
    dropped: u64,
}

/// Appended to a line that was cut at [`LogRing::max_line`].
const TRUNCATION_MARK: &str = "…[truncated]";

impl LogRing {
    /// A ring holding at most `capacity` lines of at most `max_line` bytes.
    #[must_use]
    pub fn new(capacity: usize, max_line: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            capacity: capacity.max(1),
            max_line: max_line.max(TRUNCATION_MARK.len() + 1),
            dropped: 0,
        }
    }

    /// Add one line, evicting the oldest if the ring is full.
    pub fn push(&mut self, line: &str) {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let stored = if trimmed.len() > self.max_line {
            // Cut on a character boundary: a byte-index cut through a multi-byte
            // character would panic, and these logs carry UTF-8 punctuation.
            let mut end = self.max_line.saturating_sub(TRUNCATION_MARK.len());
            while end > 0 && !trimmed.is_char_boundary(end) {
                end -= 1;
            }
            let head = trimmed.get(..end).unwrap_or_default();
            format!("{head}{TRUNCATION_MARK}")
        } else {
            trimmed.to_owned()
        };

        if self.lines.len() == self.capacity {
            let _evicted = self.lines.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.lines.push_back(stored);
    }

    /// Split a chunk of child output into lines and add each.
    ///
    /// A chunk from a pipe is not line-aligned, but these logs are read line-wise by the
    /// caller before reaching here; this exists for the one-shot path, which has the
    /// whole output at once.
    pub fn extend_from_chunk(&mut self, chunk: &str) {
        for line in chunk.lines() {
            self.push(line);
        }
    }

    /// The retained lines, oldest first.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }

    /// The retained lines joined with newlines, for an error message.
    #[must_use]
    pub fn tail(&self) -> String {
        self.lines
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// How many lines were evicted because the ring was full.
    #[must_use]
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }

    /// How many lines are retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether nothing has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Whether any retained line contains `needle`.
    ///
    /// This is how readiness is decided: `cloudflared` announces its URL and its
    /// registered connections in its log, and there is no other channel for either.
    #[must_use]
    pub fn contains(&self, needle: &str) -> bool {
        self.lines.iter().any(|line| line.contains(needle))
    }

    /// Forget everything, keeping the drop count.
    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{LogRing, TRUNCATION_MARK};

    #[test]
    fn it_keeps_the_most_recent_lines_and_counts_the_rest() {
        let mut ring = LogRing::new(3, 100);

        for index in 1..=5 {
            ring.push(&format!("line {index}"));
        }

        assert_eq!(ring.lines(), ["line 3", "line 4", "line 5"]);
        assert_eq!(ring.dropped(), 2);
        assert_eq!(ring.len(), 3);
        assert!(!ring.is_empty());
    }

    #[test]
    fn line_endings_are_stripped() {
        let mut ring = LogRing::new(4, 100);

        ring.push("unix\n");
        ring.push("windows\r\n");
        ring.push("bare\r");

        assert_eq!(ring.lines(), ["unix", "windows", "bare"]);
    }

    #[test]
    fn an_overlong_line_is_cut_and_marked() {
        let mut ring = LogRing::new(2, 32);

        ring.push(&"x".repeat(200));

        let stored = ring.lines();
        let only = stored.first().expect("one line");
        assert!(only.ends_with(TRUNCATION_MARK), "{only}");
        assert!(only.len() <= 32, "{} bytes", only.len());
    }

    #[test]
    fn a_cut_lands_on_a_character_boundary() {
        let mut ring = LogRing::new(2, 24);

        // Multi-byte characters straddling the cut point: a byte-index slice would panic.
        ring.push(&"é".repeat(100));

        let stored = ring.lines();
        assert!(
            stored
                .first()
                .is_some_and(|line| line.ends_with(TRUNCATION_MARK))
        );
    }

    #[test]
    fn a_chunk_becomes_several_lines() {
        let mut ring = LogRing::new(10, 100);

        ring.extend_from_chunk("first\nsecond\nthird\n");

        assert_eq!(ring.lines(), ["first", "second", "third"]);
        assert_eq!(ring.tail(), "first\nsecond\nthird");
    }

    #[test]
    fn contains_searches_the_retained_lines_only() {
        let mut ring = LogRing::new(2, 100);

        ring.push("Registered tunnel connection");
        ring.push("second");
        assert!(ring.contains("Registered tunnel"));

        // Once evicted, the needle is gone: readiness must not be decided on a line the
        // ring no longer holds.
        ring.push("third");
        assert!(!ring.contains("Registered tunnel"));
    }

    #[test]
    fn clear_keeps_the_drop_count() {
        let mut ring = LogRing::new(1, 100);
        ring.push("a");
        ring.push("b");
        assert_eq!(ring.dropped(), 1);

        ring.clear();

        assert!(ring.is_empty());
        assert_eq!(ring.dropped(), 1, "the count is cumulative, not per-window");
    }

    #[test]
    fn degenerate_bounds_do_not_produce_a_zero_capacity_ring() {
        let mut ring = LogRing::new(0, 0);

        ring.push("something");

        assert_eq!(
            ring.len(),
            1,
            "a zero-capacity ring would silently eat everything"
        );
    }
}
