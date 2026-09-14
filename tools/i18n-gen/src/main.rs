//! Generates the dashboard's locale files from the message keys used in its source.
//!
//! Run from the repository root:
//!
//! ```text
//! cargo run -p nullrouter-i18n-gen              # write the locale files
//! cargo run -p nullrouter-i18n-gen -- --check   # fail if they are out of date (for CI)
//! ```
//!
//! # Why this exists
//!
//! Hand-maintained locale files drift silently. A key renamed in Rust leaves a dead entry in 35
//! files and a missing entry the UI then renders as a raw key, and nothing fails a build. This tool
//! makes the source the single authority: `en-US.json` is derived from the keys actually present in
//! `apps/dashboard-leptos/src`, and every other locale is regenerated against that key set.
//!
//! # The format change it bridges
//!
//! The pre-existing locale files were keyed by English source sentence, which is what the previous
//! dashboard looked translations up by:
//!
//! ```json
//! { "Cancel": "Annuler", "Providers": "Fournisseurs" }
//! ```
//!
//! The current dashboard keys by `section.key` instead, because two identical English strings in
//! different contexts often need different translations, and a sentence used as a key cannot
//! express that. Rather than discard 34 languages of existing work, the generator bridges the two:
//! for each key, it takes the English text, looks *that* up in the old sentence-keyed file, and
//! writes the result under the new key. A sentence the old file did not cover is left untranslated
//! rather than guessed at.

// This is a command-line tool, so stdout and stderr are its interface rather than a debugging
// leftover. The workspace warns on `print_stdout` because the services must log through `tracing`.
#![allow(clippy::print_stdout, reason = "stdout is this tool's output")]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Where the dashboard's Rust source lives, relative to the repository root.
const SOURCE_DIR: &str = "apps/dashboard-leptos/src";

/// Where locale files are written. Matches the host's `/i18n/literals/{tag}.json` route.
const LOCALE_DIR: &str = "services/dashboard-actix/static/i18n/literals";

/// The locale the source is written in. Its file is the authority for the key set.
const SOURCE_LOCALE: &str = "en-US";

/// The old branding, and what it is now.
///
/// Load-bearing in one direction only: the left-hand strings must keep this spelling because they
/// are matched against translation files that still carry the old name, and both capitalisations
/// appear in that prose. A translation carried over from such a file is rewritten on the way
/// through, so a locale nobody has retranslated still reads correctly.
///
/// This applies to translated prose only. It does *not* touch the config-marker strings elsewhere
/// in the workspace, which are wire format: those have to keep their spelling to recognise config
/// files written by another program, and are documented as such where they are declared.
const REBRAND: [(&str, &str); 2] = [("9Router", "nullrouter"), ("9router", "nullrouter")];

fn main() -> ExitCode {
    let check_only = std::env::args().any(|argument| argument == "--check");

    let root = match repository_root() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("i18n-gen: {error}");
            return ExitCode::FAILURE;
        }
    };

    match run(&root, check_only) {
        Ok(Outcome::UpToDate) => {
            println!("i18n-gen: locale files are up to date");
            ExitCode::SUCCESS
        }
        Ok(Outcome::Written(count)) => {
            println!("i18n-gen: wrote {count} locale file(s)");
            ExitCode::SUCCESS
        }
        Ok(Outcome::Stale(paths)) => {
            eprintln!("i18n-gen: {} locale file(s) are out of date:", paths.len());
            for path in &paths {
                eprintln!("  {path}");
            }
            eprintln!("run `cargo run -p nullrouter-i18n-gen` to regenerate");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("i18n-gen: {error}");
            ExitCode::FAILURE
        }
    }
}

enum Outcome {
    UpToDate,
    Written(usize),
    Stale(Vec<String>),
}

/// The repository root, located from this crate's manifest rather than the working directory so the
/// tool behaves the same however it is invoked.
fn repository_root() -> Result<PathBuf, String> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("cannot find repository root above {}", manifest.display()))
}

fn run(root: &Path, check_only: bool) -> Result<Outcome, String> {
    let source_dir = root.join(SOURCE_DIR);
    let locale_dir = root.join(LOCALE_DIR);

    let keys = collect_keys(&source_dir)?;
    if keys.is_empty() {
        return Err(format!(
            "found no message keys under {}",
            source_dir.display()
        ));
    }

    // The English file is the authority for what each key *says*. Keys present in source but absent
    // here are reported rather than invented: only a human can write the English text.
    let source_path = locale_dir.join(format!("{SOURCE_LOCALE}.json"));
    let english = read_map(&source_path)?;

    let missing: Vec<&String> = keys
        .iter()
        .filter(|key| !english.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        let mut listing = String::new();
        for key in &missing {
            let _ = writeln!(listing, "  {key}");
        }
        return Err(format!(
            "{} key(s) used in source but absent from {}:\n{listing}",
            missing.len(),
            source_path.display(),
        ));
    }

    let mut stale = Vec::new();
    let mut written = 0;

    for locale_path in locale_files(&locale_dir)? {
        let tag = locale_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("unreadable locale filename: {}", locale_path.display()))?
            .to_owned();

        let rendered = if tag == SOURCE_LOCALE {
            // Prune keys no longer used in source, keep the rest verbatim.
            render(
                &keys
                    .iter()
                    .filter_map(|key| english.get(key).map(|text| (key.clone(), text.clone())))
                    .collect(),
            )
        } else {
            let existing = read_map(&locale_path)?;
            render(&translate(&keys, &english, &existing))
        };

        let current = fs::read_to_string(&locale_path).unwrap_or_default();
        if current == rendered {
            continue;
        }

        if check_only {
            stale.push(locale_path.display().to_string());
        } else {
            fs::write(&locale_path, &rendered)
                .map_err(|error| format!("writing {}: {error}", locale_path.display()))?;
            written += 1;
        }
    }

    if !stale.is_empty() {
        return Ok(Outcome::Stale(stale));
    }
    if written == 0 {
        return Ok(Outcome::UpToDate);
    }
    Ok(Outcome::Written(written))
}

/// Build one locale's table.
///
/// `existing` is the old sentence-keyed map. For each key, the English text is what gets looked up
/// there. A sentence with no entry falls back to the English text: a partially translated locale
/// then shows English for the gaps, which is readable, rather than a raw key, which is not.
fn translate(
    keys: &BTreeSet<String>,
    english: &BTreeMap<String, String>,
    existing: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    keys.iter()
        .filter_map(|key| {
            let source_text = english.get(key)?;
            // Two shapes can be present: an already-migrated file keyed by `section.key`, or an
            // original keyed by English sentence. Prefer the migrated entry so regenerating is
            // idempotent and does not undo a translation of a newly added key.
            let translated = existing
                .get(key)
                .or_else(|| existing.get(source_text))
                .map_or_else(|| source_text.clone(), |text| rebrand(text));
            Some((key.clone(), translated))
        })
        .collect()
}

/// Replace the old branding in translated prose. See [`REBRAND`].
fn rebrand(text: &str) -> String {
    let mut out = text.to_owned();
    for (from, to) in REBRAND {
        if out.contains(from) {
            out = out.replace(from, to);
        }
    }
    out
}

/// Serialize a table as stable, pretty-printed JSON with a trailing newline.
///
/// `BTreeMap` ordering keeps the output deterministic, so regenerating produces no diff when
/// nothing changed -- which is what makes `--check` meaningful.
fn render(table: &BTreeMap<String, String>) -> String {
    let mut out = String::from("{\n");
    let last = table.len().saturating_sub(1);
    for (index, (key, value)) in table.iter().enumerate() {
        let key = serde_json::to_string(key).unwrap_or_else(|_| String::from("\"\""));
        let value = serde_json::to_string(value).unwrap_or_else(|_| String::from("\"\""));
        let comma = if index == last { "" } else { "," };
        let _ = writeln!(out, "  {key}: {value}{comma}");
    }
    out.push_str("}\n");
    out
}

/// Every message key used in the dashboard's source.
///
/// Matches `locale.get("…")` and `locale.fmt("…"`, which are the only two ways a message is looked
/// up. A key built at runtime by concatenation would be invisible here, which is the reason not to
/// build one.
fn collect_keys(source_dir: &Path) -> Result<BTreeSet<String>, String> {
    let mut keys = BTreeSet::new();
    for file in rust_files(source_dir)? {
        let raw = fs::read_to_string(&file)
            .map_err(|error| format!("reading {}: {error}", file.display()))?;
        let text = strip_non_shipping(&raw);
        for call in ["get(\"", "fmt(\""] {
            let mut rest = text.as_str();
            while let Some(start) = rest.find(call) {
                let after = &rest[start + call.len()..];
                match after.find('"') {
                    Some(end) => {
                        let key = &after[..end];
                        // Message keys are namespaced. This is also what keeps unrelated `get("…")`
                        // calls -- a header lookup, a query parameter -- out of the key set.
                        if key.contains('.') && !key.contains(' ') && !key.contains('/') {
                            keys.insert(key.to_owned());
                        }
                        rest = &after[end..];
                    }
                    None => break,
                }
            }
        }
    }
    Ok(keys)
}

/// Drop the parts of a source file that do not ship: doc comments and `#[cfg(test)]` modules.
///
/// Without this the key set picks up fixtures and documentation examples. The first real run of this
/// tool reported three such keys -- `missing.key` from a test asserting the fallback, and a
/// `settings.account_created` from a doc comment showing how `fmt` is called -- and would have
/// written all three into 35 locale files as though the UI used them.
///
/// Test modules are found by brace-matching from `mod tests {`, so a nested block inside one does
/// not end the skip early.
fn strip_non_shipping(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut depth: i32 = 0;

    for line in source.lines() {
        let trimmed = line.trim_start();

        if depth > 0 {
            depth += i32::try_from(line.matches('{').count()).unwrap_or(0);
            depth -= i32::try_from(line.matches('}').count()).unwrap_or(0);
            continue;
        }

        // `mod tests {` on one line, which is how rustfmt writes it.
        if trimmed.starts_with("mod tests") && line.contains('{') {
            depth = 1;
            depth += i32::try_from(line.matches('{').count().saturating_sub(1)).unwrap_or(0);
            depth -= i32::try_from(line.matches('}').count()).unwrap_or(0);
            continue;
        }

        // Doc comments carry usage examples, which are not usage.
        if trimmed.starts_with("///") || trimmed.starts_with("//!") {
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }

    out
}

/// Every `.rs` file under a directory, recursively.
fn rust_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];

    while let Some(current) = pending.pop() {
        let entries = fs::read_dir(&current)
            .map_err(|error| format!("reading {}: {error}", current.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("reading {}: {error}", current.display()))?;
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }

    found.sort();
    Ok(found)
}

/// Every locale file, sorted.
fn locale_files(locale_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(locale_dir)
        .map_err(|error| format!("reading {}: {error}", locale_dir.display()))?;

    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("reading {}: {error}", locale_dir.display()))?;
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            found.push(path);
        }
    }

    found.sort();
    Ok(found)
}

/// Read a locale file as a flat string map.
fn read_map(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let text =
        fs::read_to_string(path).map_err(|error| format!("reading {}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("parsing {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{rebrand, render, strip_non_shipping, translate};
    use std::collections::{BTreeMap, BTreeSet};

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn keys(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|k| (*k).to_owned()).collect()
    }

    #[test]
    fn translations_are_carried_over_by_english_sentence() {
        // The whole point of the bridge: the old file knows "Cancel" -> "Annuler" and has never
        // heard of "action.cancel".
        let english = map(&[("action.cancel", "Cancel")]);
        let existing = map(&[("Cancel", "Annuler")]);
        let result = translate(&keys(&["action.cancel"]), &english, &existing);
        assert_eq!(
            result.get("action.cancel").map(String::as_str),
            Some("Annuler")
        );
    }

    #[test]
    fn an_untranslated_sentence_falls_back_to_english() {
        // English is readable; a raw key is not.
        let english = map(&[("nav.usage", "Usage")]);
        let result = translate(&keys(&["nav.usage"]), &english, &map(&[]));
        assert_eq!(result.get("nav.usage").map(String::as_str), Some("Usage"));
    }

    #[test]
    fn a_migrated_entry_wins_over_the_sentence_lookup() {
        // Once a file has been regenerated it is keyed by `section.key`. Preferring that entry is
        // what makes regeneration idempotent instead of reverting newer translations.
        let english = map(&[("action.save", "Save")]);
        let existing = map(&[("action.save", "Sauvegarder"), ("Save", "Enregistrer")]);
        let result = translate(&keys(&["action.save"]), &english, &existing);
        assert_eq!(
            result.get("action.save").map(String::as_str),
            Some("Sauvegarder")
        );
    }

    #[test]
    fn keys_absent_from_source_are_dropped() {
        // A renamed key must not leave a dead entry behind in 35 files.
        let english = map(&[("nav.usage", "Usage")]);
        let existing = map(&[("nav.usage", "Utilisation"), ("nav.removed", "Supprimé")]);
        let result = translate(&keys(&["nav.usage"]), &english, &existing);
        assert_eq!(result.len(), 1);
        assert!(!result.contains_key("nav.removed"));
    }

    #[test]
    fn a_key_missing_from_english_is_skipped_not_invented() {
        let english = map(&[]);
        let result = translate(&keys(&["nav.usage"]), &english, &map(&[]));
        assert!(result.is_empty());
    }

    #[test]
    fn legacy_branding_is_rewritten_in_both_spellings() {
        assert_eq!(rebrand("9Router Hub"), "nullrouter Hub");
        assert_eq!(rebrand("via 9router"), "via nullrouter");
        assert_eq!(rebrand("9Router and 9router"), "nullrouter and nullrouter");
    }

    #[test]
    fn unrelated_text_is_untouched() {
        assert_eq!(rebrand("Annuler"), "Annuler");
    }

    #[test]
    fn legacy_branding_is_rewritten_when_carrying_a_translation_over() {
        let english = map(&[("mitm.notice", "Restart 9Router as Administrator")]);
        let existing = map(&[("Restart 9Router as Administrator", "9Router 재시작")]);
        let result = translate(&keys(&["mitm.notice"]), &english, &existing);
        assert_eq!(
            result.get("mitm.notice").map(String::as_str),
            Some("nullrouter 재시작")
        );
    }

    #[test]
    fn output_is_stable_and_newline_terminated() {
        // `--check` compares rendered output against the file byte for byte, so the rendering has to
        // be deterministic and match what was written last time.
        let table = map(&[("b.key", "second"), ("a.key", "first")]);
        let rendered = render(&table);
        assert_eq!(
            rendered,
            "{\n  \"a.key\": \"first\",\n  \"b.key\": \"second\"\n}\n"
        );
        assert_eq!(render(&table), rendered);
    }

    #[test]
    fn rendering_escapes_json() {
        let table = map(&[("quote.key", "say \"hello\"")]);
        assert!(render(&table).contains("\\\"hello\\\""));
    }

    #[test]
    fn an_empty_table_still_renders_valid_json() {
        assert_eq!(render(&map(&[])), "{\n}\n");
    }

    #[test]
    fn test_modules_are_not_scanned_for_keys() {
        // The bug this catches was real: the first run against the actual source reported
        // `missing.key` from a fallback test as though the UI used it.
        let source = "\
fn real() { locale.get(\"nav.usage\"); }

mod tests {
    fn fixture() { locale.get(\"fixture.only\"); }
}
";
        let stripped = strip_non_shipping(source);
        assert!(stripped.contains("nav.usage"));
        assert!(!stripped.contains("fixture.only"));
    }

    #[test]
    fn doc_comments_are_not_scanned_for_keys() {
        let source = "\
/// Call as `locale.fmt(\"doc.example\", &[])`.
//! Module docs mentioning locale.get(\"module.doc\").
fn real() { locale.get(\"nav.keys\"); }
";
        let stripped = strip_non_shipping(source);
        assert!(stripped.contains("nav.keys"));
        assert!(!stripped.contains("doc.example"));
        assert!(!stripped.contains("module.doc"));
    }

    #[test]
    fn a_nested_brace_does_not_end_the_test_skip_early() {
        let source = "\
mod tests {
    fn nested() {
        if true { locale.get(\"inner.fixture\"); }
    }
}
fn shipping() { locale.get(\"nav.logs\"); }
";
        let stripped = strip_non_shipping(source);
        assert!(!stripped.contains("inner.fixture"));
        assert!(stripped.contains("nav.logs"));
    }
}
