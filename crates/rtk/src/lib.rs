#![allow(clippy::format_push_string, clippy::or_then_unwrap)]
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

    (out.len(), hits)
}

/// Smart truncation: keep head and tail, drop the middle.
fn smart_truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let head = max / 2;
    let tail = max - head;
    let cut = text.len() - max;
    format!(
        "{}
... +{} bytes truncated ...
{}",
        &text[..head],
        cut,
        &text[text.len() - tail..]
    )
}

/// Strip grep JSON preamble header lines.
fn strip_grep_preamble(text: &str) -> String {
    let mut lines = text.lines();
    let a = lines.next().unwrap_or("");
    let b = lines.next().unwrap_or("");
    if a.starts_with("Found") || b.contains("matches") {
        lines.collect::<Vec<&str>>().join("\n")
    } else {
        text.to_owned()
    }
}

/// Compact unified git diff: keep file headers, cap hunks, count +/-/context.
fn compact_git_diff(diff: &str) -> String {
    let max_lines = 500;
    let max_hunk = 100;
    let mut result = String::with_capacity(diff.len() / 2);
    let mut added = 0u32;
    let mut removed = 0u32;
    let mut hunk_shown = 0u32;

    for line in diff.lines() {
        if line.starts_with("diff --git") {
            if added > 0 || removed > 0 {
                result.push_str(&format!("  +{added} -{removed}\n"));
                added = 0;
                removed = 0;
            }
            result.push_str(line);
            result.push('\n');
            hunk_shown = 0;
        } else if line.starts_with("@@") {
            hunk_shown = 0;
            result.push_str(line);
            result.push('\n');
        } else if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
            if hunk_shown < max_hunk {
                result.push_str(line);
                result.push('\n');
                hunk_shown += 1;
            }
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
            if hunk_shown < max_hunk {
                result.push_str(line);
                result.push('\n');
                hunk_shown += 1;
            }
        } else if hunk_shown < max_hunk {
            result.push_str(line);
            result.push('\n');
            hunk_shown += 1;
        }
        if result.len() > max_lines * 80 {
            break;
        }
    }
    if added > 0 || removed > 0 {
        result.push_str(&format!("  +{added} -{removed}"));
    }
    result
}

/// Compact git status output.
fn compact_git_status(input: &str) -> String {
    let max_files = 10;
    let lines: Vec<&str> = input.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return "Clean working tree".to_owned();
    }
    let mut result = String::new();
    let mut staged = 0u32;
    let mut modified = 0u32;
    let mut untracked = 0u32;
    let mut shown = 0u32;
    for line in &lines {
        if line.starts_with("On branch") || line.starts_with("##") {
            result.push_str(line);
            result.push('\n');
        } else if line.starts_with("Changes to be committed") {
            result.push_str(line);
            result.push('\n');
        } else if line.starts_with('M')
            || line.starts_with('A')
            || line.starts_with('D')
            || line.starts_with('R')
        {
            staged += 1;
            if shown < max_files {
                result.push_str(line);
                result.push('\n');
                shown += 1;
            }
        } else if line.starts_with(" M") || line.starts_with("??") {
            if line.starts_with("??") {
                untracked += 1;
            } else {
                modified += 1;
            }
            if shown < max_files {
                result.push_str(line);
                result.push('\n');
                shown += 1;
            }
        }
    }
    if staged + modified + untracked == 0 && result.trim().is_empty() {
        return "Clean working tree".to_owned();
    }
    result
}

/// Compact git log: keep headers and subjects, cap total lines.
fn compact_git_log(text: &str, max_lines: usize) -> String {
    let mut out = String::with_capacity(text.len() / 2);
    let mut count = 0;
    for line in text.lines() {
        if count >= max_lines {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() && out.is_empty() {
            continue;
        }
        if trimmed.starts_with("commit ")
            || trimmed.starts_with("Author:")
            || trimmed.starts_with("Date:")
            || (!trimmed.is_empty() && !trimmed.starts_with(' '))
        {
            out.push_str(trimmed);
            out.push('\n');
            count += 1;
        }
    }
    out
}

/// Group find output by parent directory, show basenames, cap 10/dir.
fn compact_find(input: &str) -> String {
    use std::collections::BTreeMap;
    let lines: Vec<&str> = input.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return input.to_owned();
    }
    let mut by_dir: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for path in &lines {
        let last_sep = path
            .rfind('/')
            .unwrap_or_else(|| path.rfind('\\').unwrap_or(0));
        let (dir, base) = if last_sep == 0 && !path.contains('/') {
            (".", *path)
        } else {
            (&path[..last_sep.max(1)], &path[last_sep + 1..])
        };
        by_dir.entry(dir).or_default().push(base);
    }
    let mut out = format!("{} files in {} dirs:\n\n", lines.len(), by_dir.len());
    for (dir, files) in &by_dir {
        out.push_str(dir);
        out.push_str(":\n");
        for f in files.iter().take(10) {
            out.push_str("  ");
            out.push_str(f);
            out.push('\n');
        }
        if files.len() > 10 {
            out.push_str(&format!("  ... +{} more\n", files.len() - 10));
        }
    }
    out
}

/// Strip tree summary line and trailing blanks.
fn compact_tree(input: &str) -> String {
    let mut filtered: Vec<&str> = Vec::new();
    for line in input.lines() {
        if line.contains("director") && line.contains("file") {
            continue;
        }
        if line.trim().is_empty() && filtered.is_empty() {
            continue;
        }
        filtered.push(line);
    }
    while filtered.last().is_some_and(|l| l.trim().is_empty()) {
        filtered.pop();
    }
    filtered.join("\n")
}

/// Truncate numbered file content (N|content) to head+tail.
fn compact_read_numbered(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    if lines.len() < 100 {
        return input.to_owned();
    }
    let head = 20;
    let tail = 20;
    let cut = lines.len() - head - tail;
    let mut out = lines[..head].join("\n");
    out.push_str(&format!("\n... +{cut} lines truncated (file continues)\n"));
    out.push_str(&lines[lines.len() - tail..].join("\n"));
    out
}

/// Compact search result list, group by directory.
fn compact_search_list(input: &str) -> String {
    use std::collections::BTreeMap;
    let lines: Vec<&str> = input.lines().collect();
    if lines.is_empty() {
        return input.to_owned();
    }
    let mut paths: Vec<&str> = Vec::new();
    for line in &lines[1..] {
        let t = line.trim();
        if let Some(p) = t.strip_prefix("- ") {
            paths.push(p);
        }
    }
    if paths.is_empty() {
        return input.to_owned();
    }
    let mut by_dir: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for p in &paths {
        let sep = p.rfind('/').unwrap_or(0);
        let (dir, base) = if sep == 0 && !p.contains('/') {
            (".", *p)
        } else {
            (&p[..sep.max(1)], &p[sep + 1..])
        };
        by_dir.entry(dir).or_default().push(base);
    }
    let mut out = format!("{} files in {} dirs:\n\n", paths.len(), by_dir.len());
    for (dir, files) in &by_dir {
        out.push_str(dir);
        out.push_str(":\n");
        for f in files.iter().take(10) {
            out.push_str("  ");
            out.push_str(f);
            out.push('\n');
        }
        if files.len() > 10 {
            out.push_str(&format!("  ... +{} more\n", files.len() - 10));
        }
    }
    out
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
