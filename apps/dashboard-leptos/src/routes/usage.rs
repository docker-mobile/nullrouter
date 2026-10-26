//! Request and token totals, plus a live telemetry stream.
//!
//! # Why the two sources stay visibly separate
//!
//! `/api/usage/stats` is what the state service recorded; the `usage` stream is what is happening now.
//! Merging them into one set of numbers would make a stalled stream look like a quiet router, which is
//! the opposite of what someone watching this panel needs to know. The live card says when it is live and
//! the recorded card says what window it covers, so a zero in one and a large number in the other is
//! legible as two different facts rather than a contradiction.

use leptos::prelude::*;

use crate::t;

use crate::api::sse::{self, Connection};
use crate::api::{Hydrate, load};
use crate::routes::types::{UsageLive, UsageStats};
use crate::ui::{
    Card, CardContent, CardDescription, CardHeader, CardTitle, ConnectionDot, PageBody, PageHeader,
    Panel,
};

#[component]
pub fn Usage() -> impl IntoView {
    let (connection, set_connection) = signal(Connection::default());

    let heading = t!("nav.usage");
    let blurb = t!("usage.description");
    let live_label = Signal::derive(move || connection.get().label().to_owned());
    let live = Signal::derive(move || connection.get().is_live());

    view! {
        <PageHeader title=heading description=blurb>
            <ConnectionDot live=live label=live_label />
        </PageHeader>

        <PageBody>
            <div class="grid gap-4 lg:grid-cols-2">
                <LivePanel set_connection=set_connection />
                <RecordedPanel />
            </div>
        </PageBody>
    }
}

/// Telemetry from the events service, replaced on every frame.
#[component]
fn LivePanel(set_connection: WriteSignal<Connection>) -> impl IntoView {
    let (live, set_live) = signal(UsageLive::default());

    let stream = StoredValue::new_local(None::<sse::Stream>);
    Effect::new(move |_| {
        stream.set_value(sse::subscribe_named(
            "/api/usage/stream",
            "usage",
            set_connection,
            move |frame: UsageLive| set_live.set(frame),
        ));
    });

    let title = t!("usage.live");
    let blurb = t!("usage.live_hint");

    view! {
        <Card>
            <CardHeader>
                <div class="space-y-1">
                    <CardTitle>{title}</CardTitle>
                    <CardDescription>{blurb}</CardDescription>
                </div>
            </CardHeader>
            <CardContent>
                {move || {
                    let frame = live.get();
                    view! {
                        <div class="grid grid-cols-2 gap-3">
                            <Metric
                                label=t!("usage.active")
                                value=frame.active_requests.to_string()
                                accent=true
                            />
                            <Metric
                                label=t!("usage.today_requests")
                                value=frame.requests_today.to_string()
                            />
                            <Metric
                                label=t!("usage.today_tokens")
                                value=frame.tokens_today.to_string()
                            />
                            <Metric
                                label=t!("usage.cost")
                                value=cost_or_unknown(frame.estimated_cost)
                            />
                        </div>
                    }
                }}
            </CardContent>
        </Card>
    }
}

/// Totals the state service recorded over the selected window.
#[component]
fn RecordedPanel() -> impl IntoView {
    let (stats, set_stats) = signal(Hydrate::<UsageStats>::Loading);
    let reload = move || {
        set_stats.set(Hydrate::Loading);
        load("/api/usage/stats?period=7d", set_stats);
    };
    reload();

    let title = t!("usage.window");
    let blurb = t!("usage.window_hint");

    view! {
        <Card>
            <CardHeader>
                <div class="space-y-1">
                    <CardTitle>{title}</CardTitle>
                    <CardDescription>{blurb}</CardDescription>
                </div>
            </CardHeader>
            <CardContent>
                <Panel
                    state=stats
                    on_retry=Callback::new(move |()| reload())
                    children=move |data: UsageStats| {
                        view! { <RecordedBody data=data /> }
                    }
                />
            </CardContent>
        </Card>
    }
}

#[component]
fn RecordedBody(data: UsageStats) -> impl IntoView {
    view! {
        <div class="grid grid-cols-2 gap-3">
            <Metric label=t!("usage.requests") value=data.total_requests.to_string() />
            <Metric
                label=t!("usage.prompt")
                value=data.total_prompt_tokens.to_string()
            />
            <Metric
                label=t!("usage.completion")
                value=data.total_completion_tokens.to_string()
            />
            <Metric
                label=t!("usage.cached")
                value=data.total_cached_tokens.to_string()
            />
        </div>
    }
}

/// One figure in a card.
///
/// `accent` marks the number the card exists to show. Everything else is the same weight, because a grid
/// where every cell is emphasised is a grid where nothing is.
#[component]
fn Metric(
    label: String,
    value: String,
    #[prop(optional, into)] accent: Option<bool>,
) -> impl IntoView {
    let value_class = if accent.unwrap_or(false) {
        "mt-1 font-mono text-2xl font-semibold tracking-tight tabular-nums text-foreground"
    } else {
        "mt-1 font-mono text-lg font-medium tracking-tight tabular-nums text-foreground"
    };

    view! {
        <div class="rounded-lg border border-border/70 bg-muted/25 px-3 py-2.5">
            <p class="truncate text-xs text-muted-foreground">{label}</p>
            <p class=value_class>{value}</p>
        </div>
    }
}

/// An absent cost reads as unknown rather than as zero, which would be a claim.
fn cost_or_unknown(cost: String) -> String {
    if cost.trim().is_empty() {
        "—".to_owned()
    } else {
        cost
    }
}

#[cfg(test)]
mod tests {
    use super::cost_or_unknown;

    #[test]
    fn a_missing_cost_is_unknown_rather_than_free() {
        // Zero would be a claim about the bill; an em dash is a statement about the payload.
        assert_eq!(cost_or_unknown(String::new()), "—");
        assert_eq!(cost_or_unknown("  ".to_owned()), "—");
        assert_eq!(cost_or_unknown("0.00".to_owned()), "0.00");
        assert_eq!(cost_or_unknown("$1.20".to_owned()), "$1.20");
    }
}
