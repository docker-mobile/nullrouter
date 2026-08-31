//! Shrink oversized MCP tool results before they cross the SSE wire.
//!
//! Ports upstream's `smartFilterText` / `collapseRepeated` / `filterFrame`. The case it exists for
//! is a browser accessibility snapshot: one `tools/call` result can be hundreds of kilobytes of
//! near-identical list items, and forwarding it whole spends a client's whole context on noise.
//!
//! Two rules hold everywhere in here:
//!
//! * **Only `result.content[].text` is touched.** Anything else — ids, methods, errors, non-text
//!   content — passes through byte for byte. A filter that rewrote a JSON-RPC id would break
//!   request/response correlation.
//! * **A frame that does not parse is forwarded unchanged.** Upstream does the same. Guessing at
//!   malformed JSON risks turning a server's own error into a different error.

/// Below this length a text block is left alone. Upstream's threshold.
const FILTER_FLOOR_CHARS: usize = 2_000;
/// Hard ceiling on one text block after collapsing.
const MAX_TEXT_CHARS: usize = 50_000;
/// Consecutive same-role siblings needed before a group collapses.
const COLLAPSE_THRESHOLD: usize = 30;
/// Siblings kept at the head of a collapsed group.
const COLLAPSE_KEEP_HEAD: usize = 10;
/// Siblings kept at the tail of a collapsed group.
const COLLAPSE_KEEP_TAIL: usize = 5;

/// The `(indent, role)` of a `- role ...` list line, if it is one.
fn sibling_key(line: &str) -> Option<(&str, &str)> {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let rest = rest.strip_prefix('-')?;
    let rest = rest.trim_start();
    let role_len = rest
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    if role_len == 0 {
        return None;
    }
    Some((indent, rest.get(..role_len)?))
}

/// Whether `line` continues a group at `indent` without starting a new sibling.
fn is_continuation(line: &str, indent: &str) -> bool {
    line.starts_with(&format!("{indent} ")) || line.starts_with(&format!("{indent}\t"))
}

/// Collapse runs of same-indent, same-role siblings, keeping head and tail.
fn collapse_repeated(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut index = 0_usize;

    while index < lines.len() {
        let Some(line) = lines.get(index) else { break };
        let Some((indent, role)) = sibling_key(line) else {
            out.push((*line).to_owned());
            index += 1;
            continue;
        };

        // Extent of this group: further siblings at the same indent/role, plus their nested lines.
        let mut end = index;
        while let Some(candidate) = lines.get(end) {
            match sibling_key(candidate) {
                Some((candidate_indent, candidate_role))
                    if candidate_indent == indent && candidate_role == role =>
                {
                    end += 1;
                }
                _ if is_continuation(candidate, indent) => end += 1,
                _ => break,
            }
        }

        let group_len = end - index;
        if group_len >= COLLAPSE_THRESHOLD {
            let head_end = nth_sibling_end(&lines, index, indent, role, COLLAPSE_KEEP_HEAD);
            let tail_start = last_n_sibling_start(&lines, end, indent, role, COLLAPSE_KEEP_TAIL);
            for kept in lines.get(index..head_end).unwrap_or_default() {
                out.push((*kept).to_owned());
            }
            let omitted = group_len
                .saturating_sub(COLLAPSE_KEEP_HEAD)
                .saturating_sub(COLLAPSE_KEEP_TAIL);
            out.push(format!(
                "{indent}... [{omitted} similar \"{role}\" items omitted by the nullrouter MCP bridge]"
            ));
            for kept in lines.get(tail_start..end).unwrap_or_default() {
                out.push((*kept).to_owned());
            }
        } else {
            for kept in lines.get(index..end).unwrap_or_default() {
                out.push((*kept).to_owned());
            }
        }
        index = end;
    }
    out.join("\n")
}

/// Index just past the `n`th sibling at `indent`/`role`.
fn nth_sibling_end(lines: &[&str], start: usize, indent: &str, role: &str, n: usize) -> usize {
    let mut seen = 0_usize;
    for (offset, line) in lines.iter().enumerate().skip(start) {
        if sibling_key(line) == Some((indent, role)) {
            seen += 1;
            if seen > n {
                return offset;
            }
        }
    }
    lines.len()
}

/// Index of the `n`th-from-last sibling before `end`.
fn last_n_sibling_start(lines: &[&str], end: usize, indent: &str, role: &str, n: usize) -> usize {
    let positions: Vec<usize> = lines
        .get(..end)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter(|(_, line)| sibling_key(line) == Some((indent, role)))
        .map(|(offset, _)| offset)
        .collect();
    if positions.len() > n {
        positions.get(positions.len() - n).copied().unwrap_or(end)
    } else {
        end
    }
}

/// Drop noise nodes, collapse repeats, then hard-truncate.
fn smart_filter_text(text: &str) -> String {
    if text.len() < FILTER_FLOOR_CHARS {
        return text.to_owned();
    }
    let without_noise: Vec<&str> = text
        .split('\n')
        .filter(|line| {
            let trimmed = line.trim();
            // `- generic:` and `- text: ""` carry no information for a reader.
            !(trimmed == "- generic" || trimmed == "- generic:" || trimmed == "- text: \"\"")
        })
        .collect();
    let collapsed = collapse_repeated(&without_noise.join("\n"));
    if collapsed.len() <= MAX_TEXT_CHARS {
        return collapsed;
    }
    // Truncate on a char boundary: `MAX_TEXT_CHARS - 300` can land mid-codepoint on UTF-8 text,
    // and slicing there would panic.
    let budget = MAX_TEXT_CHARS.saturating_sub(300);
    let mut cut = budget.min(collapsed.len());
    while cut > 0 && !collapsed.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = collapsed.get(..cut).unwrap_or_default();
    let dropped = text.len().saturating_sub(head.len());
    format!(
        "{head}\n\n... [truncated {dropped} chars by the nullrouter MCP bridge. The result is \
         large; narrow it down or act on one of the refs shown above]"
    )
}

/// Filter one JSON-RPC line, leaving anything that is not an oversized text result untouched.
pub(crate) fn frame(line: &str) -> String {
    let Ok(mut message) = serde_json::from_str::<serde_json::Value>(line) else {
        return line.to_owned();
    };
    let Some(content) = message
        .get_mut("result")
        .and_then(|result| result.get_mut("content"))
        .and_then(serde_json::Value::as_array_mut)
    else {
        return line.to_owned();
    };

    let mut mutated = false;
    for item in content {
        if item.get("type").and_then(serde_json::Value::as_str) != Some("text") {
            continue;
        }
        let Some(text) = item.get("text").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let filtered = smart_filter_text(text);
        if filtered != text {
            item["text"] = serde_json::Value::String(filtered);
            mutated = true;
        }
    }

    if mutated {
        serde_json::to_string(&message).unwrap_or_else(|_| line.to_owned())
    } else {
        line.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{COLLAPSE_THRESHOLD, MAX_TEXT_CHARS, frame, smart_filter_text};

    #[test]
    fn a_frame_that_is_not_json_passes_through() {
        for raw in ["", "{", "not json at all", "[1,2"] {
            assert_eq!(frame(raw), raw);
        }
    }

    #[test]
    fn ids_and_errors_are_never_rewritten() {
        let line = r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32601,"message":"no such tool"}}"#;
        assert_eq!(frame(line), line);
    }

    #[test]
    fn a_small_text_result_is_untouched() {
        let line = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"ok"}]}}"#;
        assert_eq!(frame(line), line);
    }

    #[test]
    fn non_text_content_is_untouched_however_large() {
        let blob = "A".repeat(MAX_TEXT_CHARS * 2);
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"content":[{{"type":"image","data":"{blob}"}}]}}}}"#
        );
        assert_eq!(frame(&line), line);
    }

    #[test]
    fn a_long_repeated_group_collapses_and_says_so() {
        let mut text = String::new();
        for index in 0..COLLAPSE_THRESHOLD + 20 {
            text.push_str(&format!("  - listitem \"row {index}\"\n"));
        }
        // Padded past the filter floor so the collapse path is the one under test.
        text.push_str(&"x".repeat(3_000));
        let filtered = smart_filter_text(&text);
        assert!(filtered.contains("items omitted by the nullrouter MCP bridge"));
        assert!(filtered.contains("row 0"), "head must survive");
        assert!(
            filtered.contains(&format!("row {}", COLLAPSE_THRESHOLD + 19)),
            "tail must survive: {filtered}"
        );
        assert!(filtered.len() < text.len());
    }

    #[test]
    fn truncation_never_splits_a_codepoint() {
        // A multi-byte char repeated past the ceiling puts a boundary hazard at the cut point.
        let text = "é".repeat(MAX_TEXT_CHARS);
        let filtered = smart_filter_text(&text);
        assert!(filtered.contains("truncated"));
        // Reaching here at all means no panic; this pins the output is still valid UTF-8 text.
        assert!(filtered.chars().count() > 0);
    }

    #[test]
    fn noise_nodes_are_dropped_only_above_the_floor() {
        let small = "- generic:\n- text: \"\"\n- real: kept";
        assert_eq!(smart_filter_text(small), small, "below the floor, verbatim");

        let padded = format!(
            "- generic:\n- text: \"\"\n- real: kept\n{}",
            "y".repeat(2_500)
        );
        let filtered = smart_filter_text(&padded);
        assert!(!filtered.contains("- generic:"));
        assert!(filtered.contains("- real: kept"));
    }
}
