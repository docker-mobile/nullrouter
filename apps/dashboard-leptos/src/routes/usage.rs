//! Request and token totals, plus a live telemetry stream.
//!
//! Two sources, kept visibly separate. `/api/usage/stats` is what the state service recorded, and
//! the `usage` SSE stream is what is happening now. Merging them into one set of numbers would make
//! a stalled stream look like a quiet router, which is the opposite of what someone watching this
//! panel needs to know.

use leptos::prelude::*;

use crate::api::sse::{self, Connection};
use crate::api::{Hydrate, load};
use crate::routes::types::{UsageLive, UsageStats};
use crate::routes::{PageHeader, Panel};

#[component]
pub fn Usage() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (connection, set_connection) = signal(Connection::default());

    view! {
        <PageHeader
            title=locale.get("nav.usage").to_owned()
            description=locale.get("usage.description").to_owned()
        >
            <StreamIndicator connection=connection />
        </PageHeader>
        <div class="grid gap-4 md:grid-cols-2 mb-6">
            <LivePanel set_connection=set_connection />
            <RecordedPanel />
        </div>
    }
}

/// Whether the live stream is currently delivering.
#[component]
fn StreamIndicator(connection: ReadSignal<Connection>) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2 text-xs">
            <span class=move || {
                if connection.get().is_live() {
                    "size-1.5 rounded-full bg-success animate-pulse"
                } else {
                    "size-1.5 rounded-full bg-muted-foreground/40"
                }
            } />
            <span class="text-muted-foreground">
                {move || connection.get().label().to_owned()}
            </span>
        </div>
    }
}

/// Telemetry from the events service, replaced on every frame.
#[component]
fn LivePanel(set_connection: WriteSignal<Connection>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (live, set_live) = signal(UsageLive::default());

    let stream = StoredValue::new(None::<sse::Stream>);
    Effect::new(move || {
        stream.set_value(sse::subscribe_named(
            "/api/usage/stream",
            "usage",
            set_connection,
            move |frame: UsageLive| set_live.set(frame),
        ));
    });

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-2">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("usage.live").to_owned()}
            </h2>
            {move || {
                let frame = live.get();
                let locale = crate::i18n::use_locale();
                view! {
                    <dl class="space-y-2 text-sm">
                        <Stat
                            label=locale.get("usage.active").to_owned()
                            value=frame.active_requests.to_string()
                        />
                        <Stat
                            label=locale.get("usage.today_requests").to_owned()
                            value=frame.requests_today.to_string()
                        />
                        <Stat
                            label=locale.get("usage.today_tokens").to_owned()
                            value=frame.tokens_today.to_string()
                        />
                        <Stat
                            label=locale.get("usage.cost").to_owned()
                            value=cost_or_unknown(frame.estimated_cost)
                        />
                    </dl>
                }
            }}
        </section>
    }
}

/// Totals the state service recorded over the selected window.
#[component]
fn RecordedPanel() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (stats, set_stats) = signal(Hydrate::<UsageStats>::Loading);
    let reload = move || {
        set_stats.set(Hydrate::Loading);
        load("/api/usage/stats?period=7d", set_stats);
    };
    reload();

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-2">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("usage.window").to_owned()}
            </h2>
            <Panel
                state=stats
                on_retry=Callback::new(move |()| reload())
                children=move |data: UsageStats| {
                    let locale = crate::i18n::use_locale();
                    view! {
                        <dl class="space-y-2 text-sm">
                            <Stat
                                label=locale.get("usage.requests").to_owned()
                                value=data.total_requests.to_string()
                            />
                            <Stat
                                label=locale.get("usage.prompt").to_owned()
                                value=data.total_prompt_tokens.to_string()
                            />
                            <Stat
                                label=locale.get("usage.completion").to_owned()
                                value=data.total_completion_tokens.to_string()
                            />
                            <Stat
                                label=locale.get("usage.cached").to_owned()
                                value=data.total_cached_tokens.to_string()
                            />
                        </dl>
                    }
                }
            />
        </section>
    }
}

/// An absent cost reads as unknown rather than as zero, which would be a claim.
fn cost_or_unknown(cost: String) -> String {
    if cost.is_empty() {
        "—".to_owned()
    } else {
        cost
    }
}

#[component]
fn Stat(label: String, value: String) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between gap-4">
            <dt class="text-muted-foreground">{label}</dt>
            <dd class="font-mono">{value}</dd>
        </div>
    }
}
