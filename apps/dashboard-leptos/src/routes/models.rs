//! The model catalogue, what the router is holding back, and the three settings an operator owns.
//!
//! Four reads, kept in separate sections because they come from separate sources: the catalogue is
//! what the router will route to, availability is per-process cooldown, and disabled ids, custom
//! models, and aliases are stored by the state service. Merging them into one table would make an
//! empty read indistinguishable from a section this build does not populate.
//!
//! The three stored settings used to answer `success: true` and write nothing, so this panel only
//! showed them. They persist now, and the editors below are wired to them.

use leptos::prelude::*;

use crate::api::{
    Hydrate, Method, Save, decode, encode, load, post, request_detailed, submit, submit_reporting,
};
use crate::routes::types::{
    Availability, CustomModels, DisabledModels, ModelAliases, ModelRow, ModelTestBody,
    ModelTestResult, ModelsList, encode_query,
};
use crate::routes::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, CardContent, CardHeader,
    CardTitle, ErrorNote, Field, Icon, IconName, Input, PageBody, PageHeader, Panel, StatusDot,
    StatusTone, Table, TableCell, TableEmpty, TableFrame, TableHead, TableHeaderCell, TableRow,
    button_class,
};
use crate::t;

#[component]
pub fn Models() -> impl IntoView {
    let (catalogue, set_catalogue) = signal(Hydrate::<ModelsList>::Loading);
    let (availability, set_availability) = signal(Hydrate::<Availability>::Loading);
    let (disabled, set_disabled) = signal(Hydrate::<DisabledModels>::Loading);
    let (custom, set_custom) = signal(Hydrate::<CustomModels>::Loading);
    let (aliases, set_aliases) = signal(Hydrate::<ModelAliases>::Loading);

    let reload = move || {
        set_catalogue.set(Hydrate::Loading);
        set_availability.set(Hydrate::Loading);
        set_disabled.set(Hydrate::Loading);
        set_custom.set(Hydrate::Loading);
        set_aliases.set(Hydrate::Loading);
        load("/api/models", set_catalogue);
        load("/api/models/availability", set_availability);
        load("/api/models/disabled", set_disabled);
        load("/api/models/custom", set_custom);
        load("/api/models/alias", set_aliases);
    };
    reload();

    view! {
        <PageHeader title=t!("nav.models") description=t!("models.description") />
        <PageBody>
            <Card>
                <CardHeader>
                    <CardTitle>{t!("models.catalogue")}</CardTitle>
                </CardHeader>
                <CardContent class="px-0 pb-0">
                    <Panel
                        state=catalogue
                        on_retry=Callback::new(move |()| reload())
                        children=move |data: ModelsList| view! { <Catalogue rows=data.models /> }
                    />
                </CardContent>
            </Card>

            <div class="grid gap-4 md:grid-cols-2">
                <Card>
                    <CardHeader>
                        <CardTitle>{t!("models.availability")}</CardTitle>
                    </CardHeader>
                    <CardContent>
                        <Panel
                            state=availability
                            on_retry=Callback::new(move |()| reload())
                            children=|data: Availability| view! { <AvailabilityBody data=data /> }
                        />
                    </CardContent>
                </Card>

                <Card>
                    <CardHeader>
                        <CardTitle>{t!("models.disabled")}</CardTitle>
                    </CardHeader>
                    <CardContent>
                        <Panel
                            state=disabled
                            on_retry=Callback::new(move |()| reload())
                            children=move |data: DisabledModels| {
                                view! { <DisabledBody data=data reload=reload /> }
                            }
                        />
                    </CardContent>
                </Card>
            </div>

            <div class="grid gap-4 md:grid-cols-2">
                <Card>
                    <CardHeader>
                        <CardTitle>{t!("models.custom")}</CardTitle>
                    </CardHeader>
                    <CardContent>
                        <Panel
                            state=custom
                            on_retry=Callback::new(move |()| reload())
                            children=move |data: CustomModels| {
                                view! { <CustomBody data=data reload=reload /> }
                            }
                        />
                    </CardContent>
                </Card>

                <Card>
                    <CardHeader>
                        <CardTitle>{t!("models.aliases")}</CardTitle>
                    </CardHeader>
                    <CardContent>
                        <Panel
                            state=aliases
                            on_retry=Callback::new(move |()| reload())
                            children=move |data: ModelAliases| {
                                view! { <AliasBody data=data reload=reload /> }
                            }
                        />
                    </CardContent>
                </Card>
            </div>
        </PageBody>
    }
}

/// Every model the router can route to, with a reachability test per row.
#[component]
fn Catalogue(rows: Vec<ModelRow>) -> impl IntoView {
    if rows.is_empty() {
        return view! {
            <TableFrame class="rounded-none border-0">
                <Table>
                    <TableEmpty message=t!("models.empty") columns=5 />
                </Table>
            </TableFrame>
        }
        .into_any();
    }
    view! {
        <TableFrame class="rounded-none border-0">
            <Table>
                <TableHead>
                    <tr>
                        <TableHeaderCell>{t!("models.col_model")}</TableHeaderCell>
                        <TableHeaderCell>{t!("models.col_provider")}</TableHeaderCell>
                        <TableHeaderCell>{t!("models.col_alias")}</TableHeaderCell>
                        <TableHeaderCell>{t!("models.col_caps")}</TableHeaderCell>
                        <TableHeaderCell class="text-right">{t!("table.actions")}</TableHeaderCell>
                    </tr>
                </TableHead>
                <tbody>
                    {rows.into_iter().map(|row| view! { <ModelEntry row=row /> }).collect_view()}
                </tbody>
            </Table>
        </TableFrame>
    }
    .into_any()
}

/// One catalogue row.
///
/// The full `provider/model` name is shown rather than the bare model, because that is the string a
/// request has to carry, and two providers here offer the same model under different prefixes.
#[component]
fn ModelEntry(row: ModelRow) -> impl IntoView {
    let (test, set_test) = signal(Save::Idle);
    let (result, set_result) = signal(None::<ModelTestResult>);
    let target = row.full_model.clone();
    // An alias equal to the model name is the catalogue's default, not a configured shorthand.
    let alias = (row.alias != row.model).then_some(row.alias);

    view! {
        <TableRow hoverable=false>
            <TableCell class="font-mono text-xs">{row.full_model}</TableCell>
            <TableCell class="text-muted-foreground">{row.provider}</TableCell>
            <TableCell class="font-mono text-xs text-muted-foreground">
                {alias.unwrap_or_else(|| "—".to_owned())}
            </TableCell>
            <TableCell>
                <Caps
                    vision=row.caps.vision
                    search=row.caps.search
                    reasoning=row.caps.reasoning
                />
            </TableCell>
            <TableCell class="whitespace-nowrap text-right">
                <button
                    type="button"
                    class=button_class(ButtonVariant::Ghost, ButtonSize::Sm)
                    disabled=move || test.get().is_saving()
                    on:click=move |_| {
                        let Ok(body) = encode(&ModelTestBody { model: &target, kind: "llm" }) else {
                            return;
                        };
                        set_result.set(None);
                        submit(
                            set_test,
                            move || async move { post("/api/models/test", &body).await },
                            move |response| {
                                // A 200 here is not a pass: the verdict is in `ok`.
                                set_result.set(decode::<ModelTestResult>(&response).ok());
                            },
                        );
                    }
                >
                    {move || {
                        if test.get().is_saving() {
                            t!("models.testing")
                        } else {
                            t!("models.test")
                        }
                    }}
                </button>
                <TestOutcome test=test result=result />
            </TableCell>
        </TableRow>
    }
}

/// What a finished test says, or why it never ran.
///
/// A transport failure and a provider refusal are reported separately: the first means the router
/// could not be asked, the second means it asked and the provider said no.
#[component]
fn TestOutcome(
    test: ReadSignal<Save>,
    result: ReadSignal<Option<ModelTestResult>>,
) -> impl IntoView {
    view! {
        {move || {
            test.get()
                .message()
                .map(|message| view! { <ErrorNote message=message class="mt-1 text-xs" /> })
        }}
        {move || {
            result
                .get()
                .map(|outcome| {
                    let millis = outcome.latency_ms.to_string();
                    let latency = crate::i18n::use_locale()
                        .fmt("models.test_latency", &[("ms", &millis)]);
                    if outcome.ok {
                        view! {
                            <p class="mt-1 flex flex-wrap items-center justify-end gap-1.5">
                                <StatusDot label=t!("models.test_ok") tone=StatusTone::Success />
                                <span class="font-mono text-xs text-muted-foreground">{latency}</span>
                            </p>
                        }
                            .into_any()
                    } else {
                        // The provider's own wording. "Failed" tells nobody what to change.
                        let detail = if outcome.error.is_empty() {
                            t!("models.test_failed")
                        } else {
                            outcome.error
                        };
                        view! {
                            <ErrorNote
                                message=detail
                                class="mt-1 max-w-64 justify-end text-right text-xs"
                            />
                        }
                            .into_any()
                    }
                })
        }}
    }
}

/// Capability badges, showing only what the model has.
///
/// Absent capabilities are left off rather than drawn greyed out: every model in this build reports
/// all three false, and a row of three dim badges reads as a rendering fault.
///
/// Each label is looked up by a literal key rather than through a table of key strings. `i18n-gen`
/// finds keys by scanning for `t!("` in the source, so a key reaching the lookup through a variable
/// is invisible to it and never lands in the locale files -- which shows up as a raw key on screen.
#[component]
fn Caps(vision: bool, search: bool, reasoning: bool) -> impl IntoView {
    let present: Vec<String> = [
        (vision, t!("models.cap_vision")),
        (search, t!("models.cap_search")),
        (reasoning, t!("models.cap_reasoning")),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, label)| label)
    .collect();

    if present.is_empty() {
        return view! { <span class="text-xs text-muted-foreground">"—"</span> }.into_any();
    }
    view! {
        <div class="flex flex-wrap gap-1">
            {present
                .into_iter()
                .map(|label| view! { <Badge variant=BadgeVariant::Outline>{label}</Badge> })
                .collect_view()}
        </div>
    }
    .into_any()
}

/// Models the router is currently refusing to route to.
#[component]
fn AvailabilityBody(data: Availability) -> impl IntoView {
    let count = data.unavailable_count.to_string();
    if data.models.is_empty() {
        // The count is shown even with no rows: a non-zero count beside an empty list means the
        // router is holding models back without saying which, which is worth seeing rather than
        // rendering as "all clear".
        return if data.unavailable_count == 0 {
            view! {
                <p class="flex items-center gap-2 text-sm text-muted-foreground">
                    <Icon name=IconName::Check class="size-4 text-success" />
                    {t!("models.availability_none")}
                </p>
            }
            .into_any()
        } else {
            view! {
                <p class="flex items-center gap-2 text-sm text-muted-foreground">
                    <Icon name=IconName::Alert class="size-4 text-warning" />
                    {crate::i18n::use_locale()
                        .fmt("models.availability_count", &[("count", &count)])}
                </p>
            }
            .into_any()
        };
    }
    view! {
        <ul class="divide-y divide-border/70">
            {data
                .models
                .into_iter()
                .map(|row| {
                    view! {
                        <li class="flex items-center justify-between gap-3 py-1.5 first:pt-0 last:pb-0">
                            <code class="min-w-0 truncate font-mono text-xs">
                                {format!("{}/{}", row.provider, row.model)}
                            </code>
                            <Badge variant=BadgeVariant::Warning>{t!("models.unavailable")}</Badge>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
    .into_any()
}

/// Switched-off model ids, grouped by the provider they belong to, plus the form that changes them.
///
/// Adding one id is a replace of that provider's whole set: the store has no "append" verb, so the
/// list currently on screen is the one that gets sent back with the new id included. A concurrent
/// edit from another tab would be overwritten; that is the same trade-off as every other list editor
/// in this dashboard, and naming it here is cheaper than inventing a merge the store does not have.
#[component]
fn DisabledBody(
    data: DisabledModels,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (provider, set_provider) = signal(String::new());
    let (id, set_id) = signal(String::new());
    let (save, set_save) = signal(Save::Idle);
    // In a `StoredValue` rather than a plain `String`: the closure below and the rows in the view both
    // need this text, and a `String` moved into the closure leaves nothing for the view to read.
    let encode_failed = StoredValue::new(t!("models.encode_failed"));
    let current = StoredValue::new(data.disabled.clone());

    let disable = move || {
        let alias = provider.get().trim().to_owned();
        let model = id.get().trim().to_owned();
        if alias.is_empty() || model.is_empty() || save.get().is_saving() {
            return;
        }
        let mut ids = current.get_value().get(&alias).cloned().unwrap_or_default();
        if !ids.iter().any(|existing| existing == &model) {
            ids.push(model);
        }
        let body = serde_json::json!({ "providerAlias": alias, "ids": ids });
        let Ok(encoded) = encode(&body) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };
        submit_reporting(
            set_save,
            move || async move {
                request_detailed(Method::Post, "/api/models/disabled", Some(&encoded)).await
            },
            move |_| {
                set_id.set(String::new());
                reload();
            },
        );
    };

    view! {
        <div class="space-y-3">
            {if data.disabled.is_empty() {
                view! {
                    <p class="text-sm text-muted-foreground">{t!("models.disabled_none")}</p>
                }
                    .into_any()
            } else {
                view! {
                    <dl class="space-y-3">
                        {data
                            .disabled
                            .into_iter()
                            .map(|(provider_alias, ids)| {
                                view! {
                                    <DisabledProvider
                                        provider=provider_alias
                                        ids=ids
                                        reload=reload
                                        encode_failed=encode_failed.get_value()
                                    />
                                }
                            })
                            .collect_view()}
                    </dl>
                }
                    .into_any()
            }}

            <form
                class="grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] sm:items-end"
                on:submit=move |event| {
                    event.prevent_default();
                    disable();
                }
            >
                <Field label=t!("models.provider_placeholder")>
                    <Input
                        value=Signal::derive(move || provider.get())
                        on_input=Callback::new(move |value| set_provider.set(value))
                        placeholder=t!("models.provider_placeholder")
                        class="font-mono"
                    />
                </Field>
                <Field label=t!("models.model_placeholder")>
                    <Input
                        value=Signal::derive(move || id.get())
                        on_input=Callback::new(move |value| set_id.set(value))
                        placeholder=t!("models.model_placeholder")
                        class="font-mono"
                    />
                </Field>
                <Button
                    label=t!("models.disable")
                    variant=ButtonVariant::Outline
                    button_type="submit"
                    disabled=Signal::derive(move || {
                        save.get().is_saving() || provider.get().trim().is_empty()
                            || id.get().trim().is_empty()
                    })
                    busy=Signal::derive(move || save.get().is_saving())
                    class="sm:mb-0.5"
                />
            </form>
            <SaveLine save=save />
        </div>
    }
}

/// One provider's held-back ids, with a per-id enable and an enable-all.
#[component]
fn DisabledProvider(
    provider: String,
    ids: Vec<String>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
    encode_failed: String,
) -> impl IntoView {
    let (save, set_save) = signal(Save::Idle);
    let alias = StoredValue::new(provider.clone());
    let remaining = StoredValue::new(ids.clone());

    let enable_all = move || {
        if save.get().is_saving() {
            return;
        }
        let path = format!(
            "/api/models/disabled?providerAlias={}",
            encode_query(&alias.get_value())
        );
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Delete, &path, None).await },
            move |_| reload(),
        );
    };

    view! {
        <div class="space-y-1.5">
            <dt class="flex items-center justify-between gap-3">
                <code class="min-w-0 truncate font-mono text-xs text-foreground">{provider}</code>
                <Button
                    label=t!("models.enable_all")
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    disabled=Signal::derive(move || save.get().is_saving())
                    on_click=Callback::new(move |()| enable_all())
                />
            </dt>
            <dd class="flex flex-wrap gap-1">
                {ids
                    .into_iter()
                    .map(|id| {
                        view! {
                            <DisabledId
                                provider=alias.get_value()
                                remaining=remaining.get_value()
                                id=id
                                reload=reload
                                encode_failed=encode_failed.clone()
                            />
                        }
                    })
                    .collect_view()}
            </dd>
            <SaveLine save=save />
        </div>
    }
}

/// One held-back id, with the button that takes it out of the set.
#[component]
fn DisabledId(
    provider: String,
    remaining: Vec<String>,
    id: String,
    reload: impl Fn() + Copy + 'static + Send + Sync,
    encode_failed: String,
) -> impl IntoView {
    let (save, set_save) = signal(Save::Idle);
    // Built once rather than inside the closure: neither the set nor this id is reactive, and the
    // body is the same on every click. Doing it here also keeps `id` available for the view below.
    let body = serde_json::json!({
        "providerAlias": provider,
        "ids": remaining
            .iter()
            .filter(|existing| *existing != &id)
            .cloned()
            .collect::<Vec<String>>(),
    });
    let encoded = StoredValue::new(encode(&body).ok());
    let refusal = StoredValue::new(encode_failed);
    let enable = move || {
        if save.get().is_saving() {
            return;
        }
        let Some(encoded) = encoded.get_value() else {
            set_save.set(Save::Refused(refusal.get_value()));
            return;
        };
        submit_reporting(
            set_save,
            move || async move {
                request_detailed(Method::Post, "/api/models/disabled", Some(&encoded)).await
            },
            move |_| reload(),
        );
    };
    view! {
        <span class="inline-flex items-center gap-1 rounded-md border border-border bg-muted/40 \
                     py-0.5 pr-1 pl-1.5 text-xs">
            <code class="font-mono">{id}</code>
            <button
                type="button"
                class="rounded px-1 text-[0.6875rem] text-muted-foreground outline-none \
                       transition-colors hover:bg-accent hover:text-foreground \
                       focus-visible:ring-[3px] focus-visible:ring-ring/50 \
                       disabled:pointer-events-none disabled:opacity-50"
                aria-label=t!("models.enable")
                disabled=move || save.get().is_saving()
                on:click=move |_| enable()
            >
                <Icon name=IconName::X class="size-3" />
            </button>
        </span>
    }
}

/// Models added by hand rather than discovered from a provider.
#[component]
fn CustomBody(
    data: CustomModels,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (provider, set_provider) = signal(String::new());
    let (id, set_id) = signal(String::new());
    let (name, set_name) = signal(String::new());
    let (kind, set_kind) = signal(String::new());
    let (save, set_save) = signal(Save::Idle);
    let encode_failed = t!("models.encode_failed");

    let add = move || {
        let alias = provider.get().trim().to_owned();
        let model_id = id.get().trim().to_owned();
        if alias.is_empty() || model_id.is_empty() || save.get().is_saving() {
            return;
        }
        let display = name.get().trim().to_owned();
        let model_type = kind.get().trim().to_owned();
        let body = serde_json::json!({
            "providerAlias": alias,
            "id": model_id,
            "name": if display.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(display) },
            "type": if model_type.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(model_type) },
        });
        let Ok(encoded) = encode(&body) else {
            set_save.set(Save::Refused(encode_failed.clone()));
            return;
        };
        submit_reporting(
            set_save,
            move || async move {
                request_detailed(Method::Post, "/api/models/custom", Some(&encoded)).await
            },
            move |_| {
                set_id.set(String::new());
                set_name.set(String::new());
                set_kind.set(String::new());
                reload();
            },
        );
    };

    view! {
        <div class="space-y-3">
            {if data.models.is_empty() {
                view! {
                    <p class="text-sm text-muted-foreground">{t!("models.custom_none")}</p>
                }
                    .into_any()
            } else {
                view! {
                    <ul class="divide-y divide-border/70">
                        {data
                            .models
                            .into_iter()
                            .map(|entry| {
                                view! { <CustomRow entry=entry reload=reload /> }
                            })
                            .collect_view()}
                    </ul>
                }
                    .into_any()
            }}

            <form
                class="grid gap-2 sm:grid-cols-2"
                on:submit=move |event| {
                    event.prevent_default();
                    add();
                }
            >
                <Field label=t!("models.provider_placeholder")>
                    <Input
                        value=Signal::derive(move || provider.get())
                        on_input=Callback::new(move |value| set_provider.set(value))
                        placeholder=t!("models.provider_placeholder")
                        class="font-mono"
                    />
                </Field>
                <Field label=t!("models.model_placeholder")>
                    <Input
                        value=Signal::derive(move || id.get())
                        on_input=Callback::new(move |value| set_id.set(value))
                        placeholder=t!("models.model_placeholder")
                        class="font-mono"
                    />
                </Field>
                <Field label=t!("models.custom_name")>
                    <Input
                        value=Signal::derive(move || name.get())
                        on_input=Callback::new(move |value| set_name.set(value))
                        placeholder=t!("models.custom_name_placeholder")
                    />
                </Field>
                <Field label=t!("models.custom_type")>
                    <Input
                        value=Signal::derive(move || kind.get())
                        on_input=Callback::new(move |value| set_kind.set(value))
                        placeholder=t!("models.custom_type_placeholder")
                    />
                </Field>
                <div class="sm:col-span-2">
                    <Button
                        label=t!("models.add_custom")
                        variant=ButtonVariant::Outline
                        button_type="submit"
                        disabled=Signal::derive(move || {
                            save.get().is_saving() || provider.get().trim().is_empty()
                                || id.get().trim().is_empty()
                        })
                        busy=Signal::derive(move || save.get().is_saving())
                    />
                </div>
            </form>
            <SaveLine save=save />
        </div>
    }
}

/// One custom model, with the button that removes it.
#[component]
fn CustomRow(
    entry: crate::routes::types::CustomModel,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (save, set_save) = signal(Save::Idle);
    let name = if entry.name.is_empty() {
        entry.id.clone()
    } else {
        entry.name.clone()
    };
    let kind = entry.model_type.clone();
    let path = format!(
        "/api/models/custom?providerAlias={}&id={}",
        encode_query(&entry.provider_alias),
        encode_query(&entry.id)
    );
    let remove = move || {
        if save.get().is_saving() {
            return;
        }
        let path = path.clone();
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Delete, &path, None).await },
            move |_| reload(),
        );
    };
    view! {
        <li class="flex items-center justify-between gap-3 py-1.5 first:pt-0 last:pb-0">
            <span class="min-w-0 truncate text-sm">{name}</span>
            <span class="flex shrink-0 items-center gap-1.5">
                <Badge variant=BadgeVariant::Code>{entry.provider_alias}</Badge>
                {(!kind.is_empty()).then(|| view! { <Badge variant=BadgeVariant::Outline>{kind}</Badge> })}
                <Button
                    label=t!("models.remove")
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    disabled=Signal::derive(move || save.get().is_saving())
                    on_click=Callback::new(move |()| remove())
                />
            </span>
        </li>
        <SaveLine save=save />
    }
}

/// Alternative names for a model, plus the form that sets one.
#[component]
fn AliasBody(
    data: ModelAliases,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (alias, set_alias) = signal(String::new());
    let (model, set_model) = signal(String::new());
    let (save, set_save) = signal(Save::Idle);
    let encode_failed = t!("models.encode_failed");

    let set = move || {
        let name = alias.get().trim().to_owned();
        let target = model.get().trim().to_owned();
        if name.is_empty() || target.is_empty() || save.get().is_saving() {
            return;
        }
        let body = serde_json::json!({ "alias": name, "model": target });
        let Ok(encoded) = encode(&body) else {
            set_save.set(Save::Refused(encode_failed.clone()));
            return;
        };
        submit_reporting(
            set_save,
            move || async move {
                request_detailed(Method::Put, "/api/models/alias", Some(&encoded)).await
            },
            move |_| {
                set_alias.set(String::new());
                set_model.set(String::new());
                reload();
            },
        );
    };

    view! {
        <div class="space-y-3">
            {if data.aliases.is_empty() {
                view! {
                    <p class="text-sm text-muted-foreground">{t!("models.aliases_none")}</p>
                }
                    .into_any()
            } else {
                view! {
                    <ul class="divide-y divide-border/70">
                        {data
                            .aliases
                            .into_iter()
                            .map(|(name, target)| {
                                view! { <AliasRow alias=name model=target reload=reload /> }
                            })
                            .collect_view()}
                    </ul>
                }
                    .into_any()
            }}

            <form
                class="grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] sm:items-end"
                on:submit=move |event| {
                    event.prevent_default();
                    set();
                }
            >
                <Field label=t!("models.alias_placeholder")>
                    <Input
                        value=Signal::derive(move || alias.get())
                        on_input=Callback::new(move |value| set_alias.set(value))
                        placeholder=t!("models.alias_placeholder")
                        class="font-mono"
                    />
                </Field>
                <Field label=t!("models.model_placeholder")>
                    <Input
                        value=Signal::derive(move || model.get())
                        on_input=Callback::new(move |value| set_model.set(value))
                        placeholder=t!("models.model_placeholder")
                        class="font-mono"
                    />
                </Field>
                <Button
                    label=t!("models.set_alias")
                    variant=ButtonVariant::Outline
                    button_type="submit"
                    disabled=Signal::derive(move || {
                        save.get().is_saving() || alias.get().trim().is_empty()
                            || model.get().trim().is_empty()
                    })
                    busy=Signal::derive(move || save.get().is_saving())
                    class="sm:mb-0.5"
                />
            </form>
            <SaveLine save=save />
        </div>
    }
}

/// One alias, with the button that drops it.
#[component]
fn AliasRow(
    alias: String,
    model: String,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (save, set_save) = signal(Save::Idle);
    let path = format!("/api/models/alias?alias={}", encode_query(&alias));
    let remove = move || {
        if save.get().is_saving() {
            return;
        }
        let path = path.clone();
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Delete, &path, None).await },
            move |_| reload(),
        );
    };
    view! {
        <li class="flex items-center justify-between gap-3 py-1.5 first:pt-0 last:pb-0">
            <code class="min-w-0 truncate font-mono text-xs">{alias}</code>
            <span class="flex shrink-0 items-center gap-1.5">
                <Badge variant=BadgeVariant::Code>{model}</Badge>
                <button
                    type="button"
                    class=button_class(ButtonVariant::Ghost, ButtonSize::Sm)
                    disabled=move || save.get().is_saving()
                    on:click=move |_| remove()
                >
                    {t!("models.remove")}
                </button>
            </span>
        </li>
        <SaveLine save=save />
    }
}

/// The refusal or transport error from the last write, if any.
///
/// Quiet on success: the list above already reloaded, and a green "saved" next to every row would
/// be noise. A failure has nowhere else to go.
#[component]
fn SaveLine(save: ReadSignal<Save>) -> impl IntoView {
    view! {
        {move || {
            save.get()
                .message()
                .map(|message| view! { <ErrorNote message=message class="text-xs" /> })
        }}
    }
}
