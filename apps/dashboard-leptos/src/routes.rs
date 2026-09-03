//! The dashboard's sections, and the primitives they share.
//!
//! Every panel that reads server data goes through [`Panel`], which renders the three states of
//! [`crate::api::Hydrate`] and nothing else. That is what keeps the guarantee the API layer exists
//! to make: there is no path through this module that renders data the server did not send, and no
//! path that renders a failure as though it were an empty result.

use leptos::prelude::*;

use crate::api::{ApiError, Hydrate};

/// Section heading with optional description and trailing controls.
#[component]
pub fn PageHeader(
    title: String,
    #[prop(optional, into)] description: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="flex items-start justify-between gap-4 mb-6">
            <div class="space-y-1 min-w-0">
                <h1 class="text-2xl font-semibold tracking-tight truncate">{title}</h1>
                {description
                    .map(|text| view! { <p class="text-sm text-muted-foreground">{text}</p> })}
            </div>
            {children.map(|render| view! { <div class="flex items-center gap-2 shrink-0">{render()}</div> })}
        </div>
    }
}

/// Renders the three states of a [`Hydrate`] and nothing else.
///
/// `retry` is only offered when the error is one that could plausibly succeed on a second attempt.
/// Offering it on a 404 or a 403 is a false promise, so [`ApiError::is_retryable`] decides rather
/// than the call site.
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
            Hydrate::Failed(error) => on_retry.map_or_else(
                || view! { <PanelError error=error /> }.into_any(),
                |retry| view! { <PanelError error=error on_retry=retry /> }.into_any(),
            ),
        }}
    }
}

/// Loading placeholder.
///
/// Shaped roughly like content rather than a spinner: a block that approximates what is coming
/// reads as "nearly there", where a spinner in the middle of an empty panel reads as "stuck".
#[component]
pub fn PanelSkeleton() -> impl IntoView {
    view! {
        <div class="space-y-3" aria-busy="true" aria-live="polite">
            <div class="h-4 w-1/3 rounded bg-muted animate-pulse" />
            <div class="h-4 w-2/3 rounded bg-muted animate-pulse [animation-delay:75ms]" />
            <div class="h-4 w-1/2 rounded bg-muted animate-pulse [animation-delay:150ms]" />
        </div>
    }
}

/// A failure, with the remedy when there is one.
#[component]
pub fn PanelError(error: ApiError, #[prop(optional)] on_retry: Option<Callback<()>>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let retry_label = locale.get("error.retry").to_owned();
    let show_retry = error.is_retryable() && on_retry.is_some();

    view! {
        <div
            class="rounded-lg border border-destructive/30 bg-destructive/5 p-4 \
                   animate-in fade-in slide-in-from-top-1 duration-200"
            role="alert"
        >
            <div class="flex items-start gap-3">
                <svg
                    class="size-5 shrink-0 text-destructive mt-0.5"
                    viewBox="0 0 24 24"
                    fill="currentColor"
                    aria-hidden="true"
                >
                    <path d="M12 2 1 21h22L12 2zm1 15h-2v-2h2v2zm0-4h-2V9h2v4z" />
                </svg>
                <div class="min-w-0 flex-1 space-y-2">
                    <p class="text-sm text-foreground">{error.message()}</p>
                    {show_retry
                        .then(|| {
                            view! {
                                <button
                                    type="button"
                                    class="text-sm font-medium text-destructive underline-offset-4 hover:underline"
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
        </div>
    }
}

/// A section that has no content yet.
///
/// Explicit rather than an empty div: a section that is reachable from navigation but not yet built
/// should say so, not look like a section whose data failed to load.
#[component]
fn Placeholder(section: &'static str) -> impl IntoView {
    view! {
        <div class="rounded-lg border border-dashed border-border p-8 text-center">
            <p class="text-sm text-muted-foreground">
                {format!("{section} is not built yet.")}
            </p>
        </div>
    }
}

// Public rather than private-with-re-export: `unreachable_pub` does not model `pub use`, so a
// private module holding `pub` components warns on every one of them, and narrowing the components
// instead makes the re-export fail with E0364.
pub mod logs;
pub mod overview;

pub use logs::Logs;
pub use overview::Overview;

#[component]
pub fn Routing() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <PageHeader title=locale.get("nav.routing").to_owned() />
        <Placeholder section="Routing" />
    }
}

#[component]
pub fn Keys() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <PageHeader title=locale.get("nav.keys").to_owned() />
        <Placeholder section="API keys" />
    }
}

#[component]
pub fn Usage() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <PageHeader title=locale.get("nav.usage").to_owned() />
        <Placeholder section="Usage" />
    }
}

#[component]
pub fn Settings() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <PageHeader title=locale.get("nav.settings").to_owned() />
        <Placeholder section="Settings" />
    }
}

/// Shown for any path the router does not recognise.
#[component]
pub fn NotFound() -> impl IntoView {
    view! {
        <div class="grid place-items-center py-16 text-center">
            <div class="space-y-2">
                <p class="text-4xl font-semibold tracking-tight text-muted-foreground">"404"</p>
                <p class="text-sm text-muted-foreground">"That page does not exist."</p>
            </div>
        </div>
    }
}
