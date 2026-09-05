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
pub fn PanelError(
    error: ApiError,
    #[prop(optional)] on_retry: Option<Callback<()>>,
) -> impl IntoView {
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

/// Send a write and await it, keeping the server's own explanation when it refuses.
///
/// The `await`-shaped counterpart to [`crate::api::submit_reporting`], for a panel that drives its
/// own task rather than a [`crate::api::Save`] signal. Both extract the reason the same way, through
/// [`crate::api::refusal_message`], so a refusal reads identically whichever one reported it.
///
/// The `Err` is always displayable: the server's sentence when the body carried one, and the status
/// message when it did not.
pub async fn write_reporting(
    method: crate::api::Method,
    path: &str,
    body: Option<&str>,
) -> Result<String, String> {
    let response = crate::api::request_detailed(method, path, body)
        .await
        .map_err(|error| error.message().to_owned())?;

    if response.ok {
        return Ok(response.body);
    }
    Err(crate::api::refusal_message(&response))
}

pub mod catalog;
pub mod cli_tools;
pub mod combos;
pub mod controls;
pub mod headroom;
pub mod keys;
pub mod login;
pub mod logs;
pub mod models;
pub mod oauth_import;
pub mod overview;
pub mod pricing;
pub mod provider_nodes;
pub mod providers;
pub mod proxy_pools;
pub mod pxpipe;
pub mod settings;
pub mod status;
pub mod translator;
pub mod tunnel;
pub mod types;
pub mod usage;

pub use cli_tools::CliTools;
pub use combos::Combos;
pub use headroom::Headroom;
pub use keys::Keys;
pub use login::Login;
pub use logs::Logs;
pub use models::Models;
pub use overview::Overview;
pub use pricing::Pricing;
pub use provider_nodes::ProviderNodes;
pub use providers::Providers;
pub use proxy_pools::ProxyPools;
pub use pxpipe::Pxpipe;
pub use settings::Settings;
pub use status::StatusPage;
pub use tunnel::Tunnel;
pub use usage::Usage;

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

/// OAuth callback mount: the grant stays in the address bar for the opener to read.
#[component]
pub fn Callback() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <div class="min-h-dvh grid place-items-center bg-background p-6">
            <section class="w-full max-w-sm rounded-lg border border-border bg-card p-6 space-y-2 text-center">
                <h1 class="text-xl font-semibold tracking-tight">
                    {locale.get("callback.title").to_owned()}
                </h1>
                <p class="text-sm text-muted-foreground">
                    {locale.get("callback.copy").to_owned()}
                </p>
            </section>
        </div>
    }
}
