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
//!
//! # Why create and edit are dialogs
//!
//! Create used to be a form pinned above the table, and edit was a second `<tr>` opened beneath the
//! node it edited. Both are five-field forms, so both now open as dialogs: the table is the page, a
//! dialog keeps the form out of the list's column widths, and an edit dialog can state the one field
//! it cannot change (the family) without reserving a row of the table for it.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api::{Hydrate, Method, Save, encode, load, request_detailed, submit_reporting};
use crate::routes::types::timestamp_label;
use crate::routes::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Button, ButtonSize, ButtonVariant, Dialog,
    DialogContent, DialogDescription, DialogDismiss, DialogFooter, DialogHeader, DialogTitle,
    ErrorNote, Field, IconName, Input, Menu, MenuItem, MenuSeparator, Mono, PageBody, PageHeader,
    Panel, Select, Table, TableCell, TableEmpty, TableFrame, TableHead, TableHeaderCell, TableRow,
};
use crate::t;

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
///
/// Test-only. It is the list the tests below pin against `services/state-actix`'s validator, and it
/// documents the families in one place; the dialogs cannot read it, because `t!` only accepts a string
/// literal, so the labels are written out literally at each call site ([`type_label`], the option
/// vectors) rather than looked up through this table.
#[cfg(test)]
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

/// A family's label, falling back to the stored value for one this build does not know.
///
/// A `match` on the wire value rather than a lookup of the key in [`NODE_TYPES`], because `t!` only
/// accepts a string literal and the table's keys are runtime `&str`s.
fn type_label(node_type: &str) -> String {
    match node_type {
        OPENAI_COMPATIBLE => t!("nodes.type_openai"),
        "anthropic-compatible" => t!("nodes.type_anthropic"),
        "custom-embedding" => t!("nodes.type_embedding"),
        other => other.to_owned(),
    }
}

#[component]
pub fn ProviderNodes() -> impl IntoView {
    let (list, set_list) = signal(Hydrate::<NodesList>::Loading);
    let (creating, set_creating) = signal(false);
    let (editing, set_editing) = signal(None::<NodeRow>);
    let (deleting, set_deleting) = signal(None::<NodeRow>);
    let reload = move || {
        set_list.set(Hydrate::Loading);
        load("/api/provider-nodes", set_list);
    };
    reload();

    view! {
        <PageHeader title=t!("nav.nodes") description=t!("nodes.description")>
            <Button
                label=t!("nodes.create")
                size=ButtonSize::Sm
                on_click=Callback::new(move |()| set_creating.set(true))
            />
        </PageHeader>

        <PageBody>
            <Panel
                state=list
                on_retry=Callback::new(move |()| reload())
                children=move |data: NodesList| {
                    view! {
                        <NodeTable
                            rows=data.nodes
                            set_edit=set_editing
                            set_delete=set_deleting
                        />
                    }
                }
            />

            // Stated rather than offered as a button: the validate route answers `valid: false` for
            // every input, so a "test connection" control could only ever report a false negative.
            <Alert variant=AlertVariant::Muted>
                <AlertTitle>{t!("nodes.validation_title")}</AlertTitle>
                <AlertDescription>{t!("nodes.validate_unsupported")}</AlertDescription>
            </Alert>
        </PageBody>

        <CreateDialog open=creating set_open=set_creating reload=reload />

        // Mounted only while a node is selected: the form's signals are seeded from that node, so
        // there is no stale copy to keep in step when the list is refetched underneath it.
        {move || {
            editing
                .get()
                .map(|row| {
                    view! {
                        <EditDialog row=row set_target=set_editing reload=reload />
                    }
                })
        }}
        {move || {
            deleting
                .get()
                .map(|row| {
                    view! {
                        <DeleteDialog row=row set_target=set_deleting reload=reload />
                    }
                })
        }}
    }
}

/// Add a node.
///
/// The family select is offered here and nowhere else: `PUT` reads the family from the stored node, so
/// an edit form that showed it would appear to change something the server ignores.
#[component]
fn CreateDialog(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
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
    let encode_failed = StoredValue::new(t!("nodes.encode_failed"));

    // A `Signal` rather than a vec built inline: `Select` reads its options reactively, and a fresh
    // `Vec` per render is what allows the labels to follow a locale swap.
    let kind_options = Signal::derive(|| {
        vec![
            (OPENAI_COMPATIBLE.to_owned(), t!("nodes.type_openai")),
            (
                "anthropic-compatible".to_owned(),
                t!("nodes.type_anthropic"),
            ),
            ("custom-embedding".to_owned(), t!("nodes.type_embedding")),
        ]
    });
    let api_options = Signal::derive(|| {
        API_TYPES
            .into_iter()
            .map(|value| (value.to_owned(), value.to_owned()))
            .collect::<Vec<_>>()
    });

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
            set_save.set(Save::Refused(encode_failed.get_value()));
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
                set_open.set(false);
                reload();
            },
        );
    };

    view! {
        <Dialog
            open=Signal::derive(move || open.get())
            on_open_change=Callback::new(move |next: bool| set_open.set(next))
        >
            <DialogHeader>
                <div class="space-y-1">
                    <DialogTitle>{t!("nodes.create")}</DialogTitle>
                    <DialogDescription>{t!("nodes.create_hint")}</DialogDescription>
                </div>
                <DialogDismiss />
            </DialogHeader>
            <DialogContent class="space-y-3">
                <div class="grid gap-3 sm:grid-cols-2">
                    <Field label=t!("nodes.name") required=true>
                        <Input
                            value=Signal::derive(move || name.get())
                            on_input=Callback::new(move |value| set_name.set(value))
                            placeholder=t!("nodes.name_placeholder")
                            autocomplete="off"
                        />
                    </Field>
                    <Field
                        label=t!("nodes.prefix")
                        description=t!("nodes.prefix_hint")
                        required=true
                    >
                        <Input
                            value=Signal::derive(move || prefix.get())
                            on_input=Callback::new(move |value| set_prefix.set(value))
                            placeholder=t!("nodes.prefix_placeholder")
                            autocomplete="off"
                            class="font-mono"
                        />
                    </Field>
                    <Field label=t!("nodes.type") required=true>
                        <Select
                            value=Signal::derive(move || kind.get())
                            options=kind_options
                            on_change=Callback::new(move |chosen| set_kind.set(chosen))
                            aria_label=t!("nodes.type")
                        />
                    </Field>
                    {move || {
                        shows_api_type()
                            .then(|| {
                                view! {
                                    <Field label=t!("nodes.api_type") required=true>
                                        <Select
                                            value=Signal::derive(move || api_type.get())
                                            options=api_options
                                            on_change=Callback::new(move |chosen| {
                                                set_api_type.set(chosen);
                                            })
                                            aria_label=t!("nodes.api_type")
                                        />
                                    </Field>
                                }
                            })
                    }}
                </div>
                <Field
                    label=t!("nodes.base_url")
                    description=t!("nodes.base_url_hint")
                >
                    <Input
                        value=Signal::derive(move || base_url.get())
                        on_input=Callback::new(move |value| set_base_url.set(value))
                        placeholder=t!("nodes.base_url_placeholder")
                        autocomplete="off"
                        class="font-mono"
                    />
                </Field>
                {move || {
                    save.get().message().map(|message| view! { <ErrorNote message=message /> })
                }}
            </DialogContent>
            <DialogFooter>
                <Button
                    label=t!("nodes.cancel")
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    on_click=Callback::new(move |()| set_open.set(false))
                />
                <Button
                    label=t!("nodes.create")
                    size=ButtonSize::Sm
                    disabled=Signal::derive(incomplete)
                    busy=Signal::derive(move || save.get().is_saving())
                    on_click=Callback::new(move |()| submit())
                />
            </DialogFooter>
        </Dialog>
    }
}

#[component]
fn NodeTable(
    rows: Vec<NodeRow>,
    set_edit: WriteSignal<Option<NodeRow>>,
    set_delete: WriteSignal<Option<NodeRow>>,
) -> impl IntoView {
    if rows.is_empty() {
        return view! {
            <TableFrame>
                <Table>
                    <TableEmpty
                        message=t!("nodes.empty")
                        columns=6
                        hint=t!("nodes.empty_hint")
                    />
                </Table>
            </TableFrame>
        }
        .into_any();
    }
    view! {
        <TableFrame>
            <Table>
                <TableHead>
                    <tr>
                        <TableHeaderCell>{t!("nodes.col_name")}</TableHeaderCell>
                        <TableHeaderCell>{t!("nodes.col_prefix")}</TableHeaderCell>
                        <TableHeaderCell>{t!("nodes.col_type")}</TableHeaderCell>
                        <TableHeaderCell>{t!("nodes.col_url")}</TableHeaderCell>
                        <TableHeaderCell>{t!("nodes.col_updated")}</TableHeaderCell>
                        <TableHeaderCell class="text-right">{t!("table.actions")}</TableHeaderCell>
                    </tr>
                </TableHead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| {
                            view! {
                                <NodeLine row=row set_edit=set_edit set_delete=set_delete />
                            }
                        })
                        .collect_view()}
                </tbody>
            </Table>
        </TableFrame>
    }
    .into_any()
}

/// One node's stored values, and the menu of writes the store accepts for it.
///
/// The writes live in dialogs mounted at page level rather than here, because a `<dialog>` cannot be
/// a child of `<tbody>` -- it would be hoisted out of the table by the parser and lose its position in
/// the row's lifetime. The row therefore only *names* the node it wants to act on, and the dialog that
/// receives it is created fresh from that node's values.
#[component]
fn NodeLine(
    row: NodeRow,
    set_edit: WriteSignal<Option<NodeRow>>,
    set_delete: WriteSignal<Option<NodeRow>>,
) -> impl IntoView {
    let family = row.node_type.clone();
    let for_edit = StoredValue::new(row.clone());
    let for_delete = StoredValue::new(row.clone());

    view! {
        <TableRow class="align-top">
            <TableCell class="font-medium">{row.name.clone()}</TableCell>
            <TableCell>
                <code class="font-mono text-xs text-muted-foreground">{row.prefix.clone()}</code>
            </TableCell>
            <TableCell>
                <div class="space-y-0.5">
                    <span>{type_label(&family)}</span>
                    {row.api_type
                        .clone()
                        .filter(|value| !value.is_empty())
                        .map(|value| {
                            view! {
                                <p class="font-mono text-xs text-muted-foreground">{value}</p>
                            }
                        })}
                </div>
            </TableCell>
            <TableCell>
                <Mono value=row.base_url.clone() class="text-xs" />
            </TableCell>
            <TableCell>
                <span class="whitespace-nowrap font-mono text-xs tabular-nums text-muted-foreground">
                    {timestamp_label(&row.updated_at)}
                </span>
            </TableCell>
            <TableCell class="text-right">
                <Menu
                    aria_label=t!("table.actions")
                    icon=IconName::More
                    align_end=true
                    trigger_variant="ghost"
                >
                    <MenuItem
                        label=t!("nodes.edit")
                        icon=IconName::Pencil
                        on_select=Callback::new(move |()| {
                            set_edit.set(Some(for_edit.get_value()));
                        })
                    />
                    <MenuSeparator />
                    <MenuItem
                        label=t!("nodes.delete")
                        icon=IconName::Trash
                        destructive=true
                        on_select=Callback::new(move |()| {
                            set_delete.set(Some(for_delete.get_value()));
                        })
                    />
                </Menu>
            </TableCell>
        </TableRow>
    }
}

/// Edit one node.
///
/// Created only while a node is selected, so its field signals are seeded from that node and there is
/// no "which node is this form about?" state to keep in sync.
#[component]
fn EditDialog(
    row: NodeRow,
    set_target: WriteSignal<Option<NodeRow>>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (name, set_name) = signal(row.name.clone());
    let (prefix, set_prefix) = signal(row.prefix.clone());
    let (base_url, set_base_url) = signal(row.base_url.clone());
    let (api_type, set_api_type) =
        signal(row.api_type.clone().unwrap_or_else(|| "chat".to_owned()));
    let (save_state, set_save) = signal(Save::Idle);

    let has_api_type = takes_api_type(&row.node_type);
    // Stored rather than captured: the write is invoked from inside a reactive closure, which rebuilds
    // its handler on every run and so needs a `Copy` closure.
    let save_family = StoredValue::new(row.node_type.clone());
    let save_id = StoredValue::new(row.id.clone());
    let all_required = StoredValue::new(t!("nodes.all_required"));
    let encode_failed = StoredValue::new(t!("nodes.encode_failed"));

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
                set_target.set(None);
                reload();
            },
        );
    };

    let api_options = Signal::derive(|| {
        API_TYPES
            .into_iter()
            .map(|value| (value.to_owned(), value.to_owned()))
            .collect::<Vec<_>>()
    });
    let fixed_family = StoredValue::new(row.node_type);

    view! {
        <Dialog
            open=Signal::derive(|| true)
            on_open_change=Callback::new(move |next: bool| {
                if !next {
                    set_target.set(None);
                }
            })
        >
            <DialogHeader>
                <div class="space-y-1">
                    <DialogTitle>{t!("nodes.edit")}</DialogTitle>
                    <DialogDescription>{t!("nodes.edit_hint")}</DialogDescription>
                </div>
                <DialogDismiss />
            </DialogHeader>
            <DialogContent class="space-y-3">
                <div class="grid gap-3 sm:grid-cols-2">
                    <Field label=t!("nodes.name") required=true>
                        <Input
                            value=Signal::derive(move || name.get())
                            on_input=Callback::new(move |value| set_name.set(value))
                        />
                    </Field>
                    <Field label=t!("nodes.prefix") required=true>
                        <Input
                            value=Signal::derive(move || prefix.get())
                            on_input=Callback::new(move |value| set_prefix.set(value))
                            class="font-mono"
                        />
                    </Field>
                </div>
                // Type is stated, not offered: `PUT` reads the family from the stored node and ignores
                // the body's, so a control here would look like it changed something it cannot.
                <div class="space-y-1">
                    <p class="text-[0.8125rem] font-medium text-foreground">{t!("nodes.type")}</p>
                    <p class="text-sm text-foreground">
                        {move || type_label(&fixed_family.get_value())}
                    </p>
                    <p class="text-xs leading-relaxed text-muted-foreground">
                        {t!("nodes.type_locked")}
                    </p>
                </div>
                {has_api_type
                    .then(|| {
                        view! {
                            <Field label=t!("nodes.api_type") class="sm:max-w-xs">
                                <Select
                                    value=Signal::derive(move || api_type.get())
                                    options=api_options
                                    on_change=Callback::new(move |chosen| {
                                        set_api_type.set(chosen);
                                    })
                                    aria_label=t!("nodes.api_type")
                                />
                            </Field>
                        }
                    })}
                <Field
                    label=t!("nodes.base_url")
                    description=t!("nodes.base_url_required")
                    required=true
                >
                    <Input
                        value=Signal::derive(move || base_url.get())
                        on_input=Callback::new(move |value| set_base_url.set(value))
                        class="font-mono"
                    />
                </Field>
                {move || {
                    save_state
                        .get()
                        .message()
                        .map(|message| view! { <ErrorNote message=message /> })
                }}
            </DialogContent>
            <DialogFooter>
                <Button
                    label=t!("nodes.cancel")
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    on_click=Callback::new(move |()| set_target.set(None))
                />
                <Button
                    label=t!("nodes.save")
                    size=ButtonSize::Sm
                    disabled=Signal::derive(move || save_state.get().is_saving())
                    busy=Signal::derive(move || save_state.get().is_saving())
                    on_click=Callback::new(move |()| save())
                />
            </DialogFooter>
        </Dialog>
    }
}

/// Remove one node, after a confirmation that names what it takes with it.
#[component]
fn DeleteDialog(
    row: NodeRow,
    set_target: WriteSignal<Option<NodeRow>>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (save_state, set_save) = signal(Save::Idle);
    let save_id = StoredValue::new(row.id.clone());

    let remove = move || {
        if save_state.get().is_saving() {
            return;
        }
        let path = format!("/api/provider-nodes/{}", save_id.get_value());
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Delete, &path, None).await },
            move |_| {
                set_target.set(None);
                reload();
            },
        );
    };

    view! {
        <Dialog
            open=Signal::derive(|| true)
            on_open_change=Callback::new(move |next: bool| {
                if !next {
                    set_target.set(None);
                }
            })
        >
            <DialogHeader>
                <div class="space-y-1">
                    <DialogTitle>{t!("nodes.confirm_delete")}</DialogTitle>
                    <DialogDescription>{t!("nodes.delete_hint")}</DialogDescription>
                </div>
                <DialogDismiss />
            </DialogHeader>
            <DialogContent class="space-y-3">
                <p class="text-sm text-foreground">{row.name.clone()}</p>
                <Readback value=row.base_url.clone() />
                {move || {
                    save_state
                        .get()
                        .message()
                        .map(|message| view! { <ErrorNote message=message /> })
                }}
            </DialogContent>
            <DialogFooter>
                <Button
                    label=t!("nodes.cancel")
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    on_click=Callback::new(move |()| set_target.set(None))
                />
                <Button
                    label=t!("nodes.delete")
                    variant=ButtonVariant::Destructive
                    size=ButtonSize::Sm
                    disabled=Signal::derive(move || save_state.get().is_saving())
                    busy=Signal::derive(move || save_state.get().is_saving())
                    on_click=Callback::new(move |()| remove())
                />
            </DialogFooter>
        </Dialog>
    }
}

/// The node's endpoint, shown read-only in a confirmation so the operator can check which one it is.
#[component]
fn Readback(value: String) -> impl IntoView {
    view! {
        <Mono
            value=value
            class="block rounded-md border border-border bg-muted/40 px-2.5 py-1.5 text-xs"
        />
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
