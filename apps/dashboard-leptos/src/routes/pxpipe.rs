//! The token saver: whether the transform is installed and loaded, what it has saved, and the
//! controls that start and stop it.
//!
//! Four reads, kept as four panels rather than merged into one summary. They answer different
//! questions and can disagree in ways that matter:
//!
//! * `status` is the install and worker state. The transform runs in the runtime service, so this
//!   answers `503` when that service is unreachable — which is a different thing from "not running"
//!   and is rendered as the failure it is.
//! * `health` is the ordered checklist: installed, then loads, then transforms. A `200` here can
//!   still carry `healthy: false`, so the flag is read rather than the status code.
//! * `stats` is what the recorded events add up to.
//! * `logs` is the install and worker output, which is where a load failure explains itself.
//!
//! `install` is rendered and deliberately not pressed by anything but a user: it runs
//! `npm install pxpipe-proxy@latest`, which is a real install with real lifecycle scripts.

use leptos::prelude::*;
use serde::Deserialize;

use crate::api::{Hydrate, load};
use crate::routes::controls::{Action, Caution, Field, Flag, Outcome, OutcomeLine, Section, Tone};
use crate::routes::{PageHeader, Panel};

/// How many recent events to ask for. The server clamps to 500 and defaults to 100; a smaller
/// window is asked for explicitly because this panel renders the rows.
const RECENT_LIMIT: u32 = 25;

/// `GET /api/pxpipe/status`.
///
/// The optional fields are `skip_serializing_if = "Option::is_none"` on the wire, so they are absent
/// rather than null when there is no answer. Decoded as `Option` regardless: absent and null must
/// both mean "the server did not say", never an empty string rendered as a value.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PxpipeStatus {
    #[serde(default)]
    installed: bool,
    #[serde(default)]
    installing: bool,
    /// Whether the transform is loaded and ready.
    #[serde(default)]
    running: bool,
    #[serde(default)]
    uptime_ms: u64,
    #[serde(default)]
    npm_available: bool,
    /// Whether a `node` was found. The transform lives in a child process here, so its absence is a
    /// distinct state from npm's.
    #[serde(default)]
    node_available: bool,
    /// `worker` in this port, against upstream's in-process `library`.
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    node_version: Option<String>,
    /// The package's declared `engines.node`.
    #[serde(default)]
    requires_node: Option<String>,
}

/// `GET /api/pxpipe/health`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PxpipeHealth {
    #[serde(default)]
    healthy: bool,
    #[serde(default)]
    checks: Vec<HealthStep>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HealthStep {
    #[serde(default)]
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    detail: Option<String>,
}

/// `GET /api/pxpipe/stats`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PxpipeStats {
    #[serde(default)]
    windows: Windows,
    #[serde(default)]
    timeline: Vec<DayTotals>,
    /// Most recent first.
    #[serde(default)]
    recent: Vec<Event>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Windows {
    #[serde(default)]
    all: Totals,
    #[serde(default)]
    today: Totals,
    #[serde(default)]
    yesterday: Totals,
    #[serde(default)]
    last7d: Totals,
    #[serde(default)]
    last30d: Totals,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Totals {
    #[serde(default)]
    requests: u64,
    #[serde(default)]
    compressed: u64,
    #[serde(default)]
    bypassed: u64,
    #[serde(default)]
    errors: u64,
    #[serde(default)]
    tokens_before_est: u64,
    #[serde(default)]
    tokens_after_est: u64,
    #[serde(default)]
    tokens_saved_est: u64,
    /// Percentage saved, to two places, as the server computed it. Not recomputed here: a summary
    /// and the aggregate behind it rounding differently is how two numbers on one screen disagree.
    #[serde(default)]
    saved_pct: f64,
    #[serde(default)]
    images_generated: u64,
    #[serde(default)]
    compression_time_ms: u64,
    #[serde(default)]
    avg_compression_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DayTotals {
    #[serde(default)]
    date: String,
    #[serde(default)]
    tokens_saved_est: u64,
    #[serde(default)]
    compressed: u64,
    #[serde(default)]
    requests: u64,
}

/// One transform attempt.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    /// Epoch milliseconds.
    #[serde(default)]
    ts: u64,
    /// Whether the body was actually replaced.
    #[serde(default)]
    applied: bool,
    /// `applied`, `below_threshold`, `timeout`, `transform_error`, `not_installed`, `disabled`, …
    #[serde(default)]
    reason: String,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    tokens_before_est: u64,
    #[serde(default)]
    tokens_after_est: u64,
    #[serde(default)]
    tokens_saved_est: u64,
    #[serde(default)]
    duration_ms: u64,
}

/// `GET /api/pxpipe/logs`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PxpipeLogs {
    #[serde(default)]
    install_log: String,
    /// Worker stderr. Upstream has no equivalent, because it has no worker.
    #[serde(default)]
    worker_log: Option<String>,
    #[serde(default)]
    events: Vec<Event>,
}

#[component]
pub fn Pxpipe() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (status, set_status) = signal(Hydrate::<PxpipeStatus>::Loading);
    let (health, set_health) = signal(Hydrate::<PxpipeHealth>::Loading);
    let (savings, set_savings) = signal(Hydrate::<PxpipeStats>::Loading);
    let (logs, set_logs) = signal(Hydrate::<PxpipeLogs>::Loading);
    let (outcome, set_outcome) = signal(None::<Outcome>);

    let reload = move || {
        set_status.set(Hydrate::Loading);
        set_health.set(Hydrate::Loading);
        set_savings.set(Hydrate::Loading);
        set_logs.set(Hydrate::Loading);
        load("/api/pxpipe/status", set_status);
        // GET mirrors POST on this route, so probing on load is not a mutation.
        load("/api/pxpipe/health", set_health);
        load(
            format!("/api/pxpipe/stats?limit={RECENT_LIMIT}"),
            set_savings,
        );
        load(format!("/api/pxpipe/logs?limit={RECENT_LIMIT}"), set_logs);
    };
    reload();

    let done = Callback::new(move |result: Outcome| {
        let ok = result.ok;
        set_outcome.set(Some(result));
        if ok {
            reload();
        }
    });

    view! {
        <PageHeader
            title=locale.get("nav.pxpipe").to_owned()
            description=locale.get("pxpipe.description").to_owned()
        />
        <div class="space-y-6">
            <div class="grid gap-4 md:grid-cols-2">
                <Section title=locale.get("pxpipe.state").to_owned()>
                    <Panel
                        state=status
                        on_retry=Callback::new(move |()| reload())
                        children=|data: PxpipeStatus| view! { <StatusBody data=data /> }
                    />
                </Section>
                <Section title=locale.get("pxpipe.health").to_owned()>
                    <Panel
                        state=health
                        on_retry=Callback::new(move |()| reload())
                        children=|data: PxpipeHealth| view! { <HealthBody data=data /> }
                    />
                </Section>
            </div>

            <ControlPanel outcome=outcome on_done=done />

            <Section title=locale.get("pxpipe.savings").to_owned()>
                <Panel
                    state=savings
                    on_retry=Callback::new(move |()| reload())
                    children=|data: PxpipeStats| view! { <StatsBody data=data /> }
                />
            </Section>

            <Section title=locale.get("pxpipe.logs").to_owned()>
                <Panel
                    state=logs
                    on_retry=Callback::new(move |()| reload())
                    children=|data: PxpipeLogs| view! { <LogsBody data=data /> }
                />
            </Section>
        </div>
    }
}

/// Start, stop, reload, install.
///
/// Its own component because [`Section`]'s children are a `FnOnce`, which takes the caller's
/// `Locale` with them; a second section needing a label would then have none.
#[component]
fn ControlPanel(outcome: ReadSignal<Option<Outcome>>, on_done: Callback<Outcome>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <Section title=locale.get("pxpipe.controls").to_owned()>
            <div class="flex flex-wrap gap-2">
                <Action
                    label=locale.get("pxpipe.start").to_owned()
                    path="/api/pxpipe/start".to_owned()
                    tone=Tone::Primary
                    done_label=locale.get("pxpipe.started").to_owned()
                    on_done=on_done
                />
                <Action
                    label=locale.get("pxpipe.stop").to_owned()
                    path="/api/pxpipe/stop".to_owned()
                    done_label=locale.get("pxpipe.stopped").to_owned()
                    on_done=on_done
                />
                <Action
                    label=locale.get("pxpipe.restart").to_owned()
                    path="/api/pxpipe/restart".to_owned()
                    done_label=locale.get("pxpipe.restarted").to_owned()
                    on_done=on_done
                />
                // Runs `npm install pxpipe-proxy@latest` for real, which is why it is toned as a
                // cost rather than as the obvious next step.
                <Action
                    label=locale.get("pxpipe.install").to_owned()
                    path="/api/pxpipe/install".to_owned()
                    tone=Tone::Destructive
                    done_label=locale.get("pxpipe.installed").to_owned()
                    on_done=on_done
                />
            </div>
            <Caution text=locale.get("pxpipe.install_caution").to_owned() />
            <OutcomeLine outcome=outcome />
        </Section>
    }
}

#[component]
fn StatusBody(data: PxpipeStatus) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <dl class="space-y-2.5 text-sm">
            <Flag label=locale.get("pxpipe.installed_flag").to_owned() on=data.installed />
            <Flag label=locale.get("pxpipe.running").to_owned() on=data.running />
            {data
                .installing
                .then(|| {
                    view! {
                        <Flag label=locale.get("pxpipe.installing").to_owned() on=true />
                    }
                })}
            <Flag label=locale.get("pxpipe.npm").to_owned() on=data.npm_available />
            <Flag label=locale.get("pxpipe.node").to_owned() on=data.node_available />
            <Field
                label=locale.get("pxpipe.uptime").to_owned()
                value=if data.running { duration(data.uptime_ms) } else { String::new() }
            />
            <Field
                label=locale.get("pxpipe.version").to_owned()
                value=data.version.unwrap_or_default()
            />
            <Field
                label=locale.get("pxpipe.node_version").to_owned()
                value=data.node_version.unwrap_or_default()
            />
            <Field
                label=locale.get("pxpipe.requires_node").to_owned()
                value=data.requires_node.unwrap_or_default()
            />
            <Field label=locale.get("pxpipe.mode").to_owned() value=data.mode.unwrap_or_default() />
            <Field label=locale.get("pxpipe.path").to_owned() value=data.path.unwrap_or_default() />
        </dl>
    }
}

/// The ordered checklist, plus whatever the server said went wrong.
///
/// `healthy` is read from the body, not inferred from the checks: the server owns that judgement,
/// and recomputing it here would let the two disagree.
#[component]
fn HealthBody(data: PxpipeHealth) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let error = data.error.unwrap_or_default();
    view! {
        <div class="space-y-3 text-sm">
            <div class="flex items-center gap-2">
                <span class=if data.healthy {
                    "size-1.5 rounded-full bg-success"
                } else {
                    "size-1.5 rounded-full bg-warning"
                } />
                <span>
                    {if data.healthy {
                        locale.get("pxpipe.healthy").to_owned()
                    } else {
                        locale.get("pxpipe.unhealthy").to_owned()
                    }}
                </span>
            </div>
            {(!error.is_empty())
                .then(|| {
                    view! {
                        <p class="text-muted-foreground break-words">{error.clone()}</p>
                    }
                })}
            {if data.checks.is_empty() {
                view! {
                    <p class="text-muted-foreground">
                        {locale.get("pxpipe.no_checks").to_owned()}
                    </p>
                }
                    .into_any()
            } else {
                view! {
                    <ul class="space-y-2">
                        {data
                            .checks
                            .into_iter()
                            .map(|step| {
                                let detail = step.detail.unwrap_or_default();
                                let label = if step.label.is_empty() { step.id } else { step.label };
                                view! {
                                    <li class="flex items-start gap-2">
                                        <span class=if step.ok {
                                            "size-1.5 rounded-full bg-success mt-1.5 shrink-0"
                                        } else {
                                            "size-1.5 rounded-full bg-muted-foreground/40 mt-1.5 shrink-0"
                                        } />
                                        <span class="min-w-0">
                                            <span class="break-words">{label}</span>
                                            {(!detail.is_empty())
                                                .then(|| {
                                                    view! {
                                                        <span class="block text-xs text-muted-foreground break-words">
                                                            {detail}
                                                        </span>
                                                    }
                                                })}
                                        </span>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                    .into_any()
            }}
        </div>
    }
}

#[component]
fn StatsBody(data: PxpipeStats) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let windows = [
        (
            locale.get("pxpipe.window_today").to_owned(),
            data.windows.today,
        ),
        (
            locale.get("pxpipe.window_yesterday").to_owned(),
            data.windows.yesterday,
        ),
        (
            locale.get("pxpipe.window_7d").to_owned(),
            data.windows.last7d,
        ),
        (
            locale.get("pxpipe.window_30d").to_owned(),
            data.windows.last30d,
        ),
        (locale.get("pxpipe.window_all").to_owned(), data.windows.all),
    ];

    view! {
        <div class="space-y-4">
            <div class="rounded-lg border border-border overflow-x-auto">
                <table class="w-full text-sm">
                    <thead class="bg-muted/50 text-muted-foreground">
                        <tr>
                            <th class="text-left font-medium px-3 py-2">
                                {locale.get("pxpipe.col_window").to_owned()}
                            </th>
                            <th class="text-right font-medium px-3 py-2">
                                {locale.get("pxpipe.col_requests").to_owned()}
                            </th>
                            <th class="text-right font-medium px-3 py-2">
                                {locale.get("pxpipe.col_compressed").to_owned()}
                            </th>
                            <th class="text-right font-medium px-3 py-2">
                                {locale.get("pxpipe.col_bypassed").to_owned()}
                            </th>
                            <th class="text-right font-medium px-3 py-2">
                                {locale.get("pxpipe.col_errors").to_owned()}
                            </th>
                            <th class="text-right font-medium px-3 py-2">
                                {locale.get("pxpipe.col_tokens_before").to_owned()}
                            </th>
                            <th class="text-right font-medium px-3 py-2">
                                {locale.get("pxpipe.col_tokens_after").to_owned()}
                            </th>
                            <th class="text-right font-medium px-3 py-2">
                                {locale.get("pxpipe.col_saved").to_owned()}
                            </th>
                            <th class="text-right font-medium px-3 py-2">
                                {locale.get("pxpipe.col_saved_pct").to_owned()}
                            </th>
                            <th class="text-right font-medium px-3 py-2">
                                {locale.get("pxpipe.col_images").to_owned()}
                            </th>
                            <th class="text-right font-medium px-3 py-2">
                                {locale.get("pxpipe.col_total_ms").to_owned()}
                            </th>
                            <th class="text-right font-medium px-3 py-2">
                                {locale.get("pxpipe.col_avg_ms").to_owned()}
                            </th>
                        </tr>
                    </thead>
                    <tbody>
                        {windows
                            .into_iter()
                            .map(|(label, totals)| {
                                view! {
                                    <tr class="border-t border-border">
                                        <td class="px-3 py-2">{label}</td>
                                        <td class="px-3 py-2 text-right font-mono">
                                            {totals.requests.to_string()}
                                        </td>
                                        <td class="px-3 py-2 text-right font-mono">
                                            {totals.compressed.to_string()}
                                        </td>
                                        <td class="px-3 py-2 text-right font-mono">
                                            {totals.bypassed.to_string()}
                                        </td>
                                        <td class="px-3 py-2 text-right font-mono">
                                            {totals.errors.to_string()}
                                        </td>
                                        <td class="px-3 py-2 text-right font-mono">
                                            {totals.tokens_before_est.to_string()}
                                        </td>
                                        <td class="px-3 py-2 text-right font-mono">
                                            {totals.tokens_after_est.to_string()}
                                        </td>
                                        <td class="px-3 py-2 text-right font-mono">
                                            {totals.tokens_saved_est.to_string()}
                                        </td>
                                        <td class="px-3 py-2 text-right font-mono">
                                            {percent(totals.saved_pct)}
                                        </td>
                                        <td class="px-3 py-2 text-right font-mono">
                                            {totals.images_generated.to_string()}
                                        </td>
                                        <td class="px-3 py-2 text-right font-mono">
                                            {totals.compression_time_ms.to_string()}
                                        </td>
                                        <td class="px-3 py-2 text-right font-mono">
                                            {totals.avg_compression_ms.to_string()}
                                        </td>
                                    </tr>
                                }
                            })
                            .collect_view()}
                    </tbody>
                </table>
            </div>

            <Timeline days=data.timeline />
            <Recent events=data.recent />
        </div>
    }
}

/// Thirty days of daily totals, one square each.
///
/// Deliberately not a proportional chart: every day in a fresh install is zero, and a bar chart of
/// zeros reads as a broken widget. A square that is either lit or not answers the question this
/// strip is for — was anything compressed that day — without implying a magnitude it does not have.
#[component]
fn Timeline(days: Vec<DayTotals>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if days.is_empty() {
        return ().into_any();
    }
    let active = days.iter().filter(|day| day.compressed > 0).count();
    let total: u64 = days.iter().map(|day| day.tokens_saved_est).sum();
    let summary = locale.fmt(
        "pxpipe.timeline_summary",
        &[
            ("days", &active.to_string()),
            ("of", &days.len().to_string()),
            ("saved", &total.to_string()),
        ],
    );

    view! {
        <div class="space-y-2">
            <p class="text-xs text-muted-foreground">{summary}</p>
            <div class="flex flex-wrap gap-1">
                {days
                    .into_iter()
                    .map(|day| {
                        let title = format!(
                            "{}: {} / {} · {}",
                            day.date,
                            day.compressed,
                            day.requests,
                            day.tokens_saved_est,
                        );
                        let class = if day.compressed > 0 {
                            "size-3 rounded-sm bg-primary"
                        } else if day.requests > 0 {
                            "size-3 rounded-sm bg-warning/50"
                        } else {
                            "size-3 rounded-sm bg-muted"
                        };
                        view! { <span class=class title=title /> }
                    })
                    .collect_view()}
            </div>
        </div>
    }
    .into_any()
}

#[component]
fn Recent(events: Vec<Event>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if events.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">{locale.get("pxpipe.no_events").to_owned()}</p>
        }
        .into_any();
    }
    view! {
        <div class="rounded-lg border border-border overflow-x-auto">
            <table class="w-full text-sm">
                <thead class="bg-muted/50 text-muted-foreground">
                    <tr>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("pxpipe.col_when").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("pxpipe.col_outcome").to_owned()}
                        </th>
                        <th class="text-right font-medium px-3 py-2">
                            {locale.get("pxpipe.col_before").to_owned()}
                        </th>
                        <th class="text-right font-medium px-3 py-2">
                            {locale.get("pxpipe.col_after").to_owned()}
                        </th>
                        <th class="text-right font-medium px-3 py-2">
                            {locale.get("pxpipe.col_saved").to_owned()}
                        </th>
                        <th class="text-right font-medium px-3 py-2">
                            {locale.get("pxpipe.col_took").to_owned()}
                        </th>
                    </tr>
                </thead>
                <tbody>
                    {events
                        .into_iter()
                        .map(|event| {
                            let detail = event.detail.unwrap_or_default();
                            view! {
                                <tr class="border-t border-border align-top">
                                    <td class="px-3 py-2 font-mono text-xs whitespace-nowrap">
                                        {timestamp(event.ts)}
                                    </td>
                                    <td class="px-3 py-2">
                                        <span class="flex items-center gap-2">
                                            <span class=if event.applied {
                                                "size-1.5 rounded-full bg-success shrink-0"
                                            } else {
                                                "size-1.5 rounded-full bg-muted-foreground/40 shrink-0"
                                            } />
                                            <span class="break-words">{event.reason}</span>
                                        </span>
                                        {(!detail.is_empty())
                                            .then(|| {
                                                view! {
                                                    <span class="block text-xs text-muted-foreground break-words">
                                                        {detail}
                                                    </span>
                                                }
                                            })}
                                    </td>
                                    <td class="px-3 py-2 text-right font-mono">
                                        {event.tokens_before_est.to_string()}
                                    </td>
                                    <td class="px-3 py-2 text-right font-mono">
                                        {event.tokens_after_est.to_string()}
                                    </td>
                                    <td class="px-3 py-2 text-right font-mono">
                                        {event.tokens_saved_est.to_string()}
                                    </td>
                                    <td class="px-3 py-2 text-right font-mono">
                                        {format!("{}ms", event.duration_ms)}
                                    </td>
                                </tr>
                            }
                        })
                        .collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

#[component]
fn LogsBody(data: PxpipeLogs) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let worker = data.worker_log.unwrap_or_default();
    let events = data.events.len();

    view! {
        <div class="space-y-4">
            <LogBlock
                title=locale.get("pxpipe.install_log").to_owned()
                text=data.install_log
                empty=locale.get("pxpipe.no_install_log").to_owned()
            />
            <LogBlock
                title=locale.get("pxpipe.worker_log").to_owned()
                text=worker
                empty=locale.get("pxpipe.no_worker_log").to_owned()
            />
            <p class="text-xs text-muted-foreground">
                {locale.fmt("pxpipe.event_count", &[("count", &events.to_string())])}
            </p>
        </div>
    }
}

#[component]
fn LogBlock(title: String, text: String, empty: String) -> impl IntoView {
    view! {
        <div class="space-y-1">
            <h3 class="text-xs font-medium text-muted-foreground uppercase tracking-wide">
                {title}
            </h3>
            {if text.trim().is_empty() {
                view! { <p class="text-sm text-muted-foreground italic">{empty}</p> }.into_any()
            } else {
                view! {
                    <pre class="max-h-64 overflow-auto rounded-md border border-border bg-muted/30 \
                                p-3 font-mono text-xs whitespace-pre-wrap break-words">
                        {text}
                    </pre>
                }
                    .into_any()
            }}
        </div>
    }
}

/// A percentage as the server computed it, to the two places it reports.
fn percent(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.2}%")
    } else {
        "—".to_owned()
    }
}

/// Milliseconds as something readable, without inventing precision.
fn duration(millis: u64) -> String {
    let seconds = millis / 1000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {}s", seconds % 60);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h {}m", minutes % 60);
    }
    format!("{}d {}h", hours / 24, hours % 24)
}

/// An epoch-millisecond stamp as `HH:MM:SS` UTC.
///
/// Date is dropped rather than rendered wrong: `js_sys::Date` is the only correct local-time
/// formatter available here, and this table's rows are minutes old. `0` means the event carried no
/// timestamp, which is not the epoch.
fn timestamp(millis: u64) -> String {
    if millis == 0 {
        return "—".to_owned();
    }
    let seconds = millis / 1000 % 86_400;
    format!(
        "{:02}:{:02}:{:02}Z",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::{
        PxpipeHealth, PxpipeLogs, PxpipeStats, PxpipeStatus, duration, percent, timestamp,
    };

    /// `GET /api/pxpipe/status` on a host with node and npm but no package, captured live.
    const LIVE_STATUS: &str = r#"{"installed":false,"installing":false,"running":false,
        "uptimeMs":0,"npmAvailable":true,"nodeAvailable":true,"mode":"worker"}"#;

    /// `GET /api/pxpipe/health` on the same host.
    const LIVE_HEALTH: &str = r#"{"healthy":false,
        "checks":[{"id":"installed","label":"PXPIPE installed","ok":false}],
        "error":"pxpipe not installed"}"#;

    /// `GET /api/pxpipe/stats`, trimmed to two timeline days and one recent event.
    const LIVE_STATS: &str = r#"{
        "windows":{
            "all":{"requests":4,"compressed":3,"bypassed":1,"errors":0,"tokensBeforeEst":1000,
                   "tokensAfterEst":600,"tokensSavedEst":400,"savedPct":40.0,
                   "imagesGenerated":0,"compressionTimeMs":90,"avgCompressionMs":30},
            "today":{"requests":1,"compressed":1,"bypassed":0,"errors":0,"tokensBeforeEst":200,
                     "tokensAfterEst":120,"tokensSavedEst":80,"savedPct":40.0,
                     "imagesGenerated":0,"compressionTimeMs":30,"avgCompressionMs":30},
            "yesterday":{"requests":0,"compressed":0,"bypassed":0,"errors":0,"tokensBeforeEst":0,
                         "tokensAfterEst":0,"tokensSavedEst":0,"savedPct":0.0,
                         "imagesGenerated":0,"compressionTimeMs":0,"avgCompressionMs":0},
            "last7d":{"requests":4,"compressed":3,"bypassed":1,"errors":0,"tokensBeforeEst":1000,
                      "tokensAfterEst":600,"tokensSavedEst":400,"savedPct":40.0,
                      "imagesGenerated":0,"compressionTimeMs":90,"avgCompressionMs":30},
            "last30d":{"requests":4,"compressed":3,"bypassed":1,"errors":0,"tokensBeforeEst":1000,
                       "tokensAfterEst":600,"tokensSavedEst":400,"savedPct":40.0,
                       "imagesGenerated":0,"compressionTimeMs":90,"avgCompressionMs":30}
        },
        "timeline":[
            {"date":"2026-09-03","tokensSavedEst":0,"compressed":0,"requests":0},
            {"date":"2026-09-04","tokensSavedEst":80,"compressed":1,"requests":1}
        ],
        "recent":[{"ts":1757000000000,"applied":true,"reason":"applied","originalChars":800,
                   "tokensBeforeEst":200,"tokensAfterEst":120,"tokensSavedEst":80,
                   "imageCount":0,"durationMs":30}]
    }"#;

    #[test]
    fn the_live_status_decodes_with_its_real_fields() {
        let status: PxpipeStatus = serde_json::from_str(LIVE_STATUS).unwrap_or_default();
        assert!(!status.installed);
        assert!(!status.running);
        assert!(status.npm_available);
        assert!(status.node_available);
        assert_eq!(status.mode.as_deref(), Some("worker"));
        // Skipped on the wire when absent. Absent must not become an empty string that renders as
        // a value.
        assert!(status.version.is_none());
        assert!(status.path.is_none());
        assert!(status.node_version.is_none());
    }

    #[test]
    fn stop_returns_a_flattened_status_that_still_decodes() {
        // `POST /api/pxpipe/stop` answers `{"stopped":…}` merged with the whole status.
        let body = r#"{"stopped":false,"installed":false,"installing":false,"running":false,
            "uptimeMs":0,"npmAvailable":true,"nodeAvailable":true,"mode":"worker"}"#;
        let status: PxpipeStatus = serde_json::from_str(body).unwrap_or_default();
        assert!(!status.running);
        assert_eq!(status.mode.as_deref(), Some("worker"));
    }

    #[test]
    fn an_unhealthy_two_hundred_is_read_as_unhealthy() {
        // The route answers 200 with `healthy: false`. Trusting the status code instead of the flag
        // would report a token saver that cannot load as working.
        let health: PxpipeHealth = serde_json::from_str(LIVE_HEALTH).unwrap_or_default();
        assert!(!health.healthy);
        assert_eq!(health.error.as_deref(), Some("pxpipe not installed"));
        assert_eq!(health.checks.len(), 1);
        assert!(health.checks.first().is_some_and(|step| !step.ok));
        assert_eq!(
            health.checks.first().map(|step| step.label.as_str()),
            Some("PXPIPE installed")
        );
    }

    #[test]
    fn the_stats_windows_and_events_decode_together() {
        let stats: PxpipeStats = serde_json::from_str(LIVE_STATS).unwrap_or_default();
        assert_eq!(stats.windows.today.requests, 1);
        assert_eq!(stats.windows.today.tokens_saved_est, 80);
        assert_eq!(stats.windows.all.compressed, 3);
        assert!((stats.windows.all.saved_pct - 40.0).abs() < f64::EPSILON);
        assert_eq!(stats.timeline.len(), 2);
        assert_eq!(
            stats.timeline.last().map(|day| day.compressed),
            Some(1),
            "a day with activity has to survive the decode"
        );
        assert_eq!(stats.recent.len(), 1);
        assert!(stats.recent.first().is_some_and(|event| event.applied));
        assert_eq!(
            stats.recent.first().map(|event| event.reason.as_str()),
            Some("applied")
        );
    }

    #[test]
    fn the_logs_body_decodes_and_an_absent_worker_log_stays_absent() {
        let logs: PxpipeLogs =
            serde_json::from_str(r#"{"installLog":"","events":[]}"#).unwrap_or_default();
        assert!(logs.install_log.is_empty());
        assert!(logs.worker_log.is_none());
        assert!(logs.events.is_empty());

        let with_worker: PxpipeLogs =
            serde_json::from_str(r#"{"installLog":"npm ok","workerLog":"boom","events":[]}"#)
                .unwrap_or_default();
        assert_eq!(with_worker.worker_log.as_deref(), Some("boom"));
    }

    #[test]
    fn a_shape_change_is_a_failure_rather_than_a_default() {
        // Not `[]`: serde maps a sequence onto a struct positionally, so an empty array fills every
        // `#[serde(default)]` field and decodes clean. A bare scalar is what has no struct reading
        // at all, which is the shape change worth catching.
        assert!(serde_json::from_str::<PxpipeStats>("42").is_err());
        assert!(serde_json::from_str::<PxpipeStats>("null").is_err());
        assert!(serde_json::from_str::<PxpipeStatus>("truncated").is_err());
    }

    #[test]
    fn durations_read_as_durations() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(45_000), "45s");
        assert_eq!(duration(90_000), "1m 30s");
        assert_eq!(duration(3_700_000), "1h 1m");
        assert_eq!(duration(180_000_000), "2d 2h");
    }

    #[test]
    fn percentages_keep_the_servers_precision() {
        assert_eq!(percent(0.0), "0.00%");
        assert_eq!(percent(40.5), "40.50%");
        assert_eq!(percent(f64::NAN), "—");
        assert_eq!(percent(f64::INFINITY), "—");
    }

    #[test]
    fn an_absent_timestamp_is_not_rendered_as_the_epoch() {
        assert_eq!(timestamp(0), "—");
        // 1757000000000 ms is 2025-09-04T15:33:20Z. Checked against the epoch rather than worked
        // out by hand: the previous expectation here was 12:53:20, which is what you get from
        // dividing wrong, and it made a correct formatter look broken.
        assert_eq!(timestamp(1_757_000_000_000), "15:33:20Z");
    }
}
