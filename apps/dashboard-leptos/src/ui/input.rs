//! Text inputs.
//!
//! Fully controlled: the caller owns the `ReadSignal`/`WriteSignal` pair and this renders whatever it
//! holds. The previous component layer kept its own copy seeded from a `value` prop, which meant a
//! field that was programmatically cleared -- after a successful create, or on "cancel" -- kept
//! showing what had been typed, because re-rendering the component never reset the internal signal.

use leptos::prelude::*;

/// The class list shared by every control that looks like a text box.
///
/// One function rather than a copy per component, because a `<select>` that sits beside an `<input>`
/// has to match it to the pixel: a one-pixel height difference in a form row is the first thing the
/// eye catches.
pub const fn control_class() -> &'static str {
    "w-full min-w-0 rounded-md border border-input bg-transparent px-2.5 \
     text-sm text-foreground shadow-xs outline-none transition-[color,box-shadow,border-color] \
     placeholder:text-muted-foreground \
     focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40 \
     disabled:cursor-not-allowed disabled:opacity-50 \
     aria-invalid:border-destructive aria-invalid:ring-destructive/25 \
     dark:bg-input/20"
}

/// A single-line text field.
#[component]
pub fn Input(
    /// The current value. The caller keeps it in a signal.
    #[prop(into, optional)]
    value: MaybeProp<String>,
    /// Called with the new value on every keystroke.
    #[prop(optional)]
    on_input: Option<Callback<String>>,
    #[prop(optional, into)] placeholder: Option<String>,
    /// A native input type: `text`, `password`, `number`, `search`, `url`.
    #[prop(optional, into)]
    input_type: Option<String>,
    #[prop(optional, into)] name: Option<String>,
    #[prop(optional, into)] id: Option<String>,
    #[prop(optional, into)] autocomplete: Option<String>,
    #[prop(optional, into)] disabled: Option<Signal<bool>>,
    /// Marks the field as refusing its current content, which draws the destructive border.
    #[prop(optional, into)]
    invalid: Option<Signal<bool>>,
    /// The accessible name when there is no visible `<label>` pointing at the field.
    #[prop(optional, into)]
    aria_label: Option<String>,
    /// Describes the field for a screen reader, pointing at a help paragraph's id.
    #[prop(optional, into)]
    described_by: Option<String>,
    /// `text-[13px]`-style override or a width; appended after the base classes.
    #[prop(optional, into)]
    class: Option<String>,
) -> impl IntoView {
    let class = format!("{} h-9 {}", control_class(), class.unwrap_or_default());
    let handle = move |event| {
        if let Some(callback) = on_input {
            callback.run(event_target_value(&event));
        }
    };

    view! {
        <input
            type=input_type.unwrap_or_else(|| "text".to_owned())
            class=class
            id=id
            name=name
            autocomplete=autocomplete
            placeholder=placeholder
            aria-label=aria_label
            aria-describedby=described_by
            aria-invalid=move || invalid.is_some_and(|signal| signal.get()).then_some("true")
            prop:value=move || value.get().unwrap_or_default()
            disabled=move || disabled.is_some_and(|signal| signal.get())
            on:input=handle
        />
    }
}

/// An input with a leading glyph or a trailing button.
///
/// Used where the affordance is a hint rather than a label -- a magnifier on a filter box, a copy
/// button on a secret. `start`/`end` are views so the caller decides what the adornment is; this owns
/// only the inset that keeps the text from running under it.
#[component]
pub fn InputGroup(
    #[prop(optional)] start: Option<ViewFn>,
    #[prop(optional)] end: Option<ViewFn>,
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "flex w-full items-center gap-2 rounded-md border border-input bg-transparent \
         px-2.5 shadow-xs transition-[color,box-shadow,border-color] \
         focus-within:border-ring focus-within:ring-[3px] focus-within:ring-ring/40 \
         dark:bg-input/20 {}",
        class.unwrap_or_default(),
    );

    view! {
        <div class=class>
            {start.map(|start| view! { <span class="text-muted-foreground">{start.run()}</span> })}
            <div class="min-w-0 flex-1">{children.map(|children| children())}</div>
            {end.map(|end| end.run())}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::control_class;

    #[test]
    fn the_control_class_carries_the_states_that_matter() {
        let class = control_class();
        // A focus ring, a disabled state, and an invalid state are what make a form usable and
        // legible; losing any one of them is a silent regression.
        assert!(class.contains("focus-visible:ring"), "{class}");
        assert!(class.contains("disabled:opacity-50"), "{class}");
        assert!(class.contains("aria-invalid:border-destructive"), "{class}");
    }
}
