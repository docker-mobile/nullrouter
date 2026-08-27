//! Headroom panel: what this machine holds, and what this build will not change.
//!
//! Headroom is an external Python subsystem that compresses prompts before they
//! leave the router. `nullrouter-api` detects it for real, and refuses to mutate
//! it — see [`crate::dashboard::headroom_live`] for why the line is drawn there.
//!
//! This panel's job is to make that split legible:
//!
//! * Detection is rendered as detection: the interpreter that answered, the
//!   installed version, and each extra's state as **text** ("Installed" / "Not
//!   installed" / "State not reported"), never as a colour a reader has to
//!   decode.
//! * A refused action is rendered as a bordered explanation carrying the
//!   router's own sentence, plus the `pip` requirement to run by hand. There is
//!   no greyed-out Install button: a disabled control invites clicking and
//!   explains nothing.
//! * A completed action is only claimed when the router said `success: true`
//!   ([`ActionOutcome::changed_the_host`]). Anything else says what did not
//!   happen.
//!
//! The log region is `aria-live="polite"` with `.nr-progress-indeterminate`
//! while a request is in flight, so a screen-reader user learns of new output
//! without the panel stealing focus.

use crate::api::{ApiError, Hydrate};
use crate::dashboard::headroom_live::{
    ActionOutcome, ActionSupport, ExtraRow, ExtrasReport, LogTail, install_extras, load_log,
    load_report, restart_proxy,
};
use leptos::prelude::*;

/// Panel styles, shared verbatim with the actix host.
///
/// The CSR build links no stylesheet of its own, so the same file the host
/// serves from `/assets/dashboard/headroom.css` is inlined here. One source, two
/// delivery paths.
const HEADROOM_STYLES: &str =
    include_str!("../../../../services/dashboard-actix/static/assets/dashboard/headroom.css");

/// Which request is in flight, if any.
///
/// One at a time: an install and a restart touch the same subsystem, so the
/// panel serialises them rather than letting two answers race into one status
/// line.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Busy {
    Installing(Vec<String>),
    Restarting,
    RefreshingLog,
}

impl Busy {
    /// What the panel is waiting on, for the live region and the progress bar.
    fn label(&self) -> String {
        match self {
            Self::Installing(extras) if extras.is_empty() => {
                String::from("Requesting the headroom base install…")
            }
            Self::Installing(extras) => {
                format!("Requesting install of {}…", extras.join(", "))
            }
            Self::Restarting => String::from("Requesting a headroom proxy restart…"),
            Self::RefreshingLog => String::from("Reading the install log…"),
        }
    }
}

/// Everything this panel reads and writes.
#[derive(Clone, Copy)]
struct PanelState {
    report: RwSignal<Hydrate<ExtrasReport>>,
    log: RwSignal<Hydrate<LogTail>>,
    busy: RwSignal<Option<Busy>>,
    /// The last mutating result, kept visible until the next one replaces it.
    outcome: RwSignal<Option<ActionOutcome>>,
}

impl PanelState {
    fn new() -> Self {
        Self {
            report: RwSignal::new(Hydrate::Loading),
            log: RwSignal::new(Hydrate::Loading),
            busy: RwSignal::new(None),
            outcome: RwSignal::new(None),
        }
    }
}

/// Spawn a task on the browser's executor.
#[cfg(target_arch = "wasm32")]
fn spawn<F: std::future::Future<Output = ()> + 'static>(task: F) {
    wasm_bindgen_futures::spawn_local(task);
}

/// Native builds have no executor and no browser to fetch from.
///
/// Dropping the future is the honest outcome: the panel stays in whatever state
/// the caller set before spawning, and no fabricated success appears.
#[cfg(not(target_arch = "wasm32"))]
fn spawn<F: std::future::Future<Output = ()> + 'static>(task: F) {
    drop(task);
}

/// Load, or reload, the detection report.
fn reload_report(state: PanelState) {
    state.report.set(Hydrate::Loading);
    spawn(async move {
        let next = load_report()
            .await
            .map_or_else(Hydrate::Failed, Hydrate::Ready);
        state.report.set(next);
    });
}

/// Load, or reload, the install log tail.
///
/// `reset` is false for a poll after an action: the lines already on screen stay
/// put rather than flashing back to a skeleton.
fn reload_log(state: PanelState, reset: bool) {
    if reset {
        state.log.set(Hydrate::Loading);
    }
    spawn(async move {
        let next = load_log().await.map_or_else(Hydrate::Failed, Hydrate::Ready);
        state.log.set(next);
    });
}

/// Ask the router to install these extras.
///
/// The panel does not touch the report afterwards on its own: it re-reads it, so
/// what the rows show is what the host holds, not what this page hoped a click
/// would achieve.
fn dispatch_install(state: PanelState, extras: Vec<String>) {
    state.busy.set(Some(Busy::Installing(extras.clone())));
    state.outcome.set(None);
    spawn(async move {
        let outcome = install_extras(extras).await;
        state.busy.set(None);
        state.outcome.set(Some(outcome.clone()));
        // A refusal changed nothing, so there is nothing to re-read. Re-reading
        // after a real install is what keeps the rows honest.
        if outcome.changed_the_host() {
            reload_report(state);
        }
        reload_log(state, false);
    });
}

/// Ask the router to restart the proxy.
fn dispatch_restart(state: PanelState) {
    state.busy.set(Some(Busy::Restarting));
    state.outcome.set(None);
    spawn(async move {
        let outcome = restart_proxy().await;
        state.busy.set(None);
        state.outcome.set(Some(outcome));
        reload_log(state, false);
    });
}

/// Re-read the install log on demand.
fn dispatch_log_refresh(state: PanelState) {
    state.busy.set(Some(Busy::RefreshingLog));
    spawn(async move {
        let next = load_log().await.map_or_else(Hydrate::Failed, Hydrate::Ready);
        state.busy.set(None);
        state.log.set(next);
    });
}

#[component]
pub(super) fn HeadroomPanel() -> impl IntoView {
    let state = PanelState::new();
    reload_report(state);
    reload_log(state, true);

    view! {
        <style>{HEADROOM_STYLES}</style>
        <div class="nr-panel-stack nr-headroom-panel">
            <ExtrasCard state />
            <RestartCard state />
            <LogCard state />
        </div>
    }
}

/// Detection plus the extras rows.
#[component]
fn ExtrasCard(state: PanelState) -> impl IntoView {
    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head between">
                <div>
                    <h2>"Headroom compression"</h2>
                    <p>
                        "Read from the local router over GET /api/headroom/extras. Headroom is an external Python package, so this panel reports what is installed on this machine."
                    </p>
                </div>
                <SourcePill state />
            </div>
            {move || {
                if state.report.with(Hydrate::is_loading) {
                    view! { <ExtrasSkeleton /> }.into_any()
                } else if let Some(error) = state.report.with(Hydrate::failure) {
                    view! { <ReportFailure state error /> }.into_any()
                } else {
                    view! { <ExtrasBody state /> }.into_any()
                }
            }}
        </article>
    }
}

#[component]
fn SourcePill(state: PanelState) -> impl IntoView {
    let tone = move || {
        state.report.with(|value| match value {
            Hydrate::Loading => "is-idle",
            Hydrate::Ready(_) => "is-connected",
            Hydrate::Failed(_) => "is-degraded",
        })
    };
    let label = move || {
        state.report.with(|value| match value {
            Hydrate::Loading => "Loading",
            Hydrate::Ready(_) => "Live",
            Hydrate::Failed(_) => "Unavailable",
        })
    };

    view! {
        <span class=move || format!("nr-status-pill {}", tone())>
            <span></span>
            {label}
        </span>
    }
}

#[component]
fn ExtrasSkeleton() -> impl IntoView {
    view! {
        <div
            class="nr-headroom-skeletons"
            aria-busy="true"
            aria-label="Detecting headroom on this machine"
        >
            <div class="nr-skeleton nr-skeleton-row"></div>
            <div class="nr-skeleton nr-skeleton-row"></div>
            <div class="nr-skeleton nr-skeleton-row"></div>
        </div>
    }
}

/// The report could not be read. Says what failed and offers a retry.
#[component]
fn ReportFailure(state: PanelState, error: ApiError) -> impl IntoView {
    view! {
        <div class="nr-headroom-failure" role="alert">
            <strong>"Headroom detection unavailable"</strong>
            <p>{error.message()}</p>
            <p>
                "Nothing is being reported about this machine's Python or extras — this is not a claim that they are missing."
            </p>
            <button
                type="button"
                class="nr-button secondary small"
                on:click=move |_| reload_report(state)
            >
                "Retry detection"
            </button>
        </div>
    }
}

/// Detection banner, extras rows, and the install action or its refusal.
#[component]
fn ExtrasBody(state: PanelState) -> impl IntoView {
    let rows = move || {
        state
            .report
            .with(|value| value.ready().map(ExtrasReport::rows).unwrap_or_default())
    };
    let summary = move || {
        state.report.with(|value| {
            value.ready().map_or_else(String::new, |report| {
                let (installed, total) = report.installed_count();
                format!("{installed} of {total} compression extras installed")
            })
        })
    };

    view! {
        <div class="nr-headroom-extras">
            <PythonBanner state />
            <p class="nr-headroom-meta">{summary}</p>
            <div class="nr-stagger nr-headroom-extras" aria-label="Headroom compression extras">
                <For each=rows key=|row| row.name.clone() children=move |row| view! { <ExtraCard row /> } />
            </div>
            <InstallAction state />
        </div>
    }
}

/// Python detection, stated in words.
#[component]
fn PythonBanner(state: PanelState) -> impl IntoView {
    let python = move || state.report.with(|value| value.ready().map(ExtrasReport::python));
    let version_line = move || {
        state.report.with(|value| {
            value.ready().map(|report| {
                if report.installed {
                    format!("headroom-ai {}", report.version_label())
                } else {
                    format!(
                        "headroom-ai is not installed for this interpreter (requires Python {} or newer)",
                        report.min_python()
                    )
                }
            })
        })
    };

    view! {
        {move || {
            python()
                .map(|status| {
                    let detected = status.is_detected();
                    let class = if detected {
                        "nr-headroom-detect is-ready"
                    } else {
                        "nr-headroom-detect is-missing"
                    };
                    view! {
                        <div class=class>
                            <strong>{status.label()}</strong>
                            <p>{status.detail()}</p>
                            {version_line().map(|line| view! { <p class="nr-headroom-meta">{line}</p> })}
                        </div>
                    }
                })
        }}
    }
}

/// One extra: what it is, what it costs, and whether it is installed.
#[component]
fn ExtraCard(row: ExtraRow) -> impl IntoView {
    let state_class = match row.installed {
        Some(true) => "nr-headroom-extra-state is-on",
        Some(false) => "nr-headroom-extra-state",
        None => "nr-headroom-extra-state is-unknown",
    };
    let status_id = format!("nr-headroom-extra-{}", row.dom_suffix());
    let labelled_by = status_id.clone();
    let label = row.label();
    let description = row.description();
    let state_label = row.installed_label();

    view! {
        <div class="nr-headroom-extra" aria-labelledby=labelled_by>
            <div class="nr-headroom-extra-copy">
                <strong id=status_id>{label}</strong>
                <p>{description}</p>
            </div>
            // The dot is decoration; the word is the state, so this reads the
            // same in greyscale and to a screen reader.
            <span class=state_class>{state_label}</span>
        </div>
    }
}

/// The install control, or the router's refusal to offer one.
#[component]
fn InstallAction(state: PanelState) -> impl IntoView {
    let support = move || {
        state
            .report
            .with(|value| value.ready().map(ExtrasReport::install_support))
    };
    let missing = move || {
        state.report.with(|value| {
            value
                .ready()
                .map(|report| {
                    report
                        .rows()
                        .into_iter()
                        .filter(|row| row.installed != Some(true))
                        .map(|row| row.name)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    };
    let no_python = move || {
        state
            .report
            .with(|value| value.ready().is_some_and(|report| !report.python().is_detected()))
    };

    view! {
        {move || match support() {
            None => ().into_any(),
            Some(ActionSupport::Unsupported { reason }) => {
                view! {
                    <UnsupportedAction
                        title="Install unavailable"
                        reason
                        state
                        offer_command=true
                    />
                }
                    .into_any()
            }
            Some(ActionSupport::Supported) => {
                let extras = missing();
                let label = if extras.is_empty() {
                    String::from("Reinstall headroom base")
                } else {
                    format!("Install {}", extras.join(", "))
                };
                let aria = format!("{label} via pip on this machine");
                view! {
                    <div class="nr-headroom-actions">
                        <button
                            type="button"
                            class="nr-button primary small"
                            aria-label=aria
                            disabled=move || state.busy.with(Option::is_some) || no_python()
                            on:click=move |_| dispatch_install(state, extras.clone())
                        >
                            {label}
                        </button>
                        {move || {
                            no_python().then(|| {
                                view! {
                                    <span class="nr-headroom-status">
                                        "No suitable Python was detected, so there is nothing to install into."
                                    </span>
                                }
                            })
                        }}
                    </div>
                }
                    .into_any()
            }
        }}
        <OutcomeLine state />
    }
}

/// An action this build refuses, rendered as an explanation.
///
/// Deliberately not a disabled button. A greyed-out control tells a user only
/// that clicking is pointless; this tells them why, and what to run instead.
#[component]
fn UnsupportedAction(
    title: &'static str,
    reason: String,
    state: PanelState,
    /// Whether to offer the `pip` command. Set for the install refusal, where
    /// running it by hand is the way forward; unset for the restart refusal,
    /// where installing nothing would help.
    offer_command: bool,
) -> impl IntoView {
    // Prefer the requirement the router named in a refusal; otherwise derive it
    // from the report, so the guidance is present before any request is made.
    // Without this the install refusal would be a dead end: the action is
    // refused, so no POST ever happens to produce a `spec`.
    let command = move || {
        if !offer_command {
            return None;
        }
        let from_refusal = state.outcome.with(|outcome| match outcome {
            Some(ActionOutcome::Refused(refusal)) => refusal.spec.clone(),
            _ => None,
        });
        state.report.with(|value| {
            value.ready().map(|report| {
                from_refusal.map_or_else(
                    || report.manual_install_command(),
                    |spec| {
                        let python = report.python.as_deref().unwrap_or("python3");
                        format!("{python} -m pip install --upgrade '{spec}'")
                    },
                )
            })
        })
    };

    view! {
        <div class="nr-headroom-unsupported">
            <span class="nr-headroom-unsupported-label">
                <span aria-hidden="true">"⚠"</span>
                {title}
            </span>
            <p>{reason}</p>
            {move || {
                command()
                    .map(|command| {
                        view! {
                            <>
                                <p>"Install it yourself against the interpreter above:"</p>
                                <code class="nr-headroom-command">{command}</code>
                            </>
                        }
                    })
            }}
        </div>
    }
}

/// The result of the last mutating request.
///
/// Announced politely, and never phrased as a success unless the router said so.
#[component]
fn OutcomeLine(state: PanelState) -> impl IntoView {
    view! {
        <div aria-live="polite">
            {move || {
                state.busy.with(|busy| {
                    busy.as_ref().map(|busy| {
                        view! {
                            <>
                                <p class="nr-headroom-status">{busy.label()}</p>
                                <div
                                    class="nr-progress-indeterminate"
                                    role="progressbar"
                                    aria-label=busy.label()
                                ></div>
                            </>
                        }
                    })
                })
            }}
            {move || {
                state.outcome.with(|outcome| {
                    outcome.as_ref().map(|outcome| {
                        let class = match outcome {
                            ActionOutcome::Completed { .. } => "nr-headroom-status is-ok",
                            ActionOutcome::Refused(_) => "nr-headroom-status is-refused",
                            ActionOutcome::Failed(_) => "nr-headroom-status is-error",
                        };
                        let prefix = match outcome {
                            ActionOutcome::Completed { .. } => "",
                            // Both of these mean the host was not touched. Saying
                            // so is the point of this line.
                            ActionOutcome::Refused(_) | ActionOutcome::Failed(_) => {
                                "Nothing changed on this machine. "
                            }
                        };
                        let ignored = match outcome {
                            ActionOutcome::Refused(refusal) if !refusal.ignored.is_empty() => {
                                Some(format!(
                                    "Not recognised by this build: {}.",
                                    refusal.ignored.join(", ")
                                ))
                            }
                            _ => None,
                        };
                        view! {
                            <>
                                <p class=class>{prefix}{outcome.message()}</p>
                                {ignored.map(|text| view! { <p class="nr-headroom-status">{text}</p> })}
                            </>
                        }
                    })
                })
            }}
        </div>
    }
}

/// Proxy restart, or the router's refusal to offer one.
#[component]
fn RestartCard(state: PanelState) -> impl IntoView {
    let support = move || {
        state
            .report
            .with(|value| value.ready().map(ExtrasReport::restart_support))
    };

    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head">
                <div>
                    <h2>"Headroom proxy"</h2>
                    <p>
                        "Changing which extras are active takes effect when the local proxy restarts. POST /api/headroom/restart asks the router to do that."
                    </p>
                </div>
            </div>
            {move || match support() {
                // Until detection lands, the panel does not know what the router
                // will accept, so it offers nothing rather than guessing.
                None => view! {
                    <div class="nr-skeleton nr-skeleton-text-short" aria-hidden="true"></div>
                }
                    .into_any(),
                Some(ActionSupport::Unsupported { reason }) => {
                    view! {
                        <UnsupportedAction
                            title="Restart unavailable"
                            reason
                            state
                            offer_command=false
                        />
                    }
                        .into_any()
                }
                Some(ActionSupport::Supported) => {
                    view! {
                        <div class="nr-headroom-actions">
                            <button
                                type="button"
                                class="nr-button primary small"
                                aria-label="Restart the local headroom proxy"
                                disabled=move || state.busy.with(Option::is_some)
                                on:click=move |_| dispatch_restart(state)
                            >
                                "Restart proxy"
                            </button>
                        </div>
                    }
                        .into_any()
                }
            }}
        </article>
    }
}

/// The install log tail.
#[component]
fn LogCard(state: PanelState) -> impl IntoView {
    view! {
        <article class="nr-card nr-headroom-log-card nr-anim-rise">
            <div class="nr-card-head between">
                <div>
                    <h2>"Install log"</h2>
                    <p>
                        "The tail of the headroom install log, from GET /api/headroom/extras?log=1."
                    </p>
                </div>
                <button
                    type="button"
                    class="nr-button secondary small"
                    aria-label="Re-read the headroom install log"
                    disabled=move || state.busy.with(Option::is_some)
                    on:click=move |_| dispatch_log_refresh(state)
                >
                    "Refresh log"
                </button>
            </div>
            // Polite so new output is announced without taking focus from a
            // control the user is on.
            <div aria-live="polite">
                {move || {
                    if state.log.with(Hydrate::is_loading) {
                        view! {
                            <div
                                class="nr-skeleton nr-skeleton-row"
                                aria-label="Reading the install log"
                            ></div>
                        }
                            .into_any()
                    } else if let Some(error) = state.log.with(Hydrate::failure) {
                        view! {
                            <p class="nr-headroom-status is-error" role="alert">
                                "The install log could not be read. " {error.message()}
                            </p>
                        }
                            .into_any()
                    } else {
                        view! { <LogBody state /> }.into_any()
                    }
                }}
            </div>
            {move || {
                state
                    .busy
                    .with(|busy| {
                        busy.as_ref().map(|busy| {
                            view! {
                                <div
                                    class="nr-progress-indeterminate"
                                    role="progressbar"
                                    aria-label=busy.label()
                                ></div>
                            }
                        })
                    })
            }}
        </article>
    }
}

/// The log lines, or an explanation of why there are none.
#[component]
fn LogBody(state: PanelState) -> impl IntoView {
    let empty = move || {
        state
            .log
            .with(|value| value.ready().is_some_and(LogTail::is_empty))
    };
    let text = move || {
        state.log.with(|value| {
            value.ready().map_or_else(String::new, |log| {
                if log.is_empty() {
                    log.placeholder()
                } else {
                    log.lines().join("\n")
                }
            })
        })
    };
    let path = move || {
        state
            .log
            .with(|value| value.ready().and_then(|log| log.log_path.clone()))
    };

    view! {
        <pre
            class=move || {
                if empty() { "nr-headroom-log is-empty" } else { "nr-headroom-log" }
            }
            tabindex="0"
            aria-label="Headroom install log tail"
        >{text}</pre>
        {move || {
            path()
                .filter(|_| !empty())
                .map(|path| view! { <p class="nr-headroom-log-path">"Read from " {path}</p> })
        }}
    }
}
