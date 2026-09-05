//! User-added OpenAI-, Anthropic- and embedding-compatible endpoints.
//!
//! Routed to `nullrouter-state`, so create, edit and delete all persist. The store enforces four
//! rules the panel has to mirror, because each arrives as a 400 rather than as a corrected value:
//!
//! * an **openai-compatible** node must carry an `apiType` of `chat` or `responses`; the other two
//!   families must not, and have theirs discarded server-side. The field is therefore shown only for
//!   the family that requires it, and defaulted rather than left blank;
//! * **`type` cannot be changed by an edit.** `PUT` reads the family from the stored node and
//!   ignores the body's, so an edit form offering the select would appear to work and silently keep
//!   the old family. Type is create-only here, and shown as fixed text while editing;
//! * **`baseUrl` is optional on create and required on edit.** Create falls back to the family's
//!   default endpoint; an edit that omits it is a 400;
//! * the URL is **normalised on the way in** -- a trailing slash goes, `/embeddings` is stripped
//!   from an embedding node and `/messages` from an Anthropic one -- so what comes back may not be
//!   what was typed. The stored value is what the table shows.
//!
//! `POST /api/provider-nodes/validate` is not wired to a button. It validates its *arguments* and
//! then answers `{"valid": false, "error": "Provider node validation is not supported"}`
//! unconditionally, on this build and in the state service both: there is no input for which it can
//! return success, so a "test connection" control could only ever report failure for a node that
//! works. The limitation is stated in the panel instead.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api::{Hydrate, Method, Save, encode, load, request_detailed, submit_reporting};
use crate::routes::types::timestamp_label;
use crate::routes::{PageHeader, Panel};

/// `GET /api/provider-nodes`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodesList {
    #[serde(default)]
    nodes: Vec<NodeRow>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeRow {
    #[serde(default)]
    id: String,
    #[serde(rename = "type", default)]
    node_type: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    prefix: String,
    /// Absent for every family except openai-compatible.
    #[serde(default)]
    api_type: Option<String>,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    updated_at: String,
}
/// A `POST /api/provider-nodes` body.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateNode<'a> {
    name: &'a str,
    prefix: &'a str,
    #[serde(rename = "type")]
    node_type: &'a str,
    /// Sent only for the family that requires it; the others reject a value.
    #[serde(skip_serializing_if = "Option::is_none")]
    api_type: Option<&'a str>,
    /// Omitted to take the family's default endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<&'a str>,
}

/// A `PUT /api/provider-nodes/{id}` body.
///
/// No `type`: the server reads the family from the stored node regardless of what is sent. `base_url`
/// is not optional here, unlike create.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateNode<'a> {
    name: &'a str,
    prefix: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_type: Option<&'a str>,
    base_url: &'a str,
}

const OPENAI_COMPATIBLE: &str = "openai-compatible";

/// The three families, with their label keys.
const NODE_TYPES: [(&str, &str); 3] = [
    (OPENAI_COMPATIBLE, "nodes.type_openai"),
    ("anthropic-compatible", "nodes.type_anthropic"),
    ("custom-embedding", "nodes.type_embedding"),
];

/// The two API shapes an openai-compatible node can speak.
const API_TYPES: [&str; 2] = ["chat", "responses"];

/// Whether this family carries an `apiType`.
fn takes_api_type(node_type: &str) -> bool {
    node_type == OPENAI_COMPATIBLE
}

#[component]
pub fn ProviderNodes() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (list, set_list) = signal(Hydrate::<NodesList>::Loading);
    let reload = move || {
        set_list.set(Hydrate::Loading);
        load("/api/provider-nodes", set_list);
    };
    reload();

    view! {
        <PageHeader
            title=locale.get("nav.nodes").to_owned()
            description=locale.get("nodes.description").to_owned()
        />
        <CreateForm reload=reload />
        <Panel
            state=list
            on_retry=Callback::new(move |()| reload())
            children=move |data: NodesList| view! { <NodeTable rows=data.nodes reload=reload /> }
        />
        <p class="mt-3 text-sm text-muted-foreground">
            {locale.get("nodes.validate_unsupported").to_owned()}
        </p>
    }
}
#[component]
fn CreateForm(reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (name, set_name) = signal(String::new());
    let (prefix, set_prefix) = signal(String::new());
    let (kind, set_kind) = signal(OPENAI_COMPATIBLE.to_owned());
    let (api_type, set_api_type) = signal("chat".to_owned());
    let (base_url, set_base_url) = signal(String::new());
    let (save, set_save) = signal(Save::Idle);

    let incomplete = move || {
        save.get().is_saving() || name.get().trim().is_empty() || prefix.get().trim().is_empty()
    };
    let shows_api_type = move || takes_api_type(&kind.get());
    let encode_failed = locale.get("nodes.encode_failed").to_owned();
    // The API-shape field is inside a reactive closure, which would move the whole non-`Copy`
    // locale; its one label is taken out here instead.
    let api_type_label = locale.get("nodes.api_type").to_owned();

    let submit = move || {
        let (label, route, url) = (name.get(), prefix.get(), base_url.get());
        let (label, route, url) = (label.trim(), route.trim(), url.trim());
        if label.is_empty() || route.is_empty() {
            return;
        }
        let family = kind.get();
        let api = api_type.get();
        let Ok(body) = encode(&CreateNode {
            name: label,
            prefix: route,
            node_type: &family,
            api_type: takes_api_type(&family).then_some(api.as_str()),
            base_url: (!url.is_empty()).then_some(url),
        }) else {
            set_save.set(Save::Refused(encode_failed.clone()));
            return;
        };

        submit_reporting(
            set_save,
            move || async move {
                request_detailed(Method::Post, "/api/provider-nodes", Some(&body)).await
            },
            move |_| {
                set_name.set(String::new());
                set_prefix.set(String::new());
                set_base_url.set(String::new());
                reload();
            },
        );
    };

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4 mb-4">
            <p class="text-sm text-muted-foreground">{locale.get("nodes.create_hint").to_owned()}</p>
            <div class="grid gap-3 sm:grid-cols-2">
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">{locale.get("nodes.name").to_owned()}</span>
                    <input
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || name.get()
                        on:input=move |ev| set_name.set(event_target_value(&ev))
                        placeholder=locale.get("nodes.name_placeholder").to_owned()
                    />
                </label>
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("nodes.prefix").to_owned()}
                    </span>
                    <input
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono"
                        prop:value=move || prefix.get()
                        on:input=move |ev| set_prefix.set(event_target_value(&ev))
                        placeholder=locale.get("nodes.prefix_placeholder").to_owned()
                    />
                </label>
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">{locale.get("nodes.type").to_owned()}</span>
                    <select
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || kind.get()
                        on:change=move |ev| set_kind.set(event_target_value(&ev))
                    >
                        {NODE_TYPES
                            .into_iter()
                            .map(|(value, key)| {
                                view! {
                                    <option value=value>{locale.get(key).to_owned()}</option>
                                }
                            })
                            .collect_view()}
                    </select>
                </label>
                {move || {
                    let api_type_label = api_type_label.clone();
                    shows_api_type()
                        .then(|| {
                            view! {
                                <label class="space-y-1 text-sm">
                                    <span class="text-muted-foreground">{api_type_label}</span>
                                    <select
                                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                                        prop:value=move || api_type.get()
                                        on:change=move |ev| set_api_type.set(event_target_value(&ev))
                                    >
                                        {API_TYPES
                                            .into_iter()
                                            .map(|value| view! { <option value=value>{value}</option> })
                                            .collect_view()}
                                    </select>
                                </label>
                            }
                        })
                }}
            </div>
            <label class="block space-y-1 text-sm">
                <span class="text-muted-foreground">{locale.get("nodes.base_url").to_owned()}</span>
                <input
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono"
                    prop:value=move || base_url.get()
                    on:input=move |ev| set_base_url.set(event_target_value(&ev))
                    placeholder=locale.get("nodes.base_url_placeholder").to_owned()
                />
                <span class="block text-xs text-muted-foreground">
                    {locale.get("nodes.base_url_hint").to_owned()}
                </span>
            </label>
            <button
                type="button"
                class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                disabled=incomplete
                on:click=move |_| submit()
            >
                {locale.get("nodes.create").to_owned()}
            </button>
            {move || {
                save.get()
                    .message()
                    .map(|message| view! { <p class="text-sm text-destructive">{message}</p> })
            }}
        </section>
    }
}

#[component]
fn NodeTable(
    rows: Vec<NodeRow>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">{locale.get("nodes.empty").to_owned()}</p>
        }
        .into_any();
    }
    view! {
        <div class="rounded-lg border border-border overflow-x-auto">
            <table class="w-full text-sm">
                <thead class="bg-muted/50 text-muted-foreground">
                    <tr>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("nodes.col_name").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("nodes.col_prefix").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("nodes.col_type").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("nodes.col_url").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("nodes.col_updated").to_owned()}
                        </th>
                        <th class="px-3 py-2"></th>
                    </tr>
                </thead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| view! { <NodeLine row=row reload=reload /> })
                        .collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}
/// A family's label, falling back to the stored value for one this build does not know.
fn type_label(node_type: &str, locale: &crate::i18n::Locale) -> String {
    NODE_TYPES
        .iter()
        .find(|(value, _)| *value == node_type)
        .map_or_else(
            || node_type.to_owned(),
            |(_, key)| locale.get(key).to_owned(),
        )
}

/// One node: its stored values, and an edit form that opens beneath them.
///
/// The form is a second row rather than inputs swapped into the cells, so a base URL has the width
/// to be read.
#[component]
fn NodeLine(row: NodeRow, reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (editing, set_editing) = signal(false);
    let (armed, set_armed) = signal(false);
    let (save_state, set_save) = signal(Save::Idle);

    let (name, set_name) = signal(row.name.clone());
    let (prefix, set_prefix) = signal(row.prefix.clone());
    let (base_url, set_base_url) = signal(row.base_url.clone());
    let (api_type, set_api_type) =
        signal(row.api_type.clone().unwrap_or_else(|| "chat".to_owned()));

    let family = row.node_type.clone();
    let has_api_type = takes_api_type(&family);

    // Stored rather than captured: both writes are invoked from inside reactive closures, which
    // rebuild their handlers on every run and so need a `Copy` closure. A `String` captured
    // directly would make these `FnOnce`.
    let save_family = StoredValue::new(family.clone());
    let save_id = StoredValue::new(row.id.clone());
    let all_required = StoredValue::new(locale.get("nodes.all_required").to_owned());
    let encode_failed = StoredValue::new(locale.get("nodes.encode_failed").to_owned());

    let save = move || {
        let (label, route, url) = (name.get(), prefix.get(), base_url.get());
        let (label, route, url) = (label.trim(), route.trim(), url.trim());
        // The server requires all three on an update; an empty base URL is a 400 rather than a
        // "leave it alone", so it is caught here instead of spent on a round trip.
        if label.is_empty() || route.is_empty() || url.is_empty() {
            set_save.set(Save::Refused(all_required.get_value()));
            return;
        }
        let api = api_type.get();
        let Ok(body) = encode(&UpdateNode {
            name: label,
            prefix: route,
            api_type: takes_api_type(&save_family.get_value()).then_some(api.as_str()),
            base_url: url,
        }) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };
        let path = format!("/api/provider-nodes/{}", save_id.get_value());

        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Put, &path, Some(&body)).await },
            move |_| {
                set_editing.set(false);
                reload();
            },
        );
    };

    let remove = move || {
        let path = format!("/api/provider-nodes/{}", save_id.get_value());
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Delete, &path, None).await },
            move |_| {
                set_armed.set(false);
                reload();
            },
        );
    };

    let label = type_label(&family, &locale);

    // Every label read inside a reactive closure is taken out first: `Locale` is not `Copy`, so a
    // closure reading it directly would move the whole table away from the rest of the row. The
    // edit form needs most of the table, so it gets its own clone instead.
    let label_edit = locale.get("nodes.edit").to_owned();
    let label_close = locale.get("nodes.close").to_owned();
    let label_delete = locale.get("nodes.delete").to_owned();
    let label_confirm_delete = locale.get("nodes.confirm_delete").to_owned();
    let label_cancel = locale.get("nodes.cancel").to_owned();
    let form_locale = locale;
    let form_family = family;

    view! {
        <tr class="border-t border-border align-top">
            <td class="px-3 py-2">{row.name.clone()}</td>
            <td class="px-3 py-2 font-mono text-xs">{row.prefix.clone()}</td>
            <td class="px-3 py-2">
                <div class="space-y-0.5">
                    <span>{label}</span>
                    {row.api_type
                        .clone()
                        .filter(|value| !value.is_empty())
                        .map(|value| {
                            view! {
                                <p class="font-mono text-xs text-muted-foreground">{value}</p>
                            }
                        })}
                </div>
            </td>
            <td class="px-3 py-2 font-mono text-xs break-all">{row.base_url.clone()}</td>
            <td class="px-3 py-2 text-xs text-muted-foreground">
                {timestamp_label(&row.updated_at)}
            </td>
            <td class="px-3 py-2 text-right">
                <div class="flex flex-col items-end gap-1.5">
                    <button
                        type="button"
                        class="text-sm underline-offset-4 hover:underline"
                        on:click=move |_| set_editing.update(|open| *open = !*open)
                    >
                        {move || {
                            if editing.get() {
                                label_close.clone()
                            } else {
                                label_edit.clone()
                            }
                        }}
                    </button>
                    {move || {
                        let (confirm, cancel, delete) = (
                            label_confirm_delete.clone(),
                            label_cancel.clone(),
                            label_delete.clone(),
                        );
                        if armed.get() {
                            view! {
                                <div class="flex items-center gap-2">
                                    <button
                                        type="button"
                                        class="text-sm font-medium text-destructive underline-offset-4 \
                                               hover:underline disabled:opacity-50"
                                        disabled=move || save_state.get().is_saving()
                                        on:click=move |_| remove()
                                    >
                                        {confirm}
                                    </button>
                                    <button
                                        type="button"
                                        class="text-sm text-muted-foreground underline-offset-4 hover:underline"
                                        on:click=move |_| set_armed.set(false)
                                    >
                                        {cancel}
                                    </button>
                                </div>
                            }
                                .into_any()
                        } else {
                            view! {
                                <button
                                    type="button"
                                    class="text-sm text-destructive underline-offset-4 hover:underline"
                                    on:click=move |_| set_armed.set(true)
                                >
                                    {delete}
                                </button>
                            }
                                .into_any()
                        }
                    }}
                </div>
            </td>
        </tr>
        {move || {
            let locale = form_locale.clone();
            editing
                .get()
                .then(|| {
                    let fixed = type_label(&form_family, &locale);
                    view! {
                        <tr class="border-t border-border bg-muted/20">
                            <td colspan="6" class="px-3 py-4">
                                <div class="space-y-3">
                                    <div class="grid gap-3 sm:grid-cols-2">
                                        <label class="space-y-1 text-sm">
                                            <span class="text-muted-foreground">
                                                {locale.get("nodes.name").to_owned()}
                                            </span>
                                            <input
                                                class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                                                prop:value=move || name.get()
                                                on:input=move |ev| set_name.set(event_target_value(&ev))
                                            />
                                        </label>
                                        <label class="space-y-1 text-sm">
                                            <span class="text-muted-foreground">
                                                {locale.get("nodes.prefix").to_owned()}
                                            </span>
                                            <input
                                                class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono"
                                                prop:value=move || prefix.get()
                                                on:input=move |ev| set_prefix.set(event_target_value(&ev))
                                            />
                                        </label>
                                    </div>
                                    // Type is stated, not offered: `PUT` reads the family from the
                                    // stored node and ignores the body's.
                                    <p class="text-sm">
                                        <span class="text-muted-foreground">
                                            {locale.get("nodes.type").to_owned()}
                                        </span>
                                        <span class="ml-2">{fixed}</span>
                                        <span class="ml-2 text-xs text-muted-foreground">
                                            {locale.get("nodes.type_locked").to_owned()}
                                        </span>
                                    </p>
                                    {has_api_type
                                        .then(|| {
                                            view! {
                                                <label class="block space-y-1 text-sm sm:max-w-xs">
                                                    <span class="text-muted-foreground">
                                                        {locale.get("nodes.api_type").to_owned()}
                                                    </span>
                                                    <select
                                                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                                                        prop:value=move || api_type.get()
                                                        on:change=move |ev| {
                                                            set_api_type.set(event_target_value(&ev));
                                                        }
                                                    >
                                                        {API_TYPES
                                                            .into_iter()
                                                            .map(|value| {
                                                                view! { <option value=value>{value}</option> }
                                                            })
                                                            .collect_view()}
                                                    </select>
                                                </label>
                                            }
                                        })}
                                    <label class="block space-y-1 text-sm">
                                        <span class="text-muted-foreground">
                                            {locale.get("nodes.base_url").to_owned()}
                                        </span>
                                        <input
                                            class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono"
                                            prop:value=move || base_url.get()
                                            on:input=move |ev| set_base_url.set(event_target_value(&ev))
                                        />
                                        <span class="block text-xs text-muted-foreground">
                                            {locale.get("nodes.base_url_required").to_owned()}
                                        </span>
                                    </label>
                                    <div class="flex items-center gap-2">
                                        <button
                                            type="button"
                                            class="rounded-md bg-primary px-3 py-2 text-sm font-medium \
                                                   text-primary-foreground disabled:opacity-50"
                                            disabled=move || save_state.get().is_saving()
                                            on:click=move |_| save()
                                        >
                                            {locale.get("nodes.save").to_owned()}
                                        </button>
                                        <button
                                            type="button"
                                            class="rounded-md border border-border px-3 py-2 text-sm \
                                                   font-medium transition-colors hover:bg-accent"
                                            on:click=move |_| set_editing.set(false)
                                        >
                                            {locale.get("nodes.cancel").to_owned()}
                                        </button>
                                    </div>
                                </div>
                            </td>
                        </tr>
                    }
                })
        }}
        {move || {
            save_state
                .get()
                .message()
                .map(|message| {
                    view! {
                        <tr>
                            <td colspan="6" class="px-3 pb-2 text-sm text-destructive">{message}</td>
                        </tr>
                    }
                })
        }}
    }
}
#[cfg(test)]
mod tests {
    use super::{
        API_TYPES, CreateNode, NODE_TYPES, NodesList, OPENAI_COMPATIBLE, UpdateNode, takes_api_type,
    };

    /// `GET /api/provider-nodes` with one node, captured from the running router.
    const LIVE_LIST: &str = r#"{"nodes":[{
        "id":"openai-compatible-1","type":"openai-compatible","name":"probe","prefix":"pb",
        "apiType":"chat","baseUrl":"https://api.example.com/v1",
        "createdAt":"unix-ms:1788527313149","updatedAt":"unix-ms:1788527313149"
    }]}"#;

    #[test]
    fn the_live_list_decodes_with_its_renamed_type_field() {
        let parsed: NodesList = serde_json::from_str(LIVE_LIST).expect("must decode");
        let row = parsed.nodes.first().expect("one node");
        assert_eq!(row.id, "openai-compatible-1");
        assert_eq!(row.node_type, "openai-compatible");
        assert_eq!(row.api_type.as_deref(), Some("chat"));
        assert_eq!(row.base_url, "https://api.example.com/v1");
    }

    #[test]
    fn a_node_without_an_api_type_decodes_as_absent() {
        // The server omits `apiType` for every family but openai-compatible. Absent must not
        // become an empty string that would render as a blank API shape.
        let body = r#"{"nodes":[{"id":"anthropic-compatible-1","type":"anthropic-compatible",
                       "name":"n","prefix":"p","baseUrl":"https://api.anthropic.com/v1",
                       "createdAt":"unix-ms:1","updatedAt":"unix-ms:1"}]}"#;
        let parsed: NodesList = serde_json::from_str(body).expect("must decode");
        assert!(
            parsed
                .nodes
                .first()
                .is_some_and(|row| row.api_type.is_none())
        );
    }

    #[test]
    fn an_empty_list_is_not_a_failure() {
        let parsed: NodesList = serde_json::from_str(r#"{"nodes":[]}"#).expect("must decode");
        assert!(parsed.nodes.is_empty());
    }

    #[test]
    fn only_the_openai_family_carries_an_api_type() {
        // The store rejects an `apiType` on the other two and requires one here, so this predicate
        // is what keeps a create from being a 400 either way.
        assert!(takes_api_type(OPENAI_COMPATIBLE));
        assert!(!takes_api_type("anthropic-compatible"));
        assert!(!takes_api_type("custom-embedding"));
    }
    #[test]
    fn a_create_sends_type_under_its_wire_name() {
        // `node_type` would be serialized as `nodeType` by the camelCase rule; the server reads
        // `type`, and the rename is what stops every create from defaulting to openai-compatible.
        let body = serde_json::to_string(&CreateNode {
            name: "probe",
            prefix: "pb",
            node_type: "custom-embedding",
            api_type: None,
            base_url: Some("https://api.example.com/v1"),
        })
        .expect("encodes");
        assert!(body.contains("\"type\":\"custom-embedding\""));
        assert!(!body.contains("nodeType"));
        // Omitted rather than null: the store's `Option` check treats null as absent too, but a
        // family that rejects the field must not see it at all.
        assert!(!body.contains("apiType"));
    }

    #[test]
    fn a_create_without_a_base_url_omits_it_for_the_family_default() {
        let body = serde_json::to_string(&CreateNode {
            name: "probe",
            prefix: "pb",
            node_type: OPENAI_COMPATIBLE,
            api_type: Some("chat"),
            base_url: None,
        })
        .expect("encodes");
        assert!(!body.contains("baseUrl"));
        assert!(body.contains("\"apiType\":\"chat\""));
    }

    #[test]
    fn an_update_never_sends_a_type() {
        // `PUT` reads the family from the stored node. Sending one would suggest an edit can change
        // it, which it cannot.
        let body = serde_json::to_string(&UpdateNode {
            name: "probe",
            prefix: "pb",
            api_type: Some("responses"),
            base_url: "https://api.example.com/v1",
        })
        .expect("encodes");
        assert!(!body.contains("\"type\""));
        assert!(body.contains("\"baseUrl\":\"https://api.example.com/v1\""));
        assert!(body.contains("\"apiType\":\"responses\""));
    }

    #[test]
    fn the_offered_families_and_api_shapes_are_the_ones_the_store_accepts() {
        // Mirrors `is_valid_node_type` and `api_type_for_node` in `services/state-actix`. Anything
        // else is a 400, so an option this panel offers has to be on both lists.
        let families: Vec<&str> = NODE_TYPES.iter().map(|(value, _)| *value).collect();
        assert_eq!(
            families,
            vec![
                "openai-compatible",
                "anthropic-compatible",
                "custom-embedding"
            ]
        );
        assert_eq!(API_TYPES.to_vec(), vec!["chat", "responses"]);
        for (value, key) in NODE_TYPES {
            assert!(!value.is_empty());
            assert!(key.starts_with("nodes.type_"), "{key} should be namespaced");
        }
    }
}
