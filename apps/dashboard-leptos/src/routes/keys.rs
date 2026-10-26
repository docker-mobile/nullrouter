//! Managed API keys, used when the router requires one.
//!
//! # The one thing this page must not get wrong
//!
//! The secret is returned once, by the create call, and never again. There is no way to reveal it
//! afterwards because the store does not keep it -- only a hash. So the created key is shown in a place
//! that cannot be missed and cannot be dismissed by accident, and the copy button is offered before the
//! paragraph explaining why.

use leptos::prelude::*;

use crate::t;

use crate::api::{Hydrate, Save, decode, encode, load, post, put, submit};
use crate::routes::types::{ApiKeyRow, CreateKeyBody, CreatedKey, KeysList, UpdateKeyBody};
use crate::routes::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Badge, BadgeVariant, Button, ButtonSize,
    ButtonVariant, Card, CardContent, CardHeader, CardTitle, ErrorNote, Icon, IconName, Input,
    PageBody, PageHeader, Panel, Table, TableCell, TableEmpty, TableFrame, TableHead,
    TableHeaderCell, TableRow,
};

#[allow(
    clippy::too_many_lines,
    reason = "create form, the one-time secret and the key table are one page's subject"
)]
#[component]
pub fn Keys() -> impl IntoView {
    let (keys, set_keys) = signal(Hydrate::<KeysList>::Loading);
    let (save, set_save) = signal(Save::Idle);
    let (name, set_name) = signal(String::new());
    let (created_secret, set_created_secret) = signal(None::<String>);

    let reload = move || {
        set_keys.set(Hydrate::Loading);
        load("/api/keys", set_keys);
    };
    reload();

    let create = move || {
        let label = name.get();
        if label.trim().is_empty() {
            return;
        }
        let Ok(body) = encode(&CreateKeyBody { name: label.trim() }) else {
            return;
        };
        submit(
            set_save,
            move || async move { post("/api/keys", &body).await },
            move |response| {
                // Decoded to confirm the shape is a created key before putting its secret on screen:
                // a body that decoded to something else would otherwise render an empty code block and
                // read as a key of zero length.
                if let Ok(created) = decode::<CreatedKey>(&response) {
                    set_created_secret.set(Some(created.key.key));
                    set_name.set(String::new());
                    reload();
                }
            },
        );
    };

    // Hoisted: `view!` moves anything it references, so a value used both in the tree and inside a
    // closure has to be split before the tree is built.
    let heading = t!("nav.keys");
    let blurb = t!("keys.description");
    let create_action = t!("keys.create");
    let create_hint = t!("keys.create_hint");
    let name_placeholder = t!("keys.name_placeholder");
    let created_once = t!("keys.created_once");
    // A second binding: the success card and the create card both title themselves with this word, and
    // `view!` moves each binding into the closure that reads it.
    let create_card_title = created_once.clone();
    let stored_title = t!("keys.stored_warning");
    let stored_body = t!("keys.stored_warning_body");
    let copy_label = t!("action.copy");

    view! {
        <PageHeader title=heading description=blurb />

        <PageBody>
            {move || {
                let title = created_once.clone();
                let stored_title = stored_title.clone();
                let stored_body = stored_body.clone();
                let copy_label = copy_label.clone();
                created_secret
                    .get()
                    .map(|secret| {
                        let value = secret.clone();
                        view! {
                            <Card class="border-success/40">
                                <CardHeader>
                                    <CardTitle class="flex items-center gap-1.5 text-success">
                                        <Icon name=IconName::Check class="size-4" />
                                        {title.clone()}
                                    </CardTitle>
                                </CardHeader>
                                <CardContent class="space-y-3">
                                    <div class="flex flex-col gap-2 sm:flex-row">
                                        <code class="min-w-0 flex-1 overflow-x-auto rounded-md border \
                                                     border-border bg-muted/50 px-3 py-2 font-mono \
                                                     text-[0.8125rem] whitespace-nowrap">
                                            {secret.clone()}
                                        </code>
                                        <button
                                            type="button"
                                            class=crate::ui::button_class(
                                                ButtonVariant::Outline,
                                                ButtonSize::Default,
                                            )
                                            on:click=move |_| copy_to_clipboard(&value)
                                        >
                                            <Icon name=IconName::Copy class="size-4" />
                                            {copy_label.clone()}
                                        </button>
                                    </div>
                                    <Alert variant=AlertVariant::Warning>
                                        <AlertTitle>{stored_title.clone()}</AlertTitle>
                                        <AlertDescription>{stored_body.clone()}</AlertDescription>
                                    </Alert>
                                </CardContent>
                            </Card>
                        }
                    })
            }}

            <Card>
                <CardHeader>
                    <CardTitle>{create_card_title}</CardTitle>
                </CardHeader>
                <CardContent class="space-y-3">
                    <p class="text-[0.8125rem] leading-relaxed text-muted-foreground">
                        {create_hint}
                    </p>
                    <form
                        class="flex flex-col gap-2 sm:flex-row"
                        on:submit=move |event| {
                            event.prevent_default();
                            create();
                        }
                    >
                        <label class="min-w-0 flex-1">
                            <span class="sr-only">{name_placeholder.clone()}</span>
                            <Input
                                value=Signal::derive(move || name.get())
                                on_input=Callback::new(move |value| set_name.set(value))
                                placeholder=name_placeholder.clone()
                                class="h-9"
                            />
                        </label>
                        <Button
                            label=create_action
                            button_type="submit"
                            size=ButtonSize::Default
                            disabled=Signal::derive(move || {
                                save.get().is_saving() || name.get().trim().is_empty()
                            })
                            busy=Signal::derive(move || save.get().is_saving())
                        />
                    </form>
                    {move || {
                        save
                            .get()
                            .message()
                            .map(|message| view! { <ErrorNote message=message /> })
                    }}
                </CardContent>
            </Card>

            <Panel
                state=keys
                on_retry=Callback::new(move |()| reload())
                children=move |data: KeysList| view! { <KeysTable rows=data.keys reload=reload /> }
            />
        </PageBody>
    }
}

/// Copy a string the user cannot see again.
///
/// Fire-and-forget: the clipboard API is refused outside a secure context and on a permission prompt
/// the user dismisses, and there is nothing useful to say about either failure here -- the key is on
/// screen and can be selected by hand.
#[allow(
    clippy::missing_const_for_fn,
    reason = "calls into JS on wasm32, which is not const"
)]
fn copy_to_clipboard(value: &str) {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let _ = window.navigator().clipboard().write_text(value);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = value;
}

#[component]
fn KeysTable(
    rows: Vec<ApiKeyRow>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    // `t!` reads the locale at each use, so the labels need no shared binding -- but the empty branch
    // returns before the table branch builds its tree, and a `move`-captured locale would not survive that
    // boundary. Reading each label on its own line sidesteps it entirely.
    let empty = t!("keys.empty");
    let empty_hint = t!("keys.empty_hint");
    let col_name = t!("keys.col_name");
    let col_id = t!("keys.col_id");
    let col_status = t!("keys.col_status");
    let col_created = t!("keys.col_created");
    let col_actions = t!("table.actions");

    if rows.is_empty() {
        return view! {
            <TableFrame>
                <Table>
                    <TableEmpty message=empty columns=4 hint=empty_hint />
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
                        <TableHeaderCell>{col_name}</TableHeaderCell>
                        <TableHeaderCell>{col_id}</TableHeaderCell>
                        <TableHeaderCell>{col_status}</TableHeaderCell>
                        <TableHeaderCell>{col_created}</TableHeaderCell>
                        <TableHeaderCell class="text-right">{col_actions}</TableHeaderCell>
                    </tr>
                </TableHead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| view! { <KeyRow row=row reload=reload /> })
                        .collect_view()}
                </tbody>
            </Table>
        </TableFrame>
    }
    .into_any()
}

#[component]
fn KeyRow(row: ApiKeyRow, reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let (save, set_save) = signal(Save::Idle);
    let active = row.is_active;
    let id = row.id.clone();
    let enabled = t!("state.enabled");
    let disabled = t!("state.disabled");
    let toggle_label = if active {
        t!("keys.disable")
    } else {
        t!("keys.enable")
    };

    let toggle = move || {
        let path = format!("/api/keys/{id}");
        let Ok(body) = encode(&UpdateKeyBody { is_active: !active }) else {
            return;
        };
        submit(
            set_save,
            move || async move { put(&path, &body).await },
            move |_| reload(),
        );
    };

    view! {
        <TableRow>
            <TableCell class="font-medium">{row.name}</TableCell>
            <TableCell>
                <code class="font-mono text-xs text-muted-foreground">{row.id.clone()}</code>
            </TableCell>
            <TableCell>
                <Badge variant=if active { BadgeVariant::Success } else { BadgeVariant::Secondary }>
                    {if active { enabled } else { disabled }}
                </Badge>
            </TableCell>
            <TableCell>
                <span class="whitespace-nowrap font-mono text-xs text-muted-foreground">
                    {row.created_at}
                </span>
            </TableCell>
            <TableCell class="text-right">
                <button
                    type="button"
                    class=crate::ui::button_class(ButtonVariant::Ghost, ButtonSize::Sm)
                    disabled=move || save.get().is_saving()
                    on:click=move |_| toggle()
                >
                    {toggle_label}
                </button>
            </TableCell>
        </TableRow>
    }
}
