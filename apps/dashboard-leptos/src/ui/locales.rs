//! The language picker's inventory.
//!
//! This used to be a second, hand-maintained copy of the locale list that
//! translated nothing — selecting a language only moved a checkmark. It had
//! already drifted: `km` (ខ្មែរ) was missing, so Khmer shipped a literal file
//! that no user could reach.
//!
//! The list now comes from [`crate::i18n`], the same source normalization and
//! literal loading use, so the picker cannot offer a locale the runtime does not
//! know or omit one it does.

pub use crate::i18n::{LocaleOption, locale_name, locales as dashboard_locales};

use crate::i18n::{I18n, bind_locale_signal};
use leptos::prelude::*;

/// Make the picker's locale signal actually change the language.
///
/// The signal already existed and already recorded the selection; it just had no
/// consumer. Binding it here is what turns a click into a persisted cookie, a
/// fetched literal map, and a re-render.
pub fn install_locale_signal(
    locale: ReadSignal<&'static str>,
    set_locale: WriteSignal<&'static str>,
) -> I18n {
    bind_locale_signal(locale, set_locale)
}

/// The active locale's endonym, for the picker's heading.
///
/// Falls back to the raw id so the heading is never blank.
pub fn active_locale_label(locale: &str) -> &str {
    locale_name(locale).unwrap_or(locale)
}
