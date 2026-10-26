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
use crate::routes::controls::{
    Action, Caution, Field, Outcome, OutcomeLine, Section, StateBadge, Tone,
};
use crate::routes::{
    Alert, AlertDescription, AlertVariant, BadgeVariant, Card, CardContent, CardHeader, CardTitle,
    Checkbox, EmptyState, ErrorNote, IconName, Mono, PageBody, PageHeader, Panel,
};
use crate::t;

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
        <PageHeader title=t!("nav.headroom") description=t!("headroom.description") />

        <PageBody>
            // Supervisor state and the host's Python environment, side by side. They are separate
            // reads because they can disagree: a proxy can run with no extras installed, and extras
            // can be installed with nothing running.
            <div class="grid gap-4 md:grid-cols-2">
                <Card>
                    <CardHeader>
                        <CardTitle>{t!("headroom.proxy")}</CardTitle>
                    </CardHeader>
                    <CardContent>
                        <Panel
                            state=status
                            on_retry=Callback::new(move |()| reload())
                            children=|data: HeadroomStatus| view! { <StatusBody data=data /> }
                        />
                    </CardContent>
                </Card>

                <Card>
                    <CardHeader>
                        <CardTitle>{t!("headroom.environment")}</CardTitle>
                    </CardHeader>
                    <CardContent>
                        <Panel
                            state=extras
                            on_retry=Callback::new(move |()| reload())
                            children=|data: HeadroomExtras| view! { <EnvironmentBody data=data /> }
                        />
                    </CardContent>
                </Card>
            </div>

            <ControlPanel extras=extras held=control_held outcome=outcome on_done=done />

            <ExtrasForm extras=extras on_change=refetch />

            <Card>
                <CardHeader>
                    <CardTitle>{t!("headroom.install_log")}</CardTitle>
                </CardHeader>
                <CardContent>
                    <Panel
                        state=log
                        on_retry=Callback::new(move |()| reload())
                        children=|data: HeadroomLog| view! { <LogBody data=data /> }
                    />
                </CardContent>
            </Card>
        </PageBody>
    }
}

/// Start, stop and restart the supervised daemon.
///
/// Its own component because [`Section`]'s children are a `FnOnce`, which takes the caller's
/// `Locale` with them; a second section needing a label would then have none. `t!` re-reads the
/// locale per use, so there is no locale to lose here.
#[component]
fn ControlPanel(
    extras: ReadSignal<Hydrate<HeadroomExtras>>,
    held: Signal<bool>,
    outcome: ReadSignal<Option<Outcome>>,
    on_done: Callback<Outcome>,
) -> impl IntoView {
    view! {
        <Section title=t!("headroom.controls")>
            <div class="space-y-3">
                <div class="flex flex-wrap gap-2">
                    <Action
                        label=t!("headroom.start")
                        path="/api/headroom/start".to_owned()
                        tone=Tone::Primary
                        disabled=held
                        done_label=t!("headroom.started")
                        on_done=on_done
                    />
                    // Not held by `restart_supported`: stop is idempotent and answers 200 even with
                    // nothing running, so there is no build where it refuses.
                    <Action
                        label=t!("headroom.stop")
                        path="/api/headroom/stop".to_owned()
                        done_label=t!("headroom.stopped")
                        on_done=on_done
                    />
                    <Action
                        label=t!("headroom.restart")
                        path="/api/headroom/restart".to_owned()
                        disabled=held
                        done_label=t!("headroom.restarted")
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
                            view! {
                                <p class="text-xs text-muted-foreground break-words">{message}</p>
                            }
                        })
                }}
                <OutcomeLine outcome=outcome />
            </div>
        </Section>
    }
}

#[component]
fn StatusBody(data: HeadroomStatus) -> impl IntoView {
    let last_error = data.last_error.unwrap_or_default();
    let restarts = data.restarts;

    view! {
        <div class="space-y-3">
            // The two claims the supervisor makes, kept apart on purpose and shown as two badges: a
            // process this service owns is not the same thing as a proxy that answers. "Not
            // answering" is a warning rather than neutral, because the proxy is supposed to be up.
            <div class="flex flex-wrap items-center gap-1.5">
                <StateBadge label=t!("headroom.running") on=data.running />
                <StateBadge
                    label=t!("headroom.healthy")
                    on=data.healthy
                    off_variant=BadgeVariant::Warning
                />
            </div>
            <dl class="divide-y divide-border/70 text-sm">
                <Field label=t!("headroom.state") value=data.state.unwrap_or_default() />
                <Field
                    label=t!("headroom.pid")
                    value=data.managed_pid.map(|pid| pid.to_string()).unwrap_or_default()
                />
                <Field label=t!("headroom.url") value=data.url.unwrap_or_default() />
                <Field label=t!("headroom.restarts") value=restarts.to_string() />
                <Field label=t!("headroom.message") value=data.message.unwrap_or_default() />
            </dl>
            // The supervisor's own sentence about why the last attempt ended badly. Shown as a
            // failure, not as a muted line: it is the only explanation of a proxy that is not up.
            {(!last_error.is_empty())
                .then(|| view! { <ErrorNote message=last_error.clone() /> })}
            {(restarts > 0)
                .then(|| {
                    view! {
                        <Alert variant=AlertVariant::Warning>
                            <AlertDescription>{t!("headroom.flapping")}</AlertDescription>
                        </Alert>
                    }
                })}
        </div>
    }
}

#[component]
fn EnvironmentBody(data: HeadroomExtras) -> impl IntoView {
    let python = data.python.unwrap_or_default();
    let found = !python.is_empty();
    let extras = data.extras;
    let available = data.available;

    view! {
        <div class="space-y-3">
            <StateBadge label=t!("headroom.package") on=data.installed />
            <dl class="divide-y divide-border/70 text-sm">
                <Field label=t!("headroom.version") value=data.version.unwrap_or_default() />
                <Field label=t!("headroom.python") value=python />
                <Field
                    label=t!("headroom.python_version")
                    value=data.python_version.unwrap_or_default()
                />
                <Field
                    label=t!("headroom.python_min")
                    value=data.python_min_version.unwrap_or_default()
                />
            </dl>
            // Without an interpreter nothing on this page can be installed, so the absence is a
            // warning rather than a blank row.
            {(!found)
                .then(|| {
                    view! {
                        <Alert variant=AlertVariant::Warning>
                            <AlertDescription>{t!("headroom.no_python")}</AlertDescription>
                        </Alert>
                    }
                })}
            <div class="space-y-1.5">
                <h3 class="text-[0.6875rem] font-medium tracking-wider text-muted-foreground uppercase">
                    {t!("headroom.extras")}
                </h3>
                {if available.is_empty() {
                    view! { <p class="text-sm text-muted-foreground">{t!("headroom.no_extras")}</p> }
                        .into_any()
                } else {
                    view! {
                        <div class="flex flex-wrap gap-1.5">
                            {available
                                .into_iter()
                                .map(|name| {
                                    // Presence comes from the map the server sent, not from the
                                    // available list: an extra the build recognises but has not
                                    // installed must not read as installed.
                                    let on = extras.get(&name).copied().unwrap_or(false);
                                    let label = name;
                                    view! {
                                        <StateBadge label=label on=on />
                                    }
                                })
                                .collect_view()}
                        </div>
                    }
                        .into_any()
                }}
            </div>
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
        <Section title=t!("headroom.manage_extras")>
            <div class="space-y-3">
                <p class="text-sm text-muted-foreground">{t!("headroom.manage_hint")}</p>
                {move || {
                    let names = available.get();
                    if names.is_empty() {
                        return view! {
                            <p class="text-sm text-muted-foreground">{t!("headroom.no_extras")}</p>
                        }
                            .into_any();
                    }
                    view! {
                        <div class="flex flex-wrap gap-x-4 gap-y-2">
                            {names
                                .into_iter()
                                .map(|name| {
                                    // The `Checkbox` is controlled, so the checked state is derived
                                    // from `chosen` rather than mirrored into the input. Two
                                    // bindings for `name`: one is the toggle's label, the other is
                                    // captured by the change handler.
                                    let checked_name = name.clone();
                                    let toggled = name.clone();
                                    let label = name;
                                    view! {
                                        <Checkbox
                                            checked=Signal::derive(move || {
                                                chosen.get().contains(&checked_name)
                                            })
                                            on_change=Callback::new(move |on| {
                                                let value = toggled.clone();
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
                                            })
                                            label=label
                                        />
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
                            view! {
                                <p class="text-xs text-muted-foreground break-words">{message}</p>
                            }
                        })
                }}
                <Caution text=t!("headroom.install_caution") />
                <div class="flex flex-wrap gap-2">
                    <Action
                        label=t!("headroom.install_extras")
                        path="/api/headroom/extras".to_owned()
                        method=Method::Post
                        tone=Tone::Destructive
                        disabled=held
                        done_label=t!("headroom.extras_installed")
                        body=build_body
                        on_done=report
                    />
                    <Action
                        label=t!("headroom.remove_extras")
                        path="/api/headroom/extras".to_owned()
                        method=Method::Delete
                        tone=Tone::Destructive
                        disabled=held
                        done_label=t!("headroom.extras_removed")
                        body=build_body
                        on_done=report
                    />
                </div>
                <OutcomeLine outcome=outcome />
            </div>
        </Section>
    }
}

#[component]
fn LogBody(data: HeadroomLog) -> impl IntoView {
    let path = data.log_path.unwrap_or_default();

    view! {
        <div class="space-y-2">
            {if data.log.trim().is_empty() {
                // An `EmptyState` rather than a muted sentence: this is "there is nothing here",
                // not "this failed". Its description carries the reason -- this service never
                // writes a log, so an empty one needs the explanation rather than a blank box.
                view! {
                    <EmptyState
                        title=t!("headroom.no_log")
                        description=t!("headroom.no_log_path")
                        icon=IconName::ScrollText
                    />
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
            // Where the tail was read from. Omitted when there is no file, because the empty state
            // above already says so -- printing the same sentence twice reads as two problems.
            {(!path.is_empty())
                .then(|| {
                    view! {
                        <p class="text-xs text-muted-foreground break-all">
                            <Mono value=path.clone() />
                        </p>
                    }
                })}
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
