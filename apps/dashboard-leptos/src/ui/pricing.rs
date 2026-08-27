//! Pricing: the per-token rates this build uses to cost requests.
//!
//! This panel rendered `pricing_settings_state()`: "Total Models 0", "Providers 0",
//! a status pill reading "Preview", a card saying "Current pricing remains empty
//! until the host provides /api/pricing data", and a modal whose five rate fields
//! all showed `0.00` above "No editable provider/model rows are loaded". Every part
//! of that was wrong in the same direction — `GET /api/pricing` was implemented and
//! returning rates, and `0.00` is not a neutral placeholder for a price. It reads as
//! free.
//!
//! Now the table is `GET /api/pricing` and nothing else. A rate the server did not
//! publish renders as "not set", never as a zero, because the two mean different
//! things when someone is working out what a request costs.
//!
//! One gap is stated rather than papered over: editing a rate needs `PATCH`, and the
//! shared client in [`crate::api`] has no `PATCH` verb. The editor validates and
//! builds the exact body the endpoint expects
//! ([`crate::dashboard::pricing_live::RateDraft::patch_body`], unit-tested), and the
//! card says the send is not wired instead of offering a Save that would do nothing.
//! Reset is fully live: `DELETE /api/pricing?provider=..&model=..` drops an override
//! and this panel applies whatever the server returns.
//!
//! Parsing and settlement live in [`crate::dashboard::pricing_live`], where they are
//! unit-tested on the native target. This file holds signals and markup.

use crate::api::{ApiError, Hydrate, Save};
use crate::dashboard::pricing_live::{
    ModelRates, PricingTable, RATE_FIELDS, RateDraft, RateDraftError, RateField, ResetSettlement,
    load_pricing, reset_pricing,
};
use leptos::prelude::*;

/// Panel styles, shared verbatim with the actix host.
const PANELS_LIVE_STYLES: &str =
    include_str!("../../../../services/dashboard-actix/static/assets/dashboard/panels-live.css");

/// How cost is worked out, stated once in the terms the table uses.
///
/// Kept from the old panel, minus its two false lines: it no longer claims that
/// reasoning falls back to the output rate or that cache creation falls back to
/// input. Nothing in this repo implements those fallbacks, and the table now says
/// "not set" for a missing rate instead of implying a substitute.
const HOW_PRICING_WORKS: [(&str, &str); 3] = [
    (
        "Cost calculation",
        "Tokens of each class are multiplied by that class's rate and summed.",
    ),
    (
        "Rate format",
        "Dollars per million tokens. An input rate of 2.50 means $2.50 per 1,000,000 prompt tokens.",
    ),
    (
        "Missing rates",
        "A class with no published rate is shown as not set. Its cost is unknown, not zero, and this \
         page does not substitute another class's rate for it.",
    ),
];

/// Everything this panel reads and writes.
#[derive(Clone, Copy)]
struct PanelState {
    /// The rate table, or why it could not be read.
    table: RwSignal<Hydrate<PricingTable>>,
    /// The row whose rates are open in the editor.
    draft: RwSignal<Option<RateDraft>>,
    /// Which row has an armed reset confirmation.
    confirming: RwSignal<Option<String>>,
    /// Row keys (`provider/model`) with a request in flight.
    busy: RwSignal<Vec<String>>,
    /// Per-row status text, announced politely.
    notes: RwSignal<Vec<(String, String)>>,
    /// Panel-level status.
    save: RwSignal<Save>,
}

impl PanelState {
    fn new() -> Self {
        Self {
            table: RwSignal::new(Hydrate::Loading),
            draft: RwSignal::new(None),
            confirming: RwSignal::new(None),
            busy: RwSignal::new(Vec::new()),
            notes: RwSignal::new(Vec::new()),
            save: RwSignal::new(Save::Idle),
        }
    }

    fn is_busy(self, key: &str) -> bool {
        self.busy.with(|busy| busy.iter().any(|busy_key| busy_key == key))
    }

    fn set_busy(self, key: &str) {
        self.busy.update(|busy| {
            busy.retain(|busy_key| busy_key != key);
            busy.push(key.to_owned());
        });
    }

    fn clear_busy(self, key: &str) {
        self.busy.update(|busy| busy.retain(|busy_key| busy_key != key));
    }

    fn note(self, key: &str) -> Option<String> {
        self.notes.with(|notes| {
            notes
                .iter()
                .find(|(note_key, _note)| note_key == key)
                .map(|(_key, note)| note.clone())
        })
    }

    fn set_note(self, key: &str, text: String) {
        self.notes.update(|notes| {
            notes.retain(|(note_key, _note)| note_key != key);
            notes.push((key.to_owned(), text));
        });
    }

    fn clear_note(self, key: &str) {
        self.notes
            .update(|notes| notes.retain(|(note_key, _note)| note_key != key));
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

/// Load, or reload, the rate table.
fn reload(state: PanelState) {
    state.table.set(Hydrate::Loading);
    spawn(async move {
        let next = load_pricing()
            .await
            .map_or_else(Hydrate::Failed, Hydrate::Ready);
        state.table.set(next);
    });
}

/// Reset one model's rates to this build's defaults.
///
/// Deliberately not optimistic. The defaults live in `default_pricing()` on the
/// server, so this page cannot know what a reset will reveal; showing a guess and
/// correcting it afterwards would put invented prices on screen. The row shows a
/// spinner, then the server's answer replaces the table — or the table stays exactly
/// as it was and the row says why.
fn dispatch_reset(state: PanelState, provider: String, model: String) {
    let key = format!("{provider}/{model}");
    state.confirming.set(None);
    state.clear_note(&key);
    state.set_busy(&key);
    state.save.set(Save::Saving);

    spawn(async move {
        let settlement = reset_pricing(&provider, &model).await;
        state.clear_busy(&key);
        match settlement {
            ResetSettlement::Replaced(table) => {
                state.table.set(Hydrate::Ready(*table));
                state.save.set(Save::Saved);
                state.set_note(&key, format!("{key} reset to this build's default rates."));
            }
            ResetSettlement::Kept { error, message } => {
                state.save.set(Save::Failed(error));
                state.set_note(&key, message);
            }
        }
    });
}

#[component]
pub(super) fn PricingSettingsPanel() -> impl IntoView {
    let state = PanelState::new();
    reload(state);

    view! {
        <style>{PANELS_LIVE_STYLES}</style>
        <div class="nr-panel-stack">
            <article class="nr-card nr-card-hero nr-anim-rise">
                <div>
                    <p class="nr-eyebrow">"Settings"</p>
                    <h2>"Pricing Settings"</h2>
                    <p>
                        "Per-token rates this build uses to cost requests, read from the local \
                         pricing service."
                    </p>
                </div>
                <div class="nr-live-actions">
                    <Show when=move || state.table.with(Hydrate::is_loading)>
                        <span class="nr-spinner" aria-hidden="true"></span>
                    </Show>
                    <button
                        type="button"
                        class="nr-button secondary small"
                        disabled=move || state.table.with(Hydrate::is_loading)
                        on:click=move |_event| reload(state)
                    >
                        "Refresh"
                    </button>
                </div>
            </article>

            <PricingMetrics state />
            <RateCard state />
            <Show when=move || state.draft.with(Option::is_some)>
                <RateEditor state />
            </Show>
            <HowPricingWorks />
        </div>
    }
}

/// Counts, from the table that was actually read.
///
/// Every value here was `0` in the fixture panel. `—` while loading, because a zero
/// would be a count of something nobody has read yet.
#[component]
fn PricingMetrics(state: PanelState) -> impl IntoView {
    let models = move || {
        state
            .table
            .with(|table| table.ready().map(PricingTable::model_count))
    };
    let providers = move || {
        state
            .table
            .with(|table| table.ready().map(PricingTable::provider_count))
    };
    let published = move || {
        state
            .table
            .with(|table| table.ready().map(|table| {
                (table.published_rate_count(), table.model_count() * RATE_FIELDS.len())
            }))
    };

    view! {
        <div class="nr-pricing-metrics">
            <MetricCard
                label="Priced models"
                value=Signal::derive(move || show_count(models()))
                detail=Signal::derive(|| String::from("Models with at least one rate"))
                tone="info"
            />
            <MetricCard
                label="Providers"
                value=Signal::derive(move || show_count(providers()))
                detail=Signal::derive(|| String::from("Providers in the rate catalog"))
                tone="info"
            />
            <MetricCard
                label="Published rates"
                value=Signal::derive(move || {
                    published().map_or_else(
                        || String::from("—"),
                        |(count, total)| format!("{count}/{total}"),
                    )
                })
                detail=Signal::derive(|| {
                    String::from("Rates set, of five per model. The rest are unknown, not zero.")
                })
                tone="warn"
            />
        </div>
    }
}

/// A count, or a dash while it is unknown.
fn show_count(value: Option<usize>) -> String {
    value.map_or_else(|| String::from("—"), |count| count.to_string())
}

#[component]
fn MetricCard(
    label: &'static str,
    value: Signal<String>,
    detail: Signal<String>,
    tone: &'static str,
) -> impl IntoView {
    view! {
        <article class=format!("nr-card nr-metric-card {tone}")>
            <span class="nr-metric-label">{label}</span>
            <strong>{move || value.get()}</strong>
            <small>{move || detail.get()}</small>
        </article>
    }
}

/// The rate table, in whichever of its four states applies.
#[component]
fn RateCard(state: PanelState) -> impl IntoView {
    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"prc"</span>"Current rates"</h2>
                    <p>"Dollars per million tokens, as the local pricing service reports them."</p>
                </div>
            </div>

            <p class="nr-live-status" role="status" aria-live="polite">
                {move || state.save.with(|save| save.status().map(str::to_owned))}
            </p>

            {move || match state.table.get() {
                Hydrate::Loading => view! { <RatesSkeleton /> }.into_any(),
                Hydrate::Failed(error) => view! { <RatesFailure state error /> }.into_any(),
                Hydrate::Ready(table) if table.is_empty() => view! { <RatesEmpty /> }.into_any(),
                Hydrate::Ready(table) => {
                    view! { <RateTable state rows=table.rows().to_vec() /> }.into_any()
                }
            }}
        </article>
    }
}

/// Placeholder rows, labelled so the wait is announced rather than only shown.
#[component]
fn RatesSkeleton() -> impl IntoView {
    view! {
        <div class="nr-live-skeletons" role="status" aria-label="Loading the pricing table">
            {(0..3)
                .map(|_index| view! { <span class="nr-skeleton nr-skeleton-row"></span> })
                .collect_view()}
        </div>
    }
}

/// The request failed. Say so, and offer the only useful action.
#[component]
fn RatesFailure(state: PanelState, error: ApiError) -> impl IntoView {
    view! {
        <div class="nr-panel-notice is-error" role="alert">
            <strong>"Could not read the pricing table"</strong>
            <span>
                {error.message()}
                " No rates are shown. Costs calculated elsewhere in the dashboard may still be \
                 correct; this page simply cannot say what the rates are."
            </span>
            <button
                type="button"
                class="nr-button secondary small"
                on:click=move |_event| reload(state)
            >
                "Try again"
            </button>
        </div>
    }
}

/// The catalog prices nothing.
///
/// Reachable: the defaults are small, and every override can be reset away. It is
/// rendered as itself rather than as a grid of `0.00`, which is what the old modal
/// showed for this exact state.
#[component]
fn RatesEmpty() -> impl IntoView {
    view! {
        <div class="nr-panel-notice">
            <strong>"No pricing data configured"</strong>
            <span>
                "The pricing service returned an empty catalog, so this build has no rate for any \
                 model. Requests still route; their cost is simply not known."
            </span>
        </div>
    }
}

/// The rates, one row per model.
#[component]
fn RateTable(state: PanelState, rows: Vec<ModelRates>) -> impl IntoView {
    view! {
        <div class="nr-rate-scroll">
            <table class="nr-rate-table">
                <caption class="nr-rate-note">
                    "Rates in dollars per million tokens. \"not set\" means the pricing service \
                     publishes no rate for that token class."
                </caption>
                <thead>
                    <tr>
                        <th scope="col">"Model"</th>
                        {RATE_FIELDS
                            .iter()
                            .map(|field| {
                                view! {
                                    <th scope="col" title=field.detail()>{field.label()}</th>
                                }
                            })
                            .collect_view()}
                        <th scope="col">"Actions"</th>
                    </tr>
                </thead>
                <tbody>
                    <For
                        each=move || rows.clone()
                        key=|row| (row.full_model(), row.priced_count())
                        children=move |row| view! { <RateRow state row /> }
                    />
                </tbody>
            </table>
        </div>
    }
}

/// One model's five rates, plus its reset action.
#[component]
fn RateRow(state: PanelState, row: ModelRates) -> impl IntoView {
    let key = row.full_model();
    let row_id = row.row_id();
    let provider = row.provider.clone();
    let model = row.model.clone();
    let gap_note = row.gap_note();
    let reset_aria = row.reset_label();
    let cells: Vec<(bool, String)> = RATE_FIELDS
        .iter()
        .map(|field| {
            let rate = row.rate(*field);
            (rate.is_unset(), rate.display())
        })
        .collect();
    let editor_source = row.clone();

    let busy = {
        let key = key.clone();
        Memo::new(move |_previous| state.is_busy(&key))
    };
    let confirming = {
        let key = key.clone();
        Memo::new(move |_previous| {
            state
                .confirming
                .with(|target| target.as_deref() == Some(key.as_str()))
        })
    };
    let note = {
        let key = key.clone();
        Memo::new(move |_previous| state.note(&key))
    };
    let arm_key = key.clone();
    let reset_provider = provider.clone();
    let reset_model = model.clone();

    view! {
        <tr class="nr-rate-row" class:is-busy=move || busy.get()>
            <th scope="row" id=row_id>
                <code>{key.clone()}</code>
                {gap_note.map(|note| view! { <small class="nr-rate-note">{note}</small> })}
                <p class="nr-live-status" role="status" aria-live="polite">
                    {move || note.get()}
                </p>
            </th>
            {cells
                .into_iter()
                .map(|(unset, text)| {
                    view! {
                        <td>
                            <span class=if unset { "nr-rate-unset" } else { "" }>{text}</span>
                        </td>
                    }
                })
                .collect_view()}
            <td>
                <div class="nr-live-actions">
                    <Show when=move || busy.get()>
                        <span class="nr-spinner" aria-hidden="true"></span>
                    </Show>
                    <Show
                        when=move || confirming.get()
                        fallback={
                            let editor_source = editor_source.clone();
                            let arm_key = arm_key.clone();
                            let reset_aria = reset_aria.clone();
                            move || {
                                let editor_source = editor_source.clone();
                                let arm_key = arm_key.clone();
                                view! {
                                    <button
                                        type="button"
                                        class="nr-button secondary small"
                                        disabled=move || busy.get()
                                        on:click={
                                            let editor_source = editor_source.clone();
                                            move |_event| {
                                                state
                                                    .draft
                                                    .set(Some(RateDraft::for_row(&editor_source)));
                                            }
                                        }
                                    >
                                        "Edit"
                                    </button>
                                    <button
                                        type="button"
                                        class="nr-button danger small"
                                        aria-label=reset_aria.clone()
                                        disabled=move || busy.get()
                                        on:click={
                                            let arm_key = arm_key.clone();
                                            move |_event| {
                                                state.confirming.set(Some(arm_key.clone()));
                                            }
                                        }
                                    >
                                        "Reset"
                                    </button>
                                }
                            }
                        }
                    >
                        <button
                            type="button"
                            class="nr-button danger small"
                            on:click={
                                let reset_provider = reset_provider.clone();
                                let reset_model = reset_model.clone();
                                move |_event| {
                                    dispatch_reset(
                                        state,
                                        reset_provider.clone(),
                                        reset_model.clone(),
                                    );
                                }
                            }
                        >
                            "Discard overrides"
                        </button>
                        <button
                            type="button"
                            class="nr-button secondary small"
                            on:click=move |_event| state.confirming.set(None)
                        >
                            "Keep them"
                        </button>
                    </Show>
                </div>
            </td>
        </tr>
    }
}

/// Edit one model's rates.
///
/// The body this builds is exactly what `PATCH /api/pricing` accepts, and it is
/// validated against the endpoint's own rule (finite, non-negative). What it cannot
/// do is send: [`crate::api::Method`] has no `PATCH`, and `/api/pricing` accepts
/// neither `POST` nor `PUT`. So the card shows the request it would make and says
/// plainly that this build cannot send it — rather than a Save button that silently
/// discards the edit, which is what the old modal did.
#[component]
fn RateEditor(state: PanelState) -> impl IntoView {
    let draft = move || state.draft.get().unwrap_or_default();
    let blocking = move || {
        state
            .draft
            .with(|draft| draft.as_ref().and_then(RateDraft::validation_error))
    };
    let preview = move || {
        state
            .draft
            .with(|draft| draft.as_ref().and_then(|draft| draft.patch_body().ok()))
    };

    view! {
        <article class="nr-card nr-anim-rise" id="nr-rate-editor">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"edt"</span>"Edit rates"</h2>
                    <p>
                        {move || format!("{}/{}", draft().provider, draft().model)}
                        ". Dollars per million tokens. A blank field leaves that rate as the router \
                         has it."
                    </p>
                </div>
                <button
                    type="button"
                    class="nr-button secondary small"
                    on:click=move |_event| state.draft.set(None)
                >
                    "Close"
                </button>
            </div>

            <div class="nr-rate-editor">
                <div class="nr-rate-editor-grid">
                    {RATE_FIELDS
                        .iter()
                        .copied()
                        .map(|field| view! { <RateInput state field /> })
                        .collect_view()}
                </div>

                <p class="nr-form-error" role="status" aria-live="polite">
                    {move || blocking().map(RateDraftError::message)}
                </p>

                <div class="nr-panel-notice" role="note">
                    <strong>"This build cannot send a rate change"</strong>
                    <span>
                        "Saving a rate needs PATCH /api/pricing, and the dashboard's HTTP client has \
                         no PATCH verb. The request below is the exact body that endpoint expects; \
                         Reset, which uses DELETE, does work."
                    </span>
                    <Show when=move || preview().is_some()>
                        <code class="nr-rate-note">{move || preview()}</code>
                    </Show>
                </div>
            </div>
        </article>
    }
}

/// One rate field.
#[component]
fn RateInput(state: PanelState, field: RateField) -> impl IntoView {
    let input_id = format!("nr-rate-{}", field.wire_key());
    let label_for = input_id.clone();

    view! {
        <div class="nr-live-field">
            <label for=label_for>{field.label()}</label>
            <input
                id=input_id
                class="nr-preview-input"
                type="text"
                inputmode="decimal"
                autocomplete="off"
                placeholder="not set"
                prop:value=move || {
                    state
                        .draft
                        .with(|draft| {
                            draft.as_ref().map(|draft| draft.field(field).to_owned())
                        })
                        .unwrap_or_default()
                }
                on:input=move |event| {
                    let value = event_target_value(&event);
                    state.draft.update(|draft| {
                        if let Some(draft) = draft.as_mut() {
                            draft.set_field(field, value.clone());
                        }
                    });
                }
            />
            <small>{field.detail()}</small>
        </div>
    }
}

/// How cost is worked out.
#[component]
fn HowPricingWorks() -> impl IntoView {
    view! {
        <article class="nr-card nr-anim-rise">
            <div class="nr-card-head">
                <div>
                    <h2><span class="nr-card-icon">"inf"</span>"How pricing works"</h2>
                    <p>"What the numbers above mean, and what a blank one means."</p>
                </div>
            </div>
            <dl class="nr-connection-meta">
                {HOW_PRICING_WORKS
                    .iter()
                    .map(|(term, detail)| {
                        view! {
                            <div>
                                <dt class="nr-meta-label">{*term}</dt>
                                <dd class="nr-meta-value">{*detail}</dd>
                            </div>
                        }
                    })
                    .collect_view()}
            </dl>
            <div class="nr-connection-meta">
                {RATE_FIELDS
                    .iter()
                    .map(|field| {
                        view! {
                            <div>
                                <span class="nr-meta-label">{field.label()}</span>
                                <span class="nr-meta-value">{field.detail()}</span>
                            </div>
                        }
                    })
                    .collect_view()}
            </div>
        </article>
    }
}
