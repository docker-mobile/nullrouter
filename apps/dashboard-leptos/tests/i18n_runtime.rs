//! The runtime translation contract.
//!
//! Every assertion here guards a way the dashboard could silently lie: offering
//! a locale with no literal file, blanking a label because a key was missing, or
//! losing a stored locale because the cookie was not the first one in the header.

#![allow(
    clippy::panic,
    clippy::missing_const_for_fn,
    clippy::items_after_statements,
    clippy::redundant_closure,
    clippy::case_sensitive_file_extension_comparisons,
    reason = "integration-test file: the workspace `allow-*-in-tests` settings only reach `#[cfg(test)]` modules"
)]
use nullrouter_dashboard_wasm::i18n::{
    DEFAULT_LOCALE, LOCALE_COOKIE, Literals, cookie_value, is_supported_locale, literals_path,
    locale_from_cookies, locale_name, locales, lookup, normalize_locale, parse_literals,
};

fn literals(pairs: &[(&str, &str)]) -> Literals {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn locale_inventory_matches_upstream() {
    // Given: upstream's 35-locale list (34 literal files plus English).
    let ids = locales().iter().map(|option| option.id).collect::<Vec<_>>();

    // Then: the list is complete, ordered as upstream declares it, and free of
    // duplicates — a duplicate id would render two identical picker rows.
    assert_eq!(ids.len(), 35);
    assert_eq!(
        ids,
        vec![
            "en", "vi", "zh-CN", "zh-TW", "ja", "pt-BR", "pt-PT", "ko", "es", "de", "fr", "he",
            "ar", "ru", "pl", "cs", "nl", "tr", "uk", "tl", "id", "km", "th", "hi", "bn", "ur",
            "ro", "sv", "it", "el", "hu", "fi", "da", "no", "fa",
        ]
    );
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "duplicate locale id");

    // And: every locale names itself in its own script, so the picker is legible
    // to the person looking for their language.
    for option in locales() {
        assert!(!option.name.is_empty(), "{} has no endonym", option.id);
    }
    assert_eq!(locale_name("km"), Some("ខ្មែរ"));
    assert_eq!(locale_name("nope"), None);
}

#[test]
fn normalize_locale_accepts_exact_ids() {
    // Given: a canonical id. Then: it survives normalization unchanged.
    for option in locales() {
        assert_eq!(normalize_locale(option.id), option.id);
        assert!(is_supported_locale(option.id));
    }
}

#[test]
fn normalize_locale_ignores_case() {
    // Given: a cookie written with different casing than the canonical id.
    // Then: it still resolves, rather than silently reverting to English.
    assert_eq!(normalize_locale("EN"), "en");
    assert_eq!(normalize_locale("Fr"), "fr");
    assert_eq!(normalize_locale("zh-cn"), "zh-CN");
    assert_eq!(normalize_locale("ZH-TW"), "zh-TW");
    assert_eq!(normalize_locale("pt-br"), "pt-BR");
    assert_eq!(normalize_locale("PT-pt"), "pt-PT");
}

#[test]
fn normalize_locale_keeps_region_variants_distinct() {
    // Given: locales that differ only by region.
    // Then: each keeps its own identity. Collapsing zh-TW into zh-CN would show
    // a Traditional Chinese reader Simplified text, which is a wrong translation,
    // not a fallback.
    assert_eq!(normalize_locale("zh-CN"), "zh-CN");
    assert_eq!(normalize_locale("zh-TW"), "zh-TW");
    assert_ne!(normalize_locale("zh-TW"), normalize_locale("zh-CN"));
    assert_eq!(normalize_locale("pt-BR"), "pt-BR");
    assert_eq!(normalize_locale("pt-PT"), "pt-PT");
    assert_ne!(normalize_locale("pt-BR"), normalize_locale("pt-PT"));

    // And: bare `zh` is upstream's one alias, meaning Simplified.
    assert_eq!(normalize_locale("zh"), "zh-CN");
    assert_eq!(normalize_locale("ZH"), "zh-CN");
}

#[test]
fn normalize_locale_falls_back_to_english() {
    // Given: input that names no shipped locale — an unshipped region, a bare
    // language with no unregioned file, junk, or a path traversal attempt.
    // Then: English, because a wrong translation is worse than an untranslated
    // string, and because normalize's output is interpolated into a URL.
    for input in [
        "en-US",
        "de-AT",
        "pt",
        "klingon",
        "xx-YY",
        "../secrets",
        "zh-CN/../../etc/passwd",
        "en; DROP",
        "🙂",
    ] {
        assert_eq!(normalize_locale(input), DEFAULT_LOCALE, "{input}");
    }
    assert!(!is_supported_locale("en-US"));
    assert!(!is_supported_locale("zh"));
}

#[test]
fn normalize_locale_handles_empty_and_blank_input() {
    // Given: an absent or whitespace-only cookie value.
    // Then: English, with no attempt to fetch `/i18n/literals/.json`.
    assert_eq!(normalize_locale(""), DEFAULT_LOCALE);
    assert_eq!(normalize_locale("   "), DEFAULT_LOCALE);
    assert_eq!(normalize_locale("\t\n"), DEFAULT_LOCALE);
    // And: surrounding whitespace does not stop a real locale from resolving.
    assert_eq!(normalize_locale("  ja  "), "ja");
}

#[test]
fn cookie_parsing_finds_locale_anywhere_in_the_header() {
    // Given: a realistic header where the locale is neither first nor last.
    let header = "theme=dark; session=abc123; locale=zh-TW; sidebar=open";

    // Then: it is found by name, not by position.
    assert_eq!(
        cookie_value(header, LOCALE_COOKIE).as_deref(),
        Some("zh-TW")
    );
    assert_eq!(locale_from_cookies(header), Some("zh-TW"));

    // And: leading position and inconsistent spacing both work.
    assert_eq!(locale_from_cookies("locale=ja; theme=dark"), Some("ja"));
    assert_eq!(locale_from_cookies("theme=dark;locale=de;x=1"), Some("de"));
    assert_eq!(locale_from_cookies("  locale = fr  "), None);
}

#[test]
fn cookie_parsing_reports_absent_locale() {
    // Given: headers with no locale cookie at all.
    // Then: `None`, so the caller keeps its default instead of being handed a
    // value that was never chosen.
    for header in ["", "theme=dark; session=abc", "locales=fr", "mylocale=fr"] {
        assert_eq!(cookie_value(header, LOCALE_COOKIE), None, "{header}");
        assert_eq!(locale_from_cookies(header), None, "{header}");
    }
    // An empty value is treated as absent, not as an empty locale.
    assert_eq!(locale_from_cookies("locale=; theme=dark"), None);
}

#[test]
fn cookie_parsing_decodes_percent_escapes_and_survives_junk() {
    // Given: a cookie written by upstream's Next.js route, which percent-encodes.
    assert_eq!(locale_from_cookies("locale=zh%2DCN"), Some("zh-CN"));
    // And: a malformed escape is left as written rather than dropping the value,
    // then normalized like any other unknown input.
    assert_eq!(
        cookie_value("locale=zh%2", LOCALE_COOKIE).as_deref(),
        Some("zh%2")
    );
    assert_eq!(locale_from_cookies("locale=zh%2"), Some(DEFAULT_LOCALE));
    // And: a stored locale that is no longer supported degrades to English.
    assert_eq!(locale_from_cookies("locale=klingon"), Some(DEFAULT_LOCALE));
}

#[test]
fn translation_returns_the_localized_string_on_a_hit() {
    // Given: a loaded literal map for a non-English locale.
    let map = literals(&[("Save", "Lưu"), ("Cancel", "Hủy")]);

    // Then: the translation replaces the English source text.
    assert_eq!(lookup("vi", &map, "Save"), "Lưu");
    assert_eq!(lookup("vi", &map, "Cancel"), "Hủy");
    // And: lookup is by trimmed text, so layout padding does not cause a miss.
    assert_eq!(lookup("vi", &map, "  Save  "), "Lưu");
}

#[test]
fn translation_falls_back_to_the_original_string_on_a_miss() {
    // Given: a locale that translates some strings but not all — the normal
    // state of a partially translated locale.
    let map = literals(&[("Save", "Lưu")]);

    // Then: a missing key renders the English source. Not an empty string, and
    // not the key name: an untranslated button is usable, a blank one is not.
    let missing = lookup("vi", &map, "Rotate credentials");
    assert_eq!(missing, "Rotate credentials");
    assert!(!missing.is_empty());

    // And: the fallback preserves the caller's exact string, whitespace included.
    assert_eq!(lookup("vi", &map, "  Rotate  "), "  Rotate  ");

    // And: an empty translation counts as a miss rather than blanking the label.
    let blank = literals(&[("Save", "")]);
    assert_eq!(lookup("vi", &blank, "Save"), "Save");

    // And: an entirely unloaded map is all misses, never all blanks.
    let empty = Literals::new();
    assert_eq!(lookup("vi", &empty, "Save"), "Save");
    assert_eq!(lookup("vi", &empty, "Providers"), "Providers");
}

#[test]
fn translation_leaves_empty_and_whitespace_input_untouched() {
    // Given: a map that would happily translate a blank key.
    let map = literals(&[("", "should never render"), (" ", "nor this")]);

    // Then: blank input is returned verbatim. Whitespace in the view is spacing,
    // not copy, and must not be replaced by a translation.
    assert_eq!(lookup("vi", &map, ""), "");
    assert_eq!(lookup("vi", &map, " "), " ");
    assert_eq!(lookup("vi", &map, "\n\t"), "\n\t");
}

#[test]
fn english_short_circuits_before_any_lookup() {
    // Given: an English locale whose map nonetheless contains a matching key —
    // a stale map left over from a previous locale, or a corrupted file.
    let map = literals(&[("Save", "NOT ENGLISH")]);

    // Then: English returns its source text without consulting the map at all.
    // English is the source language; nothing may rewrite it.
    assert_eq!(lookup(DEFAULT_LOCALE, &map, "Save"), "Save");
    assert_eq!(lookup("en", &map, "  Save  "), "  Save  ");

    // And: the same map does translate for a non-English locale, proving the
    // short-circuit is what suppressed it rather than a failed lookup.
    assert_eq!(lookup("vi", &map, "Save"), "NOT ENGLISH");
}

#[test]
fn literal_files_parse_into_flat_string_maps() {
    // Given: a literal file shaped like the ones upstream ships.
    let parsed = parse_literals(r#"{"Save":"Lưu","Cancel":"Hủy"}"#);
    assert_eq!(
        parsed.as_ref().map(std::collections::BTreeMap::len),
        Some(2)
    );
    assert_eq!(
        parsed.and_then(|map| map.get("Save").cloned()),
        Some("Lưu".to_owned())
    );

    // And: a non-string value costs that one entry its translation instead of
    // discarding the whole locale.
    let mixed = parse_literals(r#"{"Save":"Lưu","Count":3,"Nested":{"a":"b"}}"#);
    assert_eq!(mixed.as_ref().map(std::collections::BTreeMap::len), Some(1));
    assert!(mixed.is_some_and(|map| map.contains_key("Save")));

    // And: a body that is not a JSON object is a parse failure, which the caller
    // renders as English rather than as an empty page.
    assert!(parse_literals("not json").is_none());
    assert!(parse_literals("[1,2,3]").is_none());
    assert!(parse_literals("\"just a string\"").is_none());
    assert_eq!(parse_literals("{}").map(|map| map.len()), Some(0));
}

#[test]
fn literal_paths_are_derived_from_canonical_ids_only() {
    // Given: every supported locale.
    // Then: its path is the flat, predictable URL the host serves. Because the
    // id is always a canonical list entry, no caller-supplied text ever reaches
    // this URL.
    assert_eq!(literals_path("zh-CN"), "/i18n/literals/zh-CN.json");
    assert_eq!(literals_path("ja"), "/i18n/literals/ja.json");
    for option in locales() {
        let path = literals_path(option.id);
        assert!(path.starts_with("/i18n/literals/"), "{path}");
        assert!(path.ends_with(".json"), "{path}");
        assert!(!path.contains(".."), "{path}");
    }
}
