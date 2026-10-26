//! Page structure: the heading every route opens with, and the three states a loaded panel renders.
//!
//! # The rule this module exists to keep
//!
//! A panel is either loading, showing data the server actually sent, or explaining why it has none.
//! [`Panel`] is the only place those three are rendered, so there is no path through a route that
//! draws invented data or draws a failure as an empty result. That matters more on a dashboard than
//! it sounds: a toggle that silently discards writes and a list of accounts that do not exist both
//! look exactly like a working page.

use leptos::prelude::*;

use crate::api::{ApiError, Hydrate};
use crate::ui::{Icon, IconName};

/// A route's heading: title, one-line description, and the actions that apply to the whole page.
///
/// The title is an `<h1>` and there is exactly one per route, which is what makes the document
/// outline navigable. `border-b` under it separates the heading from the content without wrapping
/// either in a container.
#[component]
pub fn PageHeader(
    #[prop(into)] title: String,
    #[prop(optional, into)] description: Option<String>,
    /// A short line above the title naming the section this page belongs to.
    #[prop(optional, into)]
    eyebrow: Option<String>,
    /// The page's primary action, plus any secondary ones. Passed as children so a call site reads as
    /// `view! { <PageHeader title=…><Button …/></PageHeader> }`.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    view! {
        <header class="flex flex-col gap-3 border-b border-border pb-5 sm:flex-row sm:items-end sm:justify-between">
            <div class="min-w-0 space-y-1">
                {eyebrow
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| {
                        view! {
                            <p class="text-[0.6875rem] font-medium uppercase tracking-wider text-muted-foreground">
                                {text}
                            </p>
                        }
                    })}
                <h1 class="truncate text-xl font-semibold tracking-tight text-foreground">
                    {title}
                </h1>
                {description
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| {
                        view! {
                            <p class="max-w-3xl text-sm leading-relaxed text-muted-foreground">
                                {text}
                            </p>
                        }
                    })}
            </div>
            {children
                .map(|actions| {
                    view! {
                        <div class="flex shrink-0 flex-wrap items-center gap-2">{actions()}</div>
                    }
                })}
        </header>
    }
}

/// The vertical rhythm between a heading and the sections under it.
///
/// One wrapper rather than a `space-y-*` on `<main>`: the gap between the header and the first section
/// is deliberately larger than the gap between sections, and a single `space-y` cannot express that.
#[component]
pub fn PageBody(#[prop(optional)] children: Option<Children>) -> impl IntoView {
    view! {
        <div class="space-y-4 pt-5">
            {children.map(|children| children())}
        </div>
    }
}

/// A row of trailing page actions.
#[component]
pub fn PageActions(#[prop(optional)] children: Option<Children>) -> impl IntoView {
    view! {
        <div class="flex flex-wrap items-center gap-2">
            {children.map(|children| children())}
        </div>
    }
}

/// Renders the three states of a [`Hydrate`] and nothing else.
///
/// `on_retry` is only honoured when the error could plausibly succeed on a second attempt: offering
/// to retry a 404 or a 403 is a false promise, so [`ApiError::is_retryable`] decides rather than the
/// call site.
#[component]
pub fn Panel<T, V, F>(
    state: ReadSignal<Hydrate<T>>,
    children: F,
    #[prop(optional)] on_retry: Option<Callback<()>>,
) -> impl IntoView
where
    T: Clone + Send + Sync + 'static,
    V: IntoView + 'static,
    F: Fn(T) -> V + Send + Sync + 'static,
{
    view! {
        {move || match state.get() {
            Hydrate::Loading => view! { <PanelSkeleton /> }.into_any(),
            Hydrate::Ready(data) => {
                view! { <div class="animate-in fade-in duration-200">{children(data)}</div> }
                    .into_any()
            }
            Hydrate::Failed(error) => {
                if error.is_retryable() {
                    on_retry
                        .map_or_else(
                            || view! { <PanelError error=error /> }.into_any(),
                            |retry| view! { <PanelError error=error on_retry=retry /> }.into_any(),
                        )
                } else {
                    view! { <PanelError error=error /> }.into_any()
                }
            }
        }}
    }
}

/// A loading placeholder shaped like content.
///
/// A spinner in the middle of an empty panel reads as "stuck"; a skeleton the rough shape of what is
/// coming reads as "nearly there". The three bars are staggered so the pulse travels down the block
/// instead of strobing as one.
#[component]
pub fn PanelSkeleton() -> impl IntoView {
    view! {
        <div class="space-y-2.5" aria-busy="true" aria-live="polite">
            <div class="h-4 w-1/3 rounded bg-muted animate-pulse" />
            <div class="h-4 w-2/3 rounded bg-muted animate-pulse [animation-delay:75ms]" />
            <div class="h-4 w-1/2 rounded bg-muted animate-pulse [animation-delay:150ms]" />
            <span class="sr-only">"Loading"</span>
        </div>
    }
}

/// A failure, with the remedy when there is one.
///
/// Uses `role="alert"` so it is announced when it replaces a spinner, and an icon rather than only
/// colour so it does not depend on seeing red.
#[component]
pub fn PanelError(
    error: ApiError,
    #[prop(optional)] on_retry: Option<Callback<()>>,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let retry_label = locale.get("error.retry").to_owned();
    let show_retry = error.is_retryable() && on_retry.is_some();

    view! {
        <div
            class="flex items-start gap-3 rounded-lg border border-destructive/25 \
                   bg-destructive/[0.04] px-4 py-3 animate-in fade-in slide-in-from-top-1 duration-200"
            role="alert"
        >
            <Icon name=IconName::Alert class="mt-0.5 size-4 text-destructive" />
            <div class="min-w-0 flex-1 space-y-1.5">
                <p class="text-sm leading-relaxed text-foreground">{error.message()}</p>
                {show_retry
                    .then(|| {
                        view! {
                            <button
                                type="button"
                                class="text-[0.8125rem] font-medium text-destructive \
                                       underline-offset-4 hover:underline"
                                on:click=move |_| {
                                    if let Some(callback) = on_retry {
                                        callback.run(());
                                    }
                                }
                            >
                                {retry_label.clone()}
                            </button>
                        }
                    })}
            </div>
        </div>
    }
}
