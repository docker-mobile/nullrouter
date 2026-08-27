//! The Quota Tracker panel, plus the two panels that honestly have nothing
//! behind them yet.
//!
//! Three routes live in one module because they share a single question: what is
//! this build actually able to tell the user?
//!
//! * [`QuotaTrackerPanel`] is **live**. Per-account requests, tokens, and errors
//!   come from `/api/usage/stats`, joined with `/api/providers` for names. Where
//!   upstream shows a provider-reported ceiling, this shows "limit not reported",
//!   because no provider quota API is ported — see
//!   [`crate::dashboard::quota_live`].
//! * [`TokenSaverPanel`] and [`SkillsPanel`] are **preview only**. Neither has a
//!   backing endpoint in this build, so both are rendered as inert rows behind an
//!   unmissable banner, with no toggle a user could press and no value that looks
//!   like configuration.
//!
//! Derivations live in [`crate::dashboard::quota_live`] so they stay testable on
//! the native target; this file is markup and wiring only.

use crate::api::{ApiError, Hydrate};
use crate::dashboard::quota_live::{
    LIMIT_NOT_REPORTED, LIMIT_NOT_REPORTED_DETAIL, QuotaRow, QuotaSnapshot, QuotaWindow,
    format_count, load_quota, probe_limit,
};
use crate::dashboard::{SkillSummary, skill_summaries};
use leptos::prelude::*;

/// Per-row probe status, keyed by the row's `byAccount` key.
type ProbeStatus = Vec<(String, String)>;

#[component]
pub(super) fn QuotaTrackerPanel() -> impl IntoView {
    let (window, set_window) = signal(QuotaWindow::default());
    let (snapshot, set_snapshot) = signal(Hydrate::<QuotaSnapshot>::Loading);
    let probes = RwSignal::new(ProbeStatus::new());

    // Re-fetch whenever the window changes; the first run is the initial load.
    Effect::new(move |_| {
        let selected = window.get();
        set_snapshot.set(Hydrate::Loading);
        probes.set(ProbeStatus::new());
        load_into(selected, set_snapshot);
    });

    let reload = move || {
        set_snapshot.set(Hydrate::Loading);
        probes.set(ProbeStatus::new());
        load_into(window.get_untracked(), set_snapshot);
    };

    view! {
        <div class="nr-panel-stack">
            <WindowSelector window set_window loading=Signal::derive(move || snapshot.get().is_loading()) />
            <QuotaMetrics window snapshot />
            <article class="nr-card nr-anim-rise">
                <div class="nr-card-head between">
                    <div>
                        <h2><span class="nr-card-icon" aria-hidden="true">"quo"</span>"Quota Tracker"</h2>
                        <p>
                            "Requests, tokens, and errors recorded per account by this router. "
                            "Provider-reported allowances are shown where a provider reports one."
                        </p>
                    </div>
                    {move || snapshot.get().ready().map(|ready| {
                        let label = format!(
                            "{} of {} accounts report a limit",
                            ready.rows_with_limit(),
                            ready.rows.len()
                        );
                        view! { <span class="nr-status-pill is-idle"><span></span>{label}</span> }
                    })}
                </div>

                {move || match snapshot.get() {
                    Hydrate::Loading => view! { <QuotaSkeleton /> }.into_any(),
                    Hydrate::Failed(error) => view! { <QuotaFailure error reload /> }.into_any(),
                    Hydrate::Ready(ready) if ready.is_empty() => {
                        view! { <QuotaEmpty window /> }.into_any()
                    }
                    Hydrate::Ready(ready) => {
                        view! { <QuotaTable ready set_snapshot probes /> }.into_any()
                    }
                }}
            </article>
            <LimitNotice />
        </div>
    }
}

/// Start a load for `selected`, writing the result into `setter`.
fn load_into(selected: QuotaWindow, setter: WriteSignal<Hydrate<QuotaSnapshot>>) {
    spawn(async move {
        let next = match load_quota(selected).await {
            Ok(snapshot) => Hydrate::Ready(snapshot),
            Err(error) => Hydrate::Failed(error),
        };
        setter.set(next);
    });
}

/// Run a future on the browser's task queue.
///
/// Native builds have no queue and no `fetch` behind these calls, so the future
/// is dropped rather than driven — the panel is left in its `Loading` state,
/// which is the truth on a target that cannot make the request.
#[cfg(target_arch = "wasm32")]
fn spawn<F>(task: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(task);
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn<F>(task: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    drop(task);
}

#[component]
fn WindowSelector(
    window: ReadSignal<QuotaWindow>,
    set_window: WriteSignal<QuotaWindow>,
    loading: Signal<bool>,
) -> impl IntoView {
    view! {
        <div class="nr-quota-windows" role="group" aria-label="Usage window">
            <For
                each=|| QuotaWindow::ALL
                key=|option| option.label()
                children=move |option| {
                    view! {
                        <button
                            type="button"
                            class="nr-quota-window"
                            aria-pressed=move || if window.get() == option { "true" } else { "false" }
                            disabled=move || loading.get()
                            on:click=move |_| set_window.set(option)
                        >
                            {option.label()}
                        </button>
                    }
                }
            />
        </div>
    }
}

#[component]
fn QuotaMetrics(
    window: ReadSignal<QuotaWindow>,
    snapshot: ReadSignal<Hydrate<QuotaSnapshot>>,
) -> impl IntoView {
    let accounts = move || {
        snapshot
            .get()
            .ready()
            .map(|ready| ready.rows.len().to_string())
    };
    let requests = move || {
        snapshot
            .get()
            .ready()
            .map(|ready| format_count(ready.recorded_requests()))
    };
    let tokens = move || {
        snapshot
            .get()
            .ready()
            .map(|ready| format_count(ready.recorded_tokens()))
    };

    view! {
        <div class="nr-quota-metrics nr-stagger">
            <QuotaMetric label="Accounts with usage" value=Signal::derive(accounts) detail=move || window.get().detail() tone="info" />
            <QuotaMetric label="Requests recorded" value=Signal::derive(requests) detail=move || window.get().detail() tone="info" />
            <QuotaMetric label="Tokens recorded" value=Signal::derive(tokens) detail=move || window.get().detail() tone="info" />
            <article class="nr-card nr-metric-card warn">
                <span class="nr-metric-label">"Provider limits"</span>
                <strong>"Not reported"</strong>
                <small>{LIMIT_NOT_REPORTED_DETAIL}</small>
            </article>
        </div>
    }
}

/// One metric card whose value is a reading, a skeleton, or a dash.
///
/// The three cases are visually distinct on purpose: a skeleton says "still
/// asking", and the dash says "the request failed" — neither can be mistaken for
/// a figure of zero.
#[component]
fn QuotaMetric(
    label: &'static str,
    value: Signal<Option<String>>,
    detail: impl Fn() -> &'static str + Send + Sync + 'static,
    tone: &'static str,
) -> impl IntoView {
    view! {
        <article class=format!("nr-card nr-metric-card {tone}")>
            <span class="nr-metric-label">{label}</span>
            {move || match value.get() {
                Some(ready) => view! { <strong>{ready}</strong> }.into_any(),
                None => view! {
                    <strong class="nr-skeleton nr-skeleton-text-short" aria-label="Loading">"—"</strong>
                }.into_any(),
            }}
            <small>{detail()}</small>
        </article>
    }
}

#[component]
fn QuotaSkeleton() -> impl IntoView {
    view! {
        <div class="nr-quota-table-scroll" aria-busy="true">
            <p class="nr-visually-hidden">"Loading recorded account usage."</p>
            <div class="nr-preview-list">
                <For
                    each=|| 0..4
                    key=|row| *row
                    children=|_| view! { <div class="nr-skeleton nr-skeleton-row"></div> }
                />
            </div>
        </div>
    }
}

#[component]
fn QuotaFailure(error: ApiError, reload: impl Fn() + Send + Sync + 'static) -> impl IntoView {
    view! {
        <div class="nr-empty-state" role="alert">
            <strong>"Recorded usage could not be read"</strong>
            <span>{error.message()}</span>
            <p>
                <button
                    type="button"
                    class="nr-button secondary small"
                    on:click=move |_| reload()
                >
                    "Try again"
                </button>
            </p>
        </div>
    }
}

#[component]
fn QuotaEmpty(window: ReadSignal<QuotaWindow>) -> impl IntoView {
    view! {
        <div class="nr-empty-state">
            <strong>"No account usage recorded in this window"</strong>
            <span>
                {move || format!(
                    "{} produced no per-account records. This is what the router recorded, not a failed request.",
                    window.get().detail()
                )}
            </span>
        </div>
    }
}

#[component]
fn QuotaTable(
    ready: QuotaSnapshot,
    set_snapshot: WriteSignal<Hydrate<QuotaSnapshot>>,
    probes: RwSignal<ProbeStatus>,
) -> impl IntoView {
    let window_requests = ready.window_requests;
    let rows = ready.rows.clone();
    let caption = format!(
        "{} accounts, {} requests recorded. Share is of recorded requests in this window, not of any allowance.",
        rows.len(),
        format_count(ready.recorded_requests())
    );

    view! {
        <div class="nr-quota-table-scroll">
            <table class="nr-quota-table">
                <caption>{caption}</caption>
                <thead>
                    <tr>
                        <th scope="col">"Account"</th>
                        <th scope="col">"Share of recorded requests"</th>
                        <th scope="col" class="nr-quota-numeric">"Requests"</th>
                        <th scope="col" class="nr-quota-numeric">"Tokens"</th>
                        <th scope="col" class="nr-quota-numeric">"Errors"</th>
                        <th scope="col">"Provider limit"</th>
                    </tr>
                </thead>
                <tbody class="nr-stagger">
                    <For
                        each=move || rows.clone()
                        key=|row| row.key.clone()
                        children=move |row| {
                            view! { <QuotaTableRow row window_requests set_snapshot probes /> }
                        }
                    />
                </tbody>
            </table>
        </div>
    }
}

#[component]
fn QuotaTableRow(
    row: QuotaRow,
    window_requests: u64,
    set_snapshot: WriteSignal<Hydrate<QuotaSnapshot>>,
    probes: RwSignal<ProbeStatus>,
) -> impl IntoView {
    let share = row.share_percent(window_requests);
    let summary = row.bar_summary(window_requests);
    let status_id = row.status_id();
    let key = row.key.clone();
    let account = row.account.clone();
    let provider = row.provider.clone();
    let model = row.model.clone();
    let matched = row.matched_connection;
    let requests = format_count(row.requests);
    let tokens = format_count(row.total_tokens);
    let errors = format_count(row.errors);
    let limit = row.limit;
    let limit_label = row.limit_label();
    let limit_percent = row.limit_percent();
    let connection_id = row.connection_id.clone();
    let probe_label = row.probe_label();

    let status_for_row = {
        let key = key.clone();
        move || {
            probes.get().iter().find_map(|(row_key, message)| {
                (*row_key == key).then(|| message.clone())
            })
        }
    };
    let pending = RwSignal::new(false);
    let probe = {
        let key = key.clone();
        let connection_id = connection_id.clone();
        move |_| {
            let Some(connection_id) = connection_id.clone() else {
                return;
            };
            let key = key.clone();
            pending.set(true);
            spawn(async move {
                let outcome = probe_limit(&connection_id).await;
                let message = outcome.message();
                let limit = outcome.limit();
                pending.set(false);
                probes.update(|entries| {
                    entries.retain(|(row_key, _)| *row_key != key);
                    entries.push((key.clone(), message));
                });
                // A ceiling only ever reaches the table through this path, so a
                // row can show one exactly when a provider reported one.
                if limit.is_some() {
                    set_snapshot.update(|state| {
                        if let Hydrate::Ready(snapshot) = state {
                            snapshot.set_limit(&key, limit);
                        }
                    });
                }
            });
        }
    };

    view! {
        <tr>
            <th scope="row">
                <span class="nr-quota-account">
                    <strong>{account}</strong>
                    <small>{provider}</small>
                    {model.map(|model| view! { <small>{model}</small> })}
                    {(!matched).then(|| view! {
                        <small class="nr-quota-orphan">
                            "Not in the current connection list"
                        </small>
                    })}
                </span>
            </th>
            <td>
                // The bar is decorative: `summary` carries the same information
                // as text, and the percentage is printed beneath it, so neither
                // colour nor width is ever the only signal.
                <span class="nr-quota-share">
                    <span class="nr-quota-bar" aria-hidden="true">
                        <span style=format!("width:{share}%")></span>
                    </span>
                    <small>{format!("{share}% of recorded requests")}</small>
                    <span class="nr-visually-hidden">{summary}</span>
                </span>
            </td>
            <td class="nr-quota-numeric">{requests}</td>
            <td class="nr-quota-numeric">{tokens}</td>
            <td class="nr-quota-numeric">{errors}</td>
            <td>
                {if limit.is_some() {
                    view! {
                        <span class="nr-quota-share">
                            <strong>{limit_label.clone()}</strong>
                            {limit_percent.map(|percent| view! {
                                <small>{format!("{percent}% of the reported limit")}</small>
                            })}
                        </span>
                    }.into_any()
                } else {
                    view! {
                        <span class="nr-quota-unreported">{LIMIT_NOT_REPORTED}</span>
                    }.into_any()
                }}
                {connection_id.map(|_| view! {
                    <button
                        type="button"
                        class="nr-quota-probe"
                        aria-label=probe_label.clone()
                        aria-describedby=status_id.clone()
                        disabled=move || pending.get()
                        on:click=probe
                    >
                        {move || if pending.get() { "Checking…" } else { "Check limit" }}
                    </button>
                })}
                <span id=status_id class="nr-quota-row-status" aria-live="polite">
                    {status_for_row}
                </span>
            </td>
        </tr>
    }
}

#[component]
fn LimitNotice() -> impl IntoView {
    view! {
        <aside class="nr-quota-notice is-warn">
            <strong>"Why no account allowance is shown"</strong>
            <span>
                "Upstream reads each account's remaining quota from the provider's own usage API. "
                "None of those are ported here, so "
                <code class="nr-preview-code">"GET /api/usage/{connectionId}"</code>
                " answers with an empty quota list. Every figure above is usage this router recorded; "
                "no ceiling, percentage, or reset time is inferred from it."
            </span>
        </aside>
    }
}

// ── preview-only panels ──────────────────────────────────────────────────────

/// Token Saver.
///
/// Upstream drives this page from `rtkEnabled`, `headroomEnabled`, and
/// `headroomUrl` on `/api/settings`, plus `/api/headroom/*` for the sidecar
/// process. This build's `/api/settings` projection carries none of those keys
/// (`SettingsView` in `services/state-actix/src/store.rs`), and `/api/headroom`
/// answers every mutation with `501 unsupported`. There is therefore nothing to
/// read and nothing that would persist, so the panel renders no controls at all —
/// only a description of what the settings would do, behind a banner.
///
/// Deliberately not a row of disabled toggles: a toggle showing "off" asserts a
/// stored value, and this build has no such value either way.
#[component]
pub(super) fn TokenSaverPanel() -> impl IntoView {
    view! {
        <div class="nr-panel-stack">
            <PreviewBanner
                title="Preview only — nothing here is connected to the router"
                detail="No setting on this page is stored, sent, or applied. This build's settings API carries no token-saver keys, and its headroom endpoints refuse every change, so these are descriptions of upstream behaviour rather than configuration."
            />
            <article class="nr-card nr-anim-rise">
                <div class="nr-card-head between">
                    <div>
                        <h2><span class="nr-card-icon" aria-hidden="true">"sav"</span>"Token Saver"</h2>
                        <p>"What upstream's token-saving options do, and why neither is available in this port."</p>
                    </div>
                    <span class="nr-status-pill is-degraded"><span></span>"Not implemented"</span>
                </div>
                <div class="nr-preview-list nr-stagger">
                    <PreviewRow
                        label="RTK compression"
                        detail="Upstream compacts request tokens before dispatch, toggled by the rtkEnabled setting."
                        reason="This build's /api/settings response has no rtkEnabled field, so there is no value to show or change."
                    />
                    <PreviewRow
                        label="Headroom service"
                        detail="Upstream runs a headroom sidecar on port 8787 and proxies its dashboard."
                        reason="This build answers /api/headroom/start and /stop with 501 unsupported; the sidecar is not ported."
                    />
                </div>
            </article>
        </div>
    }
}

/// Skills.
///
/// Upstream's page is a static index: it renders the `SKILLS` constant and offers
/// copy buttons for raw GitHub URLs. There is no endpoint behind it there either,
/// which is why this panel is not "unwired" so much as reference material — and
/// it is labelled as reference rather than as router state.
///
/// The endpoint column is upstream's own text, not a claim about this build's
/// routing table, and the banner says so. The upstream repository URL is
/// deliberately not offered as a copy action: those documents describe upstream,
/// and handing them over from this page would imply they describe this port.
#[component]
pub(super) fn SkillsPanel() -> impl IntoView {
    view! {
        <div class="nr-panel-stack">
            <PreviewBanner
                title="Reference only — not read from this router"
                detail="This list is upstream's own skill index, included for reference. It is not fetched from the router, does not reflect which endpoints this build serves, and nothing here can be installed or applied from this page."
            />
            <article class="nr-card nr-anim-rise">
                <div class="nr-card-head between">
                    <div>
                        <h2><span class="nr-card-icon" aria-hidden="true">"ext"</span>"Skills"</h2>
                        <p>"Upstream's agent-skill documents and the endpoint each one describes."</p>
                    </div>
                    <span class="nr-status-pill is-idle"><span></span>"Static reference"</span>
                </div>
                <ul class="nr-preview-list nr-stagger">
                    <For
                        each=|| skill_summaries().to_vec()
                        key=|skill| skill.id
                        children=|skill| view! { <SkillReferenceRow skill /> }
                    />
                </ul>
            </article>
        </div>
    }
}

#[component]
fn PreviewBanner(title: &'static str, detail: &'static str) -> impl IntoView {
    view! {
        <aside class="nr-preview-banner" role="note">
            <span class="nr-preview-mark" aria-hidden="true">"!"</span>
            <p>
                <strong>{title}</strong>
                " "
                {detail}
            </p>
        </aside>
    }
}

/// One inert row.
///
/// A `div`, not a `button`: it cannot be focused or activated, so there is no
/// interaction that could be read as a save. `reason` states why, so the row
/// explains itself without the user having to try it.
#[component]
fn PreviewRow(label: &'static str, detail: &'static str, reason: &'static str) -> impl IntoView {
    view! {
        <div class="nr-preview-row">
            <span>
                <strong>{label}</strong>
                <small>{detail}</small>
                <small>{reason}</small>
            </span>
            <span class="nr-status-pill is-degraded"><span></span>"No control"</span>
        </div>
    }
}

#[component]
fn SkillReferenceRow(skill: SkillSummary) -> impl IntoView {
    view! {
        <li class="nr-preview-row">
            <span>
                <strong>{skill.name}</strong>
                <small>{skill.description}</small>
            </span>
            {match skill.endpoint {
                Some(endpoint) => view! {
                    <code class="nr-preview-code">{endpoint}</code>
                }.into_any(),
                None => view! {
                    <span class="nr-quota-unreported">"index, no endpoint"</span>
                }.into_any(),
            }}
        </li>
    }
}
