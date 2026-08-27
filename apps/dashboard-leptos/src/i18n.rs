//! Runtime translation, ported from the upstream dashboard's `src/i18n`.
//!
//! Upstream ships one flat `string -> string` JSON file per locale and swaps
//! translations at runtime: read the locale from a cookie, fetch
//! `/i18n/literals/{locale}.json`, then look each visible English string up by
//! its exact trimmed value. There is no key namespace and no message catalogue
//! — the English source text *is* the key.
//!
//! Two consequences are load-bearing and deliberately preserved here:
//!
//! - A lookup miss renders the **original English string**, never an empty span
//!   and never a key name. A partially translated locale degrades to mixed
//!   English rather than to holes in the page.
//! - English short-circuits before any lookup and has no literal file, so the
//!   default locale costs one comparison and zero requests.
//!
//! Everything that can be decided without a browser — the locale list,
//! normalization, cookie parsing, and the lookup itself — is plain Rust so it
//! stays unit-testable on the native target. Only cookie and fetch access sit
//! behind `#[cfg(target_arch = "wasm32")]`.

use std::{collections::BTreeMap, sync::Arc};

use leptos::prelude::*;

/// One selectable language.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocaleOption {
    /// Canonical locale id, and the basename of its literal file.
    pub id: &'static str,
    /// Endonym, shown in the picker in the language's own script.
    pub name: &'static str,
}

/// The locale the dashboard falls back to, and the language the source is written in.
pub const DEFAULT_LOCALE: &str = "en";

/// Cookie the selected locale is persisted in. Must match upstream exactly, or
/// a locale chosen in one dashboard is invisible to the other.
pub const LOCALE_COOKIE: &str = "locale";

/// How long the locale cookie survives, matching upstream's `POST /api/locale`.
#[cfg(target_arch = "wasm32")]
const LOCALE_COOKIE_MAX_AGE: u32 = 60 * 60 * 24 * 365;

/// Every supported locale, in upstream's declaration order.
///
/// 35 entries: `en` plus the 34 locales that ship a literal file.
const LOCALES: [LocaleOption; 35] = [
    LocaleOption {
        id: "en",
        name: "English",
    },
    LocaleOption {
        id: "vi",
        name: "Tiếng Việt",
    },
    LocaleOption {
        id: "zh-CN",
        name: "简体中文",
    },
    LocaleOption {
        id: "zh-TW",
        name: "繁體中文",
    },
    LocaleOption {
        id: "ja",
        name: "日本語",
    },
    LocaleOption {
        id: "pt-BR",
        name: "Português (Brasil)",
    },
    LocaleOption {
        id: "pt-PT",
        name: "Português (Portugal)",
    },
    LocaleOption {
        id: "ko",
        name: "한국어",
    },
    LocaleOption {
        id: "es",
        name: "Español",
    },
    LocaleOption {
        id: "de",
        name: "Deutsch",
    },
    LocaleOption {
        id: "fr",
        name: "Français",
    },
    LocaleOption {
        id: "he",
        name: "עברית",
    },
    LocaleOption {
        id: "ar",
        name: "العربية",
    },
    LocaleOption {
        id: "ru",
        name: "Русский",
    },
    LocaleOption {
        id: "pl",
        name: "Polski",
    },
    LocaleOption {
        id: "cs",
        name: "Čeština",
    },
    LocaleOption {
        id: "nl",
        name: "Nederlands",
    },
    LocaleOption {
        id: "tr",
        name: "Türkçe",
    },
    LocaleOption {
        id: "uk",
        name: "Українська",
    },
    LocaleOption {
        id: "tl",
        name: "Tagalog",
    },
    LocaleOption {
        id: "id",
        name: "Indonesia",
    },
    LocaleOption {
        id: "km",
        name: "ខ្មែរ",
    },
    LocaleOption {
        id: "th",
        name: "ไทย",
    },
    LocaleOption {
        id: "hi",
        name: "हिन्दी",
    },
    LocaleOption {
        id: "bn",
        name: "বাংলা",
    },
    LocaleOption {
        id: "ur",
        name: "اردو",
    },
    LocaleOption {
        id: "ro",
        name: "Română",
    },
    LocaleOption {
        id: "sv",
        name: "Svenska",
    },
    LocaleOption {
        id: "it",
        name: "Italiano",
    },
    LocaleOption {
        id: "el",
        name: "Ελληνικά",
    },
    LocaleOption {
        id: "hu",
        name: "Magyar",
    },
    LocaleOption {
        id: "fi",
        name: "Suomi",
    },
    LocaleOption {
        id: "da",
        name: "Dansk",
    },
    LocaleOption {
        id: "no",
        name: "Norsk",
    },
    LocaleOption {
        id: "fa",
        name: "فارسی",
    },
];

/// The canonical locale list.
pub const fn locales() -> &'static [LocaleOption] {
    &LOCALES
}

/// Whether `locale` is exactly one of the canonical ids.
pub fn is_supported_locale(locale: &str) -> bool {
    LOCALES.iter().any(|option| option.id == locale)
}

/// Resolve arbitrary input to a canonical locale id.
///
/// Unknown, malformed, and empty input all resolve to [`DEFAULT_LOCALE`] rather
/// than erroring: a stale or hand-edited cookie must degrade to English, not
/// break the dashboard.
///
/// Region codes are significant — `zh-CN` and `zh-TW` are separate locales and
/// neither is a fallback for the other. `zh` is upstream's single alias and
/// means Simplified Chinese. A region upstream does not ship (`en-US`, `de-AT`)
/// is *not* truncated to its base language; it resolves to English, matching
/// upstream's exact-match behaviour.
///
/// Matching is case-insensitive, so a cookie carrying `ZH-tw` still finds
/// `zh-TW`. Upstream compares case-sensitively and sends such input to English;
/// this only ever turns a failed match into the right locale.
pub fn normalize_locale(locale: &str) -> &'static str {
    let trimmed = locale.trim();
    if trimmed.is_empty() {
        return DEFAULT_LOCALE;
    }
    let candidate = if trimmed.eq_ignore_ascii_case("zh") {
        "zh-CN"
    } else {
        trimmed
    };
    LOCALES
        .iter()
        .find(|option| option.id.eq_ignore_ascii_case(candidate))
        .map_or(DEFAULT_LOCALE, |option| option.id)
}

/// The endonym for a locale id, when it is one we know.
pub fn locale_name(locale: &str) -> Option<&'static str> {
    LOCALES
        .iter()
        .find(|option| option.id == locale)
        .map(|option| option.name)
}

/// Where a locale's literal file is served from.
pub fn literals_path(locale: &str) -> String {
    format!("/i18n/literals/{locale}.json")
}

/// Read one cookie's value out of a `Cookie`-header-shaped string.
///
/// Kept separate from the browser so cookie parsing is testable: real headers
/// carry several cookies in unspecified order, and the locale is rarely first.
/// Returns `None` when the cookie is absent or present but empty, so the caller
/// can tell "never chosen" from "chosen".
pub fn cookie_value(cookies: &str, name: &str) -> Option<String> {
    cookies
        .split(';')
        .filter_map(|entry| entry.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| percent_decode(value))
        .filter(|value| !value.is_empty())
}

/// The locale a cookie header selects, normalized.
///
/// A header with no locale cookie yields `None`, which the caller treats as
/// "keep the current default" rather than as an error.
pub fn locale_from_cookies(cookies: &str) -> Option<&'static str> {
    cookie_value(cookies, LOCALE_COOKIE)
        .as_deref()
        .map(normalize_locale)
}

/// Decode `%XX` escapes, leaving anything malformed as written.
///
/// Locale ids are ASCII, but the cookie is written by the upstream Next.js
/// route as well, which percent-encodes on the way out.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0_usize;
    while let Some(&byte) = bytes.get(index) {
        let decoded = (byte == b'%')
            .then(|| {
                let high = bytes.get(index + 1).copied()?;
                let low = bytes.get(index + 2).copied()?;
                let high = char::from(high).to_digit(16)?;
                let low = char::from(low).to_digit(16)?;
                u8::try_from(high * 16 + low).ok()
            })
            .flatten();
        if let Some(value) = decoded {
            out.push(value);
            index += 3;
        } else {
            out.push(byte);
            index += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| value.to_owned())
}

/// One locale's translations: trimmed English source text to translated text.
pub type Literals = BTreeMap<String, String>;

/// Parse a literal file.
///
/// Non-string values are dropped rather than rejecting the file, so one bad
/// entry costs that single string its translation instead of the whole locale.
/// `None` means the body was not a JSON object at all.
pub fn parse_literals(body: &str) -> Option<Literals> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let object = value.as_object()?;
    Some(
        object
            .iter()
            .filter_map(|(key, value)| value.as_str().map(|text| (key.clone(), text.to_owned())))
            .collect(),
    )
}

/// Translate `text` for `locale`, falling back to `text` itself.
///
/// The whole contract of the runtime port lives here:
///
/// - Empty and whitespace-only input is returned untouched — there is nothing to
///   translate, and a blank layout string must stay blank.
/// - English returns immediately without consulting `literals`, so the default
///   locale never pays for a lookup and can never be altered by a stray entry
///   in a literal file.
/// - A miss returns the original English string. Never an empty string, never a
///   key name: an untranslated label is legible, a blank one is a bug report.
pub fn lookup(locale: &str, literals: &Literals, text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || locale == DEFAULT_LOCALE {
        return text.to_owned();
    }
    literals
        .get(trimmed)
        .filter(|translated| !translated.is_empty())
        .map_or_else(|| text.to_owned(), Clone::clone)
}

/// The active locale and its loaded literals, as reactive state.
///
/// `Arc` so that swapping locales moves one pointer rather than cloning a
/// ~1400-entry map into every subscriber.
#[derive(Clone, Copy, Debug)]
pub struct I18n {
    locale: ReadSignal<&'static str>,
    literals: ReadSignal<Arc<Literals>>,
}

impl I18n {
    /// The active locale id.
    pub fn locale(&self) -> &'static str {
        self.locale.get()
    }

    /// Translate one string with the currently loaded literals.
    ///
    /// Reading both signals is what makes a locale change re-render every
    /// translated string in the view.
    pub fn translate(&self, text: &str) -> String {
        let locale = self.locale.get();
        self.literals
            .with(|literals| lookup(locale, literals, text))
    }
}

/// Wire an existing locale signal up to cookie persistence and literal loading.
///
/// Takes the caller's signal rather than owning one so the language picker keeps
/// driving the same state it already drives. Call once, above anything that
/// translates.
///
/// On mount the cookie wins over the signal's initial value, so a locale chosen
/// in a previous session survives a reload. Afterwards every change writes the
/// cookie and loads that locale's literals.
pub fn bind_locale_signal(
    locale: ReadSignal<&'static str>,
    set_locale: WriteSignal<&'static str>,
) -> I18n {
    let (literals, set_literals) = signal(Arc::new(Literals::new()));
    let i18n = I18n { locale, literals };
    provide_context(i18n);

    if let Some(stored) = stored_locale() {
        set_locale.set(stored);
    }

    Effect::new(move |_| {
        let selected = normalize_locale(locale.get());
        write_locale_cookie(selected);
        load_literals(selected, set_literals);
    });

    i18n
}

/// The bound [`I18n`], when one was installed.
pub fn use_i18n() -> Option<I18n> {
    use_context::<I18n>()
}

/// Translate `text` using the bound [`I18n`].
///
/// Without one — a component rendered outside the provider, or a native-target
/// test — this returns `text` unchanged. Missing i18n degrades to English
/// rather than to a panic or a blank label.
pub fn translate(text: &str) -> String {
    use_i18n().map_or_else(|| text.to_owned(), |i18n| i18n.translate(text))
}

/// The locale persisted in the browser's cookie jar, if any.
#[cfg(target_arch = "wasm32")]
fn stored_locale() -> Option<&'static str> {
    use wasm_bindgen::JsCast;

    let document = web_sys::window()?.document()?;
    let html_document = document.dyn_into::<web_sys::HtmlDocument>().ok()?;
    let cookies = html_document.cookie().ok()?;
    locale_from_cookies(&cookies)
}

/// Persist the selected locale so it survives a reload.
///
/// Mirrors upstream's `POST /api/locale`: same cookie name, same one-year
/// max-age, same root path, so both dashboards read each other's choice.
/// `SameSite=Lax` because the cookie is only ever read by first-party requests.
#[cfg(target_arch = "wasm32")]
fn write_locale_cookie(locale: &str) {
    use wasm_bindgen::JsCast;

    let Some(html_document) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.dyn_into::<web_sys::HtmlDocument>().ok())
    else {
        return;
    };
    let cookie =
        format!("{LOCALE_COOKIE}={locale}; path=/; max-age={LOCALE_COOKIE_MAX_AGE}; SameSite=Lax");
    // A failed write costs persistence across reloads, not this session's locale.
    drop(html_document.set_cookie(&cookie));
}

/// Load a locale's literals into `setter`.
///
/// English has no literal file: it is the source language, so it resolves to an
/// empty map without a request. A failed fetch or unparseable body also yields
/// an empty map, which renders English — the same visible outcome as upstream's
/// `catch`, and the only honest one when the translations did not arrive.
#[cfg(target_arch = "wasm32")]
fn load_literals(locale: &'static str, setter: WriteSignal<Arc<Literals>>) {
    if locale == DEFAULT_LOCALE {
        setter.set(Arc::new(Literals::new()));
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let literals = crate::api::get(&literals_path(locale))
            .await
            .ok()
            .and_then(|body| parse_literals(&body))
            .unwrap_or_default();
        setter.set(Arc::new(literals));
    });
}

/// Native builds have no cookie jar; the locale is whatever the signal holds.
#[cfg(not(target_arch = "wasm32"))]
const fn stored_locale() -> Option<&'static str> {
    None
}

/// Native builds have nowhere to persist to.
#[cfg(not(target_arch = "wasm32"))]
const fn write_locale_cookie(_locale: &str) {}

/// Native builds cannot fetch, so no locale has literals and everything renders
/// as its English source.
#[cfg(not(target_arch = "wasm32"))]
fn load_literals(_locale: &'static str, setter: WriteSignal<Arc<Literals>>) {
    setter.set(Arc::new(Literals::new()));
}
