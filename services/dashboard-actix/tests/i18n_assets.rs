//! The shipped literal files, and the route that serves them.
//!
//! The dashboard translates by fetching `/i18n/literals/{locale}.json` at
//! runtime, so a locale offered in the picker with no served file is a language
//! that silently does nothing. These tests hold the shipped set and the locale
//! list to the same number, and check that the route cannot be walked out of.

use std::{collections::BTreeMap, path::PathBuf};

// `test` is aliased so the imported module does not shadow the built-in
// `#[test]` attribute used by the synchronous file-inventory tests below.
use actix_web::{App, http::StatusCode, test as actix_test};
use nullrouter_dashboard_host::DashboardConfig;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Every locale the dashboard offers, in upstream's order.
///
/// Duplicated from `nullrouter-dashboard-wasm` rather than imported: the host
/// does not depend on the wasm crate, and this test's job is to catch the two
/// lists drifting apart, which a shared constant would hide.
const LOCALES: [&str; 35] = [
    "en", "vi", "zh-CN", "zh-TW", "ja", "pt-BR", "pt-PT", "ko", "es", "de", "fr", "he", "ar", "ru",
    "pl", "cs", "nl", "tr", "uk", "tl", "id", "km", "th", "hi", "bn", "ur", "ro", "sv", "it", "el",
    "hu", "fi", "da", "no", "fa",
];

/// English is the source language and ships no literal file.
const UNTRANSLATED_LOCALE: &str = "en";

fn literals_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static/i18n/literals")
}

fn write_file(root: &std::path::Path, path: &str, contents: &[u8]) -> std::io::Result<()> {
    let destination = root.join(path);
    let parent = destination.parent().unwrap_or(root);
    std::fs::create_dir_all(parent)?;
    std::fs::write(destination, contents)
}

fn fixture_config() -> Result<(TempDir, DashboardConfig), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    write_file(
        root.path(),
        "i18n/literals/vi.json",
        r#"{"Save":"Lưu"}"#.as_bytes(),
    )?;
    write_file(root.path(), "secret.txt", b"do not serve me")?;
    let config = DashboardConfig::new(root.path());
    Ok((root, config))
}

#[test]
fn every_locale_except_english_ships_a_literal_file() {
    // Given: the locale list the picker renders.
    let dir = literals_dir();

    // Then: each non-English locale has a file at the exact name the runtime
    // requests. A missing file means that picker entry translates nothing.
    for locale in LOCALES {
        let path = dir.join(format!("{locale}.json"));
        if locale == UNTRANSLATED_LOCALE {
            assert!(
                !path.exists(),
                "English is the source language and must ship no literal file"
            );
            continue;
        }
        assert!(path.exists(), "{locale} has no literal file at {path:?}");
    }
}

#[test]
fn literal_file_count_matches_the_locale_list() -> TestResult {
    // Given: the files actually shipped.
    let mut shipped = std::fs::read_dir(literals_dir())?
        .filter_map(Result::ok)
        .map(|entry| PathBuf::from(entry.file_name()))
        .filter(|name| name.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|name| {
            name.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .collect::<Vec<_>>();
    shipped.sort_unstable();

    let mut expected = LOCALES
        .iter()
        .filter(|locale| **locale != UNTRANSLATED_LOCALE)
        .map(|locale| (*locale).to_owned())
        .collect::<Vec<_>>();
    expected.sort_unstable();

    // Then: the sets match exactly in both directions. An extra file is a locale
    // the picker cannot reach; a missing one is a picker entry that does nothing.
    assert_eq!(
        shipped.len(),
        34,
        "expected 34 literal files, got {shipped:?}"
    );
    assert_eq!(shipped, expected);
    Ok(())
}

#[test]
fn every_literal_file_is_a_flat_string_to_string_map() -> TestResult {
    // Given: each shipped literal file.
    for locale in LOCALES.iter().filter(|l| **l != UNTRANSLATED_LOCALE) {
        let path = literals_dir().join(format!("{locale}.json"));
        let body = std::fs::read_to_string(&path)?;

        // Then: it parses as a flat object of string keys to string values. The
        // runtime looks up by exact trimmed English text, so a nested object or
        // a non-string value is a key that can never match.
        let parsed: BTreeMap<String, serde_json::Value> = serde_json::from_str(&body)
            .map_err(|error| format!("{locale}.json is not a JSON object: {error}"))?;
        assert!(!parsed.is_empty(), "{locale}.json is empty");

        for (key, value) in &parsed {
            let text = value
                .as_str()
                .ok_or_else(|| format!("{locale}.json key {key:?} is not a string: {value}"))?;
            // And: neither side is blank. A blank key can never be looked up, and
            // a blank value would render an empty label instead of English.
            assert!(!key.trim().is_empty(), "{locale}.json has a blank key");
            assert!(
                !text.trim().is_empty(),
                "{locale}.json key {key:?} translates to blank"
            );
        }
    }
    Ok(())
}

#[test]
fn untrimmed_keys_stay_at_the_four_upstream_ships() -> TestResult {
    // Lookup trims before matching, so a key stored with surrounding whitespace
    // can never be hit. Upstream ships exactly one such key, `"Security
    // required: "`, in its four largest files, and it is equally dead there —
    // upstream's runtime also indexes by the trimmed string.
    //
    // These files are copied verbatim, so the dead key is copied too. Rather
    // than silently rewriting upstream data, this pins the known count: a future
    // re-copy that introduces more unreachable keys fails here instead of
    // quietly shipping strings that never translate.
    let mut unreachable = Vec::new();
    for locale in LOCALES.iter().filter(|l| **l != UNTRANSLATED_LOCALE) {
        let path = literals_dir().join(format!("{locale}.json"));
        let body = std::fs::read_to_string(&path)?;
        let parsed: BTreeMap<String, serde_json::Value> = serde_json::from_str(&body)?;
        for key in parsed.keys().filter(|key| key.trim() != key.as_str()) {
            unreachable.push(format!("{locale}:{key:?}"));
        }
    }
    unreachable.sort_unstable();

    assert_eq!(
        unreachable,
        vec![
            r#"fa:"Security required: ""#,
            r#"km:"Security required: ""#,
            r#"th:"Security required: ""#,
            r#"zh-CN:"Security required: ""#,
        ],
        "the set of unreachable literal keys changed"
    );

    // And: no key collides with another once trimmed. A collision would make the
    // rendered translation depend on JSON key order, which is not guaranteed.
    for locale in LOCALES.iter().filter(|l| **l != UNTRANSLATED_LOCALE) {
        let path = literals_dir().join(format!("{locale}.json"));
        let body = std::fs::read_to_string(&path)?;
        let parsed: BTreeMap<String, serde_json::Value> = serde_json::from_str(&body)?;
        let mut trimmed = parsed
            .keys()
            .map(|key| key.trim().to_owned())
            .collect::<Vec<_>>();
        trimmed.sort_unstable();
        let total = trimmed.len();
        trimmed.dedup();
        assert_eq!(trimmed.len(), total, "{locale}.json has keys equal on trim");
    }
    Ok(())
}

#[actix_web::test]
async fn literal_files_are_served_over_http_when_requested() -> TestResult {
    let (_root, config) = fixture_config()?;
    let app = actix_test::init_service(App::new().configure(config.into_configurer())).await;

    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/i18n/literals/vi.json")
            .to_request(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(actix_web::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    Ok(())
}

#[actix_web::test]
async fn unknown_locale_returns_not_found_when_requested() -> TestResult {
    let (_root, config) = fixture_config()?;
    let app = actix_test::init_service(App::new().configure(config.into_configurer())).await;

    // Given: a locale with no shipped file.
    // Then: a plain 404. Not a 500, and no filesystem path in the body — an
    // unknown locale is a normal miss, not an internal error to leak details of.
    for path in ["/i18n/literals/klingon.json", "/i18n/literals/en.json"] {
        let response =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri(path).to_request())
                .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    Ok(())
}

#[actix_web::test]
async fn path_traversal_is_rejected_when_requested() -> TestResult {
    let (_root, config) = fixture_config()?;
    let app = actix_test::init_service(App::new().configure(config.into_configurer())).await;

    // Given: requests that try to escape the literals directory.
    // Then: none of them reads a file outside it. `..` segments are stripped
    // before the path is joined, so the worst case is a miss inside the
    // directory, never a read of `secret.txt` or anything above the static root.
    for path in [
        "/i18n/literals/../../secret.txt",
        "/i18n/literals/../secret.txt",
        "/i18n/literals/..%2f..%2fsecret.txt",
        "/i18n/literals/./../secret.txt",
        "/i18n/literals/....//secret.txt",
        "/i18n/literals/vi.json/../../../secret.txt",
    ] {
        let response =
            actix_test::call_service(&app, actix_test::TestRequest::get().uri(path).to_request())
                .await;
        let status = response.status();
        assert_ne!(status, StatusCode::OK, "{path} was served");
        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("do not serve me"),
            "{path} leaked file contents"
        );
    }
    Ok(())
}
