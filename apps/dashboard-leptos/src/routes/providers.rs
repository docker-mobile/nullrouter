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
use crate::routes::{
    Alert, AlertDescription, AlertVariant, Badge, BadgeVariant, Button, ButtonSize, ButtonVariant,
    Card, CardContent, CardHeader, CardTitle, EmptyState, ErrorNote, Field, Icon, IconName, Input,
    PageBody, PageHeader, Panel, Select, StatusDot, StatusTone, Table, TableCell, TableEmpty,
    TableFrame, TableHead, TableHeaderCell, TableRow, button_class,
};
use crate::t;

/// Categories in the order they are shown. Anything the registry reports outside this list falls
/// under "Other" rather than being dropped, so a new upstream category cannot hide a provider.
const CATEGORY_ORDER: &[&str] = &["apikey", "oauth", "freeTier", "free", "webCookie"];

/// The one provider the store accepts without an API key.
const KEYLESS_PROVIDER: &str = "ollama-local";

#[component]
pub fn Providers() -> impl IntoView {
    let (list, set_list) = signal(Hydrate::<ProvidersList>::Loading);
    let (query, set_query) = signal(String::new());
    let reload = move || {
        set_list.set(Hydrate::Loading);
        load("/api/providers", set_list);
    };
    reload();

    view! {
        <PageHeader title=t!("nav.providers") description=t!("providers.description") />
        <PageBody>
            // The filter sits above the panel rather than inside it, so typing does not re-mount the
            // catalogue and the caret never jumps while the list re-renders under it.
            <div class="max-w-sm">
                <Input
                    value=Signal::derive(move || query.get())
                    on_input=Callback::new(move |value| set_query.set(value))
                    input_type="search"
                    placeholder=t!("providers.search_placeholder")
                    aria_label=t!("providers.search_placeholder")
                />
            </div>
            <Panel
                state=list
                on_retry=Callback::new(move |()| reload())
                children=move |data: ProvidersList| {
                    view! { <Catalogue connections=data.connections query=query /> }
                }
            />
        </PageBody>
    }
}

/// Every provider, grouped by category, with a connection count on the ones that have any.
#[component]
fn Catalogue(connections: Vec<ProviderRow>, query: ReadSignal<String>) -> impl IntoView {
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

    view! {
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
                return view! {
                    <Card>
                        <EmptyState
                            title=t!("providers.search_none")
                            icon=IconName::Search
                        />
                    </Card>
                }
                    .into_any();
            }
            view! {
                <div class="space-y-4">
                    {sections
                        .into_iter()
                        .map(|(label, tiles)| {
                            let total = tiles.len().to_string();
                            view! {
                                <Card>
                                    <CardHeader>
                                        <CardTitle>{label}</CardTitle>
                                        <Badge variant=BadgeVariant::Secondary>{total}</Badge>
                                    </CardHeader>
                                    <CardContent>
                                        <ul class="grid gap-x-3 gap-y-0.5 sm:grid-cols-2 2xl:grid-cols-3">
                                            {tiles
                                                .into_iter()
                                                .map(|tile| view! { <ProviderTile tile=tile /> })
                                                .collect_view()}
                                        </ul>
                                    </CardContent>
                                </Card>
                            }
                        })
                        .collect_view()}
                </div>
            }
                .into_any()
        }}
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
    // Literal keys rather than a lookup table: `i18n-gen` finds keys by scanning the source for
    // `t!("`, so a key reaching the macro through a variable never lands in the locale files.
    match category {
        "apikey" => t!("providers.category_apikey"),
        "oauth" => t!("providers.category_oauth"),
        "freeTier" => t!("providers.category_freeTier"),
        "free" => t!("providers.category_free"),
        "webCookie" => t!("providers.category_webCookie"),
        _ => t!("providers.category_other"),
    }
}

#[component]
fn ProviderTile(tile: Tile) -> impl IntoView {
    let href = format!("/dashboard/providers/{}", tile.id);
    let count = tile.connections;
    let badge = (count > 0).then(|| {
        let value = count.to_string();
        crate::i18n::use_locale().fmt("providers.connections_count", &[("count", &value)])
    });
    view! {
        <li class="min-w-0">
            <a
                href=href
                class="group flex min-w-0 items-center gap-2 rounded-md px-2 py-1.5 text-sm \
                       outline-none transition-colors hover:bg-accent \
                       focus-visible:ring-[3px] focus-visible:ring-ring/50"
            >
                <span class="min-w-0 flex-1 truncate">{tile.name}</span>
                {badge
                    .map(|label| {
                        view! { <Badge variant=BadgeVariant::Success>{label}</Badge> }
                    })}
                <code class="shrink-0 truncate font-mono text-[0.6875rem] text-muted-foreground">
                    {tile.id}
                </code>
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
            let unknown = entry.is_none();
            let for_form = id.clone();
            let for_list = id.clone();
            view! {
                <PageHeader title=title description=t!("providers.detail_description")>
                    // The way back is a page action rather than a stray link above the heading: the
                    // heading is the first thing on the page, as it is on every other route.
                    <a
                        href="/dashboard/providers"
                        class=button_class(ButtonVariant::Outline, ButtonSize::Sm)
                    >
                        <Icon name=IconName::ChevronRight class="size-3.5 rotate-180" />
                        {t!("providers.back")}
                    </a>
                </PageHeader>
                <PageBody>
                    {unknown
                        .then(|| {
                            view! {
                                <Alert variant=AlertVariant::Muted>
                                    <AlertDescription>
                                        {t!("providers.unknown_provider")}
                                    </AlertDescription>
                                </Alert>
                            }
                        })}
                    <RegistryFacts id=id.clone() />
                    <AddConnection provider=for_form reload=reload />
                    <Card>
                        <CardHeader>
                            <CardTitle>{t!("providers.connections")}</CardTitle>
                        </CardHeader>
                        <CardContent class="px-0 pb-0">
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
                        </CardContent>
                    </Card>
                </PageBody>
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
        crate::i18n::use_locale().fmt("providers.models_known", &[("count", &count)])
    });

    view! {
        <Card>
            <CardHeader>
                <CardTitle>{t!("providers.registry")}</CardTitle>
            </CardHeader>
            <CardContent>
                <dl class="grid gap-x-6 gap-y-3 sm:grid-cols-2 xl:grid-cols-4">
                    {endpoint
                        .map(|url| {
                            view! {
                                <div class="min-w-0 space-y-0.5">
                                    <dt class="text-xs text-muted-foreground">
                                        {t!("providers.docs_endpoint")}
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
                                        {t!("providers.auth_header")}
                                    </dt>
                                    <dd class="truncate font-mono text-xs">{name}</dd>
                                </div>
                            }
                        })}
                    {models_label
                        .map(|label| {
                            view! {
                                <div class="min-w-0 space-y-0.5">
                                    <dt class="text-xs text-muted-foreground">
                                        {t!("providers.models")}
                                    </dt>
                                    <dd class="truncate text-sm">{label}</dd>
                                </div>
                            }
                        })}
                    {website
                        .map(|url| {
                            let href = url.clone();
                            view! {
                                <div class="min-w-0 space-y-0.5">
                                    <dt class="text-xs text-muted-foreground">
                                        {t!("providers.website")}
                                    </dt>
                                    <dd class="truncate">
                                        <a
                                            href=href
                                            target="_blank"
                                            rel="noreferrer noopener"
                                            class="inline-flex items-center gap-1 text-xs underline-offset-4 hover:underline"
                                        >
                                            {url}
                                            <Icon name=IconName::ExternalLink class="size-3" />
                                        </a>
                                    </dd>
                                </div>
                            }
                        })}
                </dl>
            </CardContent>
        </Card>
    }
    .into_any()
}

/// The form that stores a connection for this provider.
#[component]
fn AddConnection(
    provider: String,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
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
    // Cloned once for the `Select`, whose options signal is re-read on every render and therefore
    // cannot take the list by move.
    let region_options = regions;

    let provider_id = StoredValue::new(provider);
    let encode_failed = StoredValue::new(t!("providers.encode_failed"));

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
        <Card>
            <CardHeader>
                <CardTitle>{t!("providers.add_connection")}</CardTitle>
            </CardHeader>
            <CardContent class="space-y-3">
                {is_oauth
                    .then(|| {
                        view! {
                            <Alert variant=AlertVariant::Warning>
                                <AlertDescription>{t!("providers.oauth_hint")}</AlertDescription>
                            </Alert>
                        }
                    })}
                <div class="grid gap-3 sm:grid-cols-2">
                    <Field label=t!("providers.field_name") required=true>
                        <Input
                            value=Signal::derive(move || name.get())
                            on_input=Callback::new(move |value| set_name.set(value))
                            placeholder=t!("providers.name_placeholder")
                        />
                    </Field>
                    {if keyless {
                        view! {
                            <Field
                                label=t!("providers.field_api_key")
                                description=t!("providers.no_key_needed")
                            />
                        }
                            .into_any()
                    } else {
                        view! {
                            <Field label=t!("providers.field_api_key") required=true>
                                <Input
                                    value=Signal::derive(move || key.get())
                                    on_input=Callback::new(move |value| set_key.set(value))
                                    input_type="password"
                                    autocomplete="off"
                                    placeholder=t!("providers.api_key_placeholder")
                                    class="font-mono"
                                />
                            </Field>
                        }
                            .into_any()
                    }}
                    <Field label=t!("providers.field_default_model")>
                        <Input
                            value=Signal::derive(move || model.get())
                            on_input=Callback::new(move |value| set_model.set(value))
                            placeholder=t!("providers.default_model_placeholder")
                            class="font-mono"
                        />
                    </Field>
                    <Field label=t!("providers.field_priority")>
                        <Input
                            value=Signal::derive(move || priority.get())
                            on_input=Callback::new(move |value| set_priority.set(value))
                            input_type="number"
                            placeholder=t!("providers.priority_placeholder")
                        />
                    </Field>
                    {has_regions
                        .then(|| {
                            view! {
                                <Field label=t!("providers.field_region")>
                                    <Select
                                        value=Signal::derive(move || region.get())
                                        options=Signal::derive(move || {
                                            region_options
                                                .iter()
                                                .map(|value| (value.clone(), value.clone()))
                                                .collect()
                                        })
                                        on_change=Callback::new(move |value| set_region.set(value))
                                        aria_label=t!("providers.field_region")
                                    />
                                </Field>
                            }
                        })}
                </div>
            </CardContent>
            <div class="flex items-center justify-between gap-3 border-t border-border bg-muted/30 px-5 py-3">
                <div class="min-w-0">
                    {move || {
                        save.get()
                            .message()
                            .map(|message| view! { <ErrorNote message=message /> })
                    }}
                </div>
                <Button
                    label=t!("providers.add_connection")
                    disabled=Signal::derive(move || {
                        save.get().is_saving() || name.get().trim().is_empty()
                            || (!keyless && key.get().trim().is_empty())
                    })
                    busy=Signal::derive(move || save.get().is_saving())
                    on_click=Callback::new(move |()| create())
                />
            </div>
        </Card>
    }
}

/// The connections stored for one provider.
#[component]
fn ConnectionTable(
    rows: Vec<ProviderRow>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    if rows.is_empty() {
        return view! {
            <TableFrame class="rounded-none border-0">
                <Table>
                    <TableEmpty message=t!("providers.detail_empty") columns=4 />
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
                        <TableHeaderCell>{t!("providers.col_name")}</TableHeaderCell>
                        <TableHeaderCell>{t!("providers.col_auth")}</TableHeaderCell>
                        <TableHeaderCell>{t!("providers.col_status")}</TableHeaderCell>
                        <TableHeaderCell class="text-right">
                            {t!("providers.col_actions")}
                        </TableHeaderCell>
                    </tr>
                </TableHead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| view! { <ConnectionRow row=row reload=reload /> })
                        .collect_view()}
                </tbody>
            </Table>
        </TableFrame>
    }
    .into_any()
}

/// One stored connection, with test, enable/disable and delete.
#[component]
fn ConnectionRow(
    row: ProviderRow,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (save, set_save) = signal(Save::Idle);
    let (test, set_test) = signal(Save::Idle);
    let (result, set_result) = signal(None::<ProbeOutcome>);

    let active = row.is_active;
    let id = StoredValue::new(row.id.clone());
    let encode_failed = StoredValue::new(t!("providers.encode_failed"));
    let label_toggle = if active {
        t!("providers.disable")
    } else {
        t!("providers.enable")
    };

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
        <TableRow hoverable=false>
            <TableCell>
                <span class="block font-medium">{row.name}</span>
                <code class="block font-mono text-xs text-muted-foreground">{row.id}</code>
            </TableCell>
            <TableCell class="text-muted-foreground">{row.auth_type}</TableCell>
            <TableCell>
                <Badge variant=if active {
                    BadgeVariant::Success
                } else {
                    BadgeVariant::Secondary
                }>{if active { t!("state.enabled") } else { t!("state.disabled") }}</Badge>
                <TestOutcome test=test result=result />
                {move || {
                    save.get()
                        .message()
                        .map(|message| view! { <ErrorNote message=message class="mt-1 text-xs" /> })
                }}
            </TableCell>
            <TableCell class="whitespace-nowrap text-right">
                <div class="flex items-center justify-end gap-1">
                    // Rebuilt on each render rather than given a fixed label: the wording has to
                    // follow the in-flight state, and `Button` reads its label once at construction.
                    {move || {
                        view! {
                            <Button
                                label=if test.get().is_saving() {
                                    t!("providers.testing")
                                } else {
                                    t!("providers.test")
                                }
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::Sm
                                disabled=Signal::derive(move || test.get().is_saving())
                                on_click=Callback::new(move |()| probe())
                            />
                        }
                    }}
                    <Button
                        label=label_toggle
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        disabled=Signal::derive(move || save.get().is_saving())
                        on_click=Callback::new(move |()| toggle())
                    />
                    <Button
                        label=t!("providers.delete")
                        variant=ButtonVariant::Destructive
                        size=ButtonSize::Sm
                        disabled=Signal::derive(move || save.get().is_saving())
                        on_click=Callback::new(move |()| remove())
                    />
                </div>
            </TableCell>
        </TableRow>
    }
}

/// What a finished probe says.
#[component]
fn TestOutcome(test: ReadSignal<Save>, result: ReadSignal<Option<ProbeOutcome>>) -> impl IntoView {
    view! {
        {move || {
            result
                .get()
                .map(|outcome| {
                    if outcome.success {
                        let millis = outcome.latency_ms.to_string();
                        let latency = crate::i18n::use_locale()
                            .fmt("models.test_latency", &[("ms", &millis)]);
                        view! {
                            <p class="mt-1 flex flex-wrap items-center gap-1.5">
                                <StatusDot label=t!("providers.test_ok") tone=StatusTone::Success />
                                <span class="font-mono text-xs text-muted-foreground">{latency}</span>
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
                            _ => t!("providers.test_failed"),
                        };
                        view! { <ErrorNote message=detail class="mt-1 text-xs" /> }.into_any()
                    }
                })
        }}
        {move || {
            test.get()
                .message()
                .map(|message| view! { <ErrorNote message=message class="mt-1 text-xs" /> })
        }}
    }
}
