//! The MITM Proxy panel.
//!
//! This page rendered `mitm_dashboard_state()` end to end: a `const` asserting
//! the server was stopped, the certificate missing, and DNS off for all three
//! tools, with every control `disabled`. The assertions happened to be true, but
//! nothing had asked the router — the same markup would have reported "Stopped"
//! over a running proxy.
//!
//! What is live now, and what is not:
//!
//! * **Live**: the server status card and the per-tool DNS badges read
//!   `GET /api/cli-tools/antigravity-mitm`; saved model mappings read
//!   `.../alias`. Start and Stop issue real `POST`/`DELETE` requests, and Save
//!   Mappings a real `PUT`.
//! * **Not implemented upstream of us**: the MITM proxy subsystem itself. Every
//!   write is refused — `501` for start/stop, `403` for mappings — and the panel
//!   reports the refusal rather than hiding the control. [`SUBSYSTEM_NOTICE`] says
//!   so on the page, permanently, because live controls plus real readings would
//!   otherwise imply a working feature.
//! * **Static reference**: the host lists and upstream model names, which are
//!   upstream constants rather than router state, and are labelled as the entries
//!   a user would add to their own hosts file.
//!
//! Derivations live in [`crate::dashboard::mitm_live`] so they stay testable on
//! the native target.

use std::collections::BTreeMap;

use crate::api::{ApiError, Hydrate};
use crate::dashboard::mitm_live::{
    DNS_READ_ONLY_NOTICE, MitmAction as LiveAction, MitmCheck, MitmStatus, SUBSYSTEM_NOTICE,
    WriteOutcome, load_aliases, load_status, save_aliases, start_server, stop_server,
};
use crate::dashboard::{MitmDashboardState, MitmModelMapping, MitmToolState, mitm_dashboard_state};
use crate::ui::material_icons::{
    ARROW_FORWARD, CANCEL, EXPAND_MORE, PLAY_CIRCLE, SECURITY, WARNING,
};
use leptos::prelude::*;

mod contract;
mod styles;

use contract::{HOW_LABEL, PURPOSE_LABEL};
use styles::MITM_STYLES;

/// Default base URL offered in the form, matching what the status endpoint
/// reports when it has no configured value.
const DEFAULT_BASE_URL: &str = "http://localhost:20128";

/// Everything the panel's subtrees read.
///
/// `Copy` so it can be handed to nested components without cloning, and so the
/// component-call sites stay the single-token `state` the boundary test pins.
#[derive(Clone, Copy)]
struct MitmSignals {
    status: ReadSignal<Hydrate<MitmStatus>>,
    set_status: WriteSignal<Hydrate<MitmStatus>>,
    aliases: ReadSignal<Hydrate<BTreeMap<String, String>>>,
    set_aliases: WriteSignal<Hydrate<BTreeMap<String, String>>>,
    /// The last write attempted, and how the router answered it.
    outcome: RwSignal<Option<WriteOutcome>>,
    /// Which action is in flight, if any.
    pending: RwSignal<Option<LiveAction>>,
    base_url: RwSignal<String>,
    api_key: RwSignal<String>,
    /// Model-mapping drafts, keyed `tool/alias`.
    drafts: RwSignal<Vec<(String, String)>>,
    /// Static upstream reference data: hosts, tool names, model names.
    fixture: MitmDashboardState,
}

impl MitmSignals {
    /// Read the status and the saved aliases.
    fn load(self) {
        self.set_status.set(Hydrate::Loading);
        self.set_aliases.set(Hydrate::Loading);
        let set_status = self.set_status;
        let set_aliases = self.set_aliases;
        let base_url = self.base_url;

        spawn(async move {
            let next = match load_status().await {
                Ok(status) => {
                    // Adopt the router's own base URL so the field starts from
                    // what it reported rather than from a guess.
                    if let Some(reported) = status.router_base_url.clone() {
                        base_url.set(reported);
                    }
                    Hydrate::Ready(status)
                }
                Err(error) => Hydrate::Failed(error),
            };
            set_status.set(next);
        });
        spawn(async move {
            let next = match load_aliases().await {
                Ok(aliases) => Hydrate::Ready(aliases),
                Err(error) => Hydrate::Failed(error),
            };
            set_aliases.set(next);
        });
    }

    /// Whether any write is in flight.
    fn busy(self) -> bool {
        self.pending.get().is_some()
    }

    /// Attempt one write, then re-read the status.
    ///
    /// The re-read is what keeps the card honest after a refusal: the reading is
    /// fetched again rather than assumed unchanged, so the card reflects the
    /// router even if a write partially took effect.
    fn attempt(self, action: LiveAction) {
        if self.busy() {
            return;
        }
        self.pending.set(Some(action));
        self.outcome.set(None);

        let api_key = self.api_key.get_untracked();
        let base_url = self.base_url.get_untracked();
        let outcome_signal = self.outcome;
        let pending = self.pending;

        spawn(async move {
            let outcome = match action {
                LiveAction::Start => start_server(api_key, base_url).await,
                LiveAction::Stop => stop_server().await,
                LiveAction::SaveMappings => {
                    WriteOutcome::Refused(String::from("Choose a tool before saving mappings."))
                }
            };
            pending.set(None);
            outcome_signal.set(Some(outcome));
        });
        self.reload_status();
    }

    /// Save the drafted mappings for one tool.
    fn save_tool_mappings(self, tool_id: &'static str) {
        if self.busy() {
            return;
        }
        let mappings: BTreeMap<String, String> = self
            .drafts
            .get_untracked()
            .into_iter()
            .filter_map(|(key, value)| {
                key.strip_prefix(tool_id)
                    .and_then(|rest| rest.strip_prefix('/'))
                    .filter(|_| !value.trim().is_empty())
                    .map(|alias| (alias.to_owned(), value.trim().to_owned()))
            })
            .collect();

        self.pending.set(Some(LiveAction::SaveMappings));
        self.outcome.set(None);
        let outcome_signal = self.outcome;
        let pending = self.pending;
        spawn(async move {
            let outcome = save_aliases(tool_id, mappings).await;
            pending.set(None);
            outcome_signal.set(Some(outcome));
        });
    }

    /// Re-read only the status.
    fn reload_status(self) {
        let set_status = self.set_status;
        spawn(async move {
            let next = match load_status().await {
                Ok(status) => Hydrate::Ready(status),
                Err(error) => Hydrate::Failed(error),
            };
            set_status.set(next);
        });
    }

    /// One mapping draft.
    fn draft_for(self, tool_id: &str, alias: &str) -> String {
        let key = format!("{tool_id}/{alias}");
        self.drafts
            .get()
            .iter()
            .find_map(|(entry, value)| (*entry == key).then(|| value.clone()))
            .unwrap_or_default()
    }

    /// Record one mapping draft.
    fn set_draft(self, tool_id: &str, alias: &str, value: String) {
        let key = format!("{tool_id}/{alias}");
        self.drafts.update(|entries| {
            entries.retain(|(entry, _)| *entry != key);
            entries.push((key, value));
        });
    }

    /// The alias the router already has saved for a model, if any.
    fn saved_alias(self, alias: &str) -> Option<String> {
        self.aliases
            .get()
            .ready()
            .and_then(|saved| saved.get(alias).cloned())
    }
}

/// Run a future on the browser's task queue.
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
pub(super) fn MitmPanel() -> impl IntoView {
    let (status, set_status) = signal(Hydrate::<MitmStatus>::Loading);
    let (aliases, set_aliases) = signal(Hydrate::<BTreeMap<String, String>>::Loading);
    let state = MitmSignals {
        status,
        set_status,
        aliases,
        set_aliases,
        outcome: RwSignal::new(None),
        pending: RwSignal::new(None),
        base_url: RwSignal::new(String::from(DEFAULT_BASE_URL)),
        api_key: RwSignal::new(String::new()),
        drafts: RwSignal::new(Vec::new()),
        fixture: mitm_dashboard_state(),
    };
    let expanded_tool = RwSignal::new(None::<&'static str>);
    let tools = state.fixture.tools;

    Effect::new(move |_| state.load());

    view! {
        <style>{MITM_STYLES}</style>
        <div class="nr-panel-stack nr-mitm-panel">
            <aside class="nr-mitm-risk" role="alert">
                <span class="material-symbols-outlined nr-mitm-risk-mark" data-icon="warning" aria-hidden="true">{WARNING}</span>
                <p>{state.fixture.risk_warning}</p>
            </aside>
            <aside class="nr-mitm-unported" role="note">
                <strong>"Not implemented in this port"</strong>
                <span>{SUBSYSTEM_NOTICE}</span>
            </aside>
            <ServerCard state />
            <div class="nr-mitm-tool-list" aria-label="MITM IDE tools">
                <For
                    each=move || tools.to_vec()
                    key=|tool| tool.id
                    children=move |tool| view! { <ToolCard tool state expanded_tool /> }
                />
            </div>
        </div>
    }
}

#[component]
fn ServerCard(state: MitmSignals) -> impl IntoView {
    let server = state.fixture.server;

    view! {
        <article class="nr-card nr-mitm-server">
            <div class="nr-mitm-server-head">
                <div class="nr-mitm-server-title">
                    <span class="material-symbols-outlined nr-mitm-server-icon" data-icon="security" aria-hidden="true">{SECURITY}</span>
                    <h2>{server.title}</h2>
                    {move || match state.status.get() {
                        Hydrate::Loading => view! {
                            <span class="nr-mitm-badge nr-skeleton" aria-label="Reading MITM status">"…"</span>
                        }.into_any(),
                        Hydrate::Failed(_) => view! {
                            <span class="nr-mitm-badge">"Status unavailable"</span>
                        }.into_any(),
                        Hydrate::Ready(ready) => view! {
                            <span class="nr-mitm-badge">{ready.status_label()}</span>
                        }.into_any(),
                    }}
                </div>
                <div class="nr-mitm-checks" aria-label="MITM server prerequisites">
                    {move || match state.status.get() {
                        Hydrate::Ready(ready) => view! {
                            <For
                                each=move || ready.checks().to_vec()
                                key=|check| check.label
                                children=|check| view! { <StatusCheck check /> }
                            />
                        }.into_any(),
                        Hydrate::Loading | Hydrate::Failed(_) => view! {
                            <span class="nr-mitm-check">
                                <span class="material-symbols-outlined nr-mitm-check-mark" data-icon="cancel" aria-hidden="true">{CANCEL}</span>
                                "Not read"
                            </span>
                        }.into_any(),
                    }}
                </div>
            </div>
            <div class="nr-mitm-explain">
                <p><strong>{PURPOSE_LABEL}</strong> " " {server.purpose}</p>
                <p><strong>{HOW_LABEL}</strong> " " {server.how_it_works}</p>
            </div>
            {move || state.status.get().failure().map(|error| view! {
                <StatusFailure error state />
            })}
            {move || state.status.get().ready().map(|ready| view! {
                <p class="nr-mitm-privilege">
                    {ready.privilege_note()} " " {ready.pid_label()}
                </p>
            })}
            <div class="nr-mitm-fields">
                <ServerField
                    label=server.base_url.label
                    id="nr-mitm-base-url"
                    placeholder=server.base_url.placeholder
                    value=state.base_url
                    state
                />
                <ServerField
                    label=server.api_key.label
                    id="nr-mitm-api-key"
                    placeholder=server.api_key.placeholder
                    value=state.api_key
                    state
                />
            </div>
            <div class="nr-mitm-action-row">
                <button
                    type="button"
                    class="nr-button primary small"
                    disabled=move || state.busy()
                    on:click=move |_| state.attempt(LiveAction::Start)
                >
                    <span class="material-symbols-outlined nr-mitm-action-icon" data-icon="play_circle" aria-hidden="true">{PLAY_CIRCLE}</span>
                    {LiveAction::Start.label()}
                </button>
                <button
                    type="button"
                    class="nr-button secondary small"
                    disabled=move || state.busy()
                    on:click=move |_| state.attempt(LiveAction::Stop)
                >
                    {LiveAction::Stop.label()}
                </button>
                <p class="nr-mitm-unsupported" aria-live="polite">
                    {move || match (state.pending.get(), state.outcome.get()) {
                        (Some(action), _) => action.attempt_note().to_owned(),
                        (None, Some(outcome)) => outcome.message(),
                        (None, None) => String::from(
                            "Starting the proxy is refused by this build; the button reports what the router answers."
                        ),
                    }}
                </p>
            </div>
        </article>
    }
}

#[component]
fn StatusFailure(error: ApiError, state: MitmSignals) -> impl IntoView {
    view! {
        <div class="nr-mitm-status-failure" role="alert">
            <strong>"MITM status could not be read"</strong>
            <span>{error.message()}</span>
            <button
                type="button"
                class="nr-button secondary small"
                on:click=move |_| state.load()
            >
                "Try again"
            </button>
        </div>
    }
}

#[component]
fn StatusCheck(check: MitmCheck) -> impl IntoView {
    let ok = check.ok;

    view! {
        <span class="nr-mitm-check" class:is-ok=move || ok aria-label=check.aria_label()>
            <span class="material-symbols-outlined nr-mitm-check-mark" data-icon="cancel" aria-hidden="true">{CANCEL}</span>
            {check.label}
            <span class="nr-visually-hidden">{check.detail}</span>
        </span>
    }
}

#[component]
fn ServerField(
    label: &'static str,
    id: &'static str,
    placeholder: &'static str,
    value: RwSignal<String>,
    state: MitmSignals,
) -> impl IntoView {
    view! {
        <div class="nr-mitm-field">
            <label for=id>{label}</label>
            <span class="material-symbols-outlined nr-mitm-field-arrow" data-icon="arrow_forward" aria-hidden="true">{ARROW_FORWARD}</span>
            <input
                id=id
                type="text"
                placeholder=placeholder
                prop:value=move || value.get()
                disabled=move || state.busy()
                on:input=move |event| value.set(event_target_value(&event))
            />
        </div>
    }
}

#[component]
fn ToolCard(
    tool: MitmToolState,
    state: MitmSignals,
    expanded_tool: RwSignal<Option<&'static str>>,
) -> impl IntoView {
    let tool_id = tool.id;
    let toggle = move |_| {
        expanded_tool.update(|selected| {
            *selected = match *selected {
                Some(current) if current == tool_id => None,
                Some(_) | None => Some(tool_id),
            };
        });
    };
    let dns_label = move || {
        state
            .status
            .get()
            .ready()
            .map_or("DNS not read", |ready| ready.dns_label(tool_id))
    };
    let dns_class = move || {
        state
            .status
            .get()
            .ready()
            .map_or("is-idle", |ready| ready.dns_class(tool_id))
    };
    let server_label = move || {
        state
            .status
            .get()
            .ready()
            .map_or("Server not read", MitmStatus::status_label)
    };

    view! {
        <article class="nr-mitm-tool">
            <button
                type="button"
                class="nr-mitm-tool-head"
                aria-expanded=move || {
                    if expanded_tool.get() == Some(tool_id) {
                        "true"
                    } else {
                        "false"
                    }
                }
                on:click=toggle
            >
                <span class="nr-mitm-tool-title">
                    <img src=tool.image alt="" aria-hidden="true" width="32" height="32" />
                    <span class="nr-mitm-tool-copy">
                        <span class="nr-mitm-tool-heading">
                            <strong class="nr-mitm-tool-name">{tool.name}</strong>
                            <span class="nr-mitm-tool-status">
                                <span class="nr-mitm-badge">{server_label}</span>
                                <span class=move || format!("nr-mitm-badge {}", dns_class())>{dns_label}</span>
                            </span>
                        </span>
                        <p>{tool.intercept_label}</p>
                    </span>
                </span>
                <span
                    class="material-symbols-outlined nr-mitm-chevron"
                    class:is-open=move || expanded_tool.get() == Some(tool_id)
                    data-icon="expand_more"
                    aria-hidden="true"
                >
                    {EXPAND_MORE}
                </span>
            </button>
            {move || (expanded_tool.get() == Some(tool_id)).then_some(view! {
                <div class="nr-mitm-tool-body">
                    <div class="nr-mitm-hosts">
                        <p><strong>{state.fixture.hosts_instruction}</strong></p>
                        <For
                            each=move || tool.hosts.to_vec()
                            key=|host| *host
                            children=|host| view! { <code>"127.0.0.1 " {host}</code> }
                        />
                    </div>
                    <div class="nr-mitm-tool-info">
                        <p>{tool.dns_instruction}</p>
                        <p class="nr-mitm-mapping-note">{DNS_READ_ONLY_NOTICE}</p>
                    </div>
                    <div class="nr-mitm-model-list" aria-label=format!("{} model mappings", tool.name)>
                        <For
                            each=move || tool.models.to_vec()
                            key=|model| model.alias
                            children=move |model| view! { <ModelRow model tool state /> }
                        />
                    </div>
                    <div class="nr-mitm-action-row">
                        <button
                            type="button"
                            class="nr-button primary small"
                            disabled=move || state.busy()
                            on:click=move |_| state.save_tool_mappings(tool_id)
                        >
                            <span class="material-symbols-outlined nr-mitm-action-icon" data-icon="play_circle" aria-hidden="true">{PLAY_CIRCLE}</span>
                            {LiveAction::SaveMappings.label()}
                        </button>
                    </div>
                </div>
            })}
        </article>
    }
}

#[component]
fn ModelRow(model: MitmModelMapping, tool: MitmToolState, state: MitmSignals) -> impl IntoView {
    let input_id = format!("nr-mitm-{}-{}", tool.id, model.alias);
    let label_for = input_id.clone();
    let tool_id = tool.id;
    let alias = model.alias;
    let saved = move || state.saved_alias(alias);

    view! {
        <div class="nr-mitm-model-row">
            <label class="nr-mitm-model-label" for=label_for>{model.name}</label>
            <span class="material-symbols-outlined nr-mitm-model-arrow" data-icon="arrow_forward" aria-hidden="true">{ARROW_FORWARD}</span>
            <input
                id=input_id
                type="text"
                placeholder=state.fixture.mapping_placeholder
                prop:value=move || {
                    let draft = state.draft_for(tool_id, alias);
                    if draft.is_empty() {
                        saved().unwrap_or_default()
                    } else {
                        draft
                    }
                }
                disabled=move || state.busy()
                on:input=move |event| state.set_draft(tool_id, alias, event_target_value(&event))
            />
            <span class="nr-mitm-model-saved">
                {move || saved().map_or_else(
                    || String::from("no mapping saved"),
                    |saved| format!("saved: {saved}"),
                )}
            </span>
        </div>
    }
}
