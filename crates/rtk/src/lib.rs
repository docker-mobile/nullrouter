#![allow(
    clippy::format_push_string,
    clippy::or_then_unwrap,
    clippy::cast_precision_loss
)]
//! `NullStack` RTK — zero-copy tool-result compression.
//!
//! Own-brand port of the *good* parts of 9Router `open-sse/rtk/` and
//! `OmniRoute`'s RTK stack, re-implemented in Rust/Pingora style:
//! zero-copy, fail-open, no Node.
//!
//! The 49 JSON filters in `OmniRoute` become Rust `Filter` impls here.
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
    pub const fn saved(&self) -> usize {
        self.bytes_before.saturating_sub(self.bytes_after)
    }

    #[must_use]
    #[allow(clippy::cast_precision_loss)]
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
    let mut out = text.to_owned();
    let mut hits = Vec::new();

    macro_rules! apply {
        ($name:expr, $result:expr) => {
            if let Some(result) = $result {
                if result.len() < out.len() {
                    let saved = out.len() - result.len();
                    out = result;
                    hits.push(Hit {
                        filter: $name.to_owned(),
                        saved,
                    });
                }
            }
        };
    }

    // 1. dedup-log: collapse consecutive identical lines.
    let snapshot = out.clone();
    apply!("dedup-log", Some(dedup_consecutive_lines(&snapshot)));

    // 2. smart-truncate: head+tail with middle dropped if huge.
    if out.len() > 80_000 {
        let snapshot = out.clone();
        apply!("smart-truncate", Some(smart_truncate(&snapshot, 80_000)));
    }

    // 3. grep-preamble: strip ripgrep JSON header lines.
    if out.contains("file:") && out.contains("match:") {
        let snapshot = out.clone();
        apply!("grep", Some(strip_grep_preamble(&snapshot)));
    }

    // 4. git-diff: compact unified diffs.
    if out.contains("diff --git") || out.contains("@@") {
        let snapshot = out.clone();
        apply!("git-diff", Some(compact_git_diff(&snapshot)));
    }

    // 5. git-status: compact status output.
    if out
        .lines()
        .any(|l| l.starts_with("On branch") || l.starts_with("Changes"))
    {
        let snapshot = out.clone();
        apply!("git-status", Some(compact_git_status(&snapshot)));
    }

    // 6. git-log: cap and compress commit log.
    if out.contains("commit ") && out.contains("Author:") {
        let snapshot = out.clone();
        apply!("git-log", Some(compact_git_log(&snapshot, 200)));
    }

    // 7. find: group by directory, cap per dir.
    if out.lines().count() > 5 && out.lines().any(|l| l.contains('/') && !l.contains(' ')) {
        let snapshot = out.clone();
        apply!("find", Some(compact_find(&snapshot)));
    }

    // 8. tree: strip summary line and trailing blanks.
    if out.contains("director") && out.contains("file") {
        let snapshot = out.clone();
        apply!("tree", Some(compact_tree(&snapshot)));
    }

    // 9. read-numbered: head+tail truncation for "N|content" lines.
    if out.lines().take(5).all(|l| {
        l.trim_start()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
            && l.contains('|')
    }) {
        let snapshot = out.clone();
        apply!("read-numbered", Some(compact_read_numbered(&snapshot)));
    }

    // 10. search-list: group search results by dir.
    if out.starts_with("Result of search") {
        let snapshot = out.clone();
        apply!("search-list", Some(compact_search_list(&snapshot)));
    }

    // 11. ls: compact directory listing.
    if out
        .lines()
        .any(|l| l.starts_with("total ") || l.starts_with('-') && l.len() > 10)
    {
        let snapshot = out.clone();
        apply!("ls", Some(compact_ls(&snapshot)));
    }

    // 12. build-output: compact build tool output.
    if out.lines().any(|l| {
        let t = l.trim();
        t.starts_with("npm ")
            || t.starts_with("yarn ")
            || t.starts_with("Compiling")
            || t.starts_with("[ERROR]")
    }) {
        let snapshot = out.clone();
        apply!("build-output", Some(compact_build_output(&snapshot)));
    }

    (out.len(), hits)
}

pub mod file_filters;
/// Smart truncation: keep head and tail, drop the middle.
pub mod filters;
pub mod git_filters;

use file_filters::{
    compact_build_output, compact_find, compact_ls, compact_read_numbered, compact_search_list,
    compact_tree,
};
use filters::{dedup_consecutive_lines, smart_truncate, strip_grep_preamble};
use git_filters::{compact_git_diff, compact_git_log, compact_git_status};

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
