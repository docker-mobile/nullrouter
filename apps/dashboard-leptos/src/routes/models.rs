//! The model catalogue, what the router is holding back, and a per-model reachability test.
//!
//! Four reads and one write, kept in separate sections because they come from separate sources and
//! only one of them is stored. The catalogue is what the router will route to; availability,
//! disabled ids, and custom models are each their own endpoint, and merging them into one table
//! would make an empty read indistinguishable from a section this build does not populate.
//!
//! The test button is the only write here. `PUT /api/models/alias`, `POST /api/models/custom`, and
//! `POST /api/models/disabled` all answer `200` with `success: true` and store nothing -- their
//! matching `GET`s keep returning empty afterwards -- so an editor wired to them would report a
//! change it did not make. They are left out until they persist.

use leptos::prelude::*;

use crate::api::{Hydrate, Save, decode, encode, load, post, submit};
use crate::routes::types::{
    Availability, CustomModels, DisabledModels, ModelRow, ModelTestBody, ModelTestResult,
    ModelsList,
};
use crate::routes::{PageHeader, Panel};

#[component]
pub fn Models() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (catalogue, set_catalogue) = signal(Hydrate::<ModelsList>::Loading);
    let (availability, set_availability) = signal(Hydrate::<Availability>::Loading);
    let (disabled, set_disabled) = signal(Hydrate::<DisabledModels>::Loading);
    let (custom, set_custom) = signal(Hydrate::<CustomModels>::Loading);

    let reload = move || {
        set_catalogue.set(Hydrate::Loading);
        set_availability.set(Hydrate::Loading);
        set_disabled.set(Hydrate::Loading);
        set_custom.set(Hydrate::Loading);
        load("/api/models", set_catalogue);
        load("/api/models/availability", set_availability);
        load("/api/models/disabled", set_disabled);
        load("/api/models/custom", set_custom);
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
                    children=|data: DisabledModels| view! { <DisabledBody data=data /> }
                />
            </Section>
        </div>
        <div class="mt-4">
            <Section title=locale.get("models.custom").to_owned()>
                <Panel
                    state=custom
                    on_retry=Callback::new(move |()| reload())
                    children=|data: CustomModels| view! { <CustomBody data=data /> }
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

/// Switched-off model ids, grouped by the provider they belong to.
#[component]
fn DisabledBody(data: DisabledModels) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if data.disabled.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">
                {locale.get("models.disabled_none").to_owned()}
            </p>
        }
        .into_any();
    }
    view! {
        <dl class="space-y-2 text-sm">
            {data
                .disabled
                .into_iter()
                .map(|(provider, ids)| {
                    view! {
                        <div class="space-y-1">
                            <dt class="text-muted-foreground">{provider}</dt>
                            <dd class="flex flex-wrap gap-1">
                                {ids
                                    .into_iter()
                                    .map(|id| {
                                        view! {
                                            <code class="rounded border border-border px-1.5 py-0.5 text-xs">
                                                {id}
                                            </code>
                                        }
                                    })
                                    .collect_view()}
                            </dd>
                        </div>
                    }
                })
                .collect_view()}
        </dl>
    }
    .into_any()
}

/// Models added by hand rather than discovered from a provider.
#[component]
fn CustomBody(data: CustomModels) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if data.models.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">
                {locale.get("models.custom_none").to_owned()}
            </p>
        }
        .into_any();
    }
    view! {
        <ul class="space-y-1 text-sm">
            {data
                .models
                .into_iter()
                .map(|entry| {
                    let name = if entry.name.is_empty() { entry.id.clone() } else { entry.name };
                    view! {
                        <li class="flex items-center justify-between gap-3">
                            <span class="truncate">{name}</span>
                            <span class="shrink-0 flex items-center gap-2 text-xs text-muted-foreground">
                                <code>{entry.provider_alias}</code>
                                {(!entry.model_type.is_empty()).then(|| view! { <span>{entry.model_type}</span> })}
                            </span>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
    .into_any()
}
