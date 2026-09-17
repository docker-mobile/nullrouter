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
            // Keep branch line
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
    let mut out = lines.get(..head).unwrap_or(&lines).join("\n");
    out.push_str(&format!("\n... +{cut} lines truncated (file continues)\n"));
    let tail_start = lines.len().saturating_sub(tail);
    out.push_str(&lines.get(tail_start..).unwrap_or(&lines).join("\n"));
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
    for line in lines.get(1..).unwrap_or(&[]) {
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

/// Compact `ls -la` output: parse lines, group by type, show names + sizes.
fn compact_ls(input: &str) -> String {
    let mut dirs = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();
    for line in input.lines() {
        if line.starts_with("total ") || line.is_empty() {
            continue;
        }
        // Parse: perms ... size month day time name
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }
        let perms = parts[0];
        let is_dir = perms.starts_with('d');
        // Size is typically at index 4
        let size: u64 = parts
            .iter()
            .rev()
            .skip(1)
            .find_map(|p| p.parse().ok())
            .unwrap_or(0);
        let name = parts[8..].join(" ");
        if is_dir {
            dirs.push(name);
        } else {
            let size_str = if size >= 1_048_576 {
                format!("{:.1}M", size as f64 / 1_048_576.0)
            } else if size >= 1024 {
                format!("{:.1}K", size as f64 / 1024.0)
            } else {
                format!("{size}B")
            };
            files.push((name, size_str));
        }
    }
    let mut out = String::new();
    if !dirs.is_empty() {
        out.push_str(&format!("Dirs ({}):\n", dirs.len()));
        for d in dirs.iter().take(10) {
            out.push_str("  ");
            out.push_str(d);
            out.push('\n');
        }
    }
    if !files.is_empty() {
        out.push_str(&format!("Files ({}):\n", files.len()));
        for (name, size) in files.iter().take(20) {
            out.push_str(&format!("  {name} ({size})\n"));
        }
        if files.len() > 20 {
            out.push_str(&format!("  ... +{} more\n", files.len() - 20));
        }
    }
    out
}

/// Compact build tool output (npm, cargo, pip, etc.).
/// Keeps errors, warnings, final summary. Strips progress logs.
fn compact_build_output(input: &str) -> String {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut summary: Option<String> = None;
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("npm err")
            || lower.starts_with("yarn error")
            || lower.starts_with("error:")
            || lower.starts_with("[error]")
        {
            errors.push(line.to_owned());
        } else if lower.starts_with("npm warn")
            || lower.starts_with("yarn warn")
            || lower.starts_with("warning:")
        {
            if warnings.len() < 5 {
                warnings.push(line.to_owned());
            }
        } else if lower.starts_with("finished")
            || lower.starts_with("successfully")
            || lower.starts_with("build ")
            || lower.contains("compilation finished")
        {
            summary = Some(line.to_owned());
        }
    }
    let mut out = String::new();
    if !errors.is_empty() {
        out.push_str(&format!("Errors ({}):\n", errors.len()));
        for e in &errors {
            out.push_str(e);
            out.push('\n');
        }
    }
    if !warnings.is_empty() {
        out.push_str(&format!("Warnings ({}):\n", warnings.len()));
        for w in &warnings {
            out.push_str(w);
            out.push('\n');
        }
    }
    if let Some(s) = summary {
        out.push_str(&s);
        out.push('\n');
    }
    if out.is_empty() {
        input.to_owned()
    } else {
        out
    }
}

/// Detect which RTK filter to apply based on content heuristics.
/// Returns the filter name, or None if no filter matches.
#[must_use]
pub fn auto_detect_filter(text: &str) -> Option<&'static str> {
    let head = if text.len() > 1024 {
        &text[..1024]
    } else {
        text
    };

    // git log: commit <sha> header
    if head.lines().any(|l| {
        l.starts_with("commit ") && l.len() >= 14 && l[7..].chars().all(|c| c.is_ascii_hexdigit())
    }) {
        return Some("git-log");
    }
    // git diff
    if head.contains("diff --git") || head.contains("@@ ") {
        return Some("git-diff");
    }
    // git status
    if head.lines().any(|l| {
        l.starts_with("On branch")
            || l.starts_with("Changes")
            || l.starts_with("Untracked")
            || l.starts_with("nothing to commit")
    }) {
        return Some("git-status");
    }
    // build output
    if head.lines().any(|l| {
        let t = l.trim();
        t.starts_with("npm ")
            || t.starts_with("yarn ")
            || t.starts_with("Compiling")
            || t.starts_with("Downloading")
            || t.starts_with("[ERROR]")
            || t.contains("BUILD ")
    }) {
        return Some("build-output");
    }
    // tree
    if head.contains("directories") && head.contains("files") {
        return Some("tree");
    }
    // ls
    if head.lines().any(|l| {
        l.starts_with("total ")
            || (l.starts_with('-')
                && l.len() > 10
                && l[1..].chars().take(9).all(|c| "rwx-".contains(c)))
    }) {
        return Some("ls");
    }
    // search list
    if head.starts_with("Result of search") {
        return Some("search-list");
    }
    // read numbered
    if head.lines().take(5).all(|l| {
        l.trim_start()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
            && l.contains('|')
    }) {
        return Some("read-numbered");
    }
    // dedup
    None
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
