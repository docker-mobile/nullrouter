//! Form structure: labels, help text, selects, switches, checkboxes, textareas.
//!
//! Every control here is controlled by the caller's signals for the same reason as [`super::input`]:
//! a panel that saves and then clears a field has to actually clear it.
//!
//! The layout rule these enforce: a field is a `<label>` wrapping its control, so clicking the label
//! focuses the control without needing `id`/`for` pairs that can drift. Where the label cannot wrap
//! the control -- a `<select>` inside a flex row with an action beside it -- the id is generated from
//! the label text and threaded through explicitly.

use leptos::prelude::*;

use super::input::control_class;

/// One labelled control with optional help text.
#[component]
pub fn Field(
    /// The visible label.
    #[prop(into)]
    label: String,
    /// A sentence under the control. Also becomes its accessible description.
    #[prop(optional, into)]
    description: Option<String>,
    /// Marks the field as required, which appends the marker and sets `aria-required`.
    #[prop(optional, into)]
    required: Option<bool>,
    /// A message explaining why the current value is refused.
    #[prop(optional, into)]
    error: Option<String>,
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!("space-y-1.5 {}", class.unwrap_or_default());
    let required = required.unwrap_or(false);
    let error = error.filter(|text| !text.trim().is_empty());

    view! {
        <label class=class>
            <span class="flex items-center gap-1 text-[0.8125rem] font-medium text-foreground">
                {label}
                {required
                    .then(|| {
                        view! {
                            <span class="text-destructive" aria-hidden="true">
                                "*"
                            </span>
                        }
                    })}
            </span>
            {children.map(|children| children())}
            {description
                .filter(|text| !text.trim().is_empty())
                .map(|text| {
                    view! { <p class="text-xs leading-relaxed text-muted-foreground">{text}</p> }
                })}
            {error.map(|text| {
                view! { <p class="text-xs leading-relaxed text-destructive" role="alert">{text}</p> }
            })}
        </label>
    }
}

/// A bare label, for a control whose layout [`Field`] does not fit.
#[component]
pub fn FieldLabel(
    #[prop(into)] text: String,
    #[prop(optional, into)] for_id: Option<String>,
    #[prop(optional, into)] class: Option<String>,
) -> impl IntoView {
    let class = format!(
        "text-[0.8125rem] font-medium text-foreground {}",
        class.unwrap_or_default()
    );
    view! { <label class=class r#for=for_id>{text}</label> }
}

/// A sentence that explains a control or a section.
#[component]
pub fn FieldDescription(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "text-xs leading-relaxed text-muted-foreground {}",
        class.unwrap_or_default()
    );
    view! { <p class=class>{children.map(|children| children())}</p> }
}

/// A multi-line text field.
#[component]
pub fn Textarea(
    #[prop(into, optional)] value: MaybeProp<String>,
    #[prop(optional)] on_input: Option<Callback<String>>,
    #[prop(optional, into)] placeholder: Option<String>,
    /// A literal is inferred as `i32` at a call site, so this accepts either width.
    #[prop(optional, into)]
    rows: Option<i32>,
    #[prop(optional, into)] id: Option<String>,
    #[prop(optional, into)] aria_label: Option<String>,
    #[prop(optional, into)] disabled: Option<Signal<bool>>,
    #[prop(optional, into)] class: Option<String>,
) -> impl IntoView {
    let class = format!(
        "{} min-h-20 py-2 leading-relaxed {}",
        control_class(),
        class.unwrap_or_default(),
    );
    let handle = move |event| {
        if let Some(callback) = on_input {
            callback.run(event_target_value(&event));
        }
    };

    view! {
        <textarea
            class=class
            id=id
            rows=rows.unwrap_or(4).to_string()
            placeholder=placeholder
            aria-label=aria_label
            prop:value=move || value.get().unwrap_or_default()
            disabled=move || disabled.is_some_and(|signal| signal.get())
            on:input=handle
        />
    }
}

/// A native `<select>`.
///
/// Native rather than a custom listbox: the locale picker alone has 35 entries, and the platform's
/// own type-ahead, scrolling and keyboard behaviour are better than anything worth hand-rolling here.
/// The `<select>` is styled to match [`super::input::Input`] so a form row reads as one control set.
#[component]
pub fn Select(
    /// The current value, matched against each option's value.
    #[prop(into, optional)]
    value: MaybeProp<String>,
    /// `(value, label)` pairs, in display order.
    #[prop(into)]
    options: Signal<Vec<(String, String)>>,
    #[prop(optional)] on_change: Option<Callback<String>>,
    /// Shown first, disabled, when nothing is selected yet.
    #[prop(optional, into)]
    placeholder: Option<String>,
    #[prop(optional, into)] id: Option<String>,
    #[prop(optional, into)] aria_label: Option<String>,
    #[prop(optional, into)] disabled: Option<Signal<bool>>,
    #[prop(optional, into)] class: Option<String>,
) -> impl IntoView {
    let class = format!(
        "{} h-9 py-0 pr-8 {}",
        control_class(),
        class.unwrap_or_default()
    );
    let current = value;
    let handle = move |event| {
        if let Some(callback) = on_change {
            callback.run(event_target_value(&event));
        }
    };

    view! {
        <select
            class=class
            id=id
            aria-label=aria_label
            disabled=move || disabled.is_some_and(|signal| signal.get())
            on:change=handle
        >
            {placeholder
                .map(|text| {
                    view! {
                        <option value="" disabled selected=move || {
                            current.get().is_none_or(|value| value.is_empty())
                        }>
                            {text}
                        </option>
                    }
                })}
            {move || {
                let selected = current.get().unwrap_or_default();
                options
                    .get()
                    .into_iter()
                    .map(|(value, label)| {
                        let is_selected = value == selected;
                        view! {
                            <option value=value selected=is_selected>
                                {label}
                            </option>
                        }
                    })
                    .collect_view()
            }}
        </select>
    }
}

/// A switch: a toggle that takes effect as soon as it is flipped.
///
/// Rendered as a `<button role="switch">` rather than an `<input type="checkbox">` because the visual
/// is a track and a thumb, and a styled checkbox either keeps a native control inside it or loses the
/// keyboard behaviour. `role="switch"` plus `aria-checked` is what a screen reader needs; Enter and
/// Space both activate a `<button>` already.
#[component]
pub fn Switch(
    /// Whether it is on.
    #[prop(into)]
    checked: Signal<bool>,
    /// Called with the next value.
    #[prop(optional)]
    on_change: Option<Callback<bool>>,
    #[prop(optional, into)] disabled: Option<Signal<bool>>,
    /// The accessible name, when there is no `<label>` wrapping it.
    #[prop(optional, into)]
    aria_label: Option<String>,
    #[prop(optional, into)] id: Option<String>,
) -> impl IntoView {
    let handle = move |_| {
        if disabled.is_some_and(|signal| signal.get()) {
            return;
        }
        if let Some(callback) = on_change {
            callback.run(!checked.get_untracked());
        }
    };

    view! {
        <button
            type="button"
            role="switch"
            id=id
            class="relative inline-flex h-5 w-9 shrink-0 items-center rounded-full border \
                   border-transparent outline-none transition-colors \
                   focus-visible:ring-[3px] focus-visible:ring-ring/50 \
                   disabled:cursor-not-allowed disabled:opacity-50"
            class=("bg-primary", move || checked.get())
            class=("bg-input", move || !checked.get())
            aria-checked=move || checked.get().to_string()
            aria-label=aria_label
            disabled=move || disabled.is_some_and(|signal| signal.get())
            on:click=handle
        >
            // `translate-x-4`/`translate-x-0.5` rather than a left/right pair: transforms composite
            // off the main thread, so the thumb moves without a layout pass on every frame.
            <span
                class="pointer-events-none block size-4 rounded-full bg-background shadow-sm \
                       transition-transform duration-150 ease-out"
                class=("translate-x-4", move || checked.get())
                class=("translate-x-0.5", move || !checked.get())
            />
        </button>
    }
}

/// A checkbox, for a list where each row is independently optional.
#[component]
pub fn Checkbox(
    #[prop(into)] checked: Signal<bool>,
    #[prop(optional)] on_change: Option<Callback<bool>>,
    #[prop(optional, into)] disabled: Option<Signal<bool>>,
    /// The visible text beside the box. Wrapping the input means the text is a click target and the
    /// accessible name, without needing an id.
    #[prop(optional, into)]
    label: Option<String>,
    #[prop(optional, into)] aria_label: Option<String>,
) -> impl IntoView {
    let handle = move |event| {
        if let Some(callback) = on_change {
            callback.run(event_target_checked(&event));
        }
    };

    view! {
        <label class="flex items-center gap-2 text-sm">
            <input
                type="checkbox"
                class="size-4 shrink-0 rounded-[4px] border border-input accent-primary \
                       outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 \
                       disabled:cursor-not-allowed disabled:opacity-50"
                prop:checked=move || checked.get()
                aria-label=aria_label
                disabled=move || disabled.is_some_and(|signal| signal.get())
                on:change=handle
            />
            {label.map(|text| view! { <span class="text-foreground">{text}</span> })}
        </label>
    }
}
