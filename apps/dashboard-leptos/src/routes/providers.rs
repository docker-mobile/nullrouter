//! The provider catalogue, and one page per provider that adds connections to it.
//!
//! Two routes live here. The index is a searchable grid over all 117 registry entries plus every
//! provider a connection already exists for, and each tile links to a detail page. The detail page is
//! one route with a path parameter rather than 117 generated routes: what differs between providers is
//! data in the registry -- whether a key is needed, which header carries it, whether the transport is
//! region-keyed -- so a single page shaped by that data stays correct when the registry changes.
//!
//! The add form was the missing piece. `POST /api/providers` has always worked; this panel only ever
//! listed. Three registry facts shape it, because each one is a request the store would refuse or
//! quietly ignore:
//!
//! * **`ollama-local` is the one provider that needs no key.** The store rejects every other create
//!   without one, so the field is required except there, and the reason is shown rather than the
//!   refusal being left to arrive from the server.
//! * **An OAuth provider's key is not its credential.** Its transport authenticates with a token that
//!   arrives through Import. A key stored here is accepted and then unused, so the form says so
//!   instead of implying the connection is ready.
//! * **A region-keyed transport needs its region in `providerSpecificData`.** Without it the runtime
//!   has no endpoint to call, and the connection looks configured while failing every request.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use nullrouter_providers::registry;
use serde::Serialize;

use crate::api::{Hydrate, Method, Save, decode, encode, load, request_detailed, submit_reporting};
use crate::routes::types::{ProviderRow, ProvidersList, display_name, encode_query};
use crate::routes::{PageHeader, Panel};

/// Categories in the order they are shown. Anything the registry reports outside this list falls
/// under "Other" rather than being dropped, so a new upstream category cannot hide a provider.
const CATEGORY_ORDER: &[&str] = &["apikey", "oauth", "freeTier", "free", "webCookie"];

/// The one provider the store accepts without an API key.
const KEYLESS_PROVIDER: &str = "ollama-local";

#[component]
pub fn Providers() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (list, set_list) = signal(Hydrate::<ProvidersList>::Loading);
    let (query, set_query) = signal(String::new());
    let reload = move || {
        set_list.set(Hydrate::Loading);
        load("/api/providers", set_list);
    };
    reload();

    view! {
        <PageHeader
            title=locale.get("nav.providers").to_owned()
            description=locale.get("providers.description").to_owned()
        />
        <Panel
            state=list
            on_retry=Callback::new(move |()| reload())
            children=move |data: ProvidersList| {
                view! { <Catalogue connections=data.connections query=query /> }
            }
        />
        <div class="mt-4">
            <input
                type="search"
                class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                prop:value=move || query.get()
                on:input=move |ev| set_query.set(event_target_value(&ev))
                placeholder=locale.get("providers.search_placeholder").to_owned()
            />
        </div>
    }
}

/// Every provider, grouped by category, with a connection count on the ones that have any.
#[component]
fn Catalogue(connections: Vec<ProviderRow>, query: ReadSignal<String>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    // Counted once rather than per tile: the list is short but the grid is 117 wide.
    let counts = StoredValue::new(connection_counts(&connections));
    // Providers with a stored connection but no registry entry still get a tile. Otherwise a custom
    // connection would be invisible on the page that is supposed to list what exists.
    let extra: Vec<String> = {
        let mut ids: Vec<String> = connections
            .iter()
            .map(|row| row.provider.clone())
            .filter(|id| registry::entry(id).is_none())
            .filter(|id| !id.is_empty())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    };
    let extra = StoredValue::new(extra);
    let empty_label = locale.get("providers.search_none").to_owned();

    view! {
        <div class="space-y-6">
            {move || {
                let needle = query.get().trim().to_lowercase();
                let mut sections: Vec<(String, Vec<Tile>)> = Vec::new();
                for category in CATEGORY_ORDER.iter().chain(std::iter::once(&"other")) {
                    let tiles = tiles_for(category, &needle, &counts.get_value(), &extra.get_value());
                    if !tiles.is_empty() {
                        sections.push((category_label(category), tiles));
                    }
                }
                if sections.is_empty() {
                    let label = empty_label.clone();
                    return view! {
                        <p class="text-sm text-muted-foreground">{label}</p>
                    }
                        .into_any();
                }
                view! {
                    {sections
                        .into_iter()
                        .map(|(label, tiles)| {
                            view! {
                                <section class="space-y-2">
                                    <h2 class="text-sm font-medium text-muted-foreground">{label}</h2>
                                    <ul class="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
                                        {tiles
                                            .into_iter()
                                            .map(|tile| view! { <ProviderTile tile=tile /> })
                                            .collect_view()}
                                    </ul>
                                </section>
                            }
                        })
                        .collect_view()}
                }
                    .into_any()
            }}
        </div>
    }
}

/// One entry in the catalogue grid.
#[derive(Clone, Debug)]
struct Tile {
    id: String,
    name: String,
    connections: usize,
}

fn connection_counts(rows: &[ProviderRow]) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for row in rows {
        *counts.entry(row.provider.clone()).or_insert(0) += 1;
    }
    counts
}

/// Tiles in one category, filtered by the search box.
///
/// Matched on both the display name and the id: an operator looking for "Claude" and one looking for
/// `anthropic` are both looking for the same tile.
fn tiles_for(
    category: &str,
    needle: &str,
    counts: &std::collections::BTreeMap<String, usize>,
    extra: &[String],
) -> Vec<Tile> {
    let mut tiles: Vec<Tile> = registry::entries()
        .iter()
        .filter(|entry| {
            let entry_category = entry.category.as_deref().unwrap_or("other");
            if category == "other" {
                !CATEGORY_ORDER.contains(&entry_category)
            } else {
                entry_category == category
            }
        })
        .map(|entry| Tile {
            id: entry.id.clone(),
            name: display_name(entry),
            connections: counts.get(&entry.id).copied().unwrap_or(0),
        })
        .collect();
    // Unregistered ids are grouped with "other", which is where a reader would look for them.
    if category == "other" {
        tiles.extend(extra.iter().map(|id| Tile {
            id: id.clone(),
            name: id.clone(),
            connections: counts.get(id).copied().unwrap_or(0),
        }));
    }
    tiles.retain(|tile| {
        needle.is_empty()
            || tile.name.to_lowercase().contains(needle)
            || tile.id.to_lowercase().contains(needle)
    });
    tiles.sort_by_key(|tile| tile.name.to_lowercase());
    tiles
}

fn category_label(category: &str) -> String {
    let locale = crate::i18n::use_locale();
    // Literal keys rather than a lookup table: `i18n-gen` finds keys by scanning for `get("`, so a
    // key reaching the call through a variable never lands in the locale files.
    match category {
        "apikey" => locale.get("providers.category_apikey").to_owned(),
        "oauth" => locale.get("providers.category_oauth").to_owned(),
        "freeTier" => locale.get("providers.category_freeTier").to_owned(),
        "free" => locale.get("providers.category_free").to_owned(),
        "webCookie" => locale.get("providers.category_webCookie").to_owned(),
        _ => locale.get("providers.category_other").to_owned(),
    }
}

#[component]
fn ProviderTile(tile: Tile) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let href = format!("/dashboard/providers/{}", tile.id);
    let count = tile.connections;
    let badge = (count > 0).then(|| {
        let value = count.to_string();
        locale.fmt("providers.connections_count", &[("count", &value)])
    });
    view! {
        <li>
            <a
                href=href
                class="flex items-center justify-between gap-2 rounded-md border border-border px-3 py-2 text-sm transition-colors hover:bg-accent hover:text-accent-foreground"
            >
                <span class="min-w-0">
                    <span class="block truncate">{tile.name}</span>
                    <code class="block truncate text-xs text-muted-foreground">{tile.id}</code>
                </span>
                {badge
                    .map(|label| {
                        view! {
                            <span class="shrink-0 rounded-full bg-primary/10 px-2 py-0.5 text-xs text-primary">
                                {label}
                            </span>
                        }
                    })}
            </a>
        </li>
    }
}

/// Body of `POST /api/providers`.
///
/// `providerSpecificData` carries the region for a region-keyed transport. Omitted when empty rather
/// than sent as `{}`, because the store merges what it is given and an empty map would still be a
/// write.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateConnection<'a> {
    provider: &'a str,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_specific_data: Option<std::collections::BTreeMap<String, serde_json::Value>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToggleConnection {
    is_active: bool,
}

/// `POST /api/providers/{id}/test`.
///
/// The verdict is `success`, not the status code: the route answers `200` with `success: false` when
/// the provider refused, so treating the status as the answer reports a broken connection as working.
/// `error` is absent on a pass rather than empty, which is why it is an `Option`.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProbeOutcome {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    latency_ms: u64,
    #[serde(default)]
    error: Option<String>,
}

/// One provider's page: what the registry knows, the connections stored, and the form that adds one.
#[component]
pub fn Provider() -> impl IntoView {
    let params = use_params_map();
    let locale = crate::i18n::use_locale();
    let (list, set_list) = signal(Hydrate::<ProvidersList>::Loading);
    let reload = move || {
        set_list.set(Hydrate::Loading);
        load("/api/providers", set_list);
    };
    reload();

    // Read reactively so navigating between two provider pages re-renders rather than keeping the
    // first one's id.
    let provider_id = move || params.read().get("id").unwrap_or_default();

    view! {
        {move || {
            let id = provider_id();
            let entry = registry::entry(&id);
            let title = entry.map_or_else(|| id.clone(), display_name);
            let back = locale.get("providers.back").to_owned();
            let description = locale.get("providers.detail_description").to_owned();
            let unknown_note = entry
                .is_none()
                .then(|| locale.get("providers.unknown_provider").to_owned());
            let for_form = id.clone();
            let for_list = id.clone();
            view! {
                <a
                    href="/dashboard/providers"
                    class="mb-4 inline-flex items-center gap-1 text-sm text-muted-foreground underline-offset-4 hover:underline"
                >
                    "← "
                    {back}
                </a>
                <PageHeader title=title description=description />
                {unknown_note
                    .map(|note| {
                        view! {
                            <p class="mb-4 rounded-md border border-border bg-muted/40 px-3 py-2 text-sm text-muted-foreground">
                                {note}
                            </p>
                        }
                    })}
                <RegistryFacts id=id.clone() />
                <AddConnection provider=for_form reload=reload />
                <section class="mt-4 space-y-2">
                    <h2 class="text-sm font-medium text-muted-foreground">
                        {locale.get("providers.connections").to_owned()}
                    </h2>
                    <Panel
                        state=list
                        on_retry=Callback::new(move |()| reload())
                        children={
                            let provider = for_list;
                            move |data: ProvidersList| {
                                let rows: Vec<ProviderRow> = data
                                    .connections
                                    .into_iter()
                                    .filter(|row| row.provider == provider)
                                    .collect();
                                view! { <ConnectionTable rows=rows reload=reload /> }
                            }
                        }
                    />
                </section>
            }
        }}
    }
}

/// What the registry says about this provider, when it knows it.
///
/// Shown because it is what an operator needs to judge whether a connection is configured correctly:
/// the endpoint the runtime will call, the header the key goes in, and how many models are known.
#[component]
fn RegistryFacts(id: String) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let Some(entry) = registry::entry(&id) else {
        return view! { <div></div> }.into_any();
    };
    let website = entry
        .display
        .as_ref()
        .and_then(|display| display.website.clone());
    let transport = entry.transport.as_ref();
    let endpoint = transport.and_then(|transport| transport.base_url.clone());
    let header = transport
        .and_then(|transport| transport.auth.as_ref())
        .and_then(|auth| {
            auth.api_key
                .as_ref()
                .map(|spec| spec.header.clone())
                .or_else(|| auth.header.clone())
        });
    let models = entry.models.len();
    let models_label = (models > 0).then(|| {
        let count = models.to_string();
        locale.fmt("providers.models_known", &[("count", &count)])
    });

    view! {
        <dl class="mb-4 grid gap-3 rounded-lg border border-border bg-card p-4 text-sm sm:grid-cols-2">
            {endpoint
                .map(|url| {
                    view! {
                        <div class="min-w-0 space-y-0.5">
                            <dt class="text-xs text-muted-foreground">
                                {locale.get("providers.docs_endpoint").to_owned()}
                            </dt>
                            <dd class="truncate font-mono text-xs">{url}</dd>
                        </div>
                    }
                })}
            {header
                .map(|name| {
                    view! {
                        <div class="min-w-0 space-y-0.5">
                            <dt class="text-xs text-muted-foreground">
                                {locale.get("providers.auth_header").to_owned()}
                            </dt>
                            <dd class="truncate font-mono text-xs">{name}</dd>
                        </div>
                    }
                })}
            {models_label
                .map(|label| {
                    view! {
                        <div class="space-y-0.5">
                            <dt class="text-xs text-muted-foreground">
                                {locale.get("providers.category_other").to_owned()}
                            </dt>
                            <dd>{label}</dd>
                        </div>
                    }
                })}
            {website
                .map(|url| {
                    let href = url.clone();
                    view! {
                        <div class="min-w-0 space-y-0.5">
                            <dt class="text-xs text-muted-foreground">
                                {locale.get("providers.website").to_owned()}
                            </dt>
                            <dd class="truncate">
                                <a
                                    href=href
                                    target="_blank"
                                    rel="noreferrer noopener"
                                    class="text-xs underline-offset-4 hover:underline"
                                >
                                    {url}
                                </a>
                            </dd>
                        </div>
                    }
                })}
        </dl>
    }
    .into_any()
}

/// The form that stores a connection for this provider.
#[component]
fn AddConnection(
    provider: String,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (name, set_name) = signal(String::new());
    let (key, set_key) = signal(String::new());
    let (model, set_model) = signal(String::new());
    let (priority, set_priority) = signal(String::new());
    let (region, set_region) = signal(String::new());
    let (save, set_save) = signal(Save::Idle);

    let entry = registry::entry(&provider);
    let keyless = provider == KEYLESS_PROVIDER;
    let is_oauth = entry.is_some_and(|entry| entry.category.as_deref() == Some("oauth"));
    // A region-keyed transport has no endpoint until a region is chosen, so the field is offered only
    // where the registry declares regions, and defaulted to the one the transport names.
    let regions: Vec<String> = entry
        .and_then(|entry| entry.transport.as_ref())
        .and_then(|transport| transport.regions.as_ref())
        .map(|regions| regions.keys().cloned().collect())
        .unwrap_or_default();
    let default_region = entry
        .and_then(|entry| entry.transport.as_ref())
        .and_then(|transport| transport.default_region.clone());
    if let Some(default) = default_region {
        set_region.set(default);
    }
    let has_regions = !regions.is_empty();

    let provider_id = StoredValue::new(provider);
    let encode_failed = StoredValue::new(locale.get("providers.encode_failed").to_owned());
    let label_add = locale.get("providers.add_connection").to_owned();

    let create = move || {
        let label = name.get().trim().to_owned();
        let secret = key.get().trim().to_owned();
        if save.get().is_saving() {
            return;
        }
        if label.is_empty() || (!keyless && secret.is_empty()) {
            return;
        }
        let default_model = model.get().trim().to_owned();
        let priority_value = priority.get().trim().parse::<u32>().ok();
        let region_value = region.get().trim().to_owned();
        let provider_specific_data = (has_regions && !region_value.is_empty()).then(|| {
            let mut data = std::collections::BTreeMap::new();
            data.insert(
                "region".to_owned(),
                serde_json::Value::String(region_value.clone()),
            );
            data
        });
        let body = CreateConnection {
            provider: &provider_id.get_value(),
            name: &label,
            api_key: (!secret.is_empty()).then_some(secret.as_str()),
            default_model: (!default_model.is_empty()).then_some(default_model.as_str()),
            priority: priority_value,
            provider_specific_data,
        };
        let Ok(encoded) = encode(&body) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };
        submit_reporting(
            set_save,
            move || async move {
                request_detailed(Method::Post, "/api/providers", Some(&encoded)).await
            },
            move |_| {
                set_name.set(String::new());
                set_key.set(String::new());
                set_model.set(String::new());
                set_priority.set(String::new());
                reload();
            },
        );
    };

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("providers.add_connection").to_owned()}
            </h2>
            {is_oauth
                .then(|| {
                    view! {
                        <p class="rounded-md border border-border bg-muted/40 px-3 py-2 text-sm text-muted-foreground">
                            {locale.get("providers.oauth_hint").to_owned()}
                        </p>
                    }
                })}
            <div class="grid gap-3 sm:grid-cols-2">
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("providers.field_name").to_owned()}
                    </span>
                    <input
                        type="text"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || name.get()
                        on:input=move |ev| set_name.set(event_target_value(&ev))
                        placeholder=locale.get("providers.name_placeholder").to_owned()
                    />
                </label>
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("providers.field_api_key").to_owned()}
                    </span>
                    {if keyless {
                        view! {
                            <p class="rounded-md border border-border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
                                {locale.get("providers.no_key_needed").to_owned()}
                            </p>
                        }
                            .into_any()
                    } else {
                        view! {
                            <input
                                type="password"
                                autocomplete="off"
                                class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono text-xs"
                                prop:value=move || key.get()
                                on:input=move |ev| set_key.set(event_target_value(&ev))
                                placeholder=locale.get("providers.api_key_placeholder").to_owned()
                            />
                        }
                            .into_any()
                    }}
                </label>
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("providers.field_default_model").to_owned()}
                    </span>
                    <input
                        type="text"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono text-xs"
                        prop:value=move || model.get()
                        on:input=move |ev| set_model.set(event_target_value(&ev))
                        placeholder=locale.get("providers.default_model_placeholder").to_owned()
                    />
                </label>
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("providers.field_priority").to_owned()}
                    </span>
                    <input
                        type="number"
                        min="1"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || priority.get()
                        on:input=move |ev| set_priority.set(event_target_value(&ev))
                        placeholder=locale.get("providers.priority_placeholder").to_owned()
                    />
                </label>
                {has_regions
                    .then(|| {
                        view! {
                            <label class="space-y-1 text-sm">
                                <span class="text-muted-foreground">
                                    {locale.get("providers.field_region").to_owned()}
                                </span>
                                <select
                                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                                    prop:value=move || region.get()
                                    on:change=move |ev| set_region.set(event_target_value(&ev))
                                >
                                    {regions
                                        .clone()
                                        .into_iter()
                                        .map(|value| {
                                            let label = value.clone();
                                            view! { <option value=value>{label}</option> }
                                        })
                                        .collect_view()}
                                </select>
                            </label>
                        }
                    })}
            </div>
            <button
                type="button"
                class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                disabled=move || {
                    save.get().is_saving() || name.get().trim().is_empty()
                        || (!keyless && key.get().trim().is_empty())
                }
                on:click=move |_| create()
            >
                {label_add}
            </button>
            {move || {
                save.get()
                    .message()
                    .map(|message| {
                        view! { <p class="text-sm text-destructive">{message}</p> }
                    })
            }}
        </section>
    }
}

/// The connections stored for one provider.
#[component]
fn ConnectionTable(
    rows: Vec<ProviderRow>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">
                {locale.get("providers.detail_empty").to_owned()}
            </p>
        }
        .into_any();
    }
    view! {
        <div class="rounded-lg border border-border overflow-x-auto">
            <table class="w-full text-sm">
                <thead class="bg-muted/50 text-muted-foreground">
                    <tr>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("providers.col_name").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("providers.col_auth").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("providers.col_status").to_owned()}
                        </th>
                        <th class="px-3 py-2 text-right">
                            {locale.get("providers.col_actions").to_owned()}
                        </th>
                    </tr>
                </thead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| view! { <ConnectionRow row=row reload=reload /> })
                        .collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

/// One stored connection, with test, enable/disable and delete.
#[component]
fn ConnectionRow(
    row: ProviderRow,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (save, set_save) = signal(Save::Idle);
    let (test, set_test) = signal(Save::Idle);
    let (result, set_result) = signal(None::<ProbeOutcome>);

    let active = row.is_active;
    let id = StoredValue::new(row.id.clone());
    let encode_failed = StoredValue::new(locale.get("providers.encode_failed").to_owned());
    let label_toggle = if active {
        locale.get("providers.disable").to_owned()
    } else {
        locale.get("providers.enable").to_owned()
    };
    let label_delete = locale.get("providers.delete").to_owned();
    let label_test = locale.get("providers.test").to_owned();
    let label_testing = locale.get("providers.testing").to_owned();

    let toggle = move || {
        if save.get().is_saving() {
            return;
        }
        let Ok(encoded) = encode(&ToggleConnection { is_active: !active }) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };
        let path = format!("/api/providers/{}", encode_query(&id.get_value()));
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Put, &path, Some(&encoded)).await },
            move |_| reload(),
        );
    };

    let remove = move || {
        if save.get().is_saving() {
            return;
        }
        let path = format!("/api/providers/{}", encode_query(&id.get_value()));
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Delete, &path, None).await },
            move |_| reload(),
        );
    };

    let probe = move || {
        if test.get().is_saving() {
            return;
        }
        set_result.set(None);
        let path = format!("/api/providers/{}/test", encode_query(&id.get_value()));
        submit_reporting(
            set_test,
            move || async move { request_detailed(Method::Post, &path, Some("{}")).await },
            move |body| {
                // A 200 is not a pass: the verdict is in `ok`, the same shape the model test uses.
                set_result.set(decode::<ProbeOutcome>(&body).ok());
            },
        );
    };

    view! {
        <tr class="border-t border-border align-top">
            <td class="px-3 py-2">
                <span class="block">{row.name}</span>
                <code class="block text-xs text-muted-foreground">{row.id}</code>
            </td>
            <td class="px-3 py-2 text-muted-foreground">{row.auth_type}</td>
            <td class="px-3 py-2">
                {if active {
                    locale.get("state.enabled").to_owned()
                } else {
                    locale.get("state.disabled").to_owned()
                }}
                <TestOutcome test=test result=result />
                {move || {
                    save.get()
                        .message()
                        .map(|message| {
                            view! { <p class="text-xs text-destructive">{message}</p> }
                        })
                }}
            </td>
            <td class="px-3 py-2 text-right whitespace-nowrap space-x-3">
                <button
                    type="button"
                    class="text-sm underline-offset-4 hover:underline disabled:opacity-50"
                    disabled=move || test.get().is_saving()
                    on:click=move |_| probe()
                >
                    {move || {
                        if test.get().is_saving() {
                            label_testing.clone()
                        } else {
                            label_test.clone()
                        }
                    }}
                </button>
                <button
                    type="button"
                    class="text-sm underline-offset-4 hover:underline disabled:opacity-50"
                    disabled=move || save.get().is_saving()
                    on:click=move |_| toggle()
                >
                    {label_toggle}
                </button>
                <button
                    type="button"
                    class="text-sm text-destructive underline-offset-4 hover:underline disabled:opacity-50"
                    disabled=move || save.get().is_saving()
                    on:click=move |_| remove()
                >
                    {label_delete}
                </button>
            </td>
        </tr>
    }
}

/// What a finished probe says.
#[component]
fn TestOutcome(test: ReadSignal<Save>, result: ReadSignal<Option<ProbeOutcome>>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        {move || {
            result
                .get()
                .map(|outcome| {
                    if outcome.success {
                        let millis = outcome.latency_ms.to_string();
                        let latency = locale.fmt("models.test_latency", &[("ms", &millis)]);
                        view! {
                            <p class="text-xs text-success">
                                {locale.get("providers.test_ok").to_owned()}
                                {format!(" · {latency}")}
                            </p>
                        }
                            .into_any()
                    } else {
                        // The provider's own wording, and its status when there was one. "Failed"
                        // tells nobody what to change.
                        let detail = match (outcome.error, outcome.status) {
                            (Some(error), Some(status)) if !error.is_empty() => {
                                format!("{status}: {error}")
                            }
                            (Some(error), None) if !error.is_empty() => error,
                            (_, Some(status)) => format!("{status}"),
                            _ => locale.get("providers.test_failed").to_owned(),
                        };
                        view! {
                            <p class="text-xs text-destructive whitespace-normal break-words">
                                {detail}
                            </p>
                        }
                            .into_any()
                    }
                })
        }}
        {move || {
            test.get()
                .message()
                .map(|message| view! { <p class="text-xs text-destructive">{message}</p> })
        }}
    }
}
