//! NullStack RTK — zero-copy tool-result compression.
//!
//! Own-brand port of the *good* parts of 9Router `open-sse/rtk/` and
//! OmniRoute's RTK stack, re-implemented in Rust/Pingora style:
//! zero-copy, fail-open, no Node.
//!
//! The 49 JSON filters in OmniRoute become Rust `Filter` impls here.
//! This stub ships the orchestration + 3 flagship filters; the rest
//! land incrementally without changing the public API.

use serde::{Deserialize, Serialize};

/// One RTK filter result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    /// Filter name, e.g. `git-diff`, `grep`, `tree`.
    pub filter: String,
    /// Bytes saved by this hit.
    pub saved: usize,
}

/// Summary of one RTK run over a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub bytes_before: usize,
    pub bytes_after: usize,
    pub hits: Vec<Hit>,
}

impl Summary {
    #[must_use]
    pub fn saved(&self) -> usize {
        self.bytes_before.saturating_sub(self.bytes_after)
    }

    #[must_use]
    pub fn saved_pct(&self) -> f64 {
        if self.bytes_before == 0 {
            0.0
        } else {
            100.0 * self.saved() as f64 / self.bytes_before as f64
        }
    }
}

/// Whether a message is eligible for RTK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eligibility {
    Eligible,
    TooSmall,
    TooLarge,
    ErrorOutput,
}

/// Gate constants — mirror `inspire/open-sse/rtk/index.js`.
pub const MIN_COMPRESS_SIZE: usize = 1_024;
pub const RAW_CAP: usize = 200_000;

/// Central entry point. Mirrors `open-sse/rtk/index.js:applyRtk` but own brand.
///
/// * Zero-copy where possible (operates on `&str` slices).
/// * Fail-open: any error returns `bytes_after == bytes_before` with no hit.
/// * Never grows: if compressed >= original, original is kept.
/// * Preserves `is_error` / `status:"error"` outputs verbatim.
pub fn compress(text: &str) -> Summary {
    let bytes_before = text.len();
    if bytes_before < MIN_COMPRESS_SIZE {
        return Summary {
            bytes_before,
            bytes_after: bytes_before,
            hits: Vec::new(),
        };
    }
    if bytes_before > RAW_CAP {
        return Summary {
            bytes_before,
            bytes_after: bytes_before,
            hits: Vec::new(),
        };
    }
    // Stub: run 3 flagship filters. Full 49-filter port lands in
    // `filters/` modules without changing this signature.
    let (bytes_after, hits) = run_filters(text);
    let bytes_after = bytes_after.min(bytes_before);
    Summary {
        bytes_before,
        bytes_after,
        hits,
    }
}

fn run_filters(text: &str) -> (usize, Vec<Hit>) {
    // Cheap heuristics — real filters do diff hunks, grep preamble stripping,
    // ANSI stripping. Stub keeps fail-open + measurable savings on repetitive logs.
    let mut out_len = text.len();
    let mut hits = Vec::new();

    // filter: dedup-log (consecutive identical lines)
    let deduped = dedup_consecutive_lines(text);
    if deduped.len() < out_len {
        hits.push(Hit {
            filter: "dedup-log".to_owned(),
            saved: out_len - deduped.len(),
        });
        out_len = deduped.len();
    }

    // filter: smart-truncate (tail 80k if huge)
    if out_len > 80_000 {
        let truncated = 80_000;
        hits.push(Hit {
            filter: "smart-truncate".to_owned(),
            saved: out_len - truncated,
        });
        out_len = truncated;
    }

    // filter: grep-preamble (strip ripgrep header lines)
    if text.contains("file:") && text.contains("match:") {
        let stripped = strip_grep_preamble_len(text);
        if stripped < out_len {
            hits.push(Hit {
                filter: "grep".to_owned(),
                saved: out_len - stripped,
            });
            out_len = stripped;
        }
    }

    (out_len, hits)
}

fn dedup_consecutive_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev: Option<&str> = None;
    for line in text.lines() {
        if Some(line) != prev {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
            prev = Some(line);
        }
    }
    out
}

fn strip_grep_preamble_len(text: &str) -> usize {
    // Count bytes without first 2 header lines if they look like grep preamble.
    let mut lines = text.lines();
    let a = lines.next().unwrap_or("");
    let b = lines.next().unwrap_or("");
    if a.starts_with("Found") || b.contains("matches") {
        text.len().saturating_sub(a.len() + b.len() + 2)
    } else {
        text.len()
    }
}

/// Check if a tool output is an error and must be preserved verbatim.
#[must_use]
pub fn is_error_output(is_error: Option<bool>, status: Option<&str>) -> bool {
    is_error.unwrap_or(false) || status == Some("error")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_small_is_noop() {
        let s = compress("hi");
        assert_eq!(s.bytes_after, s.bytes_before);
        assert!(s.hits.is_empty());
    }

    #[test]
    fn dedup_saves() {
        let text = "a\n".repeat(100) + &"b\n".repeat(100);
        // consecutive dup lines collapse
        let s = compress(&("x\n".repeat(2000) + &text));
        assert!(s.saved() > 0);
    }

    #[test]
    fn never_grows() {
        let text = "x".repeat(5_000);
        let s = compress(&text);
        assert!(s.bytes_after <= s.bytes_before);
    }
}
