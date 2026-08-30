//! Hermes' `config.yaml`, edited one block at a time.
//!
//! Upstream edits this file with a regular expression rather than a YAML parser, and this port does
//! the same. That is not laziness in either place: a round trip through a YAML library rewrites the
//! user's whole file — quoting style, key order, comments, anchors — and Hermes' config is
//! hand-written. Replacing just the `model:` block leaves the rest byte-identical.
//!
//! Upstream's pattern is:
//!
//! ```text
//! /^model:[ \t]*\r?\n((?:[ \t]+.*\r?\n?|[ \t]*\r?\n)*)/m
//! ```
//!
//! which is "a line that is exactly `model:`, then every following line that is either indented or
//! blank". That is implemented here directly rather than with a regex crate: it is a handful of
//! line tests, and it avoids a dependency for one pattern.

/// The `model:` block's extent in `text`, as a byte range, if there is one.
///
/// Returns the range from the start of the `model:` line to the end of the block, so that both a
/// replacement and a removal can use it.
fn model_block(text: &str) -> Option<std::ops::Range<usize>> {
    let mut offset = 0_usize;
    let mut start = None;

    for line in text.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();

        let bare = line.trim_end_matches(['\n', '\r']);
        match start {
            None => {
                // The header must be exactly `model:` plus optional trailing tabs and spaces —
                // upstream anchors with `^model:[ \t]*$`, so `model: gpt-4` is not a block.
                if bare.strip_prefix("model:").is_some_and(|rest| {
                    rest.chars().all(|character| character == ' ' || character == '\t')
                }) {
                    start = Some(line_start);
                }
            }
            Some(_) => {
                // The block continues through indented and blank lines.
                let indented = bare.starts_with(' ') || bare.starts_with('\t');
                let blank = bare.trim().is_empty();
                if !indented && !blank {
                    return start.map(|from| from..line_start);
                }
            }
        }
    }
    start.map(|from| from..text.len())
}

/// The YAML block upstream writes for a model.
///
/// `api_key: ${OPENAI_API_KEY}` is a literal reference, not an interpolation: Hermes expands it
/// from the environment or from `~/.hermes/.env`, which is where the key is written. So the key
/// never appears in this file.
pub(crate) fn build_model_block(model: &str, base_url: &str) -> String {
    format!(
        "model:\n  default: \"{model}\"\n  provider: \"custom\"\n  base_url: \"{base_url}\"\n  \
         api_key: ${{OPENAI_API_KEY}}\n"
    )
}

/// Replace the `model:` block, or add one at the top.
pub(crate) fn upsert_model_block(text: &str, block: &str) -> String {
    match model_block(text) {
        Some(range) => {
            let mut output = String::with_capacity(text.len() + block.len());
            output.push_str(text.get(..range.start).unwrap_or_default());
            output.push_str(block);
            output.push_str(text.get(range.end..).unwrap_or_default());
            output
        }
        // Upstream prepends with a blank line between, so an existing config keeps its shape.
        None if text.is_empty() => block.to_owned(),
        None => format!("{block}\n{text}"),
    }
}

/// Remove the `model:` block, and any blank lines it leaves at the top.
pub(crate) fn remove_model_block(text: &str) -> String {
    let Some(range) = model_block(text) else {
        return text.to_owned();
    };
    let mut output = String::with_capacity(text.len());
    output.push_str(text.get(..range.start).unwrap_or_default());
    output.push_str(text.get(range.end..).unwrap_or_default());
    // Upstream's `.replace(/^\n+/, "")`.
    output.trim_start_matches('\n').to_owned()
}

/// Read back the fields upstream reports from the block.
///
/// Best-effort and shallow, matching upstream's own `parseModelBlock`: a simple `key: value` scan
/// with optional quotes.
pub(crate) fn parse_model_block(text: &str) -> Option<serde_json::Value> {
    let range = model_block(text)?;
    let body = text.get(range.clone())?;
    let mut map = serde_json::Map::new();
    for key in ["default", "provider", "base_url", "api_key"] {
        if let Some(value) = field(body, key) {
            map.insert(key.to_owned(), serde_json::Value::String(value));
        } else {
            // Reported as null rather than omitted, because upstream's object always has all four
            // keys and the dashboard reads them positionally.
            map.insert(key.to_owned(), serde_json::Value::Null);
        }
    }
    Some(serde_json::Value::Object(map))
}

/// One indented `key: value` from a block body.
fn field(body: &str, key: &str) -> Option<String> {
    body.lines()
        .filter(|line| line.starts_with(' ') || line.starts_with('\t'))
        .find_map(|line| {
            let trimmed = line.trim_start();
            let rest = trimmed.strip_prefix(key)?.strip_prefix(':')?;
            let value = rest.trim();
            let unquoted = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .or_else(|| {
                    value
                        .strip_prefix('\'')
                        .and_then(|value| value.strip_suffix('\''))
                })
                .unwrap_or(value)
                .trim();
            (!unquoted.is_empty()).then(|| unquoted.to_owned())
        })
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test assertions read clearer with expect than with error plumbing"
)]
mod tests {
    use super::{build_model_block, parse_model_block, remove_model_block, upsert_model_block};

    const OTHER: &str = "agent:\n  name: mine\n  tools:\n    - shell\n\nlogging:\n  level: debug\n";

    #[test]
    fn a_block_is_added_to_an_empty_file() {
        let block = build_model_block("cc/opus", "http://127.0.0.1:20128/v1");
        assert_eq!(upsert_model_block("", &block), block);
    }

    #[test]
    fn a_block_is_prepended_to_an_existing_config() {
        let block = build_model_block("m", "http://x/v1");
        let result = upsert_model_block(OTHER, &block);
        assert!(result.starts_with(&block), "{result}");
        // And the user's own config is untouched below it.
        assert!(result.contains(OTHER), "{result}");
    }

    #[test]
    fn replacing_a_block_leaves_the_rest_byte_identical() {
        // The property this module exists for.
        let first = build_model_block("old", "http://old/v1");
        let with_block = upsert_model_block(OTHER, &first);
        let second = build_model_block("new", "http://new/v1");
        let replaced = upsert_model_block(&with_block, &second);

        assert!(replaced.contains("new"), "{replaced}");
        assert!(!replaced.contains("old"), "{replaced}");
        assert!(
            replaced.contains("agent:\n  name: mine\n  tools:\n    - shell\n"),
            "the user's config must survive verbatim: {replaced}"
        );
        assert_eq!(
            replaced.matches("model:").count(),
            1,
            "a second block was added instead of replacing: {replaced}"
        );
    }

    #[test]
    fn a_block_in_the_middle_is_replaced_without_eating_what_follows() {
        let text = "logging:\n  level: debug\nmodel:\n  default: \"old\"\n  provider: \"custom\"\nagent:\n  name: mine\n";
        let block = build_model_block("new", "http://new/v1");
        let replaced = upsert_model_block(text, &block);
        assert!(replaced.starts_with("logging:\n  level: debug\n"), "{replaced}");
        assert!(replaced.contains("agent:\n  name: mine\n"), "{replaced}");
        assert!(replaced.contains("\"new\""), "{replaced}");
        assert!(!replaced.contains("\"old\""), "{replaced}");
    }

    #[test]
    fn a_scalar_model_key_is_not_a_block() {
        // `model: gpt-4` is a value, not a block header. Treating it as one would swallow every
        // following line into the replacement.
        let text = "model: gpt-4\nagent:\n  name: mine\n";
        let block = build_model_block("new", "http://new/v1");
        let result = upsert_model_block(text, &block);
        assert!(result.contains("model: gpt-4"), "the scalar must survive: {result}");
        assert!(result.contains("agent:\n  name: mine\n"), "{result}");
    }

    #[test]
    fn a_blank_line_inside_a_block_does_not_end_it() {
        // Upstream's pattern allows blank lines inside the block, so a config formatted with one
        // must not leave half a block behind.
        let text = "model:\n  default: \"old\"\n\n  provider: \"custom\"\nagent:\n  name: mine\n";
        let result = remove_model_block(text);
        assert_eq!(result, "agent:\n  name: mine\n", "{result:?}");
    }

    #[test]
    fn removing_a_block_leaves_no_leading_blank_lines() {
        let block = build_model_block("m", "http://x/v1");
        let with_block = upsert_model_block(OTHER, &block);
        assert_eq!(remove_model_block(&with_block), OTHER);
    }

    #[test]
    fn removing_when_there_is_no_block_changes_nothing() {
        assert_eq!(remove_model_block(OTHER), OTHER);
        assert_eq!(remove_model_block(""), "");
    }

    #[test]
    fn the_fields_read_back_out_of_a_block_we_wrote() {
        // The round trip that matters: the dashboard shows these values after an apply.
        let block = build_model_block("cc/claude-opus-4-7", "http://127.0.0.1:20128/v1");
        let parsed = parse_model_block(&block).expect("a block we just wrote should parse");
        assert_eq!(parsed["default"], "cc/claude-opus-4-7");
        assert_eq!(parsed["provider"], "custom");
        assert_eq!(parsed["base_url"], "http://127.0.0.1:20128/v1");
        // The key is a literal `${OPENAI_API_KEY}` reference, never the secret itself.
        assert_eq!(parsed["api_key"], "${OPENAI_API_KEY}");
    }

    #[test]
    fn the_api_key_is_never_written_into_the_yaml() {
        // It goes to `~/.hermes/.env`. A key inlined here would end up in whatever dotfile repo
        // the user keeps their config in.
        let block = build_model_block("m", "http://x/v1");
        assert!(!block.contains("sk-"), "{block}");
        assert!(block.contains("${OPENAI_API_KEY}"), "{block}");
    }

    #[test]
    fn single_quoted_and_bare_values_parse() {
        let text = "model:\n  default: 'mine'\n  provider: custom\n  base_url: http://x/v1\n";
        let parsed = parse_model_block(text).expect("parses");
        assert_eq!(parsed["default"], "mine");
        assert_eq!(parsed["provider"], "custom");
        assert_eq!(parsed["base_url"], "http://x/v1");
        // Absent keys are reported as null rather than missing.
        assert!(parsed["api_key"].is_null());
    }

    #[test]
    fn parsing_a_file_with_no_block_yields_nothing() {
        assert!(parse_model_block(OTHER).is_none());
    }

    #[test]
    fn crlf_line_endings_are_handled() {
        // Upstream's pattern allows `\r?\n`, and a Windows-authored config must not confuse the
        // block boundary.
        let text = "model:\r\n  default: \"old\"\r\nagent:\r\n  name: mine\r\n";
        let parsed = parse_model_block(text).expect("parses");
        assert_eq!(parsed["default"], "old");
        let removed = remove_model_block(text);
        assert_eq!(removed, "agent:\r\n  name: mine\r\n", "{removed:?}");
    }
}
