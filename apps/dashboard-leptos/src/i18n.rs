//! Locale resolution and message lookup.
//!
//! Three-tier fallback: browser preference → explicit cookie → en-US. Locale is chosen at startup
//! and stays stable for the session, because a locale that changes mid-workflow breaks more than it
//! helps: someone who set their browser to French and is now looking at an English error should not
//! also have to re-find the button they were clicking when the language swapped under them.
//!
//! Messages live in JSON files committed at `locales/<lang>.json`. The build does not load them; the
//! dashboard fetches its own locale file at startup. That keeps i18n out of the Rust build and lets
//! translations land without a wasm rebuild.

use leptos::prelude::*;
use std::collections::HashMap;

/// Which locale the dashboard resolved to.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Locale {
    /// IETF language tag, e.g. `"en-US"`.
    pub tag: String,
    /// Message table, keyed by `"section.key"`.
    messages: HashMap<String, String>,
}

impl Locale {
    /// Look up a message, falling back to the key itself when the entry is missing.
    ///
    /// A missing key renders as itself rather than crashing or rendering blank space, so an
    /// incomplete translation file degrades to showing English keys (which are readable) instead of
    /// taking the panel down.
    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        self.messages.get(key).map_or(key, String::as_str)
    }

    /// Shorthand for formatted messages. Call as `locale.fmt("settings.account_created", &[("name", "alice")])`.
    pub fn fmt(&self, key: &str, replacements: &[(&str, &str)]) -> String {
        let mut text = self.get(key).to_owned();
        for (placeholder, value) in replacements {
            text = text.replace(&format!("{{{placeholder}}}"), value);
        }
        text
    }

    /// Deserialize a fetched locale file.
    // Called from `load_locale`, which only exists on wasm32, and from the tests. On a native
    // non-test build it therefore has no callers.
    #[cfg_attr(
        all(not(test), not(target_arch = "wasm32")),
        expect(dead_code, reason = "load_locale, its only caller, is wasm32-only")
    )]
    fn parse(tag: String, json: &str) -> Result<Self, String> {
        let messages: HashMap<String, String> =
            serde_json::from_str(json).map_err(|error| format!("locale parse failed: {error}"))?;
        Ok(Self { tag, messages })
    }
}

/// Detect the user's preferred locale and load its message table.
///
/// Tries: explicit `locale` cookie → browser's `navigator.languages` → `en-US`. Returns the loaded
/// locale and also puts it in context.
///
/// This runs on mount, so it *will* delay first paint by one request. That is deliberate: rendering
/// English placeholder text and then swapping every label is more disruptive than a slightly longer
/// load, and the file is small (under 20 KB even with every panel's messages).
#[cfg(target_arch = "wasm32")]
pub async fn provide_locale() -> Locale {
    let tag = detect_locale().unwrap_or_else(|| "en-US".to_owned());
    let locale = load_locale(&tag).await.unwrap_or_else(|_| Locale {
        tag: "en-US".to_owned(),
        messages: HashMap::new(),
    });
    provide_context(locale.clone());
    locale
}

/// Native builds have no browser to detect from or fetch through.
#[cfg(not(target_arch = "wasm32"))]
#[expect(clippy::unused_async, reason = "mirrors the wasm signature so callers stay target-agnostic")]
pub async fn provide_locale() -> Locale {
    let locale = Locale { tag: "en-US".to_owned(), messages: HashMap::new() };
    provide_context(locale.clone());
    locale
}

/// The locale from context.
///
/// Falls back to an empty en-US rather than panicking, so a component rendered outside the provider
/// degrades to showing message keys instead of crashing.
pub fn use_locale() -> Locale {
    use_context::<Locale>().unwrap_or_else(|| Locale {
        tag: "en-US".to_owned(),
        messages: HashMap::new(),
    })
}

/// Read the user's locale preference from the cookie or the browser.
#[cfg(target_arch = "wasm32")]
fn detect_locale() -> Option<String> {
    use wasm_bindgen::JsCast;

    let window = web_sys::window()?;
    let document = window.document()?;

    // `cookie()` lives on `HtmlDocument`, not `Document`, so reading it needs the cast.
    if let Some(cookies) = document
        .dyn_ref::<web_sys::HtmlDocument>()
        .and_then(|html| html.cookie().ok())
    {
        for pair in cookies.split(';') {
            if let Some(value) = pair.trim().strip_prefix("locale=").filter(|v| !v.is_empty()) {
                return Some(value.to_owned());
            }
        }
    }

    // Otherwise the browser's first supported language.
    let navigator = window.navigator();
    let languages = navigator.languages();
    if languages.length() > 0 {
        return languages.get(0).as_string();
    }

    None
}

/// Fetch and parse a locale file.
#[cfg(target_arch = "wasm32")]
async fn load_locale(tag: &str) -> Result<Locale, String> {
    let path = format!("/locales/{tag}.json");
    let body = crate::api::get(&path)
        .await
        .map_err(|error| format!("fetch {path} failed: {error:?}"))?;
    Locale::parse(tag.to_owned(), &body)
}

#[cfg(test)]
mod tests {
    use super::Locale;

    #[test]
    fn missing_keys_fall_back_to_themselves() {
        let locale = Locale { tag: "en-US".to_owned(), messages: [].into() };
        assert_eq!(locale.get("missing.key"), "missing.key");
    }

    #[test]
    fn present_keys_return_their_messages() {
        let locale = Locale {
            tag: "en-US".to_owned(),
            messages: [("app.title".to_owned(), "Router".to_owned())].into(),
        };
        assert_eq!(locale.get("app.title"), "Router");
    }

    #[test]
    fn fmt_replaces_placeholders() {
        let locale = Locale {
            tag: "en-US".to_owned(),
            messages: [("greeting".to_owned(), "Hello, {name}!".to_owned())].into(),
        };
        assert_eq!(locale.fmt("greeting", &[("name", "Alice")]), "Hello, Alice!");
    }

    #[test]
    fn fmt_handles_multiple_replacements() {
        let locale = Locale {
            tag: "en-US".to_owned(),
            messages: [("status".to_owned(), "{count} items in {container}".to_owned())].into(),
        };
        assert_eq!(
            locale.fmt("status", &[("count", "5"), ("container", "queue")]),
            "5 items in queue"
        );
    }

    #[test]
    fn missing_keys_in_fmt_still_fall_back_to_the_key() {
        let locale = Locale { tag: "en-US".to_owned(), messages: [].into() };
        assert_eq!(locale.fmt("missing.key", &[("x", "y")]), "missing.key");
    }

    #[test]
    fn parse_deserializes_a_message_table() {
        let json = r#"{"app.title": "Router", "app.subtitle": "Gateway"}"#;
        let locale = Locale::parse("en-US".to_owned(), json).expect("valid JSON");
        assert_eq!(locale.tag, "en-US");
        assert_eq!(locale.get("app.title"), "Router");
        assert_eq!(locale.get("app.subtitle"), "Gateway");
    }

    #[test]
    fn parse_rejects_malformed_json() {
        assert!(Locale::parse("en-US".to_owned(), "{not valid").is_err());
    }
}
