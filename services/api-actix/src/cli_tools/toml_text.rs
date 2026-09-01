//! TOML edited as text, one section at a time.
//!
//! Grok Build's config is the one file this port cannot round-trip through a parser. Upstream
//! records the user's previous default model in a **comment** —
//! `# 9router-prev-default = "grok-4"` — so that a later revoke can put it back. A parse and
//! re-serialise drops comments, which would silently turn every revoke into "reset to the built-in
//! default" and lose a setting the user chose.
//!
//! So the operations here work on the raw text, the way upstream's regexes do, and for the same
//! reason [`super::yaml_block`] does: everything not named is left byte-identical.
//!
//! # Section matching
//!
//! Upstream's pattern is
//!
//! ```text
//! /^\[<section>\][ \t]*\r?\n((?:(?!\[)[^\r\n]*\r?\n?)*)/m
//! ```
//!
//! — a header line that is exactly `[section]`, then every following line that does not start with
//! `[`. Note the header must match in full: `[model.9router]` is not found by looking for
//! `[model]`, and a nested `[model.9router.extra]` is a different section. That is implemented
//! directly rather than with a regex crate, which is a handful of line tests and no dependency.

use std::ops::Range;

/// The extent of a `[section]`, from the start of its header line to the end of its body.
fn section_range(text: &str, section: &str) -> Option<Range<usize>> {
    let header = format!("[{section}]");
    let mut offset = 0_usize;
    let mut start = None;

    for line in text.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let bare = line.trim_end_matches(['\n', '\r']);

        match start {
            None => {
                // Exactly the header, plus optional trailing spaces and tabs.
                if let Some(rest) = bare.strip_prefix(&header)
                    && rest.chars().all(|character| character == ' ' || character == '\t')
                {
                    start = Some(line_start);
                }
            }
            Some(_) => {
                // The body runs until the next section header.
                if bare.starts_with('[') {
                    return start.map(|from| from..line_start);
                }
            }
        }
    }
    start.map(|from| from..text.len())
}

/// A section's body — everything after its header line.
fn body_of<'a>(text: &'a str, range: &Range<usize>) -> &'a str {
    let section = text.get(range.clone()).unwrap_or_default();
    match section.find('\n') {
        Some(end) => section.get(end + 1..).unwrap_or_default(),
        None => "",
    }
}

/// A quoted string field from a section, if it has one.
pub(crate) fn get_field(text: &str, section: &str, key: &str) -> Option<String> {
    let range = section_range(text, section)?;
    field_lines(body_of(text, &range), key).next().and_then(|line| {
        let value = line.split_once('=')?.1.trim();
        // Only the double-quoted form, which is what upstream's pattern accepts and what it
        // writes. A bare or single-quoted value is left to be read as absent rather than guessed
        // at, so this port and upstream agree on the same file.
        value
            .strip_prefix('"')
            .and_then(|value| value.split('"').next())
            .map(str::to_owned)
    })
}

/// The lines in a body that assign `key`.
fn field_lines<'a>(body: &'a str, key: &'a str) -> impl Iterator<Item = &'a str> {
    body.lines().filter(move |line| {
        line.split_once('=').is_some_and(|(name, _)| name.trim() == key)
    })
}

/// Set a quoted string field, creating the section if it is not there.
pub(crate) fn set_field(text: &str, section: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {}", quoted(value));
    let Some(range) = section_range(text, section) else {
        // Appended, with a blank line before it, matching upstream's `setSectionField`.
        let mut output = ended_with_newline(text);
        output.push_str(&format!("\n[{section}]\n{line}\n"));
        return output;
    };

    let body = body_of(text, &range);
    let replaced = if field_lines(body, key).next().is_some() {
        body.lines()
            .map(|existing| {
                if existing.split_once('=').is_some_and(|(name, _)| name.trim() == key) {
                    line.clone()
                } else {
                    existing.to_owned()
                }
            })
            .collect::<Vec<String>>()
            .join("\n")
            + "\n"
    } else {
        // Upstream prepends a new field rather than appending it.
        format!("{line}\n{body}")
    };
    splice(text, &range, &format!("[{section}]\n{replaced}"))
}

/// Remove a field, and the section itself once its body is blank.
pub(crate) fn delete_field(text: &str, section: &str, key: &str) -> String {
    let Some(range) = section_range(text, section) else {
        return text.to_owned();
    };
    let body = body_of(text, &range);
    let kept: Vec<&str> = body
        .lines()
        .filter(|line| !line.split_once('=').is_some_and(|(name, _)| name.trim() == key))
        .collect();
    if kept.iter().all(|line| line.trim().is_empty()) {
        return collapse_blank_runs(&splice(text, &range, ""));
    }
    let body = kept.join("\n") + "\n";
    splice(text, &range, &format!("[{section}]\n{body}"))
}

/// Replace a whole section, or append it when absent.
pub(crate) fn upsert_section(text: &str, section: &str, fields: &[String]) -> String {
    let mut rendered = format!("[{section}]\n");
    for field in fields {
        rendered.push_str(field);
        rendered.push('\n');
    }
    match section_range(text, section) {
        Some(range) => splice(text, &range, &rendered),
        None => {
            let mut output = ended_with_newline(text);
            output.push('\n');
            output.push_str(&rendered);
            output
        }
    }
}

/// Remove a section entirely.
pub(crate) fn remove_section(text: &str, section: &str) -> String {
    match section_range(text, section) {
        Some(range) => collapse_blank_runs(&splice(text, &range, "")),
        None => text.to_owned(),
    }
}

/// Whether a section is present at all.
pub(crate) fn has_section(text: &str, section: &str) -> bool {
    section_range(text, section).is_some()
}

/// Insert a comment line, before `anchor`'s section when there is one.
///
/// The position matters only for legibility — the marker is read by prefix wherever it sits — but
/// upstream puts it directly above the section it describes, and a config a user opens should look
/// the same whichever router wrote it.
pub(crate) fn insert_marker(text: &str, anchor: &str, marker: &str) -> String {
    match section_range(text, anchor) {
        Some(range) => {
            let mut output = String::with_capacity(text.len() + marker.len());
            output.push_str(text.get(..range.start).unwrap_or_default());
            output.push_str(marker);
            output.push_str(text.get(range.start..).unwrap_or_default());
            output
        }
        None => {
            let mut output = ended_with_newline(text);
            output.push_str(marker);
            output
        }
    }
}

/// The value of a `# <name> = "value"` comment marker.
pub(crate) fn read_marker(text: &str, name: &str) -> Option<String> {
    let prefix = format!("# {name} = \"");
    text.lines().find_map(|line| {
        line.trim_end()
            .strip_prefix(&prefix)?
            .strip_suffix('"')
            .map(str::to_owned)
    })
}

/// Drop a `# <name> = ...` comment marker.
pub(crate) fn remove_marker(text: &str, name: &str) -> String {
    let prefix = format!("# {name} = ");
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim_end().starts_with(&prefix))
        .collect();
    let mut joined = kept.join("\n");
    if !joined.is_empty() && text.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// A TOML basic string, escaped the way `JSON.stringify` does — which is what upstream uses.
fn quoted(value: &str) -> String {
    serde_json::Value::String(value.to_owned()).to_string()
}

fn splice(text: &str, range: &Range<usize>, replacement: &str) -> String {
    let mut output = String::with_capacity(text.len() + replacement.len());
    output.push_str(text.get(..range.start).unwrap_or_default());
    output.push_str(replacement);
    output.push_str(text.get(range.end..).unwrap_or_default());
    output
}

fn ended_with_newline(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') {
        text.to_owned()
    } else {
        format!("{text}\n")
    }
}

/// Upstream's `.replace(/\n{3,}/g, "\n\n")`, so removing a section does not leave a gap.
pub(crate) fn collapse_blank_runs(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut newlines = 0_usize;
    for character in text.chars() {
        if character == '\n' {
            newlines += 1;
            if newlines <= 2 {
                output.push(character);
            }
        } else {
            newlines = 0;
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test assertions read clearer with expect than with error plumbing"
)]
mod tests {
    use super::{
        delete_field, get_field, has_section, insert_marker, read_marker, remove_marker,
        remove_section, set_field, upsert_section,
    };

    const USER_CONFIG: &str = "# my notes\ntheme = \"dark\"\n\n\
                               [model.grok-4]\nmodel = \"grok-4\"\nbase_url = \"https://x\"\n\n\
                               [models]\ndefault = \"grok-4\"\n";

    #[test]
    fn a_section_header_must_match_in_full() {
        // The trap this module exists to avoid: `[model]` and `[model.9router]` are different
        // sections, and matching by prefix would rewrite the wrong one.
        let text = "[model]\na = \"1\"\n\n[model.9router]\nb = \"2\"\n";
        assert_eq!(get_field(text, "model", "a").as_deref(), Some("1"));
        assert_eq!(get_field(text, "model.9router", "b").as_deref(), Some("2"));
        assert!(get_field(text, "model", "b").is_none());
        assert!(get_field(text, "model.9router", "a").is_none());
        assert!(!has_section(text, "model.9router.extra"));
    }

    #[test]
    fn a_section_body_ends_at_the_next_header() {
        let text = "[a]\none = \"1\"\n[b]\ntwo = \"2\"\n";
        assert_eq!(get_field(text, "a", "one").as_deref(), Some("1"));
        assert!(get_field(text, "a", "two").is_none());
    }

    #[test]
    fn setting_a_field_replaces_it_in_place_and_leaves_the_rest_alone() {
        let updated = set_field(USER_CONFIG, "models", "default", "9router");
        assert!(updated.contains("default = \"9router\""), "{updated}");
        assert!(!updated.contains("default = \"grok-4\""), "{updated}");
        // Everything else is byte-identical, which is the property the whole module is for.
        assert!(updated.starts_with("# my notes\ntheme = \"dark\"\n"), "{updated}");
        assert!(updated.contains("[model.grok-4]\nmodel = \"grok-4\""), "{updated}");
    }

    #[test]
    fn setting_a_field_in_a_missing_section_appends_one() {
        let updated = set_field("theme = \"dark\"\n", "models", "default", "9router");
        assert_eq!(updated, "theme = \"dark\"\n\n[models]\ndefault = \"9router\"\n");
        // And on an empty file.
        assert_eq!(set_field("", "models", "default", "x"), "\n[models]\ndefault = \"x\"\n");
    }

    #[test]
    fn deleting_the_last_field_takes_the_section_with_it() {
        let text = "[models]\ndefault = \"9router\"\n\n[other]\nkeep = \"1\"\n";
        let updated = delete_field(text, "models", "default");
        assert!(!updated.contains("[models]"), "{updated}");
        assert!(updated.contains("[other]\nkeep = \"1\""), "{updated}");
        // A section that still has fields is kept.
        let text = "[models]\ndefault = \"9router\"\nother = \"2\"\n";
        let updated = delete_field(text, "models", "default");
        assert!(updated.contains("[models]"), "{updated}");
        assert!(updated.contains("other = \"2\""), "{updated}");
    }

    #[test]
    fn a_section_is_replaced_rather_than_duplicated() {
        let first = upsert_section(
            USER_CONFIG,
            "model.9router",
            &["model = \"a\"".to_owned()],
        );
        let second = upsert_section(&first, "model.9router", &["model = \"b\"".to_owned()]);
        assert_eq!(
            second.matches("[model.9router]").count(),
            1,
            "a second section was added instead of replacing: {second}"
        );
        assert!(second.contains("model = \"b\""), "{second}");
        assert!(!second.contains("model = \"a\""), "{second}");
        // The user's own model section is untouched throughout.
        assert!(second.contains("[model.grok-4]\nmodel = \"grok-4\""), "{second}");
    }

    #[test]
    fn removing_a_section_leaves_no_widening_gap() {
        // Upstream collapses runs of blank lines, so repeated apply/revoke cycles do not push the
        // rest of the file down a line at a time.
        let text = "a = \"1\"\n\n[gone]\nx = \"1\"\n\n[kept]\ny = \"2\"\n";
        let updated = remove_section(text, "gone");
        assert!(!updated.contains("[gone]"), "{updated}");
        assert!(updated.contains("[kept]"), "{updated}");
        assert!(!updated.contains("\n\n\n"), "{updated:?}");
    }

    #[test]
    fn a_marker_survives_a_round_trip_and_is_read_back() {
        // The reason this module exists rather than a parser: the marker is a comment, and it has
        // to still be there when a later revoke looks for it.
        let text = insert_marker(
            USER_CONFIG,
            "model.9router",
            "# 9router-prev-default = \"grok-4\"\n",
        );
        assert_eq!(read_marker(&text, "9router-prev-default").as_deref(), Some("grok-4"));

        // Surviving an unrelated edit is the part that matters.
        let edited = set_field(&text, "models", "default", "9router");
        assert_eq!(
            read_marker(&edited, "9router-prev-default").as_deref(),
            Some("grok-4"),
            "an edit elsewhere must not drop the marker: {edited}"
        );

        let cleared = remove_marker(&edited, "9router-prev-default");
        assert!(read_marker(&cleared, "9router-prev-default").is_none(), "{cleared}");
        // And the config it was sitting in is otherwise intact.
        assert!(cleared.contains("theme = \"dark\""), "{cleared}");
        assert!(cleared.contains("default = \"9router\""), "{cleared}");
    }

    #[test]
    fn a_marker_goes_above_the_section_it_describes() {
        let text = insert_marker("[model.9router]\nmodel = \"a\"\n", "model.9router", "# m = \"1\"\n");
        assert!(text.starts_with("# m = \"1\"\n[model.9router]"), "{text:?}");
        // With no anchor it is appended rather than dropped.
        let text = insert_marker("theme = \"dark\"\n", "model.9router", "# m = \"1\"\n");
        assert_eq!(text, "theme = \"dark\"\n# m = \"1\"\n");
    }

    #[test]
    fn a_value_with_a_quote_in_it_is_escaped() {
        // A model name is not attacker-controlled, but an unescaped quote produces a file that
        // does not parse, and the failure would land on the user's next launch rather than here.
        let text = set_field("", "models", "default", "a\"b");
        assert!(text.contains(r#"default = "a\"b""#), "{text:?}");
    }

    #[test]
    fn crlf_line_endings_do_not_confuse_a_section_boundary() {
        let text = "[a]\r\none = \"1\"\r\n[b]\r\ntwo = \"2\"\r\n";
        assert_eq!(get_field(text, "a", "one").as_deref(), Some("1"));
        assert_eq!(get_field(text, "b", "two").as_deref(), Some("2"));
        assert!(get_field(text, "a", "two").is_none());
    }

    #[test]
    fn a_bare_or_single_quoted_value_reads_as_absent() {
        // Upstream's pattern only accepts the double-quoted form, and only writes that form.
        // Accepting more here would make this port read a value the dashboard upstream ships
        // does not.
        let text = "[models]\ndefault = grok-4\nother = 'grok-4'\n";
        assert!(get_field(text, "models", "default").is_none());
        assert!(get_field(text, "models", "other").is_none());
    }
}
