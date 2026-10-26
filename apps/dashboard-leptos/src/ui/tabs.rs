//! Tabs: switching between views of the same subject without leaving the page.
//!
//! Index-driven rather than string-driven. The previous component layer took a `value` string per
//! trigger and a `value` per panel, which meant a typo in one of them silently produced a tab that
//! showed nothing. Here the tab list is a `&[&str]` and the selected index is the state, so a
//! mismatched pair is not representable.
//!
//! `role="tablist"`/`role="tab"`/`role="tabpanel"` with `aria-selected` and `aria-controls` are what
//! make this navigable with a screen reader. The arrow-key handling is deliberate: WAI-ARIA's tabs
//! pattern expects Left/Right to move between tabs, and a `<button>` does not do that natively.

use leptos::prelude::*;

/// The tab strip.
///
/// `selected` is the index into `labels`, and `on_select` receives a new one.
#[component]
pub fn Tabs(
    /// Tab labels, in order.
    #[prop(into)]
    labels: Signal<Vec<String>>,
    /// The selected index.
    #[prop(into)]
    selected: Signal<usize>,
    #[prop(optional)] on_select: Option<Callback<usize>>,
    #[prop(optional, into)] class: Option<String>,
) -> impl IntoView {
    let class = format!(
        "inline-flex items-center gap-0.5 rounded-lg border border-border bg-muted/50 p-0.5 {}",
        class.unwrap_or_default(),
    );

    // Arrow keys move between tabs and wrap: at the last tab, Right returns to the first. Enter and
    // Space are already handled by `<button>`.
    let handle_key = move |event: leptos::ev::KeyboardEvent| {
        let Some(callback) = on_select else {
            return;
        };
        let count = labels.get_untracked().len();
        if count == 0 {
            return;
        }
        let current = selected.get_untracked();
        match event.key().as_str() {
            "ArrowRight" => callback.run((current + 1) % count),
            "ArrowLeft" => callback.run((current + count - 1) % count),
            _ => {}
        }
    };

    view! {
        <div class=class role="tablist" on:keydown=handle_key>
            {move || {
                let active = selected.get();
                labels
                    .get()
                    .into_iter()
                    .enumerate()
                    .map(|(index, label)| {
                        let is_active = index == active;
                        view! {
                            <button
                                type="button"
                                role="tab"
                                class="rounded-[6px] px-3 py-1 text-[0.8125rem] font-medium \
                                       transition-colors outline-none \
                                       focus-visible:ring-[3px] focus-visible:ring-ring/50"
                                class=("bg-background text-foreground shadow-xs", is_active)
                                class=("text-muted-foreground hover:text-foreground", !is_active)
                                aria-selected=is_active.to_string()
                                tabindex=if is_active { "0" } else { "-1" }
                                on:click=move |_| {
                                    if let Some(callback) = on_select {
                                        callback.run(index);
                                    }
                                }
                            >
                                {label}
                            </button>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}

/// One tab's content, shown only while its index is selected.
///
/// Rendered conditionally rather than hidden with CSS: a hidden tab still mounts its children, which
/// for a panel with an SSE subscription means a stream running behind a tab nobody is looking at.
#[component]
pub fn TabContent(
    #[prop(into)] index: usize,
    #[prop(into)] selected: Signal<usize>,
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] mut children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "animate-in fade-in duration-150 {}",
        class.unwrap_or_default()
    );
    view! {
        {move || {
            (selected.get() == index)
                .then(|| view! { <div class=class.clone()>{children.take().map(|c| c())}</div> })
        }}
    }
}
