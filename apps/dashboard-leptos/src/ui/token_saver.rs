//! Token Saver panel: PXPIPE, live.
//!
//! PXPIPE renders bulky Claude-format context as dense PNGs, which bill by pixel
//! rather than by token. This panel installs it, starts and stops it, turns it on for
//! the request path, and shows what it has actually done — see
//! [`crate::dashboard::token_saver_live`] for the reading of each reply.
//!
//! Three things this panel is careful about, because each is a way it could mislead:
//!
//! * **The savings are estimates and say so, every time they appear.** They come from
//!   character counts and pixel areas, not from provider-billed usage. A panel that
//!   presented them as billing would be lying about money, so the figures carry the
//!   word "estimated" and point at the Usage page for the truth.
//! * **"Not running" and "not known" are different states.** The worker lives in the
//!   runtime service; when that is unreachable the running state is genuinely unknown,
//!   and the pill says Unknown rather than picking the reassuring side.
//! * **A refusal is rendered as the router's own sentence.** No greyed-out button that
//!   explains nothing: where an action cannot succeed, the reason it cannot is the
//!   thing on screen.
//!
//! Upstream ships this surface built but unreachable — its toggle sits behind a
//! `{false && …}` and its sidebar entry is commented out, marked "experimental, not
//! exposed to users yet". It is reachable here, because a working feature nobody can
//! find is indistinguishable from a missing one.

use crate::api::Hydrate;
use crate::dashboard::token_saver_live::{
    ActionOutcome, Event, EventTone, Health, INSTALL_PATH, Logs, RESTART_PATH, START_PATH,
    STOP_PATH, Savings, Settings, Stats, Status, StatusProblem, WindowId, control, enabled_body,
    format_tokens, format_uptime, install_blocker, load_health, load_logs, load_settings,
    load_stats, load_status, min_chars_body, node_shortfall, reason_label, run_state, save_setting,
};
use leptos::prelude::*;

/// Panel styles, shared verbatim with the actix host.
const TOKEN_SAVER_STYLES: &str =
    include_str!("../../../../services/dashboard-actix/static/assets/dashboard/token-saver.css");

/// Which request is in flight.
///
/// One at a time: install, start, stop and restart all touch the same worker, so the
/// panel serialises them rather than letting two answers race into one status line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Busy {
    Installing,
    Starting,
    Stopping,
    Restarting,
    Saving,
}

impl Busy {
    const fn label(self) -> &'static str {
        match self {
            Self::Installing => "Installing the package…",
            Self::Starting => "Loading the transform…",
            Self::Stopping => "Unloading the transform…",
            Self::Restarting => "Reloading the transform…",
            Self::Saving => "Saving the setting…",
        }
    }
}

#[derive(Clone, Copy)]
struct PanelState {
    status: RwSignal<Option<Status>>,
    /// Set when the running state could not be read, with the sentence why.
    unknown: RwSignal<Option<String>>,
    /// The install state from a failed status read, which is still this service's own.
    offline_install: RwSignal<Option<(bool, Option<String>)>>,
    health: RwSignal<Hydrate<Health>>,
    stats: RwSignal<Hydrate<Stats>>,
    logs: RwSignal<Hydrate<Logs>>,
    settings: RwSignal<Hydrate<Settings>>,
    window: RwSignal<WindowId>,
    busy: RwSignal<Option<Busy>>,
    outcome: RwSignal<Option<ActionOutcome>>,
}

impl PanelState {
    fn new() -> Self {
        Self {
            status: RwSignal::new(None),
            unknown: RwSignal::new(None),
            offline_install: RwSignal::new(None),
            health: RwSignal::new(Hydrate::Loading),
            stats: RwSignal::new(Hydrate::Loading),
            logs: RwSignal::new(Hydrate::Loading),
            settings: RwSignal::new(Hydrate::Loading),
            window: RwSignal::new(WindowId::default()),
            busy: RwSignal::new(None),
            outcome: RwSignal::new(None),
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn spawn<F: std::future::Future<Output = ()> + 'static>(task: F) {
    wasm_bindgen_futures::spawn_local(task);
}

/// Native builds have no executor and no browser to fetch from.
///
/// Dropping the future is the honest outcome: the panel stays in whatever state the
/// caller set, and no fabricated success appears.
#[cfg(not(target_arch = "wasm32"))]
fn spawn<F: std::future::Future<Output = ()> + 'static>(task: F) {
    drop(task);
}

fn reload_status(state: PanelState) {
    spawn(async move {
        match load_status().await {
            Ok(status) => {
                state.status.set(Some(status));
                state.unknown.set(None);
                state.offline_install.set(None);
            }
            Err(StatusProblem::Unreadable) => {
                state.status.set(None);
                state.unknown.set(Some(String::from(
                    "The router's answer did not have the documented shape.",
                )));
            }
            Err(StatusProblem::Unknown {
                message,
                installed,
                version,
            }) => {
                state.status.set(None);
                state.unknown.set(Some(message));
                state.offline_install.set(Some((installed, version)));
            }
        }
    });
}

fn reload_health(state: PanelState) {
    state.health.set(Hydrate::Loading);
    spawn(async move {
        let next = load_health()
            .await
            .map_or_else(Hydrate::Failed, Hydrate::Ready);
        state.health.set(next);
    });
}

fn reload_stats(state: PanelState) {
    spawn(async move {
        let next = load_stats()
            .await
            .map_or_else(Hydrate::Failed, Hydrate::Ready);
        state.stats.set(next);
    });
}

fn reload_logs(state: PanelState) {
    spawn(async move {
        let next = load_logs()
            .await
            .map_or_else(Hydrate::Failed, Hydrate::Ready);
        state.logs.set(next);
    });
}

fn reload_settings(state: PanelState) {
    spawn(async move {
        let next = load_settings()
            .await
            .map_or_else(Hydrate::Failed, Hydrate::Ready);
        state.settings.set(next);
    });
}

fn reload_all(state: PanelState) {
    reload_status(state);
    reload_health(state);
    reload_stats(state);
    reload_logs(state);
    reload_settings(state);
}

/// Run one control action, then re-read rather than assume.
///
/// What the panel shows afterwards is what the router reports, not what a click hoped
/// to achieve: a start that failed halfway would otherwise leave the pill claiming
/// the worker is up.
fn dispatch(state: PanelState, busy: Busy, path: &'static str) {
    state.busy.set(Some(busy));
    state.outcome.set(None);
    spawn(async move {
        let outcome = control(path).await;
        state.busy.set(None);
        state.outcome.set(Some(outcome));
        reload_status(state);
        reload_health(state);
        reload_logs(state);
    });
}

/// Persist one settings key.
fn dispatch_setting(state: PanelState, body: String) {
    state.busy.set(Some(Busy::Saving));
    spawn(async move {
        let saved = save_setting(body).await;
        state.busy.set(None);
        match saved {
            Ok(settings) => state.settings.set(Hydrate::Ready(settings)),
            Err(error) => {
                // The stored value is unknown now, so the toggle is not left showing a
                // state that was never saved: the settings are re-read instead.
                state.outcome.set(Some(ActionOutcome::Failed(error)));
                reload_settings(state);
            }
        }
    });
}

#[component]
pub(super) fn TokenSaverPanel() -> impl IntoView {
    let state = PanelState::new();
    reload_all(state);

    view! {
        <style>{TOKEN_SAVER_STYLES}</style>
        <div class="nr-panel-stack nr-token-saver">
            <StatusCard state />
            <SavingsCard state />
            <TimelineCard state />
            <HistoryCard state />
            <LogCard state />
        </div>
    }
}

/// What is installed, whether it is running, and the controls.
#[component]
fn StatusCard(state: PanelState) -> impl IntoView {
    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head between">
                <div>
                    <h2>
                        <span class="nr-card-icon" aria-hidden="true">"sav"</span>
                        "Token Saver"
                    </h2>
                    <p>
                        "PXPIPE renders bulky Claude-format context as dense images, which bill by \
                         pixel rather than by token."
                    </p>
                </div>
                {move || {
                    let pill = run_state(
                        state.status.get().as_ref(),
                        state.health.get().ready(),
                        state.unknown.get().as_deref(),
                    );
                    view! {
                        <span class=format!("nr-status-pill {}", pill.tone())>
                            <span></span>
                            {pill.label().to_owned()}
                        </span>
                    }
                }}
            </div>

            {move || state.unknown.get().map(|message| view! {
                // Not "stopped": the worker lives in another service, and a claim about
                // a process this page never reached would send someone to fix the wrong
                // thing.
                <p class="nr-token-saver-warning" role="status">{message}</p>
            })}

            {move || node_shortfall(state.status.get().as_ref()).map(|message| view! {
                // The failure this prevents is otherwise baffling: the package installs,
                // imports, and then fails every transform on a missing runtime global.
                <p class="nr-token-saver-warning" role="status">{message}</p>
            })}

            {move || install_blocker(state.status.get().as_ref()).map(|message| view! {
                <p class="nr-token-saver-warning" role="status">{message}</p>
            })}

            <dl class="nr-token-saver-facts">
                <Fact
                    label="Version"
                    value=Signal::derive(move || {
                        state.status.get()
                            .and_then(|status| status.version)
                            .or_else(|| state.offline_install.get().and_then(|(_, version)| version))
                            .map_or_else(|| String::from("—"), |version| format!("v{version}"))
                    })
                />
                <Fact
                    label="Loaded for"
                    value=Signal::derive(move || {
                        state.status.get().map_or_else(
                            || String::from("—"),
                            |status| format_uptime(status.uptime_ms),
                        )
                    })
                />
                <Fact
                    label="Node"
                    value=Signal::derive(move || {
                        state.status.get()
                            .and_then(|status| status.node_version)
                            .map_or_else(|| String::from("—"), |version| format!("v{version}"))
                    })
                />
                <Fact
                    label="Requires"
                    value=Signal::derive(move || {
                        state.status.get()
                            .and_then(|status| status.requires_node)
                            .unwrap_or_else(|| String::from("—"))
                    })
                />
            </dl>

            <EnabledRow state />
            <ThresholdRow state />
            <Controls state />
            <HealthList state />
        </article>
    }
}

#[component]
fn Fact(label: &'static str, value: Signal<String>) -> impl IntoView {
    view! {
        <div class="nr-token-saver-fact">
            <dt>{label}</dt>
            <dd>{move || value.get()}</dd>
        </div>
    }
}

/// The switch that governs the request path.
#[component]
fn EnabledRow(state: PanelState) -> impl IntoView {
    view! {
        <div class="nr-token-saver-row">
            <div>
                <p class="nr-token-saver-row-title">"Compress prompts as images"</p>
                <p class="nr-token-saver-row-detail">
                    "When on, Claude-format requests over the threshold are handed to the \
                     transform before dispatch. Any failure sends the original request unchanged."
                </p>
            </div>
            {move || match state.settings.get() {
                Hydrate::Ready(settings) => {
                    let enabled = settings.pxpipe_enabled;
                    let installed = state.status.get().is_some_and(|status| status.installed);
                    // Turning it on with nothing installed is allowed, and every request
                    // then records `not_installed` rather than silently doing nothing.
                    // Refusing the toggle would hide the reason.
                    view! {
                        <button
                            type="button"
                            class=move || if enabled { "nr-toggle is-on" } else { "nr-toggle" }
                            role="switch"
                            aria-checked=move || if enabled { "true" } else { "false" }
                            disabled=move || state.busy.get().is_some()
                            on:click=move |_| dispatch_setting(state, enabled_body(!enabled))
                        >
                            <span class="nr-toggle-track"><span class="nr-toggle-knob"></span></span>
                            <span class="nr-toggle-label">
                                {if enabled { "On" } else { "Off" }}
                                {(!installed && enabled).then_some(" — nothing installed")}
                            </span>
                        </button>
                    }.into_any()
                }
                Hydrate::Loading => view! {
                    <span class="nr-token-saver-row-detail">"Reading the setting…"</span>
                }.into_any(),
                // Never a toggle showing "off" here: that would assert a stored value
                // this page could not read.
                Hydrate::Failed(error) => view! {
                    <span class="nr-token-saver-warning">
                        {format!("The stored setting could not be read. {}", error.message())}
                    </span>
                }.into_any(),
            }}
        </div>
    }
}

/// The size threshold, in characters.
#[component]
fn ThresholdRow(state: PanelState) -> impl IntoView {
    let draft = RwSignal::new(String::new());
    Effect::new(move |_| {
        if let Hydrate::Ready(settings) = state.settings.get() {
            draft.set(settings.pxpipe_min_chars.to_string());
        }
    });

    view! {
        <div class="nr-token-saver-row">
            <div>
                <p class="nr-token-saver-row-title">"Minimum request size"</p>
                <p class="nr-token-saver-row-detail">
                    "Requests smaller than this are dispatched untouched. The package applies its \
                     own threshold as well, against compressible content rather than body size, \
                     so a large request can still be refused as below its minimum."
                </p>
            </div>
            <div class="nr-token-saver-threshold">
                <input
                    type="number"
                    min="0"
                    step="1000"
                    aria-label="Minimum request size in characters"
                    prop:value=move || draft.get()
                    on:input=move |event| draft.set(event_target_value(&event))
                    disabled=move || state.busy.get().is_some()
                />
                <button
                    type="button"
                    class="nr-button subtle"
                    disabled=move || state.busy.get().is_some()
                    on:click=move |_| {
                        if let Ok(value) = draft.get().trim().parse::<u64>() {
                            dispatch_setting(state, min_chars_body(value));
                        }
                    }
                >
                    "Save"
                </button>
            </div>
        </div>
    }
}

/// Install, start, stop, restart.
#[component]
fn Controls(state: PanelState) -> impl IntoView {
    view! {
        <div class="nr-token-saver-controls">
            <button
                type="button"
                class="nr-button"
                disabled=move || state.busy.get().is_some()
                on:click=move |_| dispatch(state, Busy::Installing, INSTALL_PATH)
            >
                {move || if state.status.get().is_some_and(|status| status.installed) {
                    "Repair install"
                } else {
                    "Install"
                }}
            </button>
            <button
                type="button"
                class="nr-button subtle"
                disabled=move || state.busy.get().is_some()
                on:click=move |_| dispatch(state, Busy::Starting, START_PATH)
            >
                "Start"
            </button>
            <button
                type="button"
                class="nr-button subtle"
                disabled=move || state.busy.get().is_some()
                on:click=move |_| dispatch(state, Busy::Stopping, STOP_PATH)
            >
                "Stop"
            </button>
            <button
                type="button"
                class="nr-button subtle"
                disabled=move || state.busy.get().is_some()
                on:click=move |_| dispatch(state, Busy::Restarting, RESTART_PATH)
            >
                "Reload"
            </button>
            <button
                type="button"
                class="nr-button subtle"
                disabled=move || state.busy.get().is_some()
                on:click=move |_| reload_all(state)
            >
                "Refresh"
            </button>
        </div>

        <div class="nr-token-saver-live" aria-live="polite">
            {move || state.busy.get().map(|busy| view! {
                <p class="nr-progress-indeterminate">{busy.label()}</p>
            })}
            {move || state.outcome.get().map(|outcome| match outcome {
                ActionOutcome::Completed(message) => view! {
                    <p class="nr-token-saver-done">{message}</p>
                }.into_any(),
                // The router's own sentence, with its code. A greyed-out button would
                // explain nothing.
                ActionOutcome::Refused { code, message } => view! {
                    <p class="nr-token-saver-warning">
                        {code.map(|code| format!("{code}: "))}
                        {message}
                    </p>
                }.into_any(),
                ActionOutcome::Failed(error) => view! {
                    <p class="nr-token-saver-warning">
                        {format!("The request itself failed. {}", error.message())}
                    </p>
                }.into_any(),
            })}
        </div>
    }
}

/// Installed → loads → transforms.
#[component]
fn HealthList(state: PanelState) -> impl IntoView {
    view! {
        {move || match state.health.get() {
            Hydrate::Ready(health) => {
                let steps = health.checks.clone();
                let error = health.error.clone();
                view! {
                    <ul class="nr-token-saver-checks">
                        {steps.into_iter().map(|step| view! {
                            <li class=if step.ok { "is-ok" } else { "is-bad" }>
                                <span class="nr-token-saver-check-label">{step.label}</span>
                                {step.detail.map(|detail| view! {
                                    <span class="nr-token-saver-check-detail">{detail}</span>
                                })}
                            </li>
                        }).collect_view()}
                    </ul>
                    {(!health.healthy).then(|| error.map(|error| view! {
                        <p class="nr-token-saver-warning">{error}</p>
                    }))}
                }.into_any()
            }
            Hydrate::Loading => view! {
                <p class="nr-token-saver-row-detail">"Running the health check…"</p>
            }.into_any(),
            Hydrate::Failed(error) => view! {
                <p class="nr-token-saver-warning">
                    {format!("The health check could not be run. {}", error.message())}
                </p>
            }.into_any(),
        }}
    }
}

/// The windowed totals.
#[component]
fn SavingsCard(state: PanelState) -> impl IntoView {
    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head between">
                <div>
                    <h2>"Token savings (estimated)"</h2>
                    <p>
                        "Computed from character counts and image pixel areas — not from what the \
                         provider billed. The Usage page holds the recorded cost of each request."
                    </p>
                </div>
                <div class="nr-token-saver-windows" role="group" aria-label="Time window">
                    {WindowId::ALL.into_iter().map(|id| view! {
                        <button
                            type="button"
                            class=move || if state.window.get() == id {
                                "nr-token-saver-window is-active"
                            } else {
                                "nr-token-saver-window"
                            }
                            aria-pressed=move || if state.window.get() == id { "true" } else { "false" }
                            on:click=move |_| state.window.set(id)
                        >
                            {id.label()}
                        </button>
                    }).collect_view()}
                </div>
            </div>
            {move || match state.stats.get() {
                Hydrate::Ready(stats) => {
                    let window = stats.windows.window(state.window.get()).clone();
                    view! { <SavingsFigures window /> }.into_any()
                }
                Hydrate::Loading => view! {
                    <p class="nr-token-saver-row-detail">"Reading the recorded activity…"</p>
                }.into_any(),
                Hydrate::Failed(error) => view! {
                    <p class="nr-token-saver-warning">
                        {format!("The activity could not be read. {}", error.message())}
                    </p>
                }.into_any(),
            }}
        </article>
    }
}

#[component]
fn SavingsFigures(window: Savings) -> impl IntoView {
    let requests = window.requests;
    view! {
        <dl class="nr-token-saver-figures">
            <Figure label="Requests" value=window.requests.to_string() />
            <Figure label="Compressed" value=window.compressed.to_string() />
            <Figure label="Left alone" value=window.bypassed.to_string() />
            <Figure label="Failed" value=window.errors.to_string() />
            <Figure label="Tokens before (est.)" value=format_tokens(window.tokens_before_est) />
            <Figure label="Tokens after (est.)" value=format_tokens(window.tokens_after_est) />
            <Figure label="Saved (est.)" value=format_tokens(window.tokens_saved_est) />
            <Figure label="Reduction (est.)" value=format!("{}%", window.saved_pct) />
            <Figure label="Images" value=window.images_generated.to_string() />
            <Figure label="Average time" value=format!("{}ms", window.avg_compression_ms) />
        </dl>
        {(requests == 0).then(|| view! {
            <p class="nr-token-saver-row-detail">
                "Nothing recorded in this window yet. Turn the saver on and route a large \
                 Claude-format request to see figures here."
            </p>
        })}
    }
}

#[component]
fn Figure(label: &'static str, value: String) -> impl IntoView {
    view! {
        <div class="nr-token-saver-figure">
            <dt>{label}</dt>
            <dd>{value}</dd>
        </div>
    }
}

/// Tokens saved per day, as bars.
///
/// Bars rather than a charting library: the shape is a month of one number, and a
/// dependency for that would be a dependency for nothing. Each bar carries its own
/// figure as text, so the chart is readable without seeing it.
#[component]
fn TimelineCard(state: PanelState) -> impl IntoView {
    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head">
                <div>
                    <h2>"Tokens saved — last 30 days (estimated)"</h2>
                    <p>"One bar per day. The figures are estimates, as above."</p>
                </div>
            </div>
            {move || match state.stats.get() {
                Hydrate::Ready(stats) => {
                    let peak = stats
                        .timeline
                        .iter()
                        .map(|day| day.tokens_saved_est)
                        .max()
                        .unwrap_or(0);
                    if peak == 0 {
                        return view! {
                            <p class="nr-token-saver-row-detail">
                                "No savings recorded yet."
                            </p>
                        }.into_any();
                    }
                    view! {
                        <ul class="nr-token-saver-timeline">
                            {stats.timeline.into_iter().map(|day| {
                                // Integer arithmetic: peak is non-zero here.
                                let height = day.tokens_saved_est * 100 / peak;
                                let title = format!(
                                    "{}: {} tokens saved (estimated) across {} requests",
                                    day.date,
                                    day.tokens_saved_est,
                                    day.requests,
                                );
                                let hover = title.clone();
                                view! {
                                    <li title=hover>
                                        <span
                                            class="nr-token-saver-bar"
                                            style=format!("height:{height}%")
                                        ></span>
                                        <span class="nr-visually-hidden">{title}</span>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }.into_any()
                }
                Hydrate::Loading => view! {
                    <p class="nr-token-saver-row-detail">"Reading the timeline…"</p>
                }.into_any(),
                Hydrate::Failed(error) => view! {
                    <p class="nr-token-saver-warning">
                        {format!("The timeline could not be read. {}", error.message())}
                    </p>
                }.into_any(),
            }}
        </article>
    }
}

/// The recent attempts, with why each one went the way it did.
#[component]
fn HistoryCard(state: PanelState) -> impl IntoView {
    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head">
                <div>
                    <h2>"Recent attempts"</h2>
                    <p>
                        "Every request the saver looked at, including the ones it left alone. \
                         The reason is the answer to \"why did nothing happen\"."
                    </p>
                </div>
            </div>
            {move || match state.stats.get() {
                Hydrate::Ready(stats) if !stats.recent.is_empty() => view! {
                    <div class="nr-table-scroll">
                        <table class="nr-table">
                            <thead>
                                <tr>
                                    <th scope="col">"Request size"</th>
                                    <th scope="col">"Before (est.)"</th>
                                    <th scope="col">"After (est.)"</th>
                                    <th scope="col">"Saved (est.)"</th>
                                    <th scope="col">"Images"</th>
                                    <th scope="col">"Time"</th>
                                    <th scope="col">"Outcome"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {stats.recent.into_iter().map(|event| view! {
                                    <HistoryRow event />
                                }).collect_view()}
                            </tbody>
                        </table>
                    </div>
                }.into_any(),
                Hydrate::Ready(_) => view! {
                    <p class="nr-token-saver-row-detail">"No attempts recorded yet."</p>
                }.into_any(),
                Hydrate::Loading => view! {
                    <p class="nr-token-saver-row-detail">"Reading the recent attempts…"</p>
                }.into_any(),
                Hydrate::Failed(error) => view! {
                    <p class="nr-token-saver-warning">
                        {format!("The attempts could not be read. {}", error.message())}
                    </p>
                }.into_any(),
            }}
        </article>
    }
}

#[component]
fn HistoryRow(event: Event) -> impl IntoView {
    let tone = event.outcome();
    let applied = event.applied;
    let dash = || String::from("—");
    view! {
        <tr>
            <td>{format!("{} chars", event.original_chars)}</td>
            <td>{if applied { format_tokens(event.tokens_before_est) } else { dash() }}</td>
            <td>{if applied { format_tokens(event.tokens_after_est) } else { dash() }}</td>
            <td>{if applied { format_tokens(event.tokens_saved_est) } else { dash() }}</td>
            <td>{if applied { event.image_count.to_string() } else { dash() }}</td>
            <td>{format!("{}ms", event.duration_ms)}</td>
            <td>
                <span class=format!("nr-status-pill {}", tone.class())>
                    <span></span>
                    {reason_label(&event.reason).to_owned()}
                </span>
                {event.detail.filter(|_| tone != EventTone::Compressed).map(|detail| view! {
                    <span class="nr-token-saver-check-detail">{detail}</span>
                })}
            </td>
        </tr>
    }
}

/// The install log, and the worker's own stderr.
#[component]
fn LogCard(state: PanelState) -> impl IntoView {
    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head">
                <div>
                    <h2>"Logs"</h2>
                    <p>"npm's output from the last install, and anything the transform printed."</p>
                </div>
            </div>
            {move || match state.logs.get() {
                Hydrate::Ready(logs) => view! {
                    {(!logs.install_log.trim().is_empty()).then(|| view! {
                        <pre class="nr-token-saver-log" aria-label="Install log">
                            {logs.install_log.clone()}
                        </pre>
                    })}
                    {logs.worker_log.clone().map(|log| view! {
                        <pre class="nr-token-saver-log" aria-label="Transform output">{log}</pre>
                    })}
                    {(logs.install_log.trim().is_empty() && logs.worker_log.is_none()).then(|| view! {
                        <p class="nr-token-saver-row-detail">"Nothing logged yet."</p>
                    })}
                }.into_any(),
                Hydrate::Loading => view! {
                    <p class="nr-token-saver-row-detail">"Reading the logs…"</p>
                }.into_any(),
                Hydrate::Failed(error) => view! {
                    <p class="nr-token-saver-warning">
                        {format!("The logs could not be read. {}", error.message())}
                    </p>
                }.into_any(),
            }}
        </article>
    }
}
