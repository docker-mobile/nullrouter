//! Named groups of models, stored by the state service.
//!
//! The one panel here whose writes all persist: `/api/combos` is routed to `nullrouter-state`,
//! which stores combos and enforces two rules the client cannot -- names are unique, and an unknown
//! id is a 404. Both refusals arrive as a sentence worth showing, so writes go through
//! [`crate::api::submit_reporting`] rather than a bare status.
//!
//! Names are also checked here before the request is sent. That is not a substitute for the
//! server's check: it catches the one mistake that needs no round trip (a character the store will
//! not accept) while the uniqueness rule, which needs the stored set, stays where it belongs.

use leptos::prelude::*;

use crate::api::{Hydrate, Method, Save, decode, encode, load, request_detailed, submit_reporting};
use crate::routes::types::{ComboBody, ComboRow, CombosList, is_valid_combo_name, timestamp_label};
use crate::routes::{PageHeader, Panel};

#[component]
pub fn Combos() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (list, set_list) = signal(Hydrate::<CombosList>::Loading);
    let reload = move || {
        set_list.set(Hydrate::Loading);
        load("/api/combos", set_list);
    };
    reload();

    view! {
        <PageHeader
            title=locale.get("nav.combos").to_owned()
            description=locale.get("combos.description").to_owned()
        />
        <CreateForm reload=reload />
        <Panel
            state=list
            on_retry=Callback::new(move |()| reload())
            children=move |data: CombosList| view! { <ComboList rows=data.combos reload=reload /> }
        />
    }
}

/// Split a comma or newline separated list of model names.
///
/// Empty entries are dropped rather than sent: the store would accept `["", "openai/gpt-5"]`
/// verbatim, and a combo carrying a blank member routes to nothing.
fn parse_models(raw: &str) -> Vec<String> {
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

#[component]
fn CreateForm(reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (name, set_name) = signal(String::new());
    let (models, set_models) = signal(String::new());
    let (kind, set_kind) = signal(String::new());
    let (save, set_save) = signal(Save::Idle);

    let invalid_name = Memo::new(move |_| {
        let value = name.get();
        !value.trim().is_empty() && !is_valid_combo_name(value.trim())
    });

    // Owned up front: `Locale` holds its message table and is not `Copy`, so a closure that moved it
    // would leave nothing for the view below to read.
    let encode_failed = locale.get("combos.encode_failed").to_owned();

    let create = move || {
        let label = name.get().trim().to_owned();
        if label.is_empty() || save.get().is_saving() || !is_valid_combo_name(&label) {
            return;
        }
        let requested_kind = kind.get().trim().to_owned();
        let body = ComboBody {
            name: label,
            kind: (!requested_kind.is_empty()).then_some(requested_kind),
            models: parse_models(&models.get()),
        };
        let Ok(encoded) = encode(&body) else {
            set_save.set(Save::Refused(encode_failed.clone()));
            return;
        };

        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Post, "/api/combos", Some(&encoded)).await },
            move |_| {
                set_name.set(String::new());
                set_models.set(String::new());
                set_kind.set(String::new());
                reload();
            },
        );
    };

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4 mb-4">
            <p class="text-sm text-muted-foreground">{locale.get("combos.create_hint").to_owned()}</p>
            <div class="grid gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,2fr)_minmax(0,8rem)]">
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("combos.field_name").to_owned()}
                    </span>
                    <input
                        type="text"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || name.get()
                        on:input=move |ev| set_name.set(event_target_value(&ev))
                        placeholder=locale.get("combos.name_placeholder").to_owned()
                    />
                </label>
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("combos.field_models").to_owned()}
                    </span>
                    <input
                        type="text"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono text-xs"
                        prop:value=move || models.get()
                        on:input=move |ev| set_models.set(event_target_value(&ev))
                        placeholder=locale.get("combos.models_placeholder").to_owned()
                    />
                </label>
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("combos.field_kind").to_owned()}
                    </span>
                    <input
                        type="text"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || kind.get()
                        on:input=move |ev| set_kind.set(event_target_value(&ev))
                        placeholder=locale.get("combos.kind_placeholder").to_owned()
                    />
                </label>
            </div>
            <div class="flex items-center gap-3">
                <button
                    type="button"
                    class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                    disabled=move || {
                        save.get().is_saving() || name.get().trim().is_empty()
                            || invalid_name.get()
                    }
                    on:click=move |_| create()
                >
                    {locale.get("combos.create").to_owned()}
                </button>
                <InvalidNameHint invalid=invalid_name />
            </div>
            <SaveMessage save=save />
        </section>
    }
}

/// Says why the create button is disabled, rather than leaving it dead with no explanation.
#[component]
fn InvalidNameHint(invalid: Memo<bool>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let hint = locale.get("combos.name_invalid").to_owned();
    view! {
        {move || {
            let hint = hint.clone();
            invalid.get().then(|| view! { <p class="text-sm text-muted-foreground">{hint}</p> })
        }}
    }
}

/// Whatever went wrong with the last write, in the server's words when it had any.
#[component]
fn SaveMessage(save: ReadSignal<Save>) -> impl IntoView {
    view! {
        {move || {
            save.get()
                .message()
                .map(|message| {
                    view! { <p class="text-sm text-destructive" role="alert">{message}</p> }
                })
        }}
    }
}

#[component]
fn ComboList(
    rows: Vec<ComboRow>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">{locale.get("combos.empty").to_owned()}</p>
        }
        .into_any();
    }
    view! {
        <div class="space-y-3">
            {rows
                .into_iter()
                .map(|row| view! { <ComboCard row=row reload=reload /> })
                .collect_view()}
        </div>
    }
    .into_any()
}

/// One stored combo, with its editor.
///
/// Editing swaps the card's body rather than opening a dialog, so the list keeps its place and a
/// failed save leaves the entered values on screen to correct.
#[component]
fn ComboCard(row: ComboRow, reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (editing, set_editing) = signal(false);
    let (save, set_save) = signal(Save::Idle);
    let (name, set_name) = signal(row.name.clone());
    let (models, set_models) = signal(row.models.join(", "));

    let id = StoredValue::new(row.id.clone());
    // Sent back unchanged on save. The store treats the key being present as "set this", so
    // omitting it preserves the stored value and including it as null would clear it.
    let kind = StoredValue::new(row.kind.clone());
    let stored_name = StoredValue::new(row.name.clone());
    let stored_models = StoredValue::new(row.models.join(", "));
    // The chips are rebuilt whenever the editor closes, so the list has to outlive the view body.
    let members = StoredValue::new(row.models.clone());
    let name_invalid = StoredValue::new(locale.get("combos.name_invalid").to_owned());
    let encode_failed = StoredValue::new(locale.get("combos.encode_failed").to_owned());

    let submit_edit = move || {
        let label = name.get().trim().to_owned();
        if save.get().is_saving() {
            return;
        }
        if !is_valid_combo_name(&label) {
            set_save.set(Save::Refused(name_invalid.get_value()));
            return;
        }
        let body = ComboBody {
            name: label,
            kind: kind.get_value(),
            models: parse_models(&models.get()),
        };
        let Ok(encoded) = encode(&body) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };
        let path = format!("/api/combos/{}", id.get_value());

        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Put, &path, Some(&encoded)).await },
            move |body| {
                // Decoded to confirm the store answered with a combo, rather than something this
                // panel would then re-render from as though the edit had landed.
                let _ = decode::<ComboRow>(&body);
                set_editing.set(false);
                reload();
            },
        );
    };

    let remove = move || {
        if save.get().is_saving() {
            return;
        }
        let path = format!("/api/combos/{}", id.get_value());
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Delete, &path, None).await },
            move |_| reload(),
        );
    };

    // Every label owned before the view, because the toggle below needs one inside a `move` closure
    // and `Locale` is not `Copy`.
    let label_edit = locale.get("combos.edit").to_owned();
    let label_cancel = locale.get("combos.cancel").to_owned();
    let label_delete = locale.get("combos.delete").to_owned();
    let label_save = locale.get("combos.save").to_owned();
    let label_name = locale.get("combos.field_name").to_owned();
    let label_models = locale.get("combos.field_models").to_owned();
    let updated = (!row.updated_at.is_empty()).then(|| {
        format!(
            "{} {}",
            locale.get("combos.updated"),
            timestamp_label(&row.updated_at)
        )
    });

    view! {
        <section class="rounded-lg border border-border bg-card p-4 space-y-3">
            <div class="flex items-start justify-between gap-3">
                <div class="min-w-0 space-y-1">
                    <p class="font-medium truncate">{row.name}</p>
                    <p class="text-xs text-muted-foreground">
                        <code>{row.id}</code>
                        {row.kind.map(|kind| view! { <span>{format!(" · {kind}")}</span> })}
                        {updated.map(|stamp| view! { <span>{format!(" · {stamp}")}</span> })}
                    </p>
                </div>
                <div class="flex items-center gap-3 shrink-0">
                    <button
                        type="button"
                        class="text-sm underline-offset-4 hover:underline disabled:opacity-50"
                        disabled=move || save.get().is_saving()
                        on:click=move |_| {
                            // Reopening discards an abandoned edit rather than resuming it, and
                            // clears a stale refusal from the previous attempt.
                            if !editing.get() {
                                set_name.set(stored_name.get_value());
                                set_models.set(stored_models.get_value());
                                set_save.set(Save::Idle);
                            }
                            set_editing.update(|open| *open = !*open);
                        }
                    >
                        {move || {
                            if editing.get() { label_cancel.clone() } else { label_edit.clone() }
                        }}
                    </button>
                    <button
                        type="button"
                        class="text-sm text-destructive underline-offset-4 hover:underline disabled:opacity-50"
                        disabled=move || save.get().is_saving()
                        on:click=move |_| remove()
                    >
                        {label_delete}
                    </button>
                </div>
            </div>

            {move || {
                if editing.get() {
                    view! {
                        <div class="grid gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,2fr)] pt-1">
                            <label class="space-y-1 text-sm">
                                <span class="text-muted-foreground">{label_name.clone()}</span>
                                <input
                                    type="text"
                                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                                    prop:value=move || name.get()
                                    on:input=move |ev| set_name.set(event_target_value(&ev))
                                />
                            </label>
                            <label class="space-y-1 text-sm">
                                <span class="text-muted-foreground">{label_models.clone()}</span>
                                <input
                                    type="text"
                                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-xs font-mono"
                                    prop:value=move || models.get()
                                    on:input=move |ev| set_models.set(event_target_value(&ev))
                                />
                            </label>
                            <button
                                type="button"
                                class="justify-self-start rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                                disabled=move || {
                                    save.get().is_saving() || name.get().trim().is_empty()
                                }
                                on:click=move |_| submit_edit()
                            >
                                {label_save.clone()}
                            </button>
                        </div>
                    }
                        .into_any()
                } else {
                    view! { <ModelChips models=members.get_value() /> }.into_any()
                }
            }}

            <SaveMessage save=save />
        </section>
    }
}

/// The combo's members. An empty combo says so rather than rendering a blank row.
#[component]
fn ModelChips(models: Vec<String>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if models.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">
                {locale.get("combos.no_models").to_owned()}
            </p>
        }
        .into_any();
    }
    view! {
        <div class="flex flex-wrap gap-1">
            {models
                .into_iter()
                .map(|model| {
                    view! {
                        <code class="rounded-full border border-border px-2 py-0.5 text-xs text-muted-foreground">
                            {model}
                        </code>
                    }
                })
                .collect_view()}
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::parse_models;

    #[test]
    fn models_split_on_commas_and_newlines() {
        assert_eq!(
            parse_models("openai/gpt-5, anthropic/claude-sonnet-4.5"),
            vec![
                "openai/gpt-5".to_owned(),
                "anthropic/claude-sonnet-4.5".to_owned()
            ]
        );
        assert_eq!(
            parse_models("openai/gpt-5\ngemini/gemini-2.5-pro"),
            vec![
                "openai/gpt-5".to_owned(),
                "gemini/gemini-2.5-pro".to_owned()
            ]
        );
    }

    #[test]
    fn blank_entries_are_dropped_rather_than_stored() {
        // The store would accept a blank member verbatim, and a combo carrying one routes to
        // nothing.
        assert_eq!(
            parse_models("openai/gpt-5, , ,\n\n"),
            vec!["openai/gpt-5".to_owned()]
        );
        assert!(parse_models("   ").is_empty());
        assert!(parse_models("").is_empty());
    }
}
