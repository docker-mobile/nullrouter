//! Providers: the connections this router holds, and the providers it could hold.
//!
//! This panel used to render `provider_groups()` — two cards of compile-time
//! tiles from `nullrouter-contracts`. It showed "Gemini · Needs attention · 0/1
//! active" on a machine with no Gemini credential, and showed nothing at all for
//! the keys someone had actually added, because no code path here ever asked the
//! router what it held.
//!
//! The rewrite splits the page along the line the fixtures blurred:
//!
//! * **Your connections** comes from `GET /api/providers` and nothing else. It
//!   is `.nr-skeleton` rows while loading, an explicit failure with a retry when
//!   the request fails, and an invitation to add one when the router genuinely
//!   holds none. There is no branch that falls back to a tile.
//! * **Provider catalog** comes from `nullrouter-providers`' registry. It is
//!   styled deliberately differently (dashed, muted, no actions) because an entry
//!   there is a capability of this build, not an account. The two lists never
//!   merge.
//!
//! Secrets are described, never shown. `GET /api/providers` strips `apiKey`,
//! `accessToken`, and `refreshToken`, so a card says "Key stored by the router.
//! Never sent to this page." rather than rendering a masked value this page would
//! have had to invent.
//!
//! Every decision about parsing, ordering, and rolling a failed delete back lives
//! in [`crate::dashboard::providers_live`], where it is unit-tested on the native
//! target. This file holds signals and markup.

use crate::api::{ApiError, Hydrate, Save};
use crate::dashboard::providers_live::{
    CatalogOption, Connection, ConnectionDraft, ConnectionList, DeleteSettlement, DraftError,
    ProviderGroupLive, TestOutcome, api_key_catalog, catalog, catalog_option, create_connection,
    delete_connection, load_connections, provider_accent, provider_initials, provider_label,
    settle_delete, test_connection,
};
use crate::dashboard::{ModelTile, model_catalog};
use leptos::prelude::*;

/// Panel styles, shared verbatim with the actix host.
///
/// The CSR build links no stylesheet of its own, so the same file the host serves
/// from `/assets/dashboard/providers-live.css` is inlined here. One source, two
/// delivery paths — the alternative was a second copy that would drift.
const PROVIDERS_LIVE_STYLES: &str =
    include_str!("../../../../services/dashboard-actix/static/assets/dashboard/providers-live.css");

/// Layout that predates the live rewrite and is still used by the detail and
/// create routes.
const PROVIDER_STYLES: &str = r"
.nr-provider-form{max-width:760px;margin:0 auto}
.nr-provider-form-body,.nr-provider-detail-grid,.nr-provider-model-list{display:grid;gap:10px}
.nr-provider-field{display:grid;gap:6px}
.nr-provider-field label,.nr-provider-section-label{color:var(--text-main-dark);font-size:.82rem;font-weight:700}
.nr-preview-input{width:100%;min-width:0;border:1px solid var(--border-dark);border-radius:8px;background:var(--surface-dark-2);color:var(--text-main-dark);padding:10px 12px;font:inherit}
.nr-provider-model-row,.nr-provider-summary-row{min-width:0;display:grid;gap:4px;border:1px solid var(--border-dark);border-radius:8px;background:var(--surface-dark-2);padding:12px}
.nr-provider-model-row strong,.nr-provider-summary-row strong{color:var(--text-main-dark)}
.nr-provider-model-row span,.nr-provider-summary-row span,.nr-provider-field small{color:var(--text-muted-dark);font-size:.82rem;line-height:1.45}
.nr-provider-form-actions,.nr-provider-detail-actions,.nr-provider-detail-head{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.nr-provider-form-actions,.nr-provider-detail-head{justify-content:space-between}
.nr-provider-detail-grid{grid-template-columns:minmax(0,.9fr) minmax(0,1.1fr)}
.nr-provider-hero-logo{width:52px;height:52px;display:grid;place-items:center;border:1px solid color-mix(in srgb,var(--provider-accent) 40%,var(--border-dark));border-radius:8px;background:color-mix(in srgb,var(--provider-accent) 16%,var(--surface-dark-2));color:var(--provider-accent);font-weight:800}
.nr-provider-summary{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:10px}
@media (max-width:860px){.nr-provider-detail-grid,.nr-provider-summary{grid-template-columns:1fr}.nr-provider-detail-head{align-items:flex-start;flex-direction:column}}
";

/// Everything the connections surface reads and writes.
///
/// One struct so the card components take a single `Copy` handle instead of six
/// signals, and so a write always updates the list and its status together.
#[derive(Clone, Copy)]
struct PanelState {
    /// The configured connections, or why they could not be read.
    list: RwSignal<Hydrate<ConnectionList>>,
    /// Which connection has an armed delete confirmation.
    ///
    /// One at a time: a page of primed destructive buttons is a trap.
    confirming: RwSignal<Option<String>>,
    /// Connection ids with a request in flight, and what kind.
    busy: RwSignal<Vec<(String, Busy)>>,
    /// Per-connection status text, announced politely.
    notes: RwSignal<Vec<(String, Note)>>,
    /// Panel-level status, for create and refresh.
    save: RwSignal<Save>,
    draft: RwSignal<ConnectionDraft>,
}

/// What a card is waiting on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Busy {
    Deleting,
    Testing,
}

/// A result to announce on one card.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Note {
    text: String,
    tone: Tone,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tone {
    Ok,
    Error,
    Neutral,
}

impl Tone {
    const fn class_name(self) -> &'static str {
        match self {
            Self::Ok => "nr-connection-status is-ok",
            Self::Error => "nr-connection-status is-error",
            Self::Neutral => "nr-connection-status",
        }
    }
}

impl PanelState {
    fn new() -> Self {
        Self {
            list: RwSignal::new(Hydrate::Loading),
            confirming: RwSignal::new(None),
            busy: RwSignal::new(Vec::new()),
            notes: RwSignal::new(Vec::new()),
            save: RwSignal::new(Save::Idle),
            draft: RwSignal::new(ConnectionDraft::default()),
        }
    }

    /// What this connection is waiting on, if anything.
    fn busy_kind(self, id: &str) -> Option<Busy> {
        self.busy.with(|busy| {
            busy.iter()
                .find(|(busy_id, _kind)| busy_id == id)
                .map(|(_id, kind)| *kind)
        })
    }

    fn set_busy(self, id: &str, kind: Busy) {
        self.busy.update(|busy| {
            busy.retain(|(busy_id, _kind)| busy_id != id);
            busy.push((id.to_owned(), kind));
        });
    }

    fn clear_busy(self, id: &str) {
        self.busy
            .update(|busy| busy.retain(|(busy_id, _kind)| busy_id != id));
    }

    fn note(self, id: &str) -> Option<Note> {
        self.notes.with(|notes| {
            notes
                .iter()
                .find(|(note_id, _note)| note_id == id)
                .map(|(_id, note)| note.clone())
        })
    }

    fn set_note(self, id: &str, text: String, tone: Tone) {
        self.notes.update(|notes| {
            notes.retain(|(note_id, _note)| note_id != id);
            notes.push((id.to_owned(), Note { text, tone }));
        });
    }

    fn clear_note(self, id: &str) {
        self.notes
            .update(|notes| notes.retain(|(note_id, _note)| note_id != id));
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
/// the caller set before spawning (`Loading`, `Saving`), and no fabricated
/// success appears.
#[cfg(not(target_arch = "wasm32"))]
fn spawn<F: std::future::Future<Output = ()> + 'static>(task: F) {
    drop(task);
}

/// Load, or reload, the connection list.
///
/// `reset` is false for a refresh after a write: the rows already on screen stay
/// put rather than flashing back to skeletons.
fn reload(state: PanelState, reset: bool) {
    if reset {
        state.list.set(Hydrate::Loading);
    }
    spawn(async move {
        let next = load_connections()
            .await
            .map_or_else(Hydrate::Failed, Hydrate::Ready);
        state.list.set(next);
    });
}

/// Delete one connection: remove it now, put it back if the router refuses.
fn dispatch_delete(state: PanelState, id: String) {
    // Taking the row out of the list *is* the optimistic update. If the id is
    // gone (a refresh landed first) there is nothing to delete and nothing to
    // roll back.
    let Some(pending) = state
        .list
        .try_update(|list| match list {
            Hydrate::Ready(ready) => ready.take(&id),
            Hydrate::Loading | Hydrate::Failed(_) => None,
        })
        .flatten()
    else {
        return;
    };

    state.confirming.set(None);
    state.clear_note(&id);
    state.set_busy(&id, Busy::Deleting);
    state.save.set(Save::Saving);

    spawn(async move {
        let outcome = delete_connection(&id).await;
        state.clear_busy(&id);
        match settle_delete(pending, outcome) {
            DeleteSettlement::Removed => {
                state.save.set(Save::Saved);
                state.clear_note(&id);
            }
            DeleteSettlement::RolledBack { pending, error } => {
                let name = pending.name().to_owned();
                state.list.update(|list| {
                    if let Hydrate::Ready(ready) = list {
                        ready.restore(pending);
                    }
                });
                state.save.set(Save::Failed(error));
                state.set_note(
                    &id,
                    format!("{name} was not deleted. {}", error.message()),
                    Tone::Error,
                );
            }
        }
    });
}

/// Test one connection and record what came back.
fn dispatch_test(state: PanelState, id: String) {
    state.set_busy(&id, Busy::Testing);
    state.set_note(&id, String::from("Testing connection…"), Tone::Neutral);

    spawn(async move {
        let outcome = test_connection(&id).await;
        state.clear_busy(&id);
        let tone = match outcome {
            TestOutcome::Passed => Tone::Ok,
            TestOutcome::Failed(_) | TestOutcome::Rejected(_) => Tone::Error,
            // Nothing was tested, so this is neither good nor bad news.
            TestOutcome::NotTested(_) => Tone::Neutral,
        };
        // Only a real verdict updates the row's recorded status.
        if let Some(status) = outcome.recorded_status() {
            state.list.update(|list| {
                if let Hydrate::Ready(ready) = list {
                    ready.set_test_status(&id, Some(status.to_owned()));
                }
            });
        }
        state.set_note(&id, outcome.message(), tone);
    });
}

/// Create a connection from the draft, then refresh from the server.
fn dispatch_create(state: PanelState) {
    let body = match state.draft.with_untracked(ConnectionDraft::create_body) {
        Ok(body) => body,
        // The form already shows this; a click on a disabled control cannot get
        // here, so there is nothing further to report.
        Err(_error) => return,
    };
    state.save.set(Save::Saving);

    spawn(async move {
        match create_connection(body).await {
            Ok(connection) => {
                // Show the row the router just confirmed, then re-read the list
                // so ordering and any server-applied defaults are the server's.
                state.list.update(|list| {
                    if let Hydrate::Ready(ready) = list {
                        ready.insert(connection);
                    }
                });
                state.draft.set(ConnectionDraft::default());
                state.save.set(Save::Saved);
                reload(state, false);
            }
            Err(error) => state.save.set(Save::Failed(error)),
        }
    });
}

#[component]
pub(super) fn ProvidersPanel() -> impl IntoView {
    let state = PanelState::new();
    // Land on the page already knowing what is configured: a user should not
    // have to press anything to find out.
    reload(state, true);

    view! {
        <ProviderStyles />
        <div class="nr-panel-stack">
            <ConnectionsCard state />
            <CreateConnectionCard state />
            <CatalogCard />
            <ModelsPanel />
        </div>
    }
}

/// The configured connections, in whichever of its four states applies.
#[component]
fn ConnectionsCard(state: PanelState) -> impl IntoView {
    let summary = move || {
        state.list.with(|list| {
            list.ready().map(|ready| {
                format!(
                    "{} across {} provider{}",
                    plural(ready.len(), "connection"),
                    ready.provider_count(),
                    if ready.provider_count() == 1 { "" } else { "s" },
                )
            })
        })
    };

    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"key"</span>"Your connections"</h2>
                    <p>"Provider credentials this router holds, read from the local state service."</p>
                </div>
                <div class="nr-connections-summary">
                    <Show when=move || state.list.with(Hydrate::is_loading)>
                        <span class="nr-spinner" aria-hidden="true"></span>
                    </Show>
                    <button
                        type="button"
                        class="nr-button secondary small"
                        disabled=move || state.list.with(Hydrate::is_loading)
                        on:click=move |_event| reload(state, true)
                    >
                        "Refresh"
                    </button>
                </div>
            </div>
            <p class="nr-connection-status" role="status" aria-live="polite">
                {move || summary().or_else(|| state.save.with(|save| save.status().map(str::to_owned)))}
            </p>
            {move || match state.list.get() {
                Hydrate::Loading => view! { <ConnectionsSkeleton /> }.into_any(),
                Hydrate::Failed(error) => view! { <ConnectionsFailure state error /> }.into_any(),
                Hydrate::Ready(ready) if ready.is_empty() => view! { <ConnectionsEmpty /> }.into_any(),
                Hydrate::Ready(ready) => view! { <ConnectionGroups state groups=ready.groups() /> }
                    .into_any(),
            }}
        </article>
    }
}

/// Placeholder rows, labelled so the wait is announced rather than only shown.
#[component]
fn ConnectionsSkeleton() -> impl IntoView {
    view! {
        <div class="nr-connection-grid" role="status" aria-label="Loading your provider connections">
            {(0..2)
                .map(|_index| {
                    view! {
                        <div class="nr-connection-skeleton" aria-hidden="true">
                            <span class="nr-skeleton nr-skeleton-text-short"></span>
                            <span class="nr-skeleton nr-skeleton-text"></span>
                            <span class="nr-skeleton nr-skeleton-row"></span>
                        </div>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// The request failed. Say so, and offer the only useful action.
#[component]
fn ConnectionsFailure(state: PanelState, error: ApiError) -> impl IntoView {
    view! {
        <div class="nr-panel-notice is-error" role="alert">
            <strong>"Could not read your connections"</strong>
            <span>
                {error.message()}
                " Nothing is shown below, because this page cannot tell whether you have \
                 connections configured."
            </span>
            <button
                type="button"
                class="nr-button secondary small"
                on:click=move |_event| reload(state, true)
            >
                "Try again"
            </button>
        </div>
    }
}

/// The router holds nothing. The one state the old panel could not express.
#[component]
fn ConnectionsEmpty() -> impl IntoView {
    view! {
        <div class="nr-panel-notice">
            <strong>"No provider connections yet"</strong>
            <span>
                "This router has no provider credentials configured, so it cannot reach any \
                 upstream. Add one below, or pick a provider from the catalog to see what this \
                 build supports."
            </span>
            <a class="nr-button primary small" href="#nr-add-connection">"Add a connection"</a>
        </div>
    }
}

/// Connections, grouped by provider.
#[component]
fn ConnectionGroups(state: PanelState, groups: Vec<ProviderGroupLive>) -> impl IntoView {
    view! {
        <div class="nr-panel-stack">
            <For
                each=move || groups.clone()
                key=|group| group.provider.clone()
                children=move |group| view! { <ConnectionGroup state group /> }
            />
        </div>
    }
}

#[component]
fn ConnectionGroup(state: PanelState, group: ProviderGroupLive) -> impl IntoView {
    let heading_id = format!("nr-provider-group-{}", group.provider.replace('.', "-"));
    let labelled_by = heading_id.clone();
    let detail_href = format!("/dashboard/providers/{}", group.provider);
    let connections = group.connections.clone();
    let summary = group.summary();
    let label = group.label.clone();
    let accent = group.accent;

    view! {
        <section aria-labelledby=labelled_by>
            <div class="nr-card-head between">
                <div>
                    <h3 id=heading_id>{label}</h3>
                    <p>{summary}</p>
                </div>
                <a class="nr-button secondary small" href=detail_href>"Provider detail"</a>
            </div>
            <div class="nr-connection-grid nr-stagger">
                <For
                    each=move || connections.clone()
                    key=|connection| connection.id.clone()
                    children={
                        move |connection| {
                            view! { <ConnectionCard state connection accent=accent.clone() /> }
                        }
                    }
                />
            </div>
        </section>
    }
}

/// One connection: what it is, what the router last learned about it, and the two
/// things you can do to it.
#[component]
fn ConnectionCard(state: PanelState, connection: Connection, accent: String) -> impl IntoView {
    let id = connection.id.clone();
    let heading_id = connection.heading_id();
    let labelled_by = heading_id.clone();
    let status_id = connection.status_id();
    let auth = connection.auth_kind();
    let status = connection.test_status();
    let account = connection.account_label().map(str::to_owned);
    let priority = connection.priority_label();
    let default_model = connection.default_model.clone();
    let last_error = connection.last_error.clone();
    let is_active = connection.is_active;
    let name = connection.name.clone();
    let provider_id = connection.provider.clone();
    let delete_label = connection.delete_label();
    let test_label = connection.test_label();
    let glyph = provider_initials(&connection.provider);

    let deleting = {
        let id = id.clone();
        move || state.busy_kind(&id) == Some(Busy::Deleting)
    };
    let confirming = {
        let id = id.clone();
        move || {
            state
                .confirming
                .with(|target| target.as_deref() == Some(id.as_str()))
        }
    };
    // A `Memo` so both the class and the text of the status region can read it:
    // a plain closure capturing the id is not `Copy`.
    let note = {
        let id = id.clone();
        Memo::new(move |_previous| state.note(&id))
    };

    view! {
        <article
            class="nr-connection-card"
            class:is-inactive=move || !is_active
            class:is-deleting=deleting
            style=format!("--provider-accent: {accent}")
            aria-labelledby=labelled_by
        >
            <div class="nr-connection-top">
                <span class="nr-connection-identity">
                    <span class="nr-connection-logo" aria-hidden="true">{glyph}</span>
                    <span>
                        <h4 id=heading_id>{name.clone()}</h4>
                        <span>{provider_label(&provider_id)} " · " {provider_id}</span>
                    </span>
                </span>
                <span class=format!("nr-status-pill {}", status.class_name())>
                    <span></span>{status.label()}
                </span>
            </div>

            <div class="nr-connection-meta">
                <div>
                    <span class="nr-meta-label">"Auth"</span>
                    <span class="nr-meta-value">{auth.label().to_owned()}</span>
                </div>
                <div>
                    <span class="nr-meta-label">"Routing"</span>
                    <span class="nr-meta-value">
                        {if is_active { "Active" } else { "Inactive" }}
                    </span>
                </div>
                <div>
                    <span class="nr-meta-label">"Priority"</span>
                    <span class="nr-meta-value">{priority}</span>
                </div>
                <div>
                    <span class="nr-meta-label">"Account"</span>
                    // No email on most API-key rows; named as absent rather than
                    // filled in with something plausible.
                    <span class="nr-meta-value">
                        {account.unwrap_or_else(|| String::from("Not reported"))}
                    </span>
                </div>
                {default_model
                    .map(|model| {
                        view! {
                            <div>
                                <span class="nr-meta-label">"Default model"</span>
                                <span class="nr-meta-value">{model}</span>
                            </div>
                        }
                    })}
            </div>

            // The credential line states where the secret lives. The API redacts
            // it, so this page genuinely does not know its value.
            <p class="nr-connection-credential">{auth.credential_note()}</p>

            {last_error
                .map(|error| {
                    view! { <p class="nr-connection-error">"Last error: " {error}</p> }
                })}

            <Show
                when=confirming
                fallback={
                    let actions = ActionLabels {
                        id: id.clone(),
                        test: test_label,
                        delete: delete_label,
                    };
                    move || view! { <ConnectionActions state labels=actions.clone() /> }
                }
            >
                {
                    let id = id.clone();
                    let name = name.clone();
                    move || view! { <DeleteConfirm state id=id.clone() name=name.clone() /> }
                }
            </Show>

            <p
                id=status_id
                class=move || {
                    note.with(|note| {
                        note.as_ref().map_or_else(
                            || String::from("nr-connection-status"),
                            |note| note.tone.class_name().to_owned(),
                        )
                    })
                }
                role="status"
                aria-live="polite"
            >
                {move || note.with(|note| note.as_ref().map(|note| note.text.clone()))}
            </p>
        </article>
    }
}

/// A card's id and the two accessible action labels derived from it.
#[derive(Clone)]
struct ActionLabels {
    id: String,
    test: String,
    delete: String,
}

/// Test and Delete, before the delete is armed.
#[component]
fn ConnectionActions(state: PanelState, labels: ActionLabels) -> impl IntoView {
    let ActionLabels { id, test, delete } = labels;
    // One memo, read by both buttons: a closure capturing the id is not `Copy`.
    let busy = {
        let id = id.clone();
        Memo::new(move |_previous| state.busy_kind(&id))
    };
    let test_id = id.clone();
    let arm_id = id;

    view! {
        <div class="nr-connection-actions">
            <button
                type="button"
                class="nr-button secondary small"
                aria-label=test
                disabled=move || busy.get().is_some()
                on:click=move |_event| dispatch_test(state, test_id.clone())
            >
                <Show when=move || busy.get() == Some(Busy::Testing)>
                    <span class="nr-spinner" aria-hidden="true"></span>
                </Show>
                "Test connection"
            </button>
            <button
                type="button"
                class="nr-button danger small"
                aria-label=delete
                disabled=move || busy.get().is_some()
                on:click=move |_event| state.confirming.set(Some(arm_id.clone()))
            >
                "Delete"
            </button>
        </div>
    }
}

/// The armed state of the delete action.
///
/// Deleting a connection is irreversible from here, so the first press only ever
/// gets you this far, and the confirming button says what it will do.
#[component]
fn DeleteConfirm(state: PanelState, id: String, name: String) -> impl IntoView {
    view! {
        <div class="nr-connection-confirm" role="group" aria-label="Confirm deletion">
            <p>
                "Delete " <strong>{name}</strong>
                "? The router will lose this credential and stop routing through it. This cannot \
                 be undone from the dashboard."
            </p>
            <div class="nr-connection-actions">
                <button
                    type="button"
                    class="nr-button danger small"
                    on:click=move |_event| dispatch_delete(state, id.clone())
                >
                    "Delete permanently"
                </button>
                <button
                    type="button"
                    class="nr-button secondary small"
                    on:click=move |_event| state.confirming.set(None)
                >
                    "Keep it"
                </button>
            </div>
        </div>
    }
}

/// Add a connection with an API key.
#[component]
fn CreateConnectionCard(state: PanelState) -> impl IntoView {
    let options = api_key_catalog();
    let selected_note = move || {
        state.draft.with(|draft| {
            catalog_option(draft.provider.trim()).map(|option| {
                if option.requires_api_key {
                    format!("{} accepts an API key.", option.name)
                } else {
                    format!("{} needs no credential.", option.name)
                }
            })
        })
    };
    let blocking = move || state.draft.with(ConnectionDraft::validation_error);
    let saving = move || state.save.with(Save::is_saving);

    view! {
        <article class="nr-card nr-anim-rise" id="nr-add-connection">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"add"</span>"Add a connection"</h2>
                    <p>
                        "Stores an API key with the router. OAuth and browser-cookie providers are \
                         not offered here: this build has no dashboard flow to obtain those tokens."
                    </p>
                </div>
            </div>
            <div class="nr-connection-form">
                <div class="nr-connection-form-grid">
                    <div class="nr-provider-field">
                        <label for="nr-new-connection-provider">"Provider"</label>
                        <select
                            id="nr-new-connection-provider"
                            class="nr-preview-input"
                            disabled=saving
                            on:change=move |event| {
                                let provider = event_target_value(&event);
                                state.draft.update(|draft| draft.provider = provider);
                            }
                        >
                            <option value="" selected=move || {
                                state.draft.with(|draft| draft.provider.is_empty())
                            }>
                                "Select a provider"
                            </option>
                            <For
                                each=move || options.clone()
                                key=|option| option.id.clone()
                                children=move |option| {
                                    let selected_id = option.id.clone();
                                    let value = option.id.clone();
                                    let label = format!(
                                        "{} — {}",
                                        option.name,
                                        option.auth_label(),
                                    );
                                    view! {
                                        <option
                                            value=value
                                            selected=move || {
                                                state
                                                    .draft
                                                    .with(|draft| draft.provider == selected_id)
                                            }
                                        >
                                            {label}
                                        </option>
                                    }
                                }
                            />
                        </select>
                        <small>
                            {move || {
                                selected_note()
                                    .unwrap_or_else(|| {
                                        String::from(
                                            "Every API-key provider in this build's registry.",
                                        )
                                    })
                            }}
                        </small>
                    </div>
                    <div class="nr-provider-field">
                        <label for="nr-new-connection-name">"Name"</label>
                        <input
                            id="nr-new-connection-name"
                            class="nr-preview-input"
                            type="text"
                            autocomplete="off"
                            placeholder="Optional. Defaults to the provider id."
                            disabled=saving
                            prop:value=move || state.draft.with(|draft| draft.name.clone())
                            on:input=move |event| {
                                let name = event_target_value(&event);
                                state.draft.update(|draft| draft.name = name);
                            }
                        />
                        <small>"Shown on the card. Helps when one provider has several accounts."</small>
                    </div>
                </div>
                <div class="nr-provider-field">
                    <label for="nr-new-connection-key">"API key"</label>
                    <input
                        id="nr-new-connection-key"
                        class="nr-preview-input"
                        type="password"
                        autocomplete="off"
                        spellcheck="false"
                        disabled=saving
                        prop:value=move || state.draft.with(|draft| draft.api_key.clone())
                        on:input=move |event| {
                            let api_key = event_target_value(&event);
                            state.draft.update(|draft| draft.api_key = api_key);
                        }
                    />
                    <small>
                        "Sent once to the local state service, which stores it and never returns it. \
                         This page cannot read a key back, not even to mask it."
                    </small>
                </div>
                <div class="nr-provider-form-actions">
                    <span class="nr-form-error" role="status" aria-live="polite">
                        {move || {
                            blocking()
                                .map(DraftError::message)
                                .map(str::to_owned)
                                .or_else(|| {
                                    state.save.with(|save| save.status().map(str::to_owned))
                                })
                        }}
                    </span>
                    <button
                        type="button"
                        class="nr-button primary small"
                        disabled=move || blocking().is_some() || saving()
                        on:click=move |_event| dispatch_create(state)
                    >
                        <Show when=saving>
                            <span class="nr-spinner" aria-hidden="true"></span>
                        </Show>
                        "Add connection"
                    </button>
                </div>
            </div>
        </article>
    }
}

/// How many catalog tiles are rendered before the count stands in for the rest.
const CATALOG_PREVIEW: usize = 24;

/// Providers this build knows how to talk to.
///
/// Explicitly not connections. The heading, the copy, and the dashed styling all
/// say so, because conflating the two is the bug this panel was rewritten to fix.
#[component]
fn CatalogCard() -> impl IntoView {
    let all = catalog();
    let total = all.len();
    let shown: Vec<CatalogOption> = all.into_iter().take(CATALOG_PREVIEW).collect();
    let remaining = total.saturating_sub(shown.len());

    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"cat"</span>"Provider catalog"</h2>
                    <p>
                        "Providers this build can route to. These are not your connections — \
                         nothing here is configured until you add a credential for it."
                    </p>
                </div>
                <span class="nr-status-pill is-idle">
                    <span></span>{plural(total, "provider")}
                </span>
            </div>
            <div class="nr-catalog-grid">
                <For
                    each=move || shown.clone()
                    key=|option| option.id.clone()
                    children=|option| view! { <CatalogTile option /> }
                />
            </div>
            <Show when=move || remaining != 0>
                <p class="nr-catalog-more">
                    {format!("{remaining} more in the registry.")}
                </p>
            </Show>
        </article>
    }
}

#[component]
fn CatalogTile(option: CatalogOption) -> impl IntoView {
    let detail = format!(
        "{} · {}",
        option.auth_label(),
        plural(option.model_count, "model")
    );

    view! {
        <span class="nr-catalog-tile" style=format!("--provider-accent: {}", option.accent)>
            <span class="nr-catalog-glyph" aria-hidden="true">{option.initials}</span>
            <span>
                <strong>{option.name}</strong>
                <small>{detail}</small>
            </span>
        </span>
    }
}

/// "1 connection" / "3 connections", so a count never reads as a fragment.
fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// `/dashboard/providers/new`.
///
/// The same create form as the panel, on its own route. It used to be a set of
/// disabled inputs captioned "Preview"; it now performs the write.
#[component]
pub(super) fn ProviderNewPanel() -> impl IntoView {
    let state = PanelState::new();
    // The form needs the list so a successful create lands somewhere real, and so
    // a duplicate name is visible before it is submitted.
    reload(state, true);

    view! {
        <ProviderStyles />
        <div class="nr-panel-stack nr-provider-form">
            <article class="nr-card nr-card-hero nr-anim-rise">
                <div>
                    <p class="nr-eyebrow">"Providers"</p>
                    <h2>"Add a provider connection"</h2>
                    <p>
                        "Stores an API key with the local state service and enables the connection \
                         for routing. Everything you add here appears on the Providers page."
                    </p>
                </div>
                <div class="nr-provider-detail-actions">
                    <a class="nr-button secondary small" href="/dashboard/providers">
                        "Back to Providers"
                    </a>
                </div>
            </article>
            <CreateConnectionCard state />
            <ConnectionsCard state />
        </div>
    }
}

/// `/dashboard/providers/{provider_id}`.
///
/// The route segment is a provider id, so this is a provider's page: what the
/// registry knows about it, and which of your connections use it. It used to look
/// the provider up in the fixture list and render "Provider not found" for
/// anything else, including providers that were genuinely configured.
#[component]
#[allow(
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    reason = "one provider detail view; Leptos components take owned props by convention"
)]
pub(super) fn ProviderDetailPanel(provider_id: String) -> impl IntoView {
    let state = PanelState::new();
    reload(state, true);

    let entry = catalog_option(&provider_id);
    let label = provider_label(&provider_id);
    let accent = provider_accent(&provider_id).to_owned();
    let glyph = provider_initials(&provider_id);
    let models = model_count_for(&provider_id);

    // Outer `Option` is "have we read the list yet"; inner is "does this provider
    // have any connections". Kept distinct so the summary can say "reading…"
    // rather than "0", which would be a claim.
    let mine = {
        let owned_id = provider_id.clone();
        Memo::new(move |_previous| {
            state.list.with(|list| {
                list.ready().map(|ready| {
                    ready
                        .groups()
                        .into_iter()
                        .find(|group| group.provider == owned_id)
                })
            })
        })
    };

    view! {
        <ProviderStyles />
        <div class="nr-panel-stack" style=format!("--provider-accent: {accent}")>
            <article class="nr-card nr-card-hero nr-anim-rise">
                <div class="nr-provider-detail-head">
                    <span class="nr-provider-hero-logo" aria-hidden="true">{glyph}</span>
                    <span>
                        <p class="nr-eyebrow">"Provider"</p>
                        <h2>{label}</h2>
                        <p>
                            {entry
                                .as_ref()
                                .map_or_else(
                                    || {
                                        format!(
                                            "{provider_id} is not in this build's registry. \
                                             Connections using it still work and are listed below.",
                                        )
                                    },
                                    |entry| {
                                        format!(
                                            "{} provider. {}",
                                            entry.category,
                                            entry
                                                .unavailable_note()
                                                .unwrap_or(
                                                    "Can be added with an API key from the \
                                                     Providers page.",
                                                ),
                                        )
                                    },
                                )}
                        </p>
                    </span>
                </div>
                <div class="nr-provider-detail-actions">
                    <a class="nr-button secondary small" href="/dashboard/providers">"Back"</a>
                    <a class="nr-button primary small" href="/dashboard/providers/new">
                        "Add a connection"
                    </a>
                </div>
            </article>
            <div class="nr-provider-summary">
                <ProviderSummary
                    label="Your connections"
                    value=Signal::derive(move || {
                        mine.with(|mine| {
                            mine.as_ref().map_or_else(
                                || String::from("—"),
                                |group| {
                                    group.as_ref().map_or_else(
                                        || String::from("0"),
                                        |group| group.connections.len().to_string(),
                                    )
                                },
                            )
                        })
                    })
                    detail=Signal::derive(move || {
                        mine.with(|mine| {
                            mine.as_ref().map_or_else(
                                || String::from("Reading /api/providers…"),
                                |group| {
                                    group.as_ref().map_or_else(
                                        || String::from("None configured for this provider"),
                                        ProviderGroupLive::summary,
                                    )
                                },
                            )
                        })
                    })
                />
                <ProviderSummary
                    label="Registry models"
                    value=Signal::derive(move || models.to_string())
                    detail=Signal::derive(|| String::from("Known to this build"))
                />
                <ProviderSummary
                    label="Auth"
                    value=Signal::derive({
                        let entry = entry.clone();
                        move || {
                            entry
                                .as_ref()
                                .map_or_else(|| String::from("Unknown"), |entry| {
                                    entry.auth_label().to_owned()
                                })
                        }
                    })
                    detail=Signal::derive({
                        move || {
                            entry.as_ref().map_or_else(
                                || String::from("Provider not in the registry"),
                                |entry| format!("Registry category: {}", entry.category),
                            )
                        }
                    })
                />
            </div>
            <ConnectionsCard state />
            <ModelsPanel />
        </div>
    }
}

/// How many catalog models this build lists for a provider.
fn model_count_for(provider_id: &str) -> usize {
    catalog_option(provider_id).map_or(0, |option| option.model_count)
}

#[component]
fn ProviderSummary(
    label: &'static str,
    value: Signal<String>,
    detail: Signal<String>,
) -> impl IntoView {
    view! {
        <div class="nr-provider-summary-row">
            <span>{label}</span>
            <strong>{move || value.get()}</strong>
            <span>{move || detail.get()}</span>
        </div>
    }
}

/// The model list shipped in `nullrouter-contracts`.
///
/// Kept, and relabelled: it is a static list of models this build advertises on
/// `/v1/models`, not a per-connection availability report. The old copy called it
/// "Representative model availability", which read as live data.
#[component]
pub(super) fn ModelsPanel() -> impl IntoView {
    view! {
        <article class="nr-card nr-models-card nr-anim-rise">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"mdl"</span>"Advertised models"</h2>
                    <p>
                        "The model list this build serves on /v1/models. Not a per-connection \
                         availability report: a model here still needs a working connection to \
                         reach its provider."
                    </p>
                </div>
            </div>
            <div class="nr-model-grid">
                <For
                    each=model_catalog
                    key=|model| model.id.clone()
                    children=|model| view! { <ModelTileView model /> }
                />
            </div>
        </article>
    }
}

#[component]
fn ModelTileView(model: ModelTile) -> impl IntoView {
    view! {
        <div class="nr-model-tile">
            <code>{model.id}</code>
            <span>{model.provider} " · " {model.family}</span>
            <small>{model.context}</small>
        </div>
    }
}

#[component]
fn ProviderStyles() -> impl IntoView {
    view! {
        <style>{PROVIDER_STYLES}</style>
        <style>{PROVIDERS_LIVE_STYLES}</style>
    }
}
