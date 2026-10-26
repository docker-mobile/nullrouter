//! Dialogs: a modal that traps focus and closes on Escape.
//!
//! Implemented over `<dialog>` rather than a `<div>` with `role="dialog"`, because the element gets the
//! hard parts from the platform: focus trapping, the top layer (so no `z-index` arms race), Escape to
//! close, and `::backdrop`. `showModal()` is what engages all of it, so the open/close state is driven
//! through that rather than through a class.
//!
//! State lives in the caller's signal. The element also reports its own closes back through `onclose`,
//! so Escape and a click on the backdrop put the signal back in sync instead of leaving the caller
//! believing a dialog is still open.

use leptos::prelude::*;
use leptos::wasm_bindgen::JsCast;

use super::icon::{Icon, IconName};

/// A modal dialog.
///
/// `open` is owned by the caller; this element reflects it and reports changes back. Rendering is
/// conditional so a closed dialog's contents do not stay mounted.
#[component]
pub fn Dialog(
    /// Whether the dialog is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Called with the next state when the dialog closes itself: Escape, the backdrop, the X button.
    #[prop(optional)]
    on_open_change: Option<Callback<bool>>,
    /// The dialog's visible width. `max-w-lg` by default.
    #[prop(optional, into)]
    width: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let element = NodeRef::<leptos::html::Dialog>::new();

    // Drive the platform's own modal state from the signal. `showModal` on an already-open dialog
    // throws, and so does `close` on a closed one, hence the `open` check on both sides.
    Effect::new(move |_| {
        let Some(dialog) = element.get() else {
            return;
        };
        let should_be_open = open.get();
        let is_open = dialog.open();
        if should_be_open && !is_open {
            let _ = dialog.show_modal();
        } else if !should_be_open && is_open {
            dialog.close();
        }
    });

    let close = move || {
        if let Some(callback) = on_open_change {
            callback.run(false);
        }
    };

    let width = width.unwrap_or_else(|| "max-w-lg".to_owned());
    let class = format!(
        "m-auto w-[calc(100vw-2rem)] rounded-xl border border-border bg-card \
         p-0 text-card-foreground shadow-2xl backdrop:bg-black/50 \
         backdrop:backdrop-blur-[2px] open:animate-in open:fade-in open:zoom-in-95 \
         open:duration-150 {width}"
    );

    view! {
        <dialog
            node_ref=element
            class=class
            on:close=move |_| {
                if let Some(callback) = on_open_change {
                    callback.run(false);
                }
            }
            // A click whose target is the dialog element itself, rather than any of its children,
            // landed on the backdrop.
            on:click=move |event| {
                if event.target() == event.current_target() {
                    close();
                }
            }
        >
            <div class="flex max-h-[85dvh] flex-col">
                {children.map(|children| children())}
            </div>
        </dialog>
    }
}

/// A button that opens the dialog it is nested in.
#[component]
pub fn DialogTrigger(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "inline-flex items-center justify-center rounded-md text-sm font-medium {}",
        class.unwrap_or_default(),
    );
    view! { <span class=class>{children.map(|children| children())}</span> }
}

/// The dialog's body, with the padding and the scroll boundary.
#[component]
pub fn DialogContent(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!("overflow-y-auto p-5 {}", class.unwrap_or_default());
    view! { <div class=class>{children.map(|children| children())}</div> }
}

/// The dialog's title row, with the dismiss control.
#[component]
pub fn DialogHeader(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "flex items-start justify-between gap-4 px-5 pb-3 pt-4 {}",
        class.unwrap_or_default(),
    );
    view! { <div class=class>{children.map(|children| children())}</div> }
}

/// The dialog's title. Rendered as an `<h2>`; the dialog itself is named by `aria-labelledby` in
/// [`Dialog`]'s caller, or by this being the first heading inside it.
#[component]
pub fn DialogTitle(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "text-base font-semibold tracking-tight text-foreground {}",
        class.unwrap_or_default()
    );
    view! { <h2 class=class>{children.map(|children| children())}</h2> }
}

/// A sentence under the title.
#[component]
pub fn DialogDescription(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "text-[0.8125rem] leading-relaxed text-muted-foreground {}",
        class.unwrap_or_default()
    );
    view! { <p class=class>{children.map(|children| children())}</p> }
}

/// The dialog's action row.
#[component]
pub fn DialogFooter(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "flex flex-col-reverse gap-2 border-t border-border bg-muted/30 px-5 py-3 \
         sm:flex-row sm:justify-end {}",
        class.unwrap_or_default(),
    );
    view! { <div class=class>{children.map(|children| children())}</div> }
}

/// The dismiss control in the header corner.
#[component]
pub fn DialogClose(
    /// What the close button is called for a screen reader.
    #[prop(optional, into)]
    label: Option<String>,
    #[prop(optional)] on_click: Option<Callback<()>>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="-mr-1 -mt-1 grid size-8 shrink-0 place-items-center rounded-md \
                   text-muted-foreground outline-none transition-colors \
                   hover:bg-accent hover:text-accent-foreground \
                   focus-visible:ring-[3px] focus-visible:ring-ring/50"
            aria-label=label.unwrap_or_else(|| "Close".to_owned())
            on:click=move |_| {
                if let Some(callback) = on_click {
                    callback.run(());
                }
            }
        >
            <Icon name=IconName::X class="size-4" />
        </button>
    }
}

/// A close button that reaches the containing `<dialog>` and closes it directly.
///
/// Separate from [`DialogClose`] because the header inside a dialog usually has no callback to call --
/// the state is the caller's signal, not the header's -- and `dialog.close()` fires `onclose`, which is
/// already wired to report the change back.
#[component]
pub fn DialogDismiss(
    #[prop(optional, into)] label: Option<String>,
    #[prop(optional)] on_click: Option<Callback<()>>,
) -> impl IntoView {
    let closest_dialog = move |event: leptos::ev::MouseEvent| {
        if let Some(callback) = on_click {
            callback.run(());
            return;
        }
        let Some(target) = event.target() else {
            return;
        };
        let Ok(element) = target.dyn_into::<web_sys::Element>() else {
            return;
        };
        let Ok(dialog) = element.closest("dialog") else {
            return;
        };
        if let Some(dialog) =
            dialog.and_then(|node| node.dyn_into::<web_sys::HtmlDialogElement>().ok())
        {
            dialog.close();
        }
    };

    view! {
        <button
            type="button"
            class="-mr-1 -mt-1 grid size-8 shrink-0 place-items-center rounded-md \
                   text-muted-foreground outline-none transition-colors \
                   hover:bg-accent hover:text-accent-foreground \
                   focus-visible:ring-[3px] focus-visible:ring-ring/50"
            aria-label=label.unwrap_or_else(|| "Close".to_owned())
            on:click=closest_dialog
        >
            <Icon name=IconName::X class="size-4" />
        </button>
    }
}
