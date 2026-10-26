//! Menus: a popover list anchored to a trigger.
//!
//! Used for the account menu and the per-row action menus. `role="menu"` + `role="menuitem"` with
//! `aria-expanded` on the trigger is what a screen reader needs to announce it as a menu rather than
//! as a random list of buttons.
//!
//! Dismissal is the part that is easy to get wrong: a menu has to close on Escape and on a pointer
//! press anywhere outside it. The listeners are registered with `window_event_listener`, whose handle
//! is `Send + Sync` and detaches on drop -- a raw `Closure` captured into an `on_cleanup` is not, and
//! the type error that produces is the only thing standing between this and a listener that leaks on
//! every table row.

use leptos::prelude::*;
use leptos::wasm_bindgen::JsCast;

use super::icon::{Icon, IconName};

/// How a trigger looks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MenuTriggerVariant {
    /// A bordered button: the default, for a menu that is one of several controls.
    #[default]
    Outline,
    /// No chrome at all. For a row of dots at the end of a table row.
    Ghost,
    /// A filled button, for a menu that *is* the page's primary action.
    Default,
}

impl MenuTriggerVariant {
    const fn classes(self) -> &'static str {
        match self {
            Self::Outline => {
                "border border-input bg-background shadow-xs hover:bg-accent \
                 hover:text-accent-foreground"
            }
            Self::Ghost => "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
            Self::Default => "bg-primary text-primary-foreground hover:bg-primary/90",
        }
    }
}

impl From<&str> for MenuTriggerVariant {
    fn from(raw: &str) -> Self {
        match raw {
            "ghost" => Self::Ghost,
            "default" => Self::Default,
            _ => Self::Outline,
        }
    }
}

/// A menu anchored under a trigger.
#[component]
pub fn Menu(
    /// The trigger's label, if it has visible text.
    #[prop(optional, into)]
    label: Option<String>,
    /// The accessible name, when the trigger is only an icon.
    #[prop(optional, into)]
    aria_label: Option<String>,
    /// Optional glyph shown before the label.
    #[prop(optional, into)]
    icon: Option<IconName>,
    #[prop(optional, into)] trigger_variant: Option<MenuTriggerVariant>,
    /// How wide the panel is. `w-56` by default; use `w-72` for items with descriptions.
    #[prop(optional, into)]
    width: Option<String>,
    /// Align the panel to its trigger's right edge. What a row-action menu wants.
    #[prop(optional, into)]
    align_end: Option<bool>,
    #[prop(optional, into)] class: Option<String>,
    // `ChildrenFn`, not `Children`: the panel below is built inside a reactive closure that runs
    // every time the menu opens, and a `FnOnce` box cannot be called from there. `Arc` means the
    // second opening renders the same items the first one did.
    #[prop(optional)] children: Option<ChildrenFn>,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let root = NodeRef::<leptos::html::Div>::new();
    let outside = StoredValue::new_local(None::<WindowListenerHandle>);
    let escape = StoredValue::new_local(None::<WindowListenerHandle>);

    // Registered only while open. A document listener that outlives the menu it belongs to is a leak,
    // and a table page can hold dozens of these.
    Effect::new(move |_| {
        if !open.get() {
            outside.set_value(None);
            escape.set_value(None);
            return;
        }

        outside.set_value(Some(window_event_listener_untyped(
            "pointerdown",
            move |event| {
                let inside = event
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::Node>().ok())
                    .zip(root.get_untracked().map(JsCast::unchecked_into))
                    .is_some_and(|(target, container): (web_sys::Node, web_sys::Node)| {
                        container.contains(Some(&target))
                    });
                if !inside {
                    open.set(false);
                }
            },
        )));

        escape.set_value(Some(window_event_listener(
            leptos::ev::keydown,
            move |event: leptos::ev::KeyboardEvent| {
                if event.key() == "Escape" {
                    open.set(false);
                }
            },
        )));
    });

    on_cleanup(move || {
        outside.set_value(None);
        escape.set_value(None);
    });

    let has_label = label.as_ref().is_some_and(|text| !text.trim().is_empty());
    let trigger_class = format!(
        "inline-flex shrink-0 items-center justify-center gap-2 rounded-md text-sm \
         font-medium whitespace-nowrap outline-none transition-colors \
         focus-visible:ring-[3px] focus-visible:ring-ring/50 \
         disabled:pointer-events-none disabled:opacity-50 \
         [&_svg:not([class*='size-'])]:size-4 {} {}",
        trigger_variant.unwrap_or_default().classes(),
        if has_label {
            "h-9 px-3".to_owned()
        } else {
            "size-8".to_owned()
        },
    );
    let panel_class = format!(
        "absolute z-50 mt-1.5 min-w-[11rem] overflow-hidden rounded-lg border border-border \
         bg-popover p-1 text-popover-foreground shadow-lg animate-in fade-in zoom-in-95 \
         duration-100 {} {}",
        width.unwrap_or_else(|| "w-56".to_owned()),
        if align_end.unwrap_or(false) {
            "right-0 origin-top-right"
        } else {
            "left-0 origin-top-left"
        },
    );
    let root_class = format!("relative inline-block {}", class.unwrap_or_default());

    view! {
        <div class=root_class node_ref=root>
            <button
                type="button"
                class=trigger_class
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
                aria-label=aria_label
                on:click=move |_| open.update(|open| *open = !*open)
            >
                {icon.map(|icon| view! { <Icon name=icon class="size-4" /> })}
                {label
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| view! { <span>{text}</span> })}
            </button>
            {move || {
                open.get()
                    .then(|| {
                        view! {
                            <div class=panel_class.clone() role="menu" aria-orientation="vertical">
                                {children.as_ref().map(|children| children())}
                            </div>
                        }
                    })
            }}
        </div>
    }
}

/// One action in a menu.
#[component]
pub fn MenuItem(
    #[prop(into)] label: String,
    /// Called before the menu closes, so a callback that triggers a re-render is not racing the
    /// panel's removal.
    #[prop(optional)]
    on_select: Option<Callback<()>>,
    /// A glyph shown before the label.
    #[prop(optional, into)]
    icon: Option<IconName>,
    /// A second line explaining what the item does.
    #[prop(optional, into)]
    description: Option<String>,
    /// Give the item the destructive tone: red text, for deletes and revocations.
    #[prop(optional, into)]
    destructive: Option<bool>,
    /// Render as a link to this path instead of a button.
    #[prop(optional, into)]
    href: Option<String>,
) -> impl IntoView {
    let class = format!(
        "flex w-full items-start gap-2.5 rounded-md px-2 py-1.5 text-left text-[0.8125rem] \
         outline-none transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/50 \
         {}",
        if destructive.unwrap_or(false) {
            "text-destructive hover:bg-destructive/10"
        } else {
            "text-foreground hover:bg-accent hover:text-accent-foreground"
        },
    );

    let body = move || {
        view! {
            {icon.map(|icon| view! { <Icon name=icon class="mt-px size-4" /> })}
            <span class="min-w-0 flex-1">
                <span class="block truncate">{label.clone()}</span>
                {description
                    .clone()
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| {
                        view! {
                            <span class="mt-0.5 block text-xs leading-snug text-muted-foreground">
                                {text}
                            </span>
                        }
                    })}
            </span>
        }
    };

    match href {
        Some(href) => view! {
            <a href=href role="menuitem" class=class>
                {body()}
            </a>
        }
        .into_any(),
        None => view! {
            <button
                type="button"
                role="menuitem"
                class=class
                on:click=move |_| {
                    if let Some(callback) = on_select {
                        callback.run(());
                    }
                }
            >
                {body()}
            </button>
        }
        .into_any(),
    }
}

/// A heading or a muted note inside a menu, outside the menu's item order.
#[component]
pub fn MenuLabel(
    #[prop(into)] text: String,
    #[prop(optional, into)] detail: Option<String>,
) -> impl IntoView {
    view! {
        <div class="px-2 py-1.5">
            <p class="truncate text-[0.8125rem] font-medium text-foreground">{text}</p>
            {detail
                .filter(|text| !text.trim().is_empty())
                .map(|text| {
                    view! { <p class="truncate text-xs text-muted-foreground">{text}</p> }
                })}
        </div>
    }
}

/// A rule between groups of items.
#[component]
pub fn MenuSeparator() -> impl IntoView {
    view! { <div class="-mx-1 my-1 h-px bg-border" role="separator" /> }
}
