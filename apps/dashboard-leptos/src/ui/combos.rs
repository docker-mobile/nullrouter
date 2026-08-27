//! Combos: model groups this router holds.
//!
//! This panel lived in `ui/parity.rs` and rendered `combo_summaries()` — two
//! hardcoded tiles. "coding-fallback" claimed the members `codex/gpt-5`,
//! `anthropic/claude-sonnet`, `openai/gpt-5`; "web-research" claimed
//! `9router-web-search` and `9router-web-fetch`. Both were invented by whoever
//! wrote the fixture, both were captioned "Preview" and "Not persisted by the WASM
//! dashboard", and the Create button was disabled with the note that create and
//! edit "wait for the catalog service contract". `GET /api/combos` and `POST
//! /api/combos` were both implemented and reachable the whole time.
//!
//! Now the tiles come from `GET /api/combos` and nothing else, the model picker
//! comes from `GET /api/models`, and Create performs the write.
//!
//! Editing an existing combo is deliberately not offered here: `PUT
//! /api/combos/{id}` exists, but a member-reordering editor is a bigger surface
//! than this panel needs, and a half-built one would be the same lie in a new
//! shape. Delete and create are complete, and the card says what it does not do.
//!
//! Parsing, ordering, validation, and rollback live in
//! [`crate::dashboard::combos_live`], where they are unit-tested on the native
//! target. This file holds signals and markup.

use crate::api::{ApiError, Hydrate, Save};
use crate::dashboard::combos_live::{
    Combo, ComboDraft, ComboList, DeleteSettlement, DraftError, ModelOption, create_combo,
    delete_combo, load_combos, load_models, plural, settle_delete,
};
use leptos::prelude::*;

/// Panel styles, shared verbatim with the actix host.
///
/// The CSR build links no stylesheet of its own, so the same file the host serves
/// from `/assets/dashboard/panels-live.css` is inlined here.
const PANELS_LIVE_STYLES: &str =
    include_str!("../../../../services/dashboard-actix/static/assets/dashboard/panels-live.css");

/// Everything this panel reads and writes.
#[derive(Clone, Copy)]
struct PanelState {
    /// The configured combos, or why they could not be read.
    list: RwSignal<Hydrate<ComboList>>,
    /// The models the picker can offer, or why it cannot offer any.
    ///
    /// Separate from `list` because a failed model list must not blank the combos:
    /// the tiles are still true, only the create form loses its choices.
    models: RwSignal<Hydrate<Vec<ModelOption>>>,
    draft: RwSignal<ComboDraft>,
    /// Which combo has an armed delete confirmation.
    confirming: RwSignal<Option<String>>,
    /// Combo ids with a request in flight.
    busy: RwSignal<Vec<String>>,
    /// Per-combo status text, announced politely.
    notes: RwSignal<Vec<(String, String)>>,
    /// Panel-level status, for create and refresh.
    save: RwSignal<Save>,
}

impl PanelState {
    fn new() -> Self {
        Self {
            list: RwSignal::new(Hydrate::Loading),
            models: RwSignal::new(Hydrate::Loading),
            draft: RwSignal::new(ComboDraft::default()),
            confirming: RwSignal::new(None),
            busy: RwSignal::new(Vec::new()),
            notes: RwSignal::new(Vec::new()),
            save: RwSignal::new(Save::Idle),
        }
    }

    fn is_busy(self, id: &str) -> bool {
        self.busy.with(|busy| busy.iter().any(|busy_id| busy_id == id))
    }

    fn set_busy(self, id: &str) {
        self.busy.update(|busy| {
            busy.retain(|busy_id| busy_id != id);
            busy.push(id.to_owned());
        });
    }

    fn clear_busy(self, id: &str) {
        self.busy.update(|busy| busy.retain(|busy_id| busy_id != id));
    }

    fn note(self, id: &str) -> Option<String> {
        self.notes.with(|notes| {
            notes
                .iter()
                .find(|(note_id, _note)| note_id == id)
                .map(|(_id, note)| note.clone())
        })
    }

    /// Notes are only ever failures here, so there is no tone to carry.
    fn set_note(self, id: &str, text: String) {
        self.notes.update(|notes| {
            notes.retain(|(note_id, _note)| note_id != id);
            notes.push((id.to_owned(), text));
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
#[cfg(not(target_arch = "wasm32"))]
fn spawn<F: std::future::Future<Output = ()> + 'static>(task: F) {
    drop(task);
}

/// Load, or reload, the combo list.
fn reload(state: PanelState, reset: bool) {
    if reset {
        state.list.set(Hydrate::Loading);
    }
    spawn(async move {
        let next = load_combos()
            .await
            .map_or_else(Hydrate::Failed, Hydrate::Ready);
        state.list.set(next);
    });
}

/// Load the model list the picker offers.
fn reload_models(state: PanelState) {
    state.models.set(Hydrate::Loading);
    spawn(async move {
        let next = load_models()
            .await
            .map_or_else(Hydrate::Failed, Hydrate::Ready);
        state.models.set(next);
    });
}

/// Delete one combo: remove it now, put it back if the router refuses.
fn dispatch_delete(state: PanelState, id: String) {
    // Taking the tile out of the list *is* the optimistic update.
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
    state.set_busy(&id);
    state.save.set(Save::Saving);

    spawn(async move {
        let outcome = delete_combo(&id).await;
        state.clear_busy(&id);
        match settle_delete(pending, outcome) {
            DeleteSettlement::Removed => {
                state.save.set(Save::Saved);
                state.clear_note(&id);
            }
            DeleteSettlement::RolledBack {
                pending,
                error,
                message,
            } => {
                state.list.update(|list| {
                    if let Hydrate::Ready(ready) = list {
                        ready.restore(pending);
                    }
                });
                state.save.set(Save::Failed(error));
                state.set_note(&id, message);
            }
        }
    });
}

/// Create a combo from the draft, then refresh from the server.
fn dispatch_create(state: PanelState) {
    let body = match state.list.with_untracked(|list| {
        let existing = list.ready().cloned().unwrap_or_default();
        state.draft.with_untracked(|draft| draft.body(&existing))
    }) {
        Ok(body) => body,
        // The form already shows this; a click on a disabled control cannot get
        // here, so there is nothing further to report.
        Err(_error) => return,
    };
    state.save.set(Save::Saving);

    spawn(async move {
        match create_combo(body).await {
            Ok(combo) => {
                // Show the combo the router just confirmed, then re-read the list so
                // ordering and any server-applied defaults are the server's.
                state.list.update(|list| {
                    if let Hydrate::Ready(ready) = list {
                        ready.upsert(combo);
                    }
                });
                state.draft.set(ComboDraft::default());
                state.save.set(Save::Saved);
                reload(state, false);
            }
            Err(error) => state.save.set(Save::Failed(error)),
        }
    });
}

#[component]
pub(super) fn CombosPanel() -> impl IntoView {
    let state = PanelState::new();
    // Land on the page already knowing what exists, and with a picker that can
    // offer real models.
    reload(state, true);
    reload_models(state);

    view! {
        <style>{PANELS_LIVE_STYLES}</style>
        <div class="nr-panel-stack">
            <CombosCard state />
            <CreateComboCard state />
        </div>
    }
}

/// The configured combos, in whichever of its four states applies.
#[component]
fn CombosCard(state: PanelState) -> impl IntoView {
    let summary = move || {
        state.list.with(|list| {
            list.ready().map(|ready| {
                format!(
                    "{} across {}.",
                    plural(ready.len(), "combo"),
                    plural(ready.model_count(), "distinct model"),
                )
            })
        })
    };

    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"lay"</span>"Combos"</h2>
                    <p>
                        "Named model groups this router holds, read from the local state service. \
                         A combo lists its members in the order they are tried."
                    </p>
                </div>
                <div class="nr-live-actions">
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
                    <a class="nr-button primary small" href="#nr-add-combo">"Create Combo"</a>
                </div>
            </div>

            <p class="nr-live-status" role="status" aria-live="polite">
                {move || {
                    summary().or_else(|| state.save.with(|save| save.status().map(str::to_owned)))
                }}
            </p>

            {move || match state.list.get() {
                Hydrate::Loading => view! { <CombosSkeleton /> }.into_any(),
                Hydrate::Failed(error) => view! { <CombosFailure state error /> }.into_any(),
                Hydrate::Ready(ready) if ready.is_empty() => view! { <CombosEmpty /> }.into_any(),
                Hydrate::Ready(ready) => {
                    view! { <ComboTiles state combos=ready.combos().to_vec() /> }.into_any()
                }
            }}
        </article>
    }
}

/// Placeholder tiles, labelled so the wait is announced rather than only shown.
#[component]
fn CombosSkeleton() -> impl IntoView {
    view! {
        <div class="nr-combo-grid" role="status" aria-label="Loading your combos">
            {(0..2)
                .map(|_index| {
                    view! {
                        <div class="nr-combo-skeleton" aria-hidden="true">
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
fn CombosFailure(state: PanelState, error: ApiError) -> impl IntoView {
    view! {
        <div class="nr-panel-notice is-error" role="alert">
            <strong>"Could not read your combos"</strong>
            <span>
                {error.message()}
                " No combos are shown, because this page cannot tell whether you have any."
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

/// The router holds nothing. The state the two fixtures used to hide.
#[component]
fn CombosEmpty() -> impl IntoView {
    view! {
        <div class="nr-panel-notice">
            <strong>"No combos configured yet"</strong>
            <span>
                "This router has no model groups stored. Create one below to name a set of models \
                 and the order they should be tried in."
            </span>
            <a class="nr-button primary small" href="#nr-add-combo">"Create a combo"</a>
        </div>
    }
}

/// The tiles, in list order.
#[component]
fn ComboTiles(state: PanelState, combos: Vec<Combo>) -> impl IntoView {
    view! {
        <div class="nr-combo-grid nr-stagger" role="list" aria-label="Configured combos">
            <For
                each=move || combos.clone()
                key=|combo| combo.id.clone()
                children=move |combo| view! { <ComboTile state combo /> }
            />
        </div>
    }
}

/// One combo: its name, its kind, and the models it groups.
#[component]
fn ComboTile(state: PanelState, combo: Combo) -> impl IntoView {
    let id = combo.id.clone();
    let heading_id = combo.heading_id();
    let labelled_by = heading_id.clone();
    let status_id = combo.status_id();
    let name = combo.name.clone();
    let kind = combo.kind_label().to_owned();
    let has_kind = combo.has_kind();
    let member_summary = combo.member_summary();
    let members = combo.members().to_vec();
    let updated = combo.updated_label();
    let delete_aria = combo.delete_label();

    let busy = {
        let id = id.clone();
        Memo::new(move |_previous| state.is_busy(&id))
    };
    let confirming = {
        let id = id.clone();
        Memo::new(move |_previous| {
            state
                .confirming
                .with(|target| target.as_deref() == Some(id.as_str()))
        })
    };
    let note = {
        let id = id.clone();
        Memo::new(move |_previous| state.note(&id))
    };
    let arm_id = id.clone();
    let confirm_id = id.clone();
    let confirm_name = name.clone();

    view! {
        <article
            class="nr-combo-tile"
            class:is-busy=move || busy.get()
            role="listitem"
            aria-labelledby=labelled_by
        >
            <div class="nr-combo-top">
                <h4 id=heading_id>{name.clone()}</h4>
                <span class="nr-status-pill is-connected"><span></span>{member_summary}</span>
            </div>

            <div class="nr-combo-meta">
                // A combo with no kind says so, rather than being shown as "chat".
                <span class=if has_kind {
                    "nr-status-pill is-idle"
                } else {
                    "nr-status-pill is-degraded"
                }>
                    <span></span>{kind}
                </span>
                {updated
                    .map(|value| {
                        view! { <span class="nr-status-pill is-idle"><span></span>"Updated "{value}</span> }
                    })}
            </div>

            <Show
                when={
                    let members = members.clone();
                    move || !members.is_empty()
                }
                fallback=|| {
                    view! {
                        <p class="nr-combo-empty-members">
                            "This combo has no member models, so the router has nothing to try for \
                             it. Delete it or add models from the upstream tool that created it."
                        </p>
                    }
                }
            >
                <ol class="nr-combo-members" aria-label="Member models, in the order they are tried">
                    {members
                        .iter()
                        .map(|model| view! { <li><code>{model.clone()}</code></li> })
                        .collect_view()}
                </ol>
            </Show>

            <Show
                when=move || confirming.get()
                fallback={
                    let arm_id = arm_id.clone();
                    let delete_aria = delete_aria.clone();
                    move || {
                        let arm_id = arm_id.clone();
                        view! {
                            <div class="nr-live-actions">
                                <button
                                    type="button"
                                    class="nr-button danger small"
                                    aria-label=delete_aria.clone()
                                    disabled=move || busy.get()
                                    on:click={
                                        let arm_id = arm_id.clone();
                                        move |_event| state.confirming.set(Some(arm_id.clone()))
                                    }
                                >
                                    "Delete"
                                </button>
                            </div>
                        }
                    }
                }
            >
                <ComboDeleteConfirm state id=confirm_id.clone() name=confirm_name.clone() />
            </Show>

            <p
                id=status_id
                class=move || {
                    note.with(|note| {
                        if note.is_some() {
                            String::from("nr-live-status is-error")
                        } else {
                            String::from("nr-live-status")
                        }
                    })
                }
                role="status"
                aria-live="polite"
            >
                {move || note.get()}
            </p>
        </article>
    }
}

/// The armed state of a tile's delete action.
#[component]
fn ComboDeleteConfirm(state: PanelState, id: String, name: String) -> impl IntoView {
    view! {
        <div class="nr-connection-confirm" role="group" aria-label="Confirm deletion">
            <p>
                "Delete combo "<strong>{name}</strong>
                "? Anything routing by this name will stop resolving. This cannot be undone from \
                 the dashboard."
            </p>
            <div class="nr-live-actions">
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

/// Create a combo: a name, an optional kind, and an ordered model list.
#[component]
#[allow(
    clippy::too_many_lines,
    reason = "one create form: name, kind, picker, and the chosen-member list"
)]
fn CreateComboCard(state: PanelState) -> impl IntoView {
    let blocking = move || {
        state.list.with(|list| {
            let existing = list.ready().cloned().unwrap_or_default();
            state
                .draft
                .with(|draft| draft.validation_error(&existing))
        })
    };
    let saving = move || state.save.with(Save::is_saving);
    let picked = move || state.draft.with(|draft| draft.models.clone());

    view! {
        <article class="nr-card nr-anim-rise" id="nr-add-combo">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"add"</span>"Create a combo"</h2>
                    <p>
                        "Stored by the local state service. Editing an existing combo is not offered \
                         here — delete and recreate it, or use the tool that created it."
                    </p>
                </div>
            </div>

            <div class="nr-live-form">
                <div class="nr-live-form-grid">
                    <div class="nr-live-field">
                        <label for="nr-combo-name">"Name"</label>
                        <input
                            id="nr-combo-name"
                            class="nr-preview-input"
                            type="text"
                            autocomplete="off"
                            spellcheck="false"
                            placeholder="coding-fallback"
                            disabled=saving
                            prop:value=move || state.draft.with(|draft| draft.name.clone())
                            on:input=move |event| {
                                let value = event_target_value(&event);
                                state.draft.update(|draft| draft.name = value);
                            }
                        />
                        <small>"Letters, numbers, -, _ and . only. This is the name clients route by."</small>
                    </div>
                    <div class="nr-live-field">
                        <label for="nr-combo-kind">"Kind"</label>
                        <input
                            id="nr-combo-kind"
                            class="nr-preview-input"
                            type="text"
                            autocomplete="off"
                            placeholder="Optional"
                            disabled=saving
                            prop:value=move || state.draft.with(|draft| draft.kind.clone())
                            on:input=move |event| {
                                let value = event_target_value(&event);
                                state.draft.update(|draft| draft.kind = value);
                            }
                        />
                        <small>
                            "Free text upstream, and optional. Left blank, the combo is stored with \
                             no kind rather than a guessed one."
                        </small>
                    </div>
                </div>

                <ModelPicker state />

                <div class="nr-live-field">
                    <strong>"Members, in the order they are tried"</strong>
                    <Show
                        when=move || !picked().is_empty()
                        fallback=|| {
                            view! {
                                <small>"No models chosen yet. Add at least one above."</small>
                            }
                        }
                    >
                        <div class="nr-combo-picked">
                            {move || {
                                picked()
                                    .into_iter()
                                    .enumerate()
                                    .map(|(index, model)| {
                                        let remove = model.clone();
                                        let label = format!("Remove {model} from this combo");
                                        view! {
                                            <div class="nr-combo-picked-row">
                                                <span>{index + 1}</span>
                                                <code>{model.clone()}</code>
                                                <button
                                                    type="button"
                                                    class="nr-button secondary small"
                                                    aria-label=label
                                                    disabled=saving
                                                    on:click=move |_event| {
                                                        let remove = remove.clone();
                                                        state
                                                            .draft
                                                            .update(|draft| draft.remove_model(&remove));
                                                    }
                                                >
                                                    "Remove"
                                                </button>
                                            </div>
                                        }
                                    })
                                    .collect_view()
                            }}
                        </div>
                    </Show>
                </div>

                <div class="nr-live-actions">
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
                        "Create combo"
                    </button>
                </div>
            </div>
        </article>
    }
}

/// The models this build advertises, as pickable members.
///
/// Its own hydration state: if `GET /api/models` fails, the picker says so and
/// offers a free-text field instead of an empty select that would read as "this
/// build has no models".
#[component]
fn ModelPicker(state: PanelState) -> impl IntoView {
    let saving = move || state.save.with(Save::is_saving);

    view! {
        <div class="nr-live-field">
            <label for="nr-combo-model">"Add a model"</label>
            {move || match state.models.get() {
                Hydrate::Loading => {
                    view! {
                        <span
                            class="nr-skeleton nr-skeleton-row"
                            role="status"
                            aria-label="Loading the model list"
                        ></span>
                    }
                        .into_any()
                }
                Hydrate::Failed(error) => {
                    view! {
                        <div class="nr-panel-notice is-error" role="alert">
                            <strong>"Could not read the model list"</strong>
                            <span>
                                {error.message()}
                                " Type a full model id instead, in the form provider/model."
                            </span>
                            <button
                                type="button"
                                class="nr-button secondary small"
                                on:click=move |_event| reload_models(state)
                            >
                                "Try again"
                            </button>
                        </div>
                        <ModelFreeText state />
                    }
                        .into_any()
                }
                Hydrate::Ready(models) if models.is_empty() => {
                    view! {
                        <div class="nr-panel-notice">
                            <strong>"This build advertises no models"</strong>
                            <span>
                                "/api/models returned an empty list. A combo can still name a model \
                                 id directly."
                            </span>
                        </div>
                        <ModelFreeText state />
                    }
                        .into_any()
                }
                Hydrate::Ready(models) => {
                    view! {
                        <select
                            id="nr-combo-model"
                            class="nr-preview-input"
                            disabled=saving
                            on:change=move |event| {
                                let value = event_target_value(&event);
                                if !value.is_empty() {
                                    state.draft.update(|draft| draft.add_model(&value));
                                }
                            }
                        >
                            <option value="" selected>"Choose a model to add"</option>
                            {models
                                .into_iter()
                                .map(|model| {
                                    let value = model.full_model.clone();
                                    let already = value.clone();
                                    let label = format!("{} — {}", model.label(), model.full_model);
                                    view! {
                                        <option
                                            value=value
                                            disabled=move || {
                                                state.draft.with(|draft| draft.contains(&already))
                                            }
                                        >
                                            {label}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                        <small>
                            "From /api/models. A model already in this combo cannot be added twice."
                        </small>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

/// Fallback member entry when the model list is unavailable.
///
/// A combo stores plain strings, so a typed id is as valid as a picked one. This
/// exists so a failed `/api/models` does not make the form unusable.
#[component]
fn ModelFreeText(state: PanelState) -> impl IntoView {
    let (typed, set_typed) = signal(String::new());

    view! {
        <div class="nr-live-actions">
            <input
                class="nr-preview-input"
                type="text"
                autocomplete="off"
                spellcheck="false"
                placeholder="openai/gpt-5"
                aria-label="Model id to add to this combo"
                prop:value=move || typed.get()
                on:input=move |event| set_typed.set(event_target_value(&event))
            />
            <button
                type="button"
                class="nr-button secondary small"
                disabled=move || typed.with(|value| value.trim().is_empty())
                on:click=move |_event| {
                    let value = typed.get();
                    state.draft.update(|draft| draft.add_model(&value));
                    set_typed.set(String::new());
                }
            >
                "Add model"
            </button>
        </div>
    }
}
