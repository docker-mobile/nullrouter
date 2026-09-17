//! Utility text filters: truncation, grep preamble, dedup.

pub(crate) fn smart_truncate(text: &str, max: usize) -> String {
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
pub(crate) fn strip_grep_preamble(text: &str) -> String {
    let mut lines = text.lines();
    let a = lines.next().unwrap_or("");
    let b = lines.next().unwrap_or("");
    if a.starts_with("Found") || b.contains("matches") {
        lines.collect::<Vec<&str>>().join("\n")
    } else {
        text.to_owned()
    }
}

pub(crate) fn dedup_consecutive_lines(text: &str) -> String {
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
