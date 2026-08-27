//! The Usage panel.
//!
//! Everything here is server-owned. The panel previously rendered
//! `usage_snapshot()` — a fixture of zeros — which meant a user could not tell a
//! quiet router from an unwired one. Totals, breakdowns, the request log, the
//! 10-minute strip, and the live counters now all come from `/api/usage/*`, and
//! loading, empty, and failed are three visually distinct states.
//!
//! Derivations live in [`crate::dashboard::usage_live`] so they are testable on
//! the native target; this file is markup and wiring only.

use crate::api::{self, ApiError, Hydrate};
use crate::dashboard::usage_live::{
    LiveChanges, LiveUsage, NO_READING, SparkBar, StreamState, UsageBreakdownRow, UsageLogEntry,
    UsagePeriod, UsageStats, format_age, format_cost, format_count, format_latency,
    format_optional_count, sparkline, sparkline_summary,
};
use crate::dashboard::{UsageProviderNode, usage_snapshot};
use leptos::prelude::*;

const USAGE_STYLES: &str = r"
.nr-usage-metrics{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:10px}
.nr-usage-layout{display:grid;grid-template-columns:minmax(0,2fr) minmax(280px,.85fr);gap:10px;align-items:stretch}
.nr-usage-topology,.nr-usage-log{display:grid;gap:14px}
.nr-topology-canvas{position:relative;min-height:320px;overflow:hidden;border:1px solid var(--border-dark);border-radius:8px;background:var(--surface-dark-2)}
.nr-topology-lines{position:absolute;inset:0;width:100%;height:100%;color:var(--border-dark);opacity:.78}
.nr-router-node,.nr-topology-provider{position:absolute;z-index:1;border:1px solid var(--border-dark);border-radius:8px;background:var(--surface-dark);box-shadow:var(--shadow-soft)}
.nr-router-node{left:50%;top:50%;width:112px;min-height:66px;display:grid;place-items:center;gap:3px;transform:translate(-50%,-50%);border-color:var(--brand);color:var(--brand)}
.nr-router-node strong{font-size:1.08rem;color:var(--text-main-dark)}
.nr-router-node span,.nr-topology-provider small,.nr-usage-empty span{color:var(--text-muted-dark);font-size:.78rem}
.nr-topology-provider{min-width:154px;display:grid;grid-template-columns:auto minmax(0,1fr);align-items:center;gap:8px;padding:9px 10px;transform:translate(-50%,-50%);opacity:.72}
.nr-topology-provider.is-active{opacity:1;border-color:var(--brand)}
.nr-topology-provider .nr-provider-logo{width:30px;height:30px}
.nr-topology-copy{min-width:0;display:flex;flex-wrap:wrap;align-items:baseline;column-gap:4px;row-gap:1px}
.nr-topology-provider strong{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:.9rem}
.nr-topology-provider small{white-space:nowrap}
.slot-one{left:50%;top:13%}.slot-two{left:82%;top:30%}.slot-three{left:82%;top:70%}.slot-four{left:50%;top:87%}.slot-five{left:18%;top:70%}.slot-six{left:18%;top:30%}
.nr-usage-log-list{display:grid;gap:8px}.nr-usage-empty{margin-top:0}
@media (max-width:1180px){.nr-usage-metrics{grid-template-columns:repeat(2,minmax(0,1fr))}.nr-usage-layout{grid-template-columns:1fr}}
@media (max-width:680px){.nr-usage-metrics{grid-template-columns:1fr}.nr-topology-canvas{display:grid;gap:8px;min-height:auto;padding:10px}.nr-topology-lines{display:none}.nr-router-node,.nr-topology-provider{position:static;transform:none;width:100%;min-width:0}.nr-router-node{min-height:56px}}
";

/// Everything the panel's subtrees read.
#[derive(Clone, Copy)]
struct UsageSignals {
    period: ReadSignal<UsagePeriod>,
    set_period: WriteSignal<UsagePeriod>,
    stats: ReadSignal<Hydrate<UsageStats>>,
    set_stats: WriteSignal<Hydrate<UsageStats>>,
    logs: ReadSignal<Hydrate<Vec<UsageLogEntry>>>,
    set_logs: WriteSignal<Hydrate<Vec<UsageLogEntry>>>,
    live: ReadSignal<LiveUsage>,
    changes: ReadSignal<LiveChanges>,
    stream: ReadSignal<StreamState>,
}

impl UsageSignals {
    /// Re-request stats and logs for the current period.
    fn reload(self) {
        self.set_stats.set(Hydrate::Loading);
        self.set_logs.set(Hydrate::Loading);
        api::hydrate(
            self.period.get_untracked().stats_path(),
            self.set_stats,
            crate::dashboard::usage_live::parse_stats,
        );
        api::hydrate(
            "/api/usage/logs",
            self.set_logs,
            crate::dashboard::usage_live::parse_logs,
        );
    }

    /// The newest timestamp any source has reported, used as "now" for ages.
    ///
    /// Derived from the data rather than the browser clock so a request's age is
    /// measured against the router's own sense of time.
    fn clock(self) -> u64 {
        let from_stats = self.stats.get().ready().map_or(0, |stats| {
            stats
                .last_ten_minutes
                .last()
                .map_or(0, |minute| minute.timestamp.saturating_add(60_000))
        });
        let from_logs = self
            .logs
            .get()
            .ready()
            .and_then(|entries| entries.first().map(|entry| entry.timestamp))
            .unwrap_or(0);
        from_stats.max(from_logs)
    }
}

#[component]
pub(super) fn UsagePanel() -> impl IntoView {
    let (period, set_period) = signal(UsagePeriod::default());
    let (stats, set_stats) = signal(Hydrate::<UsageStats>::Loading);
    let (logs, set_logs) = signal(Hydrate::<Vec<UsageLogEntry>>::Loading);
    let (live, set_live) = signal(LiveUsage::default());
    let (changes, set_changes) = signal(LiveChanges::default());
    let (stream, set_stream) = signal(StreamState::default());

    let signals = UsageSignals {
        period,
        set_period,
        stats,
        set_stats,
        logs,
        set_logs,
        live,
        changes,
        stream,
    };

    // Re-fetch whenever the period changes; the first run is the initial load.
    Effect::new(move |_| {
        let selected = period.get();
        set_stats.set(Hydrate::Loading);
        api::hydrate(
            selected.stats_path(),
            set_stats,
            crate::dashboard::usage_live::parse_stats,
        );
    });
    Effect::new(move |_| {
        api::hydrate(
            "/api/usage/logs",
            set_logs,
            crate::dashboard::usage_live::parse_logs,
        );
    });
    subscribe_usage_stream(set_live, set_changes, set_stream);

    view! {
        <style>{USAGE_STYLES}</style>
        <div class="nr-panel-stack">
            <LiveHeader signals />
            <StatsTotals signals />
            <MinuteStrip signals />
            <div class="nr-usage-breakdowns">
                <BreakdownCard signals dimension=Breakdown::Provider />
                <BreakdownCard signals dimension=Breakdown::Model />
            </div>
            <div class="nr-usage-layout">
                <TopologyCard signals />
                <RequestLogCard signals />
            </div>
        </div>
    }
}

/// Live counters plus the period selector.
///
/// The counters are the SSE feed, so they carry the `aria-live` region: a screen
/// reader hears each update without the whole panel being re-announced.
#[component]
fn LiveHeader(signals: UsageSignals) -> impl IntoView {
    let stream = signals.stream;
    let live = signals.live;
    let changes = signals.changes;

    view! {
        <article class="nr-card">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"use"</span>"Usage"</h2>
                    <p>"Live counters stream from the local router; the totals below cover the selected window."</p>
                </div>
                <span
                    class=move || format!("nr-status-pill {}", stream.get().class_name())
                    aria-label=move || format!("Live usage stream: {}", stream.get().label())
                >
                    <span></span>
                    {move || stream.get().label()}
                </span>
            </div>
            <PeriodSelector signals />
            <div
                class="nr-usage-metrics nr-stagger"
                aria-live="polite"
                aria-label="Live usage counters"
            >
                <LiveMetric
                    label="Active"
                    detail="In flight now"
                    tone="warn"
                    value=Signal::derive(move || live_reading(&live.get(), stream.get(), |usage| usage.active_requests))
                    ticked=Signal::derive(move || changes.get().active_requests)
                />
                <LiveMetric
                    label="Requests"
                    detail="Recorded to date"
                    tone="info"
                    value=Signal::derive(move || live_reading(&live.get(), stream.get(), |usage| usage.requests_today))
                    ticked=Signal::derive(move || changes.get().requests_today)
                />
                <LiveMetric
                    label="Tokens"
                    detail="Counted locally"
                    tone="info"
                    value=Signal::derive(move || live_reading(&live.get(), stream.get(), |usage| usage.tokens_today))
                    ticked=Signal::derive(move || changes.get().tokens_today)
                />
                <LiveMetric
                    label="Cost"
                    detail="Estimated spend"
                    tone="success"
                    value=Signal::derive(move || {
                        if stream.get().carries_readings() {
                            live.get()
                                .estimated_cost
                                .unwrap_or_else(|| NO_READING.to_owned())
                        } else {
                            NO_READING.to_owned()
                        }
                    })
                    ticked=Signal::derive(move || changes.get().estimated_cost)
                />
            </div>
            <Show when=move || !stream.get().carries_readings()>
                <p class="nr-usage-log-meta">
                    {move || match signals.stream.get() {
                        StreamState::Connecting => "Waiting for the first frame from /api/usage/stream.",
                        StreamState::Degraded => "The events service is connected but cannot read usage state, so live counters are unknown.",
                        StreamState::Interrupted => "The live stream dropped and the browser is retrying. Counters below are from the last completed fetch.",
                        StreamState::Unavailable => "Live counters need a browser EventSource; this build has none.",
                        StreamState::Live => "",
                    }}
                </p>
            </Show>
        </article>
    }
}

/// A live counter's value, or the no-reading marker when it cannot be trusted.
fn live_reading(
    usage: &LiveUsage,
    stream: StreamState,
    pick: impl Fn(&LiveUsage) -> Option<u64>,
) -> String {
    if stream.carries_readings() {
        format_optional_count(pick(usage))
    } else {
        NO_READING.to_owned()
    }
}

/// One live metric card. Pulses via `.nr-tick` when its value changes.
#[component]
fn LiveMetric(
    label: &'static str,
    detail: &'static str,
    tone: &'static str,
    value: Signal<String>,
    ticked: Signal<bool>,
) -> impl IntoView {
    view! {
        <article
            class=format!("nr-card nr-metric-card {tone}")
            class:nr-tick=move || ticked.get()
        >
            <span class="nr-metric-label">{label}</span>
            <strong>{move || value.get()}</strong>
            <small>{detail}</small>
        </article>
    }
}

/// The period selector, restricted to the values the API accepts.
#[component]
fn PeriodSelector(signals: UsageSignals) -> impl IntoView {
    let period = signals.period;
    let stats = signals.stats;

    view! {
        <div class="nr-usage-periods" role="group" aria-label="Usage period">
            {UsagePeriod::ALL
                .into_iter()
                .map(|option| {
                    view! {
                        <button
                            type="button"
                            class="nr-usage-period"
                            aria-pressed=move || (period.get() == option).to_string()
                            disabled=move || period.get() == option && stats.get().is_loading()
                            on:click=move |_| signals.set_period.set(option)
                        >
                            {option.label()}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// Window totals from `GET /api/usage/stats`.
#[component]
fn StatsTotals(signals: UsageSignals) -> impl IntoView {
    let stats = signals.stats;
    let period = signals.period;

    view! {
        <article class="nr-card">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"sum"</span>"Totals"</h2>
                    <p>{move || format!("Aggregated over: {}.", period.get().detail())}</p>
                </div>
            </div>
            {move || match stats.get() {
                Hydrate::Loading => {
                    view! {
                        <div class="nr-usage-metrics" aria-label="Loading usage totals" aria-busy="true">
                            <MetricSkeleton /><MetricSkeleton /><MetricSkeleton /><MetricSkeleton />
                        </div>
                    }
                        .into_any()
                }
                Hydrate::Failed(error) => {
                    view! { <FailureNotice error signals heading="Usage totals could not be loaded" /> }
                        .into_any()
                }
                Hydrate::Ready(ready) => {
                    let cost = format_cost(ready.total_cost);
                    let counts = [
                        ("Requests", format_count(ready.total_requests), "info"),
                        ("Prompt tokens", format_count(ready.total_prompt_tokens), "info"),
                        (
                            "Completion tokens",
                            format_count(ready.total_completion_tokens),
                            "info",
                        ),
                        ("Cached tokens", format_count(ready.total_cached_tokens), "success"),
                    ];
                    let empty = ready.is_empty();
                    view! {
                        <div class="nr-usage-metrics nr-stagger">
                            {counts
                                .into_iter()
                                .map(|(label, value, tone)| {
                                    view! { <UsageMetric label value detail=period.get().detail() tone /> }
                                })
                                .collect_view()}
                        </div>
                        <p class="nr-usage-log-meta nr-anim-rise">
                            <span>{format!("Total tokens: {}", format_count(ready.total_tokens()))}</span>
                            <span>{format!("Estimated cost: {cost}")}</span>
                        </p>
                        <Show when=move || empty>
                            <div class="nr-empty-state nr-usage-empty">
                                <strong>"No requests recorded yet"</strong>
                                <span>
                                    "The router answered, and has recorded nothing in this window. Send a request through it, or widen the period."
                                </span>
                            </div>
                        </Show>
                    }
                        .into_any()
                }
            }}
        </article>
    }
}

/// One totals card.
#[component]
fn UsageMetric(
    label: &'static str,
    value: String,
    detail: &'static str,
    tone: &'static str,
) -> impl IntoView {
    view! {
        <article class=format!("nr-card nr-metric-card {tone}")>
            <span class="nr-metric-label">{label}</span>
            <strong>{value}</strong>
            <small>{detail}</small>
        </article>
    }
}

/// A metric-shaped placeholder while the first fetch is in flight.
#[component]
fn MetricSkeleton() -> impl IntoView {
    view! {
        <article class="nr-card nr-metric-card">
            <span class="nr-skeleton nr-skeleton-text-short">"loading"</span>
            <span class="nr-skeleton nr-skeleton-text">"loading"</span>
        </article>
    }
}

/// A failed fetch, with the reason and a retry.
///
/// Bordered in warn and never dashed, so it cannot be confused with the empty
/// state.
#[component]
fn FailureNotice(error: ApiError, signals: UsageSignals, heading: &'static str) -> impl IntoView {
    view! {
        <div class="nr-usage-failure" role="alert">
            <strong>{heading}</strong>
            <span>{error.message()}</span>
            <button
                type="button"
                class="nr-button secondary small"
                on:click=move |_| signals.reload()
            >
                "Retry"
            </button>
        </div>
    }
}

/// The `last10Minutes` bar strip.
#[component]
fn MinuteStrip(signals: UsageSignals) -> impl IntoView {
    let stats = signals.stats;

    view! {
        <article class="nr-card">
            <div class="nr-card-head">
                <div>
                    <h2><span class="nr-card-icon">"min"</span>"Last 10 Minutes"</h2>
                    <p>"Requests per minute, oldest on the left. Bar heights are relative to the busiest minute."</p>
                </div>
            </div>
            {move || match stats.get() {
                Hydrate::Loading => {
                    view! {
                        <div class="nr-skeleton nr-skeleton-row" aria-label="Loading per-minute activity" aria-busy="true">
                            "loading"
                        </div>
                    }
                        .into_any()
                }
                Hydrate::Failed(error) => {
                    view! { <FailureNotice error signals heading="Per-minute activity could not be loaded" /> }
                        .into_any()
                }
                Hydrate::Ready(ready) => {
                    let series = ready.last_ten_minutes;
                    let summary = sparkline_summary(&series);
                    let bars = sparkline(&series);
                    if bars.is_empty() {
                        view! {
                            <div class="nr-empty-state nr-usage-empty">
                                <strong>"No per-minute activity reported"</strong>
                                <span>"The router did not return a 10-minute series for this window."</span>
                            </div>
                        }
                            .into_any()
                    } else {
                        let oldest = bars.first().map_or_else(String::new, |bar| bar.label.clone());
                        let newest = bars.last().map_or_else(String::new, |bar| bar.label.clone());
                        view! {
                            <div class="nr-usage-spark" role="img" aria-label=summary.clone()>
                                {bars.into_iter().map(|bar| view! { <SparkColumn bar /> }).collect_view()}
                            </div>
                            <div class="nr-usage-spark-axis" aria-hidden="true">
                                <span>{oldest}</span>
                                <span>{newest}</span>
                            </div>
                            <p class="nr-usage-log-meta">{summary}</p>
                        }
                            .into_any()
                    }
                }
            }}
        </article>
    }
}

/// One minute of the strip.
#[component]
fn SparkColumn(bar: SparkBar) -> impl IntoView {
    let quiet = bar.requests == 0;
    let title = format!(
        "{}: {} requests, {} tokens",
        bar.label,
        format_count(bar.requests),
        format_count(bar.tokens)
    );

    view! {
        // The strip as a whole carries the accessible summary, so an individual
        // bar must not be announced on its own.
        <span
            class=move || {
                if quiet { "nr-usage-spark-bar is-quiet" } else { "nr-usage-spark-bar" }
            }
            title=title
            aria-hidden="true"
        >
            <span style=format!("height:{}%", bar.height_percent)></span>
        </span>
    }
}

/// Which breakdown map a card renders.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Breakdown {
    Provider,
    Model,
}

impl Breakdown {
    const fn title(self) -> &'static str {
        match self {
            Self::Provider => "By Provider",
            Self::Model => "By Model",
        }
    }

    const fn column(self) -> &'static str {
        match self {
            Self::Provider => "Provider",
            Self::Model => "Model",
        }
    }

    const fn caption(self) -> &'static str {
        match self {
            Self::Provider => "Requests and tokens per upstream provider, busiest first.",
            Self::Model => "Requests and tokens per model, busiest first.",
        }
    }

    const fn empty_detail(self) -> &'static str {
        match self {
            Self::Provider => {
                "The router answered, and has attributed no requests to any provider in this window."
            }
            Self::Model => {
                "The router answered, and has attributed no requests to any model in this window."
            }
        }
    }

    fn rows(self, stats: &UsageStats) -> Vec<UsageBreakdownRow> {
        match self {
            Self::Provider => stats.by_provider.clone(),
            Self::Model => stats.by_model.clone(),
        }
    }
}

/// A breakdown table for one dimension.
#[component]
fn BreakdownCard(signals: UsageSignals, dimension: Breakdown) -> impl IntoView {
    let stats = signals.stats;

    view! {
        <article class="nr-card">
            <div class="nr-card-head">
                <div>
                    <h2><span class="nr-card-icon">"tbl"</span>{dimension.title()}</h2>
                    <p>{dimension.caption()}</p>
                </div>
            </div>
            {move || match stats.get() {
                Hydrate::Loading => {
                    view! {
                        <div
                            class="nr-usage-skeletons"
                            aria-label=format!("Loading {} breakdown", dimension.column().to_lowercase())
                            aria-busy="true"
                        >
                            <span class="nr-skeleton nr-skeleton-row">"loading"</span>
                            <span class="nr-skeleton nr-skeleton-row">"loading"</span>
                            <span class="nr-skeleton nr-skeleton-row">"loading"</span>
                        </div>
                    }
                        .into_any()
                }
                Hydrate::Failed(error) => {
                    view! { <FailureNotice error signals heading="This breakdown could not be loaded" /> }
                        .into_any()
                }
                Hydrate::Ready(ready) => {
                    let rows = dimension.rows(&ready);
                    if rows.is_empty() {
                        view! {
                            <div class="nr-empty-state nr-usage-empty">
                                <strong>"No requests recorded yet"</strong>
                                <span>{dimension.empty_detail()}</span>
                            </div>
                        }
                            .into_any()
                    } else {
                        let total = ready.total_requests;
                        view! { <BreakdownTable dimension rows total /> }.into_any()
                    }
                }
            }}
        </article>
    }
}

/// The table itself, split out so the card stays readable.
#[component]
fn BreakdownTable(dimension: Breakdown, rows: Vec<UsageBreakdownRow>, total: u64) -> impl IntoView {
    view! {
        <div class="nr-usage-table-scroll">
            <table class="nr-usage-table">
                <caption>{dimension.caption()}</caption>
                <thead>
                    <tr>
                        <th scope="col">{dimension.column()}</th>
                        <th scope="col">"Requests"</th>
                        <th scope="col">"Prompt"</th>
                        <th scope="col">"Completion"</th>
                        <th scope="col">"Cached"</th>
                        <th scope="col">"Errors"</th>
                    </tr>
                </thead>
                <tbody class="nr-stagger">
                    {rows
                        .into_iter()
                        .map(|row| {
                            let share = row.share_percent(total);
                            let error_class = if row.errors == 0 {
                                "nr-usage-errors-none"
                            } else {
                                "nr-usage-errors"
                            };
                            view! {
                                <tr>
                                    <th scope="row">
                                        <span class="nr-usage-table-name">{row.name.clone()}</span>
                                        <span
                                            class="nr-usage-share"
                                            title=format!("{share}% of requests in this window")
                                            aria-hidden="true"
                                        >
                                            <span style=format!("width:{share}%")></span>
                                        </span>
                                    </th>
                                    <td>{format_count(row.requests)}</td>
                                    <td>{format_count(row.prompt_tokens)}</td>
                                    <td>{format_count(row.completion_tokens)}</td>
                                    <td>{format_count(row.cached_tokens)}</td>
                                    <td class=error_class>{format_count(row.errors)}</td>
                                </tr>
                            }
                        })
                        .collect_view()}
                </tbody>
            </table>
        </div>
    }
}

/// Recent requests, newest first.
///
/// Prefers the SSE feed's `recentRequests` when the stream is live, since it is
/// fresher than the last `/api/usage/logs` fetch, and falls back to that fetch
/// otherwise.
#[component]
fn RequestLogCard(signals: UsageSignals) -> impl IntoView {
    let logs = signals.logs;
    let live = signals.live;
    let stream = signals.stream;

    view! {
        <article class="nr-card nr-usage-log">
            <div class="nr-card-head">
                <div>
                    <h2><span class="nr-card-icon">"log"</span>"Recent Requests"</h2>
                    <p>"The newest records the router has retained, with provider, model, tokens, and latency."</p>
                </div>
            </div>
            {move || {
                let from_stream = live.get().recent_requests;
                let use_stream = stream.get().carries_readings() && !from_stream.is_empty();
                if use_stream {
                    let clock = signals.clock().max(
                        from_stream.first().map_or(0, |entry| entry.timestamp),
                    );
                    return view! { <LogList entries=from_stream clock /> }.into_any();
                }
                match logs.get() {
                    Hydrate::Loading => {
                        view! {
                            <div class="nr-usage-skeletons" aria-label="Loading recent requests" aria-busy="true">
                                <span class="nr-skeleton nr-skeleton-row">"loading"</span>
                                <span class="nr-skeleton nr-skeleton-row">"loading"</span>
                                <span class="nr-skeleton nr-skeleton-row">"loading"</span>
                            </div>
                        }
                            .into_any()
                    }
                    Hydrate::Failed(error) => {
                        view! { <FailureNotice error signals heading="The request log could not be loaded" /> }
                            .into_any()
                    }
                    Hydrate::Ready(entries) => {
                        if entries.is_empty() {
                            view! {
                                <div class="nr-empty-state nr-usage-empty">
                                    <strong>"No requests recorded yet"</strong>
                                    <span>"The router answered, and has retained no request records. The log fills as traffic flows through it."</span>
                                </div>
                            }
                                .into_any()
                        } else {
                            let clock = signals.clock();
                            view! { <LogList entries clock /> }.into_any()
                        }
                    }
                }
            }}
        </article>
    }
}

/// The log rows.
#[component]
fn LogList(entries: Vec<UsageLogEntry>, clock: u64) -> impl IntoView {
    let label = format!("{} recent requests, newest first", entries.len());

    view! {
        <ul class="nr-usage-log-list nr-stagger" aria-label=label>
            {entries
                .into_iter()
                .map(|entry| {
                    let age = format_age(entry.timestamp, clock);
                    let status = entry
                        .status_code
                        .map_or_else(|| entry.status.clone(), |code| format!("{} {code}", entry.status));
                    view! {
                        <li class="nr-usage-log-row">
                            <strong>{format!("{} · {}", entry.provider, entry.model)}</strong>
                            <span class=format!("nr-status-pill {}", entry.status_class())>
                                <span></span>
                                {status}
                            </span>
                            <span class="nr-usage-log-meta">
                                <span>{age}</span>
                                <span>{format!("{} tokens", format_count(entry.total_tokens))}</span>
                                <span>{format_latency(entry.latency_ms)}</span>
                                {entry
                                    .endpoint
                                    .clone()
                                    .map(|endpoint| view! { <span>{endpoint}</span> })}
                            </span>
                            {entry
                                .error
                                .map(|error| view! { <span class="nr-usage-log-error">{error}</span> })}
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
}

/// The provider topology, annotated with real per-provider request counts.
///
/// The node layout is still the catalog shell — it is a fixed six-slot diagram —
/// but each node's count comes from `byProvider`, and a provider with no
/// recorded requests says so rather than borrowing another's number.
#[component]
fn TopologyCard(signals: UsageSignals) -> impl IntoView {
    let stats = signals.stats;
    let stream = signals.stream;
    let live = signals.live;
    let providers = usage_snapshot().topology_providers;
    let active = Signal::derive(move || {
        if stream.get().carries_readings() {
            format_optional_count(live.get().active_requests)
        } else {
            NO_READING.to_owned()
        }
    });

    view! {
        <article class="nr-card nr-usage-topology">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"net"</span>"Provider Topology"</h2>
                    <p>"Catalog providers, annotated with the requests attributed to each in the selected window."</p>
                </div>
                <span
                    class=move || format!("nr-status-pill {}", stream.get().class_name())
                    aria-label=move || format!("Live usage stream: {}", stream.get().label())
                >
                    <span></span>
                    {move || stream.get().label()}
                </span>
            </div>
            <div
                class="nr-topology-canvas"
                aria-label=move || format!("Provider topology, {} requests in flight", active.get())
            >
                <svg class="nr-topology-lines" viewBox="0 0 100 100" aria-hidden="true">
                    <path
                        d="M50 50 L50 13 M50 50 L82 30 M50 50 L82 70 M50 50 L50 87 M50 50 L18 70 M50 50 L18 30"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="0.7"
                        stroke-dasharray="2 2"
                    />
                </svg>
                <div class="nr-router-node">
                    <strong>"9Router"</strong>
                    <span>{move || format!("{} active", active.get())}</span>
                </div>
                <For
                    each=move || providers.clone()
                    key=|provider| provider.id.clone()
                    children=move |provider| {
                        let requests = Signal::derive({
                            let id = provider.id.clone();
                            let name = provider.name.clone();
                            move || {
                                stats
                                    .get()
                                    .ready()
                                    .map(|ready| provider_requests(ready, &id, &name))
                            }
                        });
                        view! { <TopologyProvider provider requests /> }
                    }
                />
            </div>
        </article>
    }
}

/// Requests attributed to one catalog provider.
///
/// `byProvider` is keyed by whatever the runtime recorded, so both the catalog id
/// and its display name are tried before concluding there were none.
fn provider_requests(stats: &UsageStats, id: &str, name: &str) -> u64 {
    stats
        .by_provider
        .iter()
        .find(|row| row.name.eq_ignore_ascii_case(id) || row.name.eq_ignore_ascii_case(name))
        .map_or(0, |row| row.requests)
}

/// One topology node.
#[component]
fn TopologyProvider(provider: UsageProviderNode, requests: Signal<Option<u64>>) -> impl IntoView {
    let initial = provider
        .name
        .chars()
        .next()
        .map_or_else(|| "?".to_owned(), |value| value.to_string());
    let class_name = format!("nr-topology-provider {}", provider.slot_class);
    let detail = move || match requests.get() {
        None => "Loading…".to_owned(),
        Some(0) => "No requests".to_owned(),
        Some(count) => format!("{} requests", format_count(count)),
    };

    view! {
        <div
            class=class_name
            class:is-active=move || requests.get().is_some_and(|count| count > 0)
            style=format!("--provider-accent: {}", provider.accent)
        >
            <span class="nr-provider-logo">{initial}</span>
            <span class="nr-topology-copy">
                <strong>{provider.name}</strong>
                <small>{detail}</small>
            </span>
        </div>
    }
}

/// Subscribe to `/api/usage/stream` and drive the live signals.
///
/// The `EventSource` and its closures are parked in a thread-local so
/// [`on_cleanup`] — which takes a `Send + Sync` closure, and neither of those is
/// — can close them by token. Closing matters: an `EventSource` left open
/// reconnects forever after the panel is gone.
#[cfg(target_arch = "wasm32")]
fn subscribe_usage_stream(
    set_live: WriteSignal<LiveUsage>,
    set_changes: WriteSignal<LiveChanges>,
    set_stream: WriteSignal<StreamState>,
) {
    use crate::dashboard::usage_live::parse_usage_frame;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::{EventSource, MessageEvent};

    let Ok(source) = EventSource::new("/api/usage/stream") else {
        // No stream to subscribe to; counters stay marked as having no reading.
        set_stream.set(StreamState::Unavailable);
        return;
    };

    let on_usage = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Some(data) = event.data().as_string() else {
            return;
        };
        let Some(frame) = parse_usage_frame(&data) else {
            return;
        };
        set_live.update(|current| {
            set_changes.set(frame.changes_from(current));
            *current = frame.clone();
        });
        set_stream.set(if frame.live_telemetry {
            StreamState::Live
        } else {
            StreamState::Degraded
        });
    });

    let on_error = Closure::<dyn FnMut()>::new(move || {
        // The browser retries on its own; say so rather than showing stale
        // counters as if they were current.
        set_stream.set(StreamState::Interrupted);
    });

    let listener_added = source
        .add_event_listener_with_callback("usage", on_usage.as_ref().unchecked_ref())
        .is_ok();
    if !listener_added {
        source.close();
        set_stream.set(StreamState::Unavailable);
        return;
    }
    source.set_onerror(Some(on_error.as_ref().unchecked_ref()));

    let token = stream_registry::register(
        source,
        vec![on_usage.into_js_value(), on_error.into_js_value()],
    );
    on_cleanup(move || stream_registry::close(token));
}

/// Live subscriptions, parked so cleanup can reach them without capturing
/// browser handles in a `Send + Sync` closure.
#[cfg(target_arch = "wasm32")]
mod stream_registry {
    use std::cell::RefCell;

    use wasm_bindgen::JsValue;
    use web_sys::EventSource;

    /// One live subscription: the source, plus the closures that must outlive
    /// the call that registered them.
    struct Subscription {
        token: u64,
        source: EventSource,
        _closures: Vec<JsValue>,
    }

    thread_local! {
        static NEXT_TOKEN: RefCell<u64> = const { RefCell::new(0) };
        static SUBSCRIPTIONS: RefCell<Vec<Subscription>> = const { RefCell::new(Vec::new()) };
    }

    /// Park a subscription and return its cleanup token.
    pub(super) fn register(source: EventSource, closures: Vec<JsValue>) -> u64 {
        let token = NEXT_TOKEN.with_borrow_mut(|next| {
            *next = next.saturating_add(1);
            *next
        });
        SUBSCRIPTIONS.with_borrow_mut(|live| {
            live.push(Subscription {
                token,
                source,
                _closures: closures,
            });
        });
        token
    }

    /// Close and drop the subscription with this token.
    pub(super) fn close(token: u64) {
        let found = SUBSCRIPTIONS.with_borrow_mut(|live| {
            live.iter()
                .position(|entry| entry.token == token)
                .map(|index| live.swap_remove(index))
        });
        if let Some(entry) = found {
            entry.source.close();
        }
    }
}

/// Native builds have no `EventSource`, so there are no live readings to show.
#[cfg(not(target_arch = "wasm32"))]
fn subscribe_usage_stream(
    _set_live: WriteSignal<LiveUsage>,
    _set_changes: WriteSignal<LiveChanges>,
    set_stream: WriteSignal<StreamState>,
) {
    set_stream.set(StreamState::Unavailable);
}
