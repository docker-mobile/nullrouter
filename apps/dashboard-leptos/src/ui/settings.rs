//! Settings panel wired to `GET`/`PUT /api/settings`.
//!
//! The panel this replaces rendered three toggles over local `signal()`s: a
//! click moved the knob and nothing was ever sent, so the UI asserted a setting
//! the router did not have. Every value here comes from the server, every change
//! is a `PUT`, and a refused `PUT` puts the old value back — see
//! [`crate::dashboard::resolve`].
//!
//! Rows save independently, so one failing field does not block the others and a
//! failure is reported on the row that caused it.

use crate::{
    api::{self, ApiError, Hydrate, Save},
    dashboard::{
        LOGIN_ALWAYS_REQUIRED, REQUIRE_API_KEY_UNAVAILABLE, Resolution, SETTINGS_GROUPS,
        SETTINGS_PATH, SettingsControl, SettingsField, SettingsGroup, SettingsSnapshot,
        SettingsValue, WriteOutcome, parse_settings, patch_body, resolve,
    },
};
use leptos::prelude::*;

/// The panel's server-owned state.
#[derive(Clone, Copy)]
struct SettingsSignals {
    settings: ReadSignal<Hydrate<SettingsSnapshot>>,
    set_settings: WriteSignal<Hydrate<SettingsSnapshot>>,
}

/// One row's own state.
///
/// `save` is per row so a failing field reports on itself and does not disable
/// the rest of the panel. `reverted` records that a rollback happened, which is
/// the difference between "your change was undone" and "it was stored but the
/// reply was unreadable" — two failures a user has to act on differently.
/// `draft` is unused by toggle rows.
#[derive(Clone, Copy)]
struct RowSignals {
    panel: SettingsSignals,
    save: RwSignal<Save>,
    reverted: RwSignal<bool>,
    draft: RwSignal<Option<String>>,
}

#[component]
pub(super) fn SettingsPanel() -> impl IntoView {
    let (settings, set_settings) = signal(Hydrate::<SettingsSnapshot>::Loading);
    let panel = SettingsSignals {
        settings,
        set_settings,
    };
    load(set_settings);

    view! {
        <div class="nr-panel-stack">
            {SETTINGS_GROUPS
                .into_iter()
                .map(|group| view! { <SettingsCard group panel set_settings /> })
                .collect::<Vec<_>>()}
        </div>
    }
}

/// One card of the panel: access settings, OIDC, or SAML.
///
/// Each card renders the same loading/failure/rows states, because they all read
/// the one `GET /api/settings` body — a card cannot be "loaded" while its
/// neighbour is not.
#[component]
fn SettingsCard(
    group: SettingsGroup,
    panel: SettingsSignals,
    set_settings: WriteSignal<Hydrate<SettingsSnapshot>>,
) -> impl IntoView {
    let settings = panel.settings;

    view! {
        <article class="nr-card nr-settings-card">
            <div class="nr-card-head between">
                <div>
                    <h2>{group.title()}</h2>
                    <p>{group.blurb()}</p>
                </div>
                <SourcePill panel />
            </div>
            {move || {
                if settings.with(Hydrate::is_loading) {
                    view! { <SettingsSkeleton group /> }.into_any()
                } else if let Some(error) = settings.with(Hydrate::failure) {
                    view! { <SettingsFailure error set_settings /> }.into_any()
                } else {
                    view! { <SettingsRows group panel /> }.into_any()
                }
            }}
        </article>
    }
}

/// Fetch (or re-fetch) the whole panel.
///
/// Resets to [`Hydrate::Loading`] first so a retry cannot leave the previous
/// error on screen next to fresh data.
fn load(set_settings: WriteSignal<Hydrate<SettingsSnapshot>>) {
    set_settings.set(Hydrate::Loading);
    api::hydrate(SETTINGS_PATH, set_settings, parse_settings);
}

#[component]
fn SourcePill(panel: SettingsSignals) -> impl IntoView {
    let tone = move || {
        panel.settings.with(|state| match state {
            Hydrate::Loading => "is-idle",
            Hydrate::Ready(_) => "is-connected",
            Hydrate::Failed(_) => "is-degraded",
        })
    };
    let label = move || {
        panel.settings.with(|state| match state {
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
fn SettingsSkeleton(group: SettingsGroup) -> impl IntoView {
    view! {
        <div class="nr-settings-rows" aria-busy="true" aria-label="Loading settings">
            {group
                .fields()
                .into_iter()
                .map(|_| view! { <div class="nr-skeleton nr-skeleton-row"></div> })
                .collect::<Vec<_>>()}
        </div>
    }
}

#[component]
fn SettingsFailure(
    error: ApiError,
    set_settings: WriteSignal<Hydrate<SettingsSnapshot>>,
) -> impl IntoView {
    view! {
        <div class="nr-empty-state nr-settings-failure" role="alert">
            <strong>"Settings could not be loaded"</strong>
            <span>{error.message()}</span>
            <span>
                "No values are shown, because the dashboard will not display settings it has not read from the router."
            </span>
            <button
                type="button"
                class="nr-button secondary small"
                on:click=move |_| load(set_settings)
            >
                "Retry"
            </button>
        </div>
    }
}

#[component]
fn SettingsRows(group: SettingsGroup, panel: SettingsSignals) -> impl IntoView {
    view! {
        <div class="nr-settings-rows">
            {group
                .fields()
                .into_iter()
                .map(|field| view! { <SettingsRow field panel /> })
                .collect::<Vec<_>>()}
            <Show when=move || group == SettingsGroup::Access>
                <UnavailableRow
                    field_key="requireApiKey"
                    label="Require API key"
                    note=REQUIRE_API_KEY_UNAVAILABLE
                    pill="Not readable"
                />
                <UnavailableRow
                    field_key="requireLogin"
                    label="Require dashboard login"
                    note=LOGIN_ALWAYS_REQUIRED
                    pill="Always on"
                />
            </Show>
        </div>
    }
}

#[component]
fn SettingsRow(field: SettingsField, panel: SettingsSignals) -> impl IntoView {
    let row = RowSignals {
        panel,
        save: RwSignal::new(Save::Idle),
        reverted: RwSignal::new(false),
        draft: RwSignal::new(None),
    };

    match field.control() {
        SettingsControl::Toggle => view! { <ToggleRow field row /> }.into_any(),
        SettingsControl::Text => view! { <TextRow field row /> }.into_any(),
        SettingsControl::Secret => view! { <SecretRow field row /> }.into_any(),
    }
}

#[component]
fn ToggleRow(field: SettingsField, row: RowSignals) -> impl IntoView {
    let dom_id = field.dom_id();
    let desc_id = format!("{dom_id}-desc");
    let label_id = format!("{dom_id}-label");
    // The button carries no text node of its own (its children are the label and
    // description), so its accessible name comes from `aria-labelledby`.
    let labelled_by = label_id.clone();
    let described_by = format!("{desc_id} {}", field.status_id());
    let flag = move || field_flag(row.panel.settings, field);
    let saving = move || row.save.with(Save::is_saving);

    view! {
        <div class="nr-setting-block nr-anim-rise" data-field=field.json_key()>
            <button
                type="button"
                id=dom_id
                class="nr-setting-row"
                class:nr-setting-row-active=move || flag().unwrap_or(false)
                aria-labelledby=labelled_by
                aria-describedby=described_by
                // Omitted rather than guessed when the value is unknown: a
                // toggle that announces "off" it never read is the bug this
                // panel replaced. Unknown also disables the button below.
                aria-pressed=move || flag().map(|on| on.to_string())
                aria-busy=move || saving().to_string()
                disabled=move || saving() || flag().is_none()
                on:click=move |_| {
                    if let Some(current) = flag() {
                        begin(row, field, SettingsValue::Flag(!current));
                    }
                }
            >
                <span>
                    <strong id=label_id>{field.label()}</strong>
                    <small id=desc_id>{field.description()}</small>
                </span>
                <span class="nr-setting-control">
                    <Show when=saving>
                        <span class="nr-spinner" aria-hidden="true"></span>
                    </Show>
                    <span class="nr-toggle" class:is-on=move || flag().unwrap_or(false) aria-hidden="true">
                        <span></span>
                    </span>
                </span>
            </button>
            <SaveStatus field row />
        </div>
    }
}

#[component]
fn TextRow(field: SettingsField, row: RowSignals) -> impl IntoView {
    let dom_id = field.dom_id();
    let server_text = move || field_text(row.panel.settings, field);
    let saving = move || row.save.with(Save::is_saving);
    // The draft is what the user has typed and not yet committed. `None` means
    // the input is following the server value, so a confirmed write or a
    // rollback is picked up without clobbering an edit in progress.
    let shown = move || row.draft.get().or_else(server_text).unwrap_or_default();
    let dirty = move || {
        row.draft.with(|draft| {
            draft
                .as_deref()
                .is_some_and(|text| Some(text) != server_text().as_deref())
        })
    };
    let commit = move || {
        if let Some(text) = row.draft.get() {
            begin(row, field, SettingsValue::Text(text));
        }
    };

    view! {
        <div class="nr-setting-block nr-anim-rise" data-field=field.json_key()>
            <div class="nr-setting-row nr-setting-row-text">
                <span>
                    <label for=dom_id.clone()><strong>{field.label()}</strong></label>
                    <small id=format!("{dom_id}-desc")>{field.description()}</small>
                </span>
                <span class="nr-setting-control">
                    <Show when=saving>
                        <span class="nr-spinner" aria-hidden="true"></span>
                    </Show>
                    <input
                        id=dom_id
                        class="nr-setting-input"
                        type="text"
                        spellcheck="false"
                        autocomplete="off"
                        aria-describedby=format!("{}-desc {}", field.dom_id(), field.status_id())
                        aria-busy=move || saving().to_string()
                        prop:value=shown
                        disabled=move || saving() || server_text().is_none()
                        on:input=move |event| row.draft.set(Some(event_target_value(&event)))
                        on:change=move |_| commit()
                        on:keydown=move |event: web_sys::KeyboardEvent| {
                            if event.key() == "Enter" {
                                commit();
                            }
                        }
                    />
                    <button
                        type="button"
                        class="nr-button secondary small"
                        disabled=move || saving() || !dirty()
                        on:click=move |_| commit()
                    >
                        "Save"
                    </button>
                </span>
            </div>
            <SaveStatus field row />
        </div>
    }
}

/// A write-only row for a secret.
///
/// Deliberately different from [`TextRow`] in three ways, all following from the
/// fact that `GET /api/settings` does not return the value:
///
/// - the input starts empty on every load and after every save, because there is
///   nothing to prefill it with. It is never seeded from server state;
/// - what it shows instead is a "Configured"/"Not set" pill driven by the
///   `…Set` boolean, which is the only thing the router reports;
/// - "Remove" sends an explicit `""`. Clearing has to be its own request, since
///   an omitted key means "leave it alone" — that is what stops saving a
///   neighbouring row from destroying a stored credential.
#[component]
fn SecretRow(field: SettingsField, row: RowSignals) -> impl IntoView {
    let dom_id = field.dom_id();
    let saving = move || row.save.with(Save::is_saving);
    let stored = move || {
        row.panel
            .settings
            .with(|state| state.ready().and_then(|snapshot| snapshot.is_set(field)))
    };
    let typed = move || row.draft.get().unwrap_or_default();
    let commit = move || {
        let text = typed();
        if !text.is_empty() {
            begin_secret(row, field, text);
        }
    };

    view! {
        <div class="nr-setting-block nr-anim-rise" data-field=field.json_key()>
            <div class="nr-setting-row nr-setting-row-text">
                <span>
                    <label for=dom_id.clone()><strong>{field.label()}</strong></label>
                    <small id=format!("{dom_id}-desc")>{field.description()}</small>
                </span>
                <span class="nr-setting-control">
                    <Show when=saving>
                        <span class="nr-spinner" aria-hidden="true"></span>
                    </Show>
                    <span
                        class=move || {
                            format!(
                                "nr-status-pill {}",
                                match stored() {
                                    Some(true) => "is-connected",
                                    Some(false) => "is-idle",
                                    None => "is-degraded",
                                },
                            )
                        }
                    >
                        <span></span>
                        {move || match stored() {
                            Some(true) => "Configured",
                            Some(false) => "Not set",
                            None => "Unknown",
                        }}
                    </span>
                    <input
                        id=dom_id
                        class="nr-setting-input"
                        type="password"
                        spellcheck="false"
                        autocomplete="off"
                        placeholder=move || {
                            if stored() == Some(true) { "Replace stored value" } else { "Enter value" }
                        }
                        aria-describedby=format!("{}-desc {}", field.dom_id(), field.status_id())
                        aria-busy=move || saving().to_string()
                        prop:value=typed
                        disabled=move || saving() || stored().is_none()
                        on:input=move |event| row.draft.set(Some(event_target_value(&event)))
                        on:keydown=move |event: web_sys::KeyboardEvent| {
                            if event.key() == "Enter" {
                                commit();
                            }
                        }
                    />
                    <button
                        type="button"
                        class="nr-button secondary small"
                        disabled=move || saving() || typed().is_empty()
                        on:click=move |_| commit()
                    >
                        "Save"
                    </button>
                    <button
                        type="button"
                        class="nr-button secondary small"
                        disabled=move || saving() || stored() != Some(true)
                        on:click=move |_| begin_secret(row, field, String::new())
                    >
                        "Remove"
                    </button>
                </span>
            </div>
            <SaveStatus field row />
        </div>
    }
}

/// The row's save state, announced politely so a screen reader hears the result.
#[component]
fn SaveStatus(field: SettingsField, row: RowSignals) -> impl IntoView {
    // `is-quiet` collapses the row's height while it has nothing to say. It is
    // deliberately not `display:none`: the live region has to be in the
    // accessibility tree *before* its text changes, or the change is not
    // announced.
    let tone = move || {
        row.save.with(|save| match save {
            Save::Idle => "is-quiet",
            Save::Saving => "is-saving",
            Save::Saved => "is-saved nr-tick",
            Save::Failed(_) => "is-failed",
        })
    };

    view! {
        <p
            id=field.status_id()
            class=move || format!("nr-setting-status {}", tone())
            role="status"
            aria-live="polite"
            aria-atomic="true"
        >
            {move || status_text(row)}
        </p>
    }
}

/// A row explaining a setting that has no control.
///
/// Rendered rather than dropped so the panel does not look like it forgot about
/// API-key enforcement or dashboard login. `requireApiKey` has no control because
/// `/api/settings` does not report it; `requireLogin` has none because it is not
/// a setting at all — login is unconditional, and the toggle that used to sit
/// here never wrote anything.
#[component]
fn UnavailableRow(
    field_key: &'static str,
    label: &'static str,
    note: &'static str,
    pill: &'static str,
) -> impl IntoView {
    view! {
        <div class="nr-setting-block" data-field=field_key>
            <div class="nr-setting-row nr-setting-row-unavailable">
                <span>
                    <strong>{label}</strong>
                    <small>{note}</small>
                </span>
                <span class="nr-status-pill is-idle"><span></span>{pill}</span>
            </div>
        </div>
    }
}

/// Status line for one row.
fn status_text(row: RowSignals) -> String {
    row.save.with(|save| match save {
        Save::Failed(error) if row.reverted.get() => {
            format!(
                "Not saved. {} Reverted to the router value.",
                error.message()
            )
        }
        Save::Failed(error) => format!(
            "The router accepted the change but its reply could not be read. {}",
            error.message()
        ),
        Save::Idle | Save::Saving | Save::Saved => save.status().unwrap_or_default().to_owned(),
    })
}

/// The current value of a toggle field, or `None` when nothing was loaded.
fn field_flag(
    settings: ReadSignal<Hydrate<SettingsSnapshot>>,
    field: SettingsField,
) -> Option<bool> {
    settings.with(|state| {
        state
            .ready()
            .and_then(|snapshot| snapshot.value(field).flag())
    })
}

/// The current value of a text field, or `None` when nothing was loaded.
fn field_text(
    settings: ReadSignal<Hydrate<SettingsSnapshot>>,
    field: SettingsField,
) -> Option<String> {
    settings.with(|state| {
        state
            .ready()
            .and_then(|snapshot| snapshot.value(field).text().map(str::to_owned))
    })
}

/// Apply a change optimistically and send it.
///
/// Returns without doing anything when nothing is loaded, when a write for this
/// row is already in flight, or when the value already matches the server — so a
/// double-click cannot race and `Enter` on an unchanged input is not a write.
fn begin(row: RowSignals, field: SettingsField, next: SettingsValue) {
    if row.save.with(Save::is_saving) {
        return;
    }
    let Some(previous) = row
        .panel
        .settings
        .with(|state| state.ready().map(|snapshot| snapshot.value(field)))
    else {
        return;
    };
    if previous == next {
        row.draft.set(None);
        return;
    }

    row.save.set(Save::Saving);
    row.reverted.set(false);
    let body = patch_body(field, &next);
    row.panel.set_settings.update(move |state| {
        if let Hydrate::Ready(snapshot) = state {
            snapshot.set(field, next);
        }
    });
    submit(row, field, previous, body);
}

/// Send a secret, with no optimistic step.
///
/// Separate from [`begin`] because a secret has no readable current value, which
/// removes both halves of the optimistic dance: there is nothing to flip on
/// screen, and nothing to roll back to. The `…Set` indicator only ever moves when
/// the server's reply says it did, so this row cannot claim a credential is
/// stored that the router refused.
///
/// `next` is `""` for a deliberate removal, which is why the "already equal"
/// guard in [`begin`] is not reused: an empty draft is filtered out by the
/// caller, but an explicit clear must still be sent.
fn begin_secret(row: RowSignals, field: SettingsField, next: String) {
    if row.save.with(Save::is_saving) {
        return;
    }
    if row.panel.settings.with(|state| state.ready().is_none()) {
        return;
    }

    row.save.set(Save::Saving);
    row.reverted.set(false);
    let body = patch_body(field, &SettingsValue::Text(next));
    // The previous value handed to `resolve` is the empty text a secret always
    // reads as, so a rejection restores exactly what was on screen: nothing.
    submit(row, field, SettingsValue::Text(String::new()), body);
}

/// Settle a finished write: adopt the outcome and report it on the row.
fn finish(row: RowSignals, field: SettingsField, previous: &SettingsValue, outcome: WriteOutcome) {
    let Some(optimistic) = row.panel.settings.with(|state| state.ready().cloned()) else {
        // The panel was reloaded while the write was in flight; the fresh
        // hydrate is the newer truth, so there is nothing to roll back onto.
        row.save.set(Save::Idle);
        return;
    };
    let Resolution {
        snapshot,
        error,
        committed,
    } = resolve(&optimistic, field, previous, outcome);

    row.panel.set_settings.set(Hydrate::Ready(snapshot));
    row.reverted.set(!committed);
    row.draft.set(None);
    row.save.set(error.map_or(Save::Saved, Save::Failed));
}

#[cfg(target_arch = "wasm32")]
fn submit(row: RowSignals, field: SettingsField, previous: SettingsValue, body: String) {
    wasm_bindgen_futures::spawn_local(async move {
        let outcome = match api::put(SETTINGS_PATH, &body).await {
            // PUT returns the updated SettingsView, so a parseable reply is the
            // server's own state and is preferred over the optimistic guess.
            Ok(reply) => parse_settings(&reply)
                .map_or(WriteOutcome::Unconfirmed, |snapshot| {
                    WriteOutcome::Confirmed(Box::new(snapshot))
                }),
            Err(error) => WriteOutcome::Rejected(error),
        };
        finish(row, field, &previous, outcome);
    });
}

/// Native builds have no browser, so the write is reported as impossible.
///
/// It must still roll back: leaving the optimistic value on screen would be the
/// exact failure this panel exists to prevent.
#[cfg(not(target_arch = "wasm32"))]
#[allow(
    clippy::needless_pass_by_value,
    reason = "signature must match the wasm arm, which moves both into a spawned future"
)]
fn submit(row: RowSignals, field: SettingsField, previous: SettingsValue, _body: String) {
    finish(
        row,
        field,
        &previous,
        WriteOutcome::Rejected(ApiError::Environment),
    );
}
