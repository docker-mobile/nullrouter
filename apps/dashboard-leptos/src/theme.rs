//! Light, dark, and follow-the-OS.
//!
//! Three pieces have to agree or the theme visibly misbehaves:
//!
//! - The inline script in `index.html` sets the class before the first paint. It duplicates
//!   [`Theme::resolve`]'s logic in JavaScript because the wasm bundle has not loaded yet at that
//!   point. [`STORAGE_KEY`] and the `"dark"` class name are the contract between them; changing
//!   either here without changing the script reintroduces the flash it exists to prevent.
//! - This module owns the state afterwards, and is what a control in the UI talks to.
//! - The token layer in `styles/input.css` keys off `.dark` on the document element.
//!
//! [`Selection::System`] is a live subscription, not a one-time read: the OS theme can change while
//! the page is open, and a dashboard left running overnight should follow it.

use leptos::prelude::*;

/// Where the user's choice is persisted, and the key the inline bootstrap script reads.
pub const STORAGE_KEY: &str = "nullrouter.theme";

/// What the user asked for.
///
/// Distinct from [`Theme`]: `System` is a *rule* for picking one, not a colour scheme, and it has
/// to survive a reload as itself. Collapsing the two at the point of storage is what makes a
/// dashboard stop following the OS the first time the OS is dark.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Selection {
    #[default]
    System,
    Light,
    Dark,
}

impl Selection {
    /// The value written to storage.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// Parse a stored value.
    ///
    /// Anything unrecognised — a hand-edited entry, a value from a future version — resolves to
    /// `System` rather than erroring, so a bad entry degrades to sensible behaviour.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }

    /// The colour scheme this selection produces, given the OS preference.
    pub const fn resolve(self, os_prefers_dark: bool) -> Theme {
        match self {
            Self::Light => Theme::Light,
            Self::Dark => Theme::Dark,
            Self::System if os_prefers_dark => Theme::Dark,
            Self::System => Theme::Light,
        }
    }

    /// The order the UI control cycles through.
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];
}

/// A resolved colour scheme. Always concrete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub const fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }
}

/// Reactive theme state, provided in context for any component to read or change.
#[derive(Clone, Copy, Debug)]
pub struct ThemeState {
    /// What the user chose.
    pub selection: RwSignal<Selection>,
    /// What that currently resolves to.
    pub resolved: Signal<Theme>,
}

impl ThemeState {
    /// Set the selection, persist it, and apply it to the document.
    pub fn set(self, next: Selection) {
        self.selection.set(next);
    }

    /// Advance to the next selection in [`Selection::ALL`].
    pub fn cycle(self) {
        let current = self.selection.get_untracked();
        let index = Selection::ALL.iter().position(|candidate| *candidate == current);
        let next = index
            .and_then(|index| Selection::ALL.get((index + 1) % Selection::ALL.len()))
            .copied()
            .unwrap_or_default();
        self.selection.set(next);
    }
}

/// Read the persisted selection, subscribe to the OS preference, and keep the document class in
/// sync for as long as the app is mounted.
///
/// Returns the state and also puts it in context, so a deeply nested control does not have to be
/// threaded a prop.
pub fn provide_theme() -> ThemeState {
    let selection = RwSignal::new(stored_selection());
    let os_prefers_dark = os_dark_signal();
    let resolved = Signal::derive(move || selection.get().resolve(os_prefers_dark.get()));

    Effect::new(move |_| {
        let selection = selection.get();
        persist(selection);
        apply(selection.resolve(os_prefers_dark.get()));
    });

    let state = ThemeState { selection, resolved };
    provide_context(state);
    state
}

/// The theme state from context.
///
/// Falls back to a detached default rather than panicking: a component rendered outside the
/// provider should degrade to light mode, not take the page down.
pub fn use_theme() -> ThemeState {
    use_context::<ThemeState>().unwrap_or_else(|| ThemeState {
        selection: RwSignal::new(Selection::default()),
        resolved: Signal::derive(|| Theme::Light),
    })
}

#[cfg(target_arch = "wasm32")]
fn stored_selection() -> Selection {
    storage()
        .and_then(|storage| storage.get_item(STORAGE_KEY).ok().flatten())
        .map_or(Selection::default(), |raw| Selection::parse(&raw))
}

#[cfg(not(target_arch = "wasm32"))]
const fn stored_selection() -> Selection {
    Selection::System
}

#[cfg(target_arch = "wasm32")]
fn persist(selection: Selection) {
    if let Some(storage) = storage() {
        // A failed write is not worth surfacing: the theme still applies for this session, and the
        // only cost is that it is not remembered.
        let _ = storage.set_item(STORAGE_KEY, selection.as_str());
    }
}

#[cfg(not(target_arch = "wasm32"))]
const fn persist(_selection: Selection) {}

/// `localStorage`, when it is both present and permitted.
///
/// Firefox in private mode and any browser with storage blocked by policy *throw* on access rather
/// than returning null. `web-sys` surfaces that as `Err`, so this cannot be an `unwrap`.
#[cfg(target_arch = "wasm32")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// A signal tracking the OS colour-scheme preference, updated as it changes.
#[cfg(target_arch = "wasm32")]
fn os_dark_signal() -> Signal<bool> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    let query = web_sys::window()
        .and_then(|window| window.match_media("(prefers-color-scheme: dark)").ok().flatten());

    let Some(query) = query else {
        return Signal::derive(|| false);
    };

    let dark = RwSignal::new(query.matches());

    let listener = Closure::<dyn Fn(web_sys::MediaQueryListEvent)>::new(
        move |event: web_sys::MediaQueryListEvent| dark.set(event.matches()),
    );
    // `addListener` rather than `addEventListener`: Safari did not support the latter on
    // `MediaQueryList` until 14, and this costs nothing to keep.
    let _ = query.add_listener_with_opt_callback(Some(listener.as_ref().unchecked_ref()));
    // The closure has to outlive this function or the listener fires into freed memory. The app
    // lives as long as the page, so leaking it deliberately is the whole lifetime.
    listener.forget();

    dark.into()
}

#[cfg(not(target_arch = "wasm32"))]
fn os_dark_signal() -> Signal<bool> {
    Signal::derive(|| false)
}

/// Put the resolved theme on the document element.
///
/// Toggles the same `dark` class the inline bootstrap script sets and the token layer keys off, and
/// keeps `color-scheme` aligned so form controls, scrollbars, and the canvas behind an overscroll
/// match the page.
#[cfg(target_arch = "wasm32")]
fn apply(theme: Theme) {
    use wasm_bindgen::JsCast;

    let Some(root) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
    else {
        return;
    };

    let classes = root.class_list();
    let _ = if theme.is_dark() {
        classes.add_1("dark")
    } else {
        classes.remove_1("dark")
    };

    if let Some(element) = root.dyn_ref::<web_sys::HtmlElement>() {
        let _ = element.style().set_property("color-scheme", theme.as_str());
    }
}

#[cfg(not(target_arch = "wasm32"))]
const fn apply(_theme: Theme) {}

#[cfg(test)]
mod tests {
    use super::{Selection, Theme};

    #[test]
    fn explicit_selection_ignores_the_os() {
        for os_dark in [true, false] {
            assert_eq!(Selection::Light.resolve(os_dark), Theme::Light);
            assert_eq!(Selection::Dark.resolve(os_dark), Theme::Dark);
        }
    }

    #[test]
    fn system_selection_follows_the_os() {
        assert_eq!(Selection::System.resolve(true), Theme::Dark);
        assert_eq!(Selection::System.resolve(false), Theme::Light);
    }

    #[test]
    fn unrecognised_storage_values_fall_back_to_system() {
        // A hand-edited entry, a value from a newer build, or a cleared key must all degrade to
        // following the OS rather than pinning a scheme.
        for raw in ["", "System", "DARK", "auto", "true", "garbage"] {
            assert_eq!(Selection::parse(raw), Selection::System, "{raw:?}");
        }
    }

    #[test]
    fn storage_values_round_trip() {
        for selection in Selection::ALL {
            assert_eq!(Selection::parse(selection.as_str()), selection);
        }
    }

    #[test]
    fn the_inline_script_only_honours_explicit_choices() {
        // index.html checks `stored === "light" || stored === "dark"` and otherwise falls through
        // to `prefers-color-scheme`. That is only correct while "system" is not one of those two
        // spellings, or a stored "system" would be read as an explicit scheme before first paint.
        assert_eq!(Selection::System.as_str(), "system");
        assert_ne!(Selection::System.as_str(), Selection::Light.as_str());
        assert_ne!(Selection::System.as_str(), Selection::Dark.as_str());
    }
}
