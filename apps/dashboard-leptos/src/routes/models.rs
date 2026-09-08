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
use crate::routes::{PageHeader, Panel};

#[component]
pub fn Models() -> impl IntoView {
    let locale = crate::i18n::use_locale();
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
        <PageHeader
            title=locale.get("nav.models").to_owned()
            description=locale.get("models.description").to_owned()
        />
        <Panel
            state=catalogue
            on_retry=Callback::new(move |()| reload())
            children=move |data: ModelsList| view! { <Catalogue rows=data.models /> }
        />
        <div class="grid gap-4 md:grid-cols-2 mt-6">
            <Section title=locale.get("models.availability").to_owned()>
                <Panel
                    state=availability
                    on_retry=Callback::new(move |()| reload())
                    children=|data: Availability| view! { <AvailabilityBody data=data /> }
                />
            </Section>
            <Section title=locale.get("models.disabled").to_owned()>
                <Panel
                    state=disabled
                    on_retry=Callback::new(move |()| reload())
                    children=move |data: DisabledModels| {
                        view! { <DisabledBody data=data reload=reload /> }
                    }
                />
            </Section>
        </div>
        <div class="grid gap-4 md:grid-cols-2 mt-4">
            <Section title=locale.get("models.custom").to_owned()>
                <Panel
                    state=custom
                    on_retry=Callback::new(move |()| reload())
                    children=move |data: CustomModels| {
                        view! { <CustomBody data=data reload=reload /> }
                    }
                />
            </Section>
            <Section title=locale.get("models.aliases").to_owned()>
                <Panel
                    state=aliases
                    on_retry=Callback::new(move |()| reload())
                    children=move |data: ModelAliases| {
                        view! { <AliasBody data=data reload=reload /> }
                    }
                />
            </Section>
        </div>
    }
}

#[component]
fn Section(title: String, children: Children) -> impl IntoView {
    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-3">
            <h2 class="text-sm font-medium text-muted-foreground">{title}</h2>
            {children()}
        </section>
    }
}

/// Every model the router can route to, with a reachability test per row.
#[component]
fn Catalogue(rows: Vec<ModelRow>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">{locale.get("models.empty").to_owned()}</p>
        }
        .into_any();
    }
    view! {
        <div class="rounded-lg border border-border overflow-x-auto">
            <table class="w-full text-sm">
                <thead class="bg-muted/50 text-muted-foreground">
                    <tr>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("models.col_model").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("models.col_provider").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("models.col_alias").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("models.col_caps").to_owned()}
                        </th>
                        <th class="px-3 py-2"></th>
                    </tr>
                </thead>
                <tbody>
                    {rows.into_iter().map(|row| view! { <ModelEntry row=row /> }).collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

/// One catalogue row.
///
/// The full `provider/model` name is shown rather than the bare model, because that is the string a
/// request has to carry, and two providers here offer the same model under different prefixes.
#[component]
fn ModelEntry(row: ModelRow) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (test, set_test) = signal(Save::Idle);
    let (result, set_result) = signal(None::<ModelTestResult>);
    let target = row.full_model.clone();
    // An alias equal to the model name is the catalogue's default, not a configured shorthand.
    let alias = (row.alias != row.model).then_some(row.alias);
    // Owned before the button's closure takes them: `Locale` holds its table and is not `Copy`.
    let label_test = locale.get("models.test").to_owned();
    let label_testing = locale.get("models.testing").to_owned();

    view! {
        <tr class="border-t border-border align-top">
            <td class="px-3 py-2 font-mono text-xs">{row.full_model}</td>
            <td class="px-3 py-2 text-muted-foreground">{row.provider}</td>
            <td class="px-3 py-2 font-mono text-xs text-muted-foreground">
                {alias.unwrap_or_else(|| "—".to_owned())}
            </td>
            <td class="px-3 py-2">
                <Caps
                    vision=row.caps.vision
                    search=row.caps.search
                    reasoning=row.caps.reasoning
                />
            </td>
            <td class="px-3 py-2 text-right whitespace-nowrap">
                <button
                    type="button"
                    class="text-sm underline-offset-4 hover:underline disabled:opacity-50"
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
                            label_testing.clone()
                        } else {
                            label_test.clone()
                        }
                    }}
                </button>
                <TestOutcome test=test result=result />
            </td>
        </tr>
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
    let locale = crate::i18n::use_locale();
    view! {
        {move || {
            test.get()
                .message()
                .map(|message| {
                    view! { <p class="mt-1 text-xs text-destructive text-right">{message}</p> }
                })
        }}
        {move || {
            result
                .get()
                .map(|outcome| {
                    let millis = outcome.latency_ms.to_string();
                    let latency = locale.fmt("models.test_latency", &[("ms", &millis)]);
                    if outcome.ok {
                        view! {
                            <p class="mt-1 text-xs text-right text-muted-foreground">
                                <span class="text-success">
                                    {locale.get("models.test_ok").to_owned()}
                                </span>
                                {format!(" · {latency}")}
                            </p>
                        }
                            .into_any()
                    } else {
                        // The provider's own wording. "Failed" tells nobody what to change.
                        let detail = if outcome.error.is_empty() {
                            locale.get("models.test_failed").to_owned()
                        } else {
                            outcome.error
                        };
                        view! {
                            <p class="mt-1 text-xs text-destructive text-right max-w-64 ml-auto whitespace-normal break-words">
                                {detail}
                            </p>
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
/// finds keys by scanning for `get("` in the source, so a key reaching the lookup through a variable
/// is invisible to it and never lands in the locale files -- which shows up as a raw key on screen.
#[component]
fn Caps(vision: bool, search: bool, reasoning: bool) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let present: Vec<String> = [
        (vision, locale.get("models.cap_vision")),
        (search, locale.get("models.cap_search")),
        (reasoning, locale.get("models.cap_reasoning")),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, label)| label.to_owned())
    .collect();

    if present.is_empty() {
        return view! { <span class="text-xs text-muted-foreground">"—"</span> }.into_any();
    }
    view! {
        <div class="flex flex-wrap gap-1">
            {present
                .into_iter()
                .map(|label| {
                    view! {
                        <span class="rounded-full border border-border px-2 py-0.5 text-xs text-muted-foreground">
                            {label}
                        </span>
                    }
                })
                .collect_view()}
        </div>
    }
    .into_any()
}

/// Models the router is currently refusing to route to.
#[component]
fn AvailabilityBody(data: Availability) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if data.models.is_empty() {
        // The count is shown even with no rows: a non-zero count beside an empty list means the
        // router is holding models back without saying which, which is worth seeing rather than
        // rendering as "all clear".
        let label = if data.unavailable_count == 0 {
            locale.get("models.availability_none").to_owned()
        } else {
            let count = data.unavailable_count.to_string();
            locale.fmt("models.availability_count", &[("count", &count)])
        };
        return view! { <p class="text-sm text-muted-foreground">{label}</p> }.into_any();
    }
    view! {
        <ul class="space-y-1 text-sm">
            {data
                .models
                .into_iter()
                .map(|row| {
                    view! {
                        <li class="flex items-center justify-between gap-3">
                            <span class="font-mono text-xs truncate">
                                {format!("{}/{}", row.provider, row.model)}
                            </span>
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
    let locale = crate::i18n::use_locale();
    let (provider, set_provider) = signal(String::new());
    let (id, set_id) = signal(String::new());
    let (save, set_save) = signal(Save::Idle);
    // In a `StoredValue` rather than a plain `String`: the closure below and the rows in the view both
    // need this text, and a `String` moved into the closure leaves nothing for the view to read.
    let encode_failed = StoredValue::new(locale.get("models.encode_failed").to_owned());
    let label_disable = locale.get("models.disable").to_owned();
    let label_enable = locale.get("models.enable").to_owned();
    let label_enable_all = locale.get("models.enable_all").to_owned();
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
        <div class="space-y-4">
            {if data.disabled.is_empty() {
                view! {
                    <p class="text-sm text-muted-foreground">
                        {locale.get("models.disabled_none").to_owned()}
                    </p>
                }
                    .into_any()
            } else {
                view! {
                    <dl class="space-y-2 text-sm">
                        {data
                            .disabled
                            .into_iter()
                            .map(|(provider_alias, ids)| {
                                view! {
                                    <DisabledProvider
                                        provider=provider_alias
                                        ids=ids
                                        reload=reload
                                        enable_label=label_enable.clone()
                                        enable_all_label=label_enable_all.clone()
                                        encode_failed=encode_failed.get_value()
                                    />
                                }
                            })
                            .collect_view()}
                    </dl>
                }
                    .into_any()
            }}
            <div class="grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]">
                <input
                    type="text"
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono text-xs"
                    prop:value=move || provider.get()
                    on:input=move |ev| set_provider.set(event_target_value(&ev))
                    placeholder=locale.get("models.provider_placeholder").to_owned()
                />
                <input
                    type="text"
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono text-xs"
                    prop:value=move || id.get()
                    on:input=move |ev| set_id.set(event_target_value(&ev))
                    placeholder=locale.get("models.model_placeholder").to_owned()
                />
                <button
                    type="button"
                    class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                    disabled=move || {
                        save.get().is_saving() || provider.get().trim().is_empty()
                            || id.get().trim().is_empty()
                    }
                    on:click=move |_| disable()
                >
                    {label_disable}
                </button>
            </div>
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
    enable_label: String,
    enable_all_label: String,
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
        <div class="space-y-1">
            <dt class="flex items-center justify-between gap-3 text-muted-foreground">
                <span>{provider}</span>
                <button
                    type="button"
                    class="text-xs underline-offset-4 hover:underline disabled:opacity-50"
                    disabled=move || save.get().is_saving()
                    on:click=move |_| enable_all()
                >
                    {enable_all_label}
                </button>
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
                                enable_label=enable_label.clone()
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
    enable_label: String,
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
        <span class="inline-flex items-center gap-1 rounded border border-border px-1.5 py-0.5 text-xs">
            <code>{id}</code>
            <button
                type="button"
                class="underline-offset-4 hover:underline disabled:opacity-50"
                disabled=move || save.get().is_saving()
                on:click=move |_| enable()
            >
                {enable_label}
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
    let locale = crate::i18n::use_locale();
    let (provider, set_provider) = signal(String::new());
    let (id, set_id) = signal(String::new());
    let (name, set_name) = signal(String::new());
    let (kind, set_kind) = signal(String::new());
    let (save, set_save) = signal(Save::Idle);
    let encode_failed = locale.get("models.encode_failed").to_owned();
    let label_add = locale.get("models.add_custom").to_owned();
    let label_remove = locale.get("models.remove").to_owned();

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
        <div class="space-y-4">
            {if data.models.is_empty() {
                view! {
                    <p class="text-sm text-muted-foreground">
                        {locale.get("models.custom_none").to_owned()}
                    </p>
                }
                    .into_any()
            } else {
                view! {
                    <ul class="space-y-1 text-sm">
                        {data
                            .models
                            .into_iter()
                            .map(|entry| {
                                view! {
                                    <CustomRow
                                        entry=entry
                                        reload=reload
                                        remove_label=label_remove.clone()
                                    />
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                    .into_any()
            }}
            <div class="grid gap-2 sm:grid-cols-2">
                <input
                    type="text"
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono text-xs"
                    prop:value=move || provider.get()
                    on:input=move |ev| set_provider.set(event_target_value(&ev))
                    placeholder=locale.get("models.provider_placeholder").to_owned()
                />
                <input
                    type="text"
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono text-xs"
                    prop:value=move || id.get()
                    on:input=move |ev| set_id.set(event_target_value(&ev))
                    placeholder=locale.get("models.model_placeholder").to_owned()
                />
                <input
                    type="text"
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                    prop:value=move || name.get()
                    on:input=move |ev| set_name.set(event_target_value(&ev))
                    placeholder=locale.get("models.custom_name_placeholder").to_owned()
                />
                <input
                    type="text"
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                    prop:value=move || kind.get()
                    on:input=move |ev| set_kind.set(event_target_value(&ev))
                    placeholder=locale.get("models.custom_type_placeholder").to_owned()
                />
            </div>
            <button
                type="button"
                class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                disabled=move || {
                    save.get().is_saving() || provider.get().trim().is_empty()
                        || id.get().trim().is_empty()
                }
                on:click=move |_| add()
            >
                {label_add}
            </button>
            <SaveLine save=save />
        </div>
    }
}

/// One custom model, with the button that removes it.
#[component]
fn CustomRow(
    entry: crate::routes::types::CustomModel,
    reload: impl Fn() + Copy + 'static + Send + Sync,
    remove_label: String,
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
        <li class="flex items-center justify-between gap-3">
            <span class="truncate">{name}</span>
            <span class="shrink-0 flex items-center gap-2 text-xs text-muted-foreground">
                <code>{entry.provider_alias}</code>
                {(!kind.is_empty()).then(|| view! { <span>{kind}</span> })}
                <button
                    type="button"
                    class="underline-offset-4 hover:underline disabled:opacity-50"
                    disabled=move || save.get().is_saving()
                    on:click=move |_| remove()
                >
                    {remove_label}
                </button>
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
    let locale = crate::i18n::use_locale();
    let (alias, set_alias) = signal(String::new());
    let (model, set_model) = signal(String::new());
    let (save, set_save) = signal(Save::Idle);
    let encode_failed = locale.get("models.encode_failed").to_owned();
    let label_set = locale.get("models.set_alias").to_owned();
    let label_remove = locale.get("models.remove").to_owned();

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
        <div class="space-y-4">
            {if data.aliases.is_empty() {
                view! {
                    <p class="text-sm text-muted-foreground">
                        {locale.get("models.aliases_none").to_owned()}
                    </p>
                }
                    .into_any()
            } else {
                view! {
                    <ul class="space-y-1 text-sm">
                        {data
                            .aliases
                            .into_iter()
                            .map(|(name, target)| {
                                view! {
                                    <AliasRow
                                        alias=name
                                        model=target
                                        reload=reload
                                        remove_label=label_remove.clone()
                                    />
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                    .into_any()
            }}
            <div class="grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]">
                <input
                    type="text"
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono text-xs"
                    prop:value=move || alias.get()
                    on:input=move |ev| set_alias.set(event_target_value(&ev))
                    placeholder=locale.get("models.alias_placeholder").to_owned()
                />
                <input
                    type="text"
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono text-xs"
                    prop:value=move || model.get()
                    on:input=move |ev| set_model.set(event_target_value(&ev))
                    placeholder=locale.get("models.model_placeholder").to_owned()
                />
                <button
                    type="button"
                    class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                    disabled=move || {
                        save.get().is_saving() || alias.get().trim().is_empty()
                            || model.get().trim().is_empty()
                    }
                    on:click=move |_| set()
                >
                    {label_set}
                </button>
            </div>
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
    remove_label: String,
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
        <li class="flex items-center justify-between gap-3">
            <span class="font-mono text-xs truncate">{alias}</span>
            <span class="shrink-0 flex items-center gap-2 text-xs text-muted-foreground">
                <code>{model}</code>
                <button
                    type="button"
                    class="underline-offset-4 hover:underline disabled:opacity-50"
                    disabled=move || save.get().is_saving()
                    on:click=move |_| remove()
                >
                    {remove_label}
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
                .map(|message| {
                    view! { <p class="text-xs text-destructive">{message}</p> }
                })
        }}
    }
}
