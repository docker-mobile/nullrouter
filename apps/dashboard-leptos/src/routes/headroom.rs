//! Headroom: the supervised compression proxy, the Python environment it needs, and its extras.
//!
//! Two reads that answer different questions and must not be merged. `status` is the supervisor's
//! view of the daemon — is a process ours, is it answering, how many times has it restarted.
//! `extras` is the host's Python environment — which interpreter was found, whether `headroom-ai`
//! is installed, and which of the two compression extras are present. A proxy can be running while
//! no extras are installed, and extras can be installed with nothing running.
//!
//! `running` and `healthy` are both rendered, separately. The server keeps them apart on purpose:
//! the process existing is not the same claim as the proxy answering, and collapsing them would
//! report a wedged daemon as healthy.
//!
//! # The install controls are rendered, not exercised
//!
//! `POST /api/headroom/extras` runs a real `pip install`. The `ml` extra pulls `torch`, which is a
//! multi-gigabyte download the server gives a fifteen-minute deadline. The button is here because
//! the server supports it and a panel that hides a supported action is its own kind of lie, but it
//! is behind an explicit warning and nothing on this page triggers it on load.

use std::collections::BTreeMap;

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api::{Hydrate, Method, encode, load};
use crate::routes::controls::{Action, Caution, Field, Flag, Outcome, OutcomeLine, Section, Tone};
use crate::routes::{PageHeader, Panel};

/// `GET /api/headroom/status`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadroomStatus {
    /// A process this service owns exists.
    #[serde(default)]
    running: bool,
    /// The proxy is answering. Separate from `running`, and deliberately so.
    #[serde(default)]
    healthy: bool,
    #[serde(default)]
    url: Option<String>,
    /// The pid this service owns, when it owns one. Sent as an explicit `null` otherwise, rather
    /// than omitted.
    #[serde(default)]
    managed_pid: Option<u32>,
    /// `stopped`, `starting`, `running`, `backoff`, `failed`.
    #[serde(default)]
    state: Option<String>,
    /// Restarts since the last manual start, so a flapping proxy is visible.
    #[serde(default)]
    restarts: u32,
    /// Why the last attempt ended badly. Omitted when there was no bad ending.
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// `GET /api/headroom/extras`.
///
/// `installSupported` and `restartSupported` are the server telling a client whether its buttons do
/// anything. An earlier build of this service answered `501` to both; the flags are what say the
/// build in front of you does not. They gate the controls here rather than being ignored, so a
/// downgrade shows disabled buttons instead of live-looking ones that refuse.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadroomExtras {
    /// The extras this build recognises. `proxy` is the base and is not listed.
    #[serde(default)]
    available: Vec<String>,
    /// Whether `headroom-ai` itself is installed.
    #[serde(default)]
    installed: bool,
    #[serde(default)]
    version: Option<String>,
    /// Which extras are present, judged by their marker packages.
    #[serde(default)]
    extras: BTreeMap<String, bool>,
    /// The interpreter that answered.
    #[serde(default)]
    python: Option<String>,
    #[serde(default)]
    python_version: Option<String>,
    #[serde(default)]
    python_min_version: Option<String>,
    #[serde(default)]
    install_supported: bool,
    #[serde(default)]
    install_message: Option<String>,
    #[serde(default)]
    restart_supported: bool,
    #[serde(default)]
    restart_message: Option<String>,
}

/// `GET /api/headroom/extras?log=1`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadroomLog {
    #[serde(default)]
    log: String,
    /// Where the tail was read from, or absent when no log exists yet. Worth showing: this service
    /// never writes one, so an empty log needs the explanation.
    #[serde(default)]
    log_path: Option<String>,
}

/// The body both extras mutations take.
#[derive(Debug, Serialize)]
struct ExtrasBody {
    extras: Vec<String>,
}

#[component]
pub fn Headroom() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (status, set_status) = signal(Hydrate::<HeadroomStatus>::Loading);
    let (extras, set_extras) = signal(Hydrate::<HeadroomExtras>::Loading);
    let (log, set_log) = signal(Hydrate::<HeadroomLog>::Loading);
    let (outcome, set_outcome) = signal(None::<Outcome>);

    let reload = move || {
        set_status.set(Hydrate::Loading);
        set_extras.set(Hydrate::Loading);
        set_log.set(Hydrate::Loading);
        load("/api/headroom/status", set_status);
        // GET only. The POST on this path is the pip run, and nothing here calls it on load.
        load("/api/headroom/extras", set_extras);
        load("/api/headroom/extras?log=1", set_log);
    };
    reload();

    // For the proxy controls, which report into the controls card.
    let done = Callback::new(move |result: Outcome| {
        let ok = result.ok;
        set_outcome.set(Some(result));
        if ok {
            reload();
        }
    });

    // The extras form keeps its own outcome line, so it is handed a refetch and nothing else.
    // Sharing `done` would print one pip result in two cards at once.
    let refetch = Callback::new(move |()| reload());

    // Held until the server says restart is supported. A downgraded build answers 501 here, and the
    // flag is how it says so before a button is pressed.
    let control_held = Signal::derive(move || {
        extras
            .get()
            .ready()
            .is_some_and(|report| !report.restart_supported)
    });

    view! {
        <PageHeader
            title=locale.get("nav.headroom").to_owned()
            description=locale.get("headroom.description").to_owned()
        />
        <div class="space-y-6">
            <div class="grid gap-4 md:grid-cols-2">
                <Section title=locale.get("headroom.proxy").to_owned()>
                    <Panel
                        state=status
                        on_retry=Callback::new(move |()| reload())
                        children=|data: HeadroomStatus| view! { <StatusBody data=data /> }
                    />
                </Section>
                <Section title=locale.get("headroom.environment").to_owned()>
                    <Panel
                        state=extras
                        on_retry=Callback::new(move |()| reload())
                        children=|data: HeadroomExtras| view! { <EnvironmentBody data=data /> }
                    />
                </Section>
            </div>

            <ControlPanel
                extras=extras
                held=control_held
                outcome=outcome
                on_done=done
            />

            <ExtrasForm extras=extras on_change=refetch />

            <Section title=locale.get("headroom.install_log").to_owned()>
                <Panel
                    state=log
                    on_retry=Callback::new(move |()| reload())
                    children=|data: HeadroomLog| view! { <LogBody data=data /> }
                />
            </Section>
        </div>
    }
}

/// Start, stop and restart the supervised daemon.
///
/// Its own component because [`Section`]'s children are a `FnOnce`, which takes the caller's
/// `Locale` with them; a second section needing a label would then have none.
#[component]
fn ControlPanel(
    extras: ReadSignal<Hydrate<HeadroomExtras>>,
    held: Signal<bool>,
    outcome: ReadSignal<Option<Outcome>>,
    on_done: Callback<Outcome>,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <Section title=locale.get("headroom.controls").to_owned()>
            <div class="flex flex-wrap gap-2">
                <Action
                    label=locale.get("headroom.start").to_owned()
                    path="/api/headroom/start".to_owned()
                    tone=Tone::Primary
                    disabled=held
                    done_label=locale.get("headroom.started").to_owned()
                    on_done=on_done
                />
                // Not held by `restart_supported`: stop is idempotent and answers 200 even with
                // nothing running, so there is no build where it refuses.
                <Action
                    label=locale.get("headroom.stop").to_owned()
                    path="/api/headroom/stop".to_owned()
                    done_label=locale.get("headroom.stopped").to_owned()
                    on_done=on_done
                />
                <Action
                    label=locale.get("headroom.restart").to_owned()
                    path="/api/headroom/restart".to_owned()
                    disabled=held
                    done_label=locale.get("headroom.restarted").to_owned()
                    on_done=on_done
                />
            </div>
            // The server's own sentence about what these buttons do in this build.
            {move || {
                extras
                    .get()
                    .ready()
                    .and_then(|report| report.restart_message.clone())
                    .filter(|message| !message.is_empty())
                    .map(|message| {
                        view! { <p class="text-xs text-muted-foreground">{message}</p> }
                    })
            }}
            <OutcomeLine outcome=outcome />
        </Section>
    }
}

#[component]
fn StatusBody(data: HeadroomStatus) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let last_error = data.last_error.unwrap_or_default();
    let restarts = data.restarts;

    view! {
        <dl class="space-y-2.5 text-sm">
            <Flag label=locale.get("headroom.running").to_owned() on=data.running />
            <Flag label=locale.get("headroom.healthy").to_owned() on=data.healthy />
            <Field
                label=locale.get("headroom.state").to_owned()
                value=data.state.unwrap_or_default()
            />
            <Field
                label=locale.get("headroom.pid").to_owned()
                value=data.managed_pid.map(|pid| pid.to_string()).unwrap_or_default()
            />
            <Field label=locale.get("headroom.url").to_owned() value=data.url.unwrap_or_default() />
            <Field
                label=locale.get("headroom.restarts").to_owned()
                value=restarts.to_string()
            />
            <Field
                label=locale.get("headroom.message").to_owned()
                value=data.message.unwrap_or_default()
            />
        </dl>
        {(!last_error.is_empty())
            .then(|| {
                view! {
                    <p class="text-sm text-destructive break-words" role="alert">
                        {last_error.clone()}
                    </p>
                }
            })}
        {(restarts > 0)
            .then(|| {
                view! {
                    <p class="text-xs text-muted-foreground">
                        {locale.get("headroom.flapping").to_owned()}
                    </p>
                }
            })}
    }
}

#[component]
fn EnvironmentBody(data: HeadroomExtras) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let python = data.python.unwrap_or_default();
    let found = !python.is_empty();
    let extras = data.extras;
    let available = data.available;

    view! {
        <dl class="space-y-2.5 text-sm">
            <Flag label=locale.get("headroom.package").to_owned() on=data.installed />
            <Field
                label=locale.get("headroom.version").to_owned()
                value=data.version.unwrap_or_default()
            />
            <Field label=locale.get("headroom.python").to_owned() value=python />
            <Field
                label=locale.get("headroom.python_version").to_owned()
                value=data.python_version.unwrap_or_default()
            />
            <Field
                label=locale.get("headroom.python_min").to_owned()
                value=data.python_min_version.unwrap_or_default()
            />
        </dl>
        {(!found)
            .then(|| {
                view! {
                    <p class="text-sm text-warning break-words">
                        {locale.get("headroom.no_python").to_owned()}
                    </p>
                }
            })}
        <div class="space-y-2">
            <h3 class="text-xs font-medium text-muted-foreground uppercase tracking-wide">
                {locale.get("headroom.extras").to_owned()}
            </h3>
            {if available.is_empty() {
                view! {
                    <p class="text-sm text-muted-foreground">
                        {locale.get("headroom.no_extras").to_owned()}
                    </p>
                }
                    .into_any()
            } else {
                view! {
                    <dl class="space-y-2 text-sm">
                        {available
                            .into_iter()
                            .map(|name| {
                                // Presence comes from the map the server sent, not from the
                                // available list: an extra the build recognises but has not
                                // installed must not read as installed.
                                let on = extras.get(&name).copied().unwrap_or(false);
                                view! { <Flag label=name on=on /> }
                            })
                            .collect_view()}
                    </dl>
                }
                    .into_any()
            }}
        </div>
    }
}

/// Install or remove compression extras.
///
/// The checkbox set is built from the server's own `available` list rather than a constant here, so
/// an extra added server-side appears without a rebuild and one removed stops being offered.
#[component]
fn ExtrasForm(
    extras: ReadSignal<Hydrate<HeadroomExtras>>,
    /// Refetch, run only after pip actually changed something.
    on_change: Callback<()>,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (chosen, set_chosen) = signal(Vec::<String>::new());
    let (outcome, set_outcome) = signal(None::<Outcome>);

    let available = Memo::new(move |_| {
        extras
            .get()
            .ready()
            .map_or_else(Vec::new, |report| report.available.clone())
    });
    let install_supported = Memo::new(move |_| {
        extras
            .get()
            .ready()
            .is_some_and(|report| report.install_supported)
    });
    // Nothing selected is not a request the server accepts: it answers `NO_EXTRAS` for a remove and
    // installs the bare base for an install. Held rather than sent so neither is a surprise.
    let held = Signal::derive(move || chosen.get().is_empty() || !install_supported.get());

    let build_body = Callback::new(move |()| {
        let names = chosen.get();
        if names.is_empty() {
            return None;
        }
        encode(&ExtrasBody { extras: names }).ok()
    });

    let report = Callback::new(move |result: Outcome| {
        let changed = result.ok;
        set_outcome.set(Some(result));
        // Only on success: a failed pip run leaves the environment exactly as the panel already
        // shows it, and refetching would replace the reason with a fresh, identical report.
        if changed {
            on_change.run(());
        }
    });

    view! {
        <Section title=locale.get("headroom.manage_extras").to_owned()>
            <p class="text-sm text-muted-foreground">
                {locale.get("headroom.manage_hint").to_owned()}
            </p>
            {move || {
                // Re-acquired: capturing the outer `Locale` would move it out of the rest of the
                // form, and it is not `Copy`.
                let locale = crate::i18n::use_locale();
                let names = available.get();
                if names.is_empty() {
                    return view! {
                        <p class="text-sm text-muted-foreground">
                            {locale.get("headroom.no_extras").to_owned()}
                        </p>
                    }
                        .into_any();
                }
                view! {
                    <div class="flex flex-wrap gap-4">
                        {names
                            .into_iter()
                            .map(|name| {
                                let value = name.clone();
                                view! {
                                    <label class="flex items-center gap-2 text-sm">
                                        <input
                                            type="checkbox"
                                            class="size-4"
                                            prop:checked=move || chosen.get().contains(&value)
                                            on:change={
                                                let value = name.clone();
                                                move |ev| {
                                                    let on = event_target_checked(&ev);
                                                    let value = value.clone();
                                                    set_chosen
                                                        .update(|names| {
                                                            if on {
                                                                if !names.contains(&value) {
                                                                    names.push(value);
                                                                }
                                                            } else {
                                                                names.retain(|held| *held != value);
                                                            }
                                                        });
                                                }
                                            }
                                        />
                                        <span>{name}</span>
                                    </label>
                                }
                            })
                            .collect_view()}
                    </div>
                }
                    .into_any()
            }}
            {move || {
                extras
                    .get()
                    .ready()
                    .and_then(|report| report.install_message.clone())
                    .filter(|message| !message.is_empty())
                    .map(|message| {
                        view! { <p class="text-xs text-muted-foreground">{message}</p> }
                    })
            }}
            <Caution text=locale.get("headroom.install_caution").to_owned() />
            <div class="flex flex-wrap gap-2">
                <Action
                    label=locale.get("headroom.install_extras").to_owned()
                    path="/api/headroom/extras".to_owned()
                    method=Method::Post
                    tone=Tone::Destructive
                    disabled=held
                    done_label=locale.get("headroom.extras_installed").to_owned()
                    body=build_body
                    on_done=report
                />
                <Action
                    label=locale.get("headroom.remove_extras").to_owned()
                    path="/api/headroom/extras".to_owned()
                    method=Method::Delete
                    tone=Tone::Destructive
                    disabled=held
                    done_label=locale.get("headroom.extras_removed").to_owned()
                    body=build_body
                    on_done=report
                />
            </div>
            <OutcomeLine outcome=outcome />
        </Section>
    }
}

#[component]
fn LogBody(data: HeadroomLog) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let path = data.log_path.unwrap_or_default();

    view! {
        <div class="space-y-2">
            {if data.log.trim().is_empty() {
                view! {
                    <p class="text-sm text-muted-foreground italic">
                        {locale.get("headroom.no_log").to_owned()}
                    </p>
                }
                    .into_any()
            } else {
                view! {
                    <pre class="max-h-64 overflow-auto rounded-md border border-border bg-muted/30 \
                                p-3 font-mono text-xs whitespace-pre-wrap break-words">
                        {data.log}
                    </pre>
                }
                    .into_any()
            }}
            <p class="text-xs text-muted-foreground break-all">
                {if path.is_empty() {
                    locale.get("headroom.no_log_path").to_owned()
                } else {
                    path
                }}
            </p>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{ExtrasBody, HeadroomExtras, HeadroomLog, HeadroomStatus};

    /// `GET /api/headroom/status` with nothing running, captured live.
    const LIVE_STATUS: &str = r#"{"running":false,"healthy":false,"url":"http://127.0.0.1:8787",
        "managedPid":null,"state":"stopped","restarts":0,
        "message":"no headroom proxy is running under this service"}"#;

    /// `GET /api/headroom/extras` on a host with Python 3.11 and no `headroom-ai`.
    const LIVE_EXTRAS: &str = r#"{"available":["code","ml"],"installed":false,"version":null,
        "extras":{"code":false,"ml":false},"python":"/usr/bin/python3.11","pythonVersion":"3.11",
        "pythonMinVersion":"3.10","installSupported":true,
        "installMessage":"Installing extras runs pip against the interpreter reported in `python`.",
        "restartSupported":true,
        "restartMessage":"Start, stop and restart supervise the headroom proxy."}"#;

    #[test]
    fn the_live_status_decodes_including_the_explicit_null_pid() {
        let status: HeadroomStatus = serde_json::from_str(LIVE_STATUS).unwrap_or_default();
        assert!(!status.running);
        assert!(!status.healthy);
        assert_eq!(status.state.as_deref(), Some("stopped"));
        assert_eq!(status.managed_pid, None);
        assert_eq!(status.restarts, 0);
        assert_eq!(status.url.as_deref(), Some("http://127.0.0.1:8787"));
        // Omitted when there was no failure; must not become an empty error line.
        assert!(status.last_error.is_none());
    }

    #[test]
    fn running_and_healthy_stay_separate() {
        // The supervisor owning a pid is not the proxy answering. Collapsing the two would report a
        // wedged daemon as healthy.
        let body = r#"{"running":true,"healthy":false,"state":"backoff","managedPid":4242,
            "restarts":3,"lastError":"proxy exited during startup"}"#;
        let status: HeadroomStatus = serde_json::from_str(body).unwrap_or_default();
        assert!(status.running);
        assert!(!status.healthy);
        assert_eq!(status.managed_pid, Some(4242));
        assert_eq!(status.restarts, 3);
        assert_eq!(
            status.last_error.as_deref(),
            Some("proxy exited during startup")
        );
    }

    #[test]
    fn the_live_extras_report_decodes_with_its_support_flags() {
        let report: HeadroomExtras = serde_json::from_str(LIVE_EXTRAS).unwrap_or_default();
        assert_eq!(report.available, vec!["code", "ml"]);
        assert!(!report.installed);
        assert!(report.version.is_none());
        assert_eq!(report.extras.get("code"), Some(&false));
        assert_eq!(report.extras.get("ml"), Some(&false));
        assert_eq!(report.python.as_deref(), Some("/usr/bin/python3.11"));
        assert_eq!(report.python_version.as_deref(), Some("3.11"));
        assert_eq!(report.python_min_version.as_deref(), Some("3.10"));
        assert!(report.install_supported);
        assert!(report.restart_supported);
    }

    #[test]
    fn an_earlier_build_that_refuses_is_read_as_refusing() {
        // This service used to answer 501 to install and restart. Defaulting the flags to true
        // would draw live-looking buttons against a build that rejects them.
        let report: HeadroomExtras =
            serde_json::from_str(r#"{"available":["code"],"extras":{"code":false}}"#)
                .unwrap_or_default();
        assert!(!report.install_supported);
        assert!(!report.restart_supported);
    }

    #[test]
    fn an_extra_missing_from_the_map_is_not_read_as_installed() {
        let report: HeadroomExtras =
            serde_json::from_str(r#"{"available":["code","ml"],"extras":{"code":true}}"#)
                .unwrap_or_default();
        assert_eq!(report.extras.get("code"), Some(&true));
        assert_eq!(report.extras.get("ml"), None);
        assert!(!report.extras.get("ml").copied().unwrap_or(false));
    }

    #[test]
    fn the_log_tail_decodes_and_an_absent_path_stays_absent() {
        let log: HeadroomLog =
            serde_json::from_str(r#"{"log":"","logPath":null}"#).unwrap_or_default();
        assert!(log.log.is_empty());
        assert!(log.log_path.is_none());
    }

    #[test]
    fn the_extras_body_is_the_shape_the_route_reads() {
        // `requested_extras` reads `body.extras` as an array of strings and ignores anything else.
        let encoded = serde_json::to_string(&ExtrasBody {
            extras: vec!["ml".to_owned()],
        })
        .unwrap_or_default();
        assert_eq!(encoded, r#"{"extras":["ml"]}"#);
    }

    #[test]
    fn a_shape_change_is_a_failure_rather_than_a_default() {
        // Not `[]`: serde maps a sequence onto a struct positionally, so an empty array fills every
        // `#[serde(default)]` field and decodes clean. A bare scalar has no struct reading at all.
        assert!(serde_json::from_str::<HeadroomStatus>("42").is_err());
        assert!(serde_json::from_str::<HeadroomStatus>("null").is_err());
        assert!(serde_json::from_str::<HeadroomExtras>("truncated").is_err());
    }
}
