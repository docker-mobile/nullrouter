//! Buttons: one implementation of the focus ring, disabled state, and icon sizing.
//!
//! Kept as a function that *returns* the class list as well as a component, because a link has to
//! look like a button. `<A>` from `leptos_router` renders an `<a>`, and wrapping one in a `<button>`
//! would nest an interactive element inside another. [`button_class`] lets a router link borrow the
//! appearance without the invalid markup.

use leptos::prelude::*;

/// How much weight a button carries.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ButtonVariant {
    /// The one action a panel expects to be taken.
    #[default]
    Default,
    /// A destructive action: deletes, stops, revocations.
    Destructive,
    /// A bordered action at the same weight as the surface behind it.
    Outline,
    /// A filled action that is not the primary one.
    Secondary,
    /// No fill or border until hovered. For rows and toolbars.
    Ghost,
    /// Reads as a link. For the third action in a list, where three buttons is too heavy.
    Link,
}

impl ButtonVariant {
    /// The variant a class-name string names, for `variant="outline"` at a call site.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "destructive" => Self::Destructive,
            "outline" => Self::Outline,
            "secondary" => Self::Secondary,
            "ghost" => Self::Ghost,
            "link" => Self::Link,
            _ => Self::Default,
        }
    }

    const fn classes(self) -> &'static str {
        match self {
            Self::Default => {
                "bg-primary text-primary-foreground shadow-xs hover:bg-primary/90 \
                 active:bg-primary/95"
            }
            Self::Destructive => {
                "bg-destructive text-white shadow-xs hover:bg-destructive/90 \
                 focus-visible:ring-destructive/40"
            }
            Self::Outline => {
                "border border-input bg-background shadow-xs hover:bg-accent \
                 hover:text-accent-foreground"
            }
            Self::Secondary => "bg-secondary text-secondary-foreground hover:bg-secondary/80",
            Self::Ghost => "hover:bg-accent hover:text-accent-foreground",
            Self::Link => "text-foreground underline-offset-4 hover:underline",
        }
    }
}

impl From<&str> for ButtonVariant {
    fn from(raw: &str) -> Self {
        Self::parse(raw)
    }
}

impl From<String> for ButtonVariant {
    fn from(raw: String) -> Self {
        Self::parse(&raw)
    }
}

/// How much room a button takes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ButtonSize {
    /// The default, and the one to use unless there is a reason.
    #[default]
    Default,
    /// Sits inside a dense row or a card header.
    Sm,
    /// The one action on a page: sign-in, an empty-state call to action.
    Lg,
    /// Square, for a glyph with no label. The caller must supply an `aria-label`.
    Icon,
    /// A square at the small size, for the dismiss control on a card.
    IconSm,
}

impl ButtonSize {
    /// The size a class-name string names.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "sm" => Self::Sm,
            "lg" => Self::Lg,
            "icon" => Self::Icon,
            "icon-sm" => Self::IconSm,
            _ => Self::Default,
        }
    }

    const fn classes(self) -> &'static str {
        match self {
            Self::Default => "h-9 gap-2 px-4",
            Self::Sm => "h-8 gap-1.5 px-3 text-[0.8125rem]",
            Self::Lg => "h-10 gap-2 px-6",
            Self::Icon => "size-9",
            Self::IconSm => "size-8",
        }
    }
}

impl From<&str> for ButtonSize {
    fn from(raw: &str) -> Self {
        Self::parse(raw)
    }
}

impl From<String> for ButtonSize {
    fn from(raw: String) -> Self {
        Self::parse(&raw)
    }
}

/// The full class list for a button of this shape.
///
/// `shrink-0` because a button in a flex row is the thing that must not be squeezed: the label
/// beside it can wrap, and a button whose text wraps to two lines is worse than a truncated label.
pub fn button_class(variant: ButtonVariant, size: ButtonSize) -> String {
    format!(
        "inline-flex shrink-0 items-center justify-center whitespace-nowrap rounded-md \
         text-sm font-medium transition-colors outline-none \
         focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:border-ring \
         disabled:pointer-events-none disabled:opacity-50 \
         [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-4 \
         {} {}",
        variant.classes(),
        size.classes(),
    )
}

/// A button that runs one action.
///
/// `on_click` rather than `on:click` so the button owns its disabled-admission check: a disabled
/// button in a browser does not fire `click`, but nothing stops a keyboard-activated script from
/// calling it, and the panels here write real state.
#[component]
pub fn Button(
    /// The label, if any. An icon-only button leaves this out and supplies `aria_label`.
    #[prop(optional, into)]
    label: Option<String>,
    #[prop(optional, into)] variant: Option<ButtonVariant>,
    #[prop(optional, into)] size: Option<ButtonSize>,
    /// Whether the button refuses input.
    #[prop(optional, into)]
    disabled: Option<Signal<bool>>,
    /// Shown instead of the label while the action is in flight, and implies `disabled`.
    #[prop(optional, into)]
    busy: Option<Signal<bool>>,
    /// The accessible name for a button whose visible content is only an icon.
    #[prop(optional, into)]
    aria_label: Option<String>,
    /// `submit` for the one button in a `<form>`, otherwise unset.
    #[prop(optional, into)]
    button_type: Option<String>,
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] on_click: Option<Callback<()>>,
) -> impl IntoView {
    let base = button_class(variant.unwrap_or_default(), size.unwrap_or_default());
    let extra = class.unwrap_or_default();
    let class = format!("{base} {extra}");

    let is_disabled = move || {
        disabled.is_some_and(|signal| signal.get()) || busy.is_some_and(|signal| signal.get())
    };

    let label = Signal::derive({
        move || {
            let Some(label) = label.clone() else {
                return String::new();
            };
            if busy.is_some_and(|signal| signal.get()) {
                format!("{label}…")
            } else {
                label
            }
        }
    });

    // An icon-only button is a glyph with no text, so it has to carry a name of its own or it is
    // announced as "button". A button that renders no label and supplies no name gets no attribute
    // rather than an empty one.
    let name = aria_label.filter(|value| !value.trim().is_empty());

    // A disabled button is already inert in the browser. Checking here as well means a keyboard
    // activation cannot slip past it, which matters because these buttons write real state.
    let handle = move |_| {
        if is_disabled() {
            return;
        }
        if let Some(callback) = on_click {
            callback.run(());
        }
    };

    // Resolved before the tree, as an owned `String`: `view!` moves what it reads and the attribute is
    // written into a closure, so a borrow of the `Option` cannot outlive this function.
    let resolved_type = button_type.unwrap_or_else(|| "button".to_owned());

    view! {
        <button
            type=resolved_type
            class=class
            disabled=is_disabled
            aria-label=name
            on:click=handle
        >
            {move || {
                let text = label.get();
                (!text.is_empty()).then(|| view! { <span>{text}</span> })
            }}
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::{ButtonSize, ButtonVariant, button_class};

    #[test]
    fn every_variant_and_size_produce_a_class_list() {
        for variant in [
            ButtonVariant::Default,
            ButtonVariant::Destructive,
            ButtonVariant::Outline,
            ButtonVariant::Secondary,
            ButtonVariant::Ghost,
            ButtonVariant::Link,
        ] {
            for size in [
                ButtonSize::Default,
                ButtonSize::Sm,
                ButtonSize::Lg,
                ButtonSize::Icon,
                ButtonSize::IconSm,
            ] {
                let class = button_class(variant, size);
                assert!(class.contains("inline-flex"), "{variant:?} {size:?}");
                assert!(class.contains("focus-visible:ring"), "{variant:?} {size:?}");
            }
        }
    }

    #[test]
    fn variants_parse_from_their_snake_case_names() {
        assert_eq!(ButtonVariant::from("outline"), ButtonVariant::Outline);
        assert_eq!(
            ButtonVariant::from("destructive"),
            ButtonVariant::Destructive
        );
        assert_eq!(ButtonVariant::from("nonsense"), ButtonVariant::Default);
        assert_eq!(ButtonSize::from("icon-sm"), ButtonSize::IconSm);
        assert_eq!(ButtonSize::from("nonsense"), ButtonSize::Default);
    }

    #[test]
    fn an_icon_button_is_square_and_a_labelled_one_is_not() {
        assert!(button_class(ButtonVariant::Ghost, ButtonSize::Icon).contains("size-9"));
        assert!(!button_class(ButtonVariant::Ghost, ButtonSize::Default).contains("size-9"));
    }
}
