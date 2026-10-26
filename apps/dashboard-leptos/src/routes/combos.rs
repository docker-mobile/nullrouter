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
use crate::routes::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, CardContent, CardHeader,
    CardTitle, EmptyState, ErrorNote, Field, Icon, IconName, Input, PageBody, PageHeader, Panel,
    Textarea, button_class,
};
use crate::t;

#[component]
pub fn Combos() -> impl IntoView {
    let (list, set_list) = signal(Hydrate::<CombosList>::Loading);
    let reload = move || {
        set_list.set(Hydrate::Loading);
        load("/api/combos", set_list);
    };
    reload();

    view! {
        <PageHeader title=t!("nav.combos") description=t!("combos.description") />
        <PageBody>
            <CreateForm reload=reload />
            <Panel
                state=list
                on_retry=Callback::new(move |()| reload())
                children=move |data: CombosList| { view! { <ComboList rows=data.combos reload=reload /> } }
            />
        </PageBody>
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
    let (name, set_name) = signal(String::new());
    let (models, set_models) = signal(String::new());
    let (kind, set_kind) = signal(String::new());
    let (save, set_save) = signal(Save::Idle);

    let invalid_name = Memo::new(move |_| {
        let value = name.get();
        !value.trim().is_empty() && !is_valid_combo_name(value.trim())
    });

    // Owned up front: `Locale` holds its message table, `t!` re-reads it at each use, and the closure
    // still needs its own copy for the refusal path.
    let encode_failed = StoredValue::new(t!("combos.encode_failed"));

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
            set_save.set(Save::Refused(encode_failed.get_value()));
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
        <Card>
            <CardHeader>
                <div class="space-y-1">
                    <CardTitle>{t!("combos.create")}</CardTitle>
                    <p class="text-xs leading-relaxed text-muted-foreground">
                        {t!("combos.create_hint")}
                    </p>
                </div>
            </CardHeader>
            <CardContent class="space-y-3">
                <form
                    class="grid gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,8rem)]"
                    on:submit=move |event| {
                        event.prevent_default();
                        create();
                    }
                >
                    <Field label=t!("combos.field_name") required=true>
                        <Input
                            value=Signal::derive(move || name.get())
                            on_input=Callback::new(move |value| set_name.set(value))
                            placeholder=t!("combos.name_placeholder")
                            invalid=Signal::derive(move || invalid_name.get())
                        />
                    </Field>
                    <Field label=t!("combos.field_kind")>
                        <Input
                            value=Signal::derive(move || kind.get())
                            on_input=Callback::new(move |value| set_kind.set(value))
                            placeholder=t!("combos.kind_placeholder")
                        />
                    </Field>
                    <Field
                        label=t!("combos.field_models")
                        class="sm:col-span-2"
                        description=t!("combos.models_hint")
                    >
                        <Textarea
                            value=Signal::derive(move || models.get())
                            on_input=Callback::new(move |value| set_models.set(value))
                            placeholder=t!("combos.models_placeholder")
                            rows=2
                            class="font-mono text-xs"
                        />
                    </Field>
                    <div class="flex flex-wrap items-center gap-3 sm:col-span-2">
                        <Button
                            label=t!("combos.create")
                            button_type="submit"
                            disabled=Signal::derive(move || {
                                save.get().is_saving() || name.get().trim().is_empty()
                                    || invalid_name.get()
                            })
                            busy=Signal::derive(move || save.get().is_saving())
                        />
                        <InvalidNameHint invalid=invalid_name />
                    </div>
                </form>
                <SaveMessage save=save />
            </CardContent>
        </Card>
    }
}

/// Says why the create button is disabled, rather than leaving it dead with no explanation.
#[component]
fn InvalidNameHint(invalid: Memo<bool>) -> impl IntoView {
    view! {
        {move || {
            invalid
                .get()
                .then(|| {
                    view! {
                        <p class="flex items-center gap-1.5 text-xs text-warning">
                            <Icon name=IconName::Alert class="size-3.5" />
                            {t!("combos.name_invalid")}
                        </p>
                    }
                })
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
                .map(|message| view! { <ErrorNote message=message /> })
        }}
    }
}

#[component]
fn ComboList(
    rows: Vec<ComboRow>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    if rows.is_empty() {
        return view! {
            <Card>
                <EmptyState
                    title=t!("combos.empty")
                    description=t!("combos.empty_hint")
                    icon=IconName::Combine
                />
            </Card>
        }
        .into_any();
    }
    view! {
        <div class="grid gap-3 xl:grid-cols-2">
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
    let name_invalid = StoredValue::new(t!("combos.name_invalid"));
    let encode_failed = StoredValue::new(t!("combos.encode_failed"));

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

    let title = row.name.clone();
    let row_id = row.id.clone();
    let meta_kind = row.kind.clone().filter(|kind| !kind.trim().is_empty());
    let updated = (!row.updated_at.is_empty()).then(|| {
        format!(
            "{} {}",
            t!("combos.updated"),
            timestamp_label(&row.updated_at)
        )
    });
    let member_count = row.models.len().to_string();

    view! {
        <Card>
            <CardHeader>
                <div class="min-w-0 space-y-1">
                    <CardTitle class="truncate">{title}</CardTitle>
                    <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
                        <code class="font-mono text-[0.6875rem] text-muted-foreground">
                            {row_id}
                        </code>
                        {meta_kind
                            .map(|kind| view! { <Badge variant=BadgeVariant::Outline>{kind}</Badge> })}
                        <Badge variant=BadgeVariant::Secondary>{member_count}</Badge>
                        {updated
                            .map(|stamp| {
                                view! {
                                    <span class="text-[0.6875rem] text-muted-foreground">
                                        {stamp}
                                    </span>
                                }
                            })}
                    </div>
                </div>
                <div class="flex shrink-0 items-center gap-1">
                    <button
                        type="button"
                        class=button_class(ButtonVariant::Ghost, ButtonSize::Sm)
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
                            if editing.get() {
                                t!("combos.cancel")
                            } else {
                                t!("combos.edit")
                            }
                        }}
                    </button>
                    <button
                        type="button"
                        class=button_class(ButtonVariant::Ghost, ButtonSize::Sm)
                        disabled=move || save.get().is_saving()
                        on:click=move |_| remove()
                    >
                        <span class="text-destructive">{t!("combos.delete")}</span>
                    </button>
                </div>
            </CardHeader>
            <CardContent class="space-y-3">
                {move || {
                    if editing.get() {
                        view! {
                            <form
                                class="grid gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,2fr)]"
                                on:submit=move |event| {
                                    event.prevent_default();
                                    submit_edit();
                                }
                            >
                                <Field label=t!("combos.field_name") required=true>
                                    <Input
                                        value=Signal::derive(move || name.get())
                                        on_input=Callback::new(move |value| set_name.set(value))
                                    />
                                </Field>
                                <Field label=t!("combos.field_models")>
                                    <Textarea
                                        value=Signal::derive(move || models.get())
                                        on_input=Callback::new(move |value| set_models.set(value))
                                        rows=2
                                        class="font-mono text-xs"
                                    />
                                </Field>
                                <div class="sm:col-span-2">
                                    <Button
                                        label=t!("combos.save")
                                        button_type="submit"
                                        size=ButtonSize::Sm
                                        disabled=Signal::derive(move || {
                                            save.get().is_saving() || name.get().trim().is_empty()
                                        })
                                        busy=Signal::derive(move || save.get().is_saving())
                                    />
                                </div>
                            </form>
                        }
                            .into_any()
                    } else {
                        view! { <ModelChips models=members.get_value() /> }.into_any()
                    }
                }}
                <SaveMessage save=save />
            </CardContent>
        </Card>
    }
}

/// The combo's members. An empty combo says so rather than rendering a blank row.
#[component]
fn ModelChips(models: Vec<String>) -> impl IntoView {
    if models.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">{t!("combos.no_models")}</p>
        }
        .into_any();
    }
    view! {
        <div class="flex flex-wrap gap-1">
            {models
                .into_iter()
                .map(|model| view! { <Badge variant=BadgeVariant::Code>{model}</Badge> })
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
            parse_models("openai/gpt-5\ngemini/gemini-3.8-flash"),
            vec![
                "openai/gpt-5".to_owned(),
                "gemini/gemini-3.8-flash".to_owned()
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
