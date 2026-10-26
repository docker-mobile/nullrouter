//! Accounts that can sign in, and what each may do.
//!
//! The gateway is what enforces the roles: it holds every request to a minimum role, and `/api/users`
//! is admin-only there. This panel is the surface, not the control. Two consequences worth naming,
//! because both look like bugs otherwise:
//!
//! * a non-admin sees the refusal rather than a hidden page. Hiding it would suggest the section does
//!   not exist, and an operator wondering why they cannot find user management is worse served than one
//!   told they lack the role;
//! * every refusal here is the server's own sentence. The store enforces rules the client cannot --
//!   a username is unique, the last admin cannot be removed, disabled, or demoted -- and each arrives
//!   as a 400 or a 409 with a message worth reading.
//!
//! # Why the table is the page
//!
//! The previous version opened with a create form occupying the top third of the screen and put every
//! per-account action inline: a role `<select>`, a password box and a "Set password" link, an
//! enable/disable link and a delete link, all stacked in the last cell. Reading *who exists* meant
//! reading around thirteen controls. The list is what this page is for, so it comes first, the create
//! form is a dialog opened from the heading, and the row actions are behind one menu. Password and
//! delete each need input or confirmation, so they open a dialog of their own rather than living in a
//! table cell -- and a delete that changes the last-admin rule is worth confirming once.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api::{Hydrate, Method, Save, encode, load, request_detailed, submit_reporting};
use crate::routes::types::{encode_query, timestamp_label};
use crate::routes::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Badge, BadgeVariant, Button, ButtonSize,
    ButtonVariant, Dialog, DialogContent, DialogDescription, DialogDismiss, DialogFooter,
    DialogHeader, DialogTitle, ErrorNote, Field, IconName, Input, Menu, MenuItem, MenuSeparator,
    PageBody, PageHeader, Panel, Select, Table, TableCell, TableEmpty, TableFrame, TableHead,
    TableHeaderCell, TableRow,
};
use crate::t;

/// `GET /api/users`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsersList {
    #[serde(default)]
    users: Vec<UserRow>,
    /// True while no account exists, which is when the legacy shared password still works.
    #[serde(default)]
    shared_password_active: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserRow {
    #[serde(default)]
    id: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    is_active: bool,
    #[serde(default)]
    last_login_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateUser<'a> {
    username: &'a str,
    password: &'a str,
    role: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<&'a str>,
}

/// A partial update. Every field is optional, and an omitted one is left alone by the store.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateUser {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<String>,
}

/// The account a dialog was opened for.
///
/// `id` is what the request goes to, `username` is what the dialog says it is about. Held together so a
/// dialog cannot be given one account's id and another's name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct AccountTarget {
    id: String,
    username: String,
}

impl AccountTarget {
    fn of(row: &UserRow) -> Self {
        Self {
            id: row.id.clone(),
            username: row.username.clone(),
        }
    }

    /// The path a write to this account goes to.
    fn path(&self) -> String {
        format!("/api/users/{}", encode_query(&self.id))
    }
}

#[component]
pub fn Users() -> impl IntoView {
    let (list, set_list) = signal(Hydrate::<UsersList>::Loading);
    let (creating, set_creating) = signal(false);
    let (password_target, set_password_target) = signal(None::<AccountTarget>);
    let (delete_target, set_delete_target) = signal(None::<AccountTarget>);
    let reload = move || {
        set_list.set(Hydrate::Loading);
        load("/api/users", set_list);
    };
    reload();

    view! {
        <PageHeader title=t!("nav.users") description=t!("users.description")>
            <Button
                label=t!("users.create")
                size=ButtonSize::Sm
                on_click=Callback::new(move |()| set_creating.set(true))
            />
        </PageHeader>

        <PageBody>
            <Panel
                state=list
                on_retry=Callback::new(move |()| reload())
                children=move |data: UsersList| {
                    view! {
                        <Body
                            data=data
                            reload=reload
                            set_password=set_password_target
                            set_delete=set_delete_target
                        />
                    }
                }
            />
        </PageBody>

        <CreateDialog open=creating set_open=set_creating reload=reload />
        <PasswordDialog
            target=password_target
            set_target=set_password_target
            reload=reload
        />
        <DeleteDialog target=delete_target set_target=set_delete_target reload=reload />
    }
}

#[component]
fn Body(
    data: UsersList,
    reload: impl Fn() + Copy + 'static + Send + Sync,
    set_password: WriteSignal<Option<AccountTarget>>,
    set_delete: WriteSignal<Option<AccountTarget>>,
) -> impl IntoView {
    view! {
        {data
            .shared_password_active
            .then(|| {
                view! {
                    <Alert variant=AlertVariant::Warning>
                        <AlertTitle>{t!("users.shared_password_title")}</AlertTitle>
                        <AlertDescription>{t!("users.shared_password_notice")}</AlertDescription>
                    </Alert>
                }
            })}
        <UserTable rows=data.users reload=reload set_password=set_password set_delete=set_delete />
    }
}

/// The accounts, one row each.
#[component]
fn UserTable(
    rows: Vec<UserRow>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
    set_password: WriteSignal<Option<AccountTarget>>,
    set_delete: WriteSignal<Option<AccountTarget>>,
) -> impl IntoView {
    if rows.is_empty() {
        return view! {
            <TableFrame>
                <Table>
                    <TableEmpty message=t!("users.empty") columns=5 hint=t!("users.empty_hint") />
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
                        <TableHeaderCell>{t!("users.col_user")}</TableHeaderCell>
                        <TableHeaderCell>{t!("users.col_role")}</TableHeaderCell>
                        <TableHeaderCell>{t!("users.col_status")}</TableHeaderCell>
                        <TableHeaderCell>{t!("users.col_last_login")}</TableHeaderCell>
                        <TableHeaderCell class="text-right">{t!("table.actions")}</TableHeaderCell>
                    </tr>
                </TableHead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| {
                            view! {
                                <Row
                                    row=row
                                    reload=reload
                                    set_password=set_password
                                    set_delete=set_delete
                                />
                            }
                        })
                        .collect_view()}
                </tbody>
            </Table>
        </TableFrame>
    }
    .into_any()
}

/// One account: role, state and the menu of writes the store accepts for it.
#[component]
fn Row(
    row: UserRow,
    reload: impl Fn() + Copy + 'static + Send + Sync,
    set_password: WriteSignal<Option<AccountTarget>>,
    set_delete: WriteSignal<Option<AccountTarget>>,
) -> impl IntoView {
    let (save, set_save) = signal(Save::Idle);

    let active = row.is_active;
    // `Copy` wrappers for everything a rebuildable handler below needs: the menu's items are built
    // inside an `Fn` closure that runs on every open, so its captures have to survive more than one run.
    let target = StoredValue::new(AccountTarget::of(&row));
    let encode_failed = StoredValue::new(t!("users.encode_failed"));
    let last_login = row
        .last_login_at
        .as_deref()
        .map_or_else(|| t!("users.never"), timestamp_label);
    let current_role = StoredValue::new(row.role.clone());

    let send = move |update: UpdateUser| {
        if save.get().is_saving() {
            return;
        }
        let Ok(encoded) = encode(&update) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };
        let path = target.get_value().path();
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Put, &path, Some(&encoded)).await },
            move |_| reload(),
        );
    };

    let toggle = move || {
        send(UpdateUser {
            is_active: Some(!active),
            ..UpdateUser::default()
        });
    };

    let role_options = Signal::derive(|| {
        vec![
            ("admin".to_owned(), t!("users.role_admin_short")),
            ("operator".to_owned(), t!("users.role_operator_short")),
            ("viewer".to_owned(), t!("users.role_viewer_short")),
        ]
    });

    view! {
        <TableRow class="align-top">
            <TableCell>
                <span class="block font-medium text-foreground">
                    {if row.display_name.is_empty() {
                        row.username.clone()
                    } else {
                        row.display_name.clone()
                    }}
                </span>
                <code class="block font-mono text-xs text-muted-foreground">{row.username.clone()}</code>
                {(!row.email.is_empty())
                    .then(|| {
                        view! {
                            <span class="block text-xs text-muted-foreground">{row.email.clone()}</span>
                        }
                    })}
            </TableCell>
            <TableCell>
                <Select
                    value=Signal::derive(move || current_role.get_value())
                    options=role_options
                    on_change=Callback::new(move |chosen| {
                        send(UpdateUser {
                            role: Some(chosen),
                            ..UpdateUser::default()
                        });
                    })
                    disabled=Signal::derive(move || save.get().is_saving())
                    aria_label=t!("users.col_role")
                    class="w-36"
                />
            </TableCell>
            <TableCell>
                <Badge variant=if active { BadgeVariant::Success } else { BadgeVariant::Secondary }>
                    {if active { t!("state.enabled") } else { t!("state.disabled") }}
                </Badge>
                {move || {
                    save.get().message().map(|message| view! { <ErrorNote message=message class="mt-1.5" /> })
                }}
            </TableCell>
            <TableCell>
                <span class="whitespace-nowrap font-mono text-xs text-muted-foreground tabular-nums">
                    {last_login}
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
                        label=t!("users.set_password")
                        icon=IconName::Key
                        on_select=Callback::new(move |()| {
                            set_password.set(Some(target.get_value()));
                        })
                    />
                    <MenuItem
                        label=if active {
                            t!("users.disable")
                        } else {
                            t!("users.enable")
                        }
                        icon=IconName::Power
                        on_select=Callback::new(move |()| toggle())
                    />
                    <MenuSeparator />
                    <MenuItem
                        label=t!("users.delete")
                        icon=IconName::Trash
                        destructive=true
                        on_select=Callback::new(move |()| {
                            set_delete.set(Some(target.get_value()));
                        })
                    />
                </Menu>
            </TableCell>
        </TableRow>
    }
}

/// Create an account.
///
/// A dialog rather than a form at the top of the page: it is used once per account and then not again,
/// and while it was inline it pushed the list -- the thing the page is for -- below the fold.
#[component]
fn CreateDialog(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (username, set_username) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (display, set_display) = signal(String::new());
    let (email, set_email) = signal(String::new());
    // Operator by default: it is the role most accounts need, and defaulting to admin would make the
    // safe choice the deliberate one.
    let (role, set_role) = signal("operator".to_owned());
    let (save, set_save) = signal(Save::Idle);
    let encode_failed = StoredValue::new(t!("users.encode_failed"));

    let role_options = Signal::derive(|| {
        vec![
            ("admin".to_owned(), t!("users.role_admin")),
            ("operator".to_owned(), t!("users.role_operator")),
            ("viewer".to_owned(), t!("users.role_viewer")),
        ]
    });

    let create = move || {
        let name = username.get().trim().to_owned();
        let secret = password.get();
        if save.get().is_saving() || name.is_empty() || secret.is_empty() {
            return;
        }
        let display_name = display.get().trim().to_owned();
        let address = email.get().trim().to_owned();
        let chosen = role.get();
        let body = CreateUser {
            username: &name,
            password: &secret,
            role: &chosen,
            display_name: (!display_name.is_empty()).then_some(display_name.as_str()),
            email: (!address.is_empty()).then_some(address.as_str()),
        };
        let Ok(encoded) = encode(&body) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Post, "/api/users", Some(&encoded)).await },
            move |_| {
                set_username.set(String::new());
                set_password.set(String::new());
                set_display.set(String::new());
                set_email.set(String::new());
                set_open.set(false);
                reload();
            },
        );
    };

    view! {
        <Dialog
            open=Signal::derive(move || open.get())
            on_open_change=Callback::new(move |next| set_open.set(next))
        >
            <DialogHeader>
                <div class="space-y-1">
                    <DialogTitle>{t!("users.create")}</DialogTitle>
                    <DialogDescription>{t!("users.create_hint")}</DialogDescription>
                </div>
                <DialogDismiss />
            </DialogHeader>
            <DialogContent class="space-y-3">
                <div class="grid gap-3 sm:grid-cols-2">
                    <Field label=t!("users.field_username") required=true>
                        <Input
                            value=Signal::derive(move || username.get())
                            on_input=Callback::new(move |value| set_username.set(value))
                            placeholder=t!("users.username_placeholder")
                            autocomplete="off"
                            class="font-mono"
                        />
                    </Field>
                    <Field label=t!("users.field_password") required=true>
                        <Input
                            input_type="password"
                            value=Signal::derive(move || password.get())
                            on_input=Callback::new(move |value| set_password.set(value))
                            placeholder=t!("users.password_placeholder")
                            autocomplete="new-password"
                        />
                    </Field>
                    <Field label=t!("users.field_display_name")>
                        <Input
                            value=Signal::derive(move || display.get())
                            on_input=Callback::new(move |value| set_display.set(value))
                            placeholder=t!("users.display_name_placeholder")
                            autocomplete="off"
                        />
                    </Field>
                    <Field label=t!("users.field_email")>
                        <Input
                            input_type="email"
                            value=Signal::derive(move || email.get())
                            on_input=Callback::new(move |value| set_email.set(value))
                            placeholder=t!("users.email_placeholder")
                            autocomplete="off"
                        />
                    </Field>
                </div>
                <Field
                    label=t!("users.field_role")
                    description=t!("users.role_hint")
                    class="sm:max-w-sm"
                >
                    <Select
                        value=Signal::derive(move || role.get())
                        options=role_options
                        on_change=Callback::new(move |chosen| set_role.set(chosen))
                        aria_label=t!("users.field_role")
                    />
                </Field>
                {move || {
                    save.get().message().map(|message| view! { <ErrorNote message=message /> })
                }}
            </DialogContent>
            <DialogFooter>
                <Button
                    label=t!("users.cancel")
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    on_click=Callback::new(move |()| set_open.set(false))
                />
                <Button
                    label=t!("users.create")
                    size=ButtonSize::Sm
                    disabled=Signal::derive(move || {
                        save.get().is_saving() || username.get().trim().is_empty()
                            || password.get().is_empty()
                    })
                    busy=Signal::derive(move || save.get().is_saving())
                    on_click=Callback::new(move |()| create())
                />
            </DialogFooter>
        </Dialog>
    }
}

/// Set a new password for one account.
///
/// Its own dialog rather than a box in the row: a password box in a table cell is a password box
/// rendered at 12px next to a delete link, and the write it performs is the one that must not be
/// mistyped.
#[component]
fn PasswordDialog(
    target: ReadSignal<Option<AccountTarget>>,
    set_target: WriteSignal<Option<AccountTarget>>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (password, set_password) = signal(String::new());
    let (save, set_save) = signal(Save::Idle);
    let encode_failed = StoredValue::new(t!("users.encode_failed"));

    let submit = move || {
        let Some(account) = target.get_untracked() else {
            return;
        };
        if save.get().is_saving() || password.get().is_empty() {
            return;
        }
        let Ok(body) = encode(&UpdateUser {
            password: Some(password.get()),
            ..UpdateUser::default()
        }) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };
        let path = account.path();
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Put, &path, Some(&body)).await },
            move |_| {
                set_password.set(String::new());
                set_target.set(None);
                reload();
            },
        );
    };

    view! {
        <Dialog
            open=Signal::derive(move || target.get().is_some())
            on_open_change=Callback::new(move |next: bool| {
                if !next {
                    set_target.set(None);
                }
            })
        >
            <DialogHeader>
                <div class="space-y-1">
                    <DialogTitle>{t!("users.set_password")}</DialogTitle>
                    <DialogDescription>
                        {move || {
                            target
                                .get()
                                .map(|account| t!("users.set_password_for", name => account.username))
                                .unwrap_or_default()
                        }}
                    </DialogDescription>
                </div>
                <DialogDismiss />
            </DialogHeader>
            <DialogContent class="space-y-3">
                <Field label=t!("users.field_password") required=true>
                    <Input
                        input_type="password"
                        value=Signal::derive(move || password.get())
                        on_input=Callback::new(move |value| set_password.set(value))
                        placeholder=t!("users.password_placeholder")
                        autocomplete="new-password"
                    />
                </Field>
                <p class="text-xs leading-relaxed text-muted-foreground">
                    {t!("users.set_password_hint")}
                </p>
                {move || {
                    save.get().message().map(|message| view! { <ErrorNote message=message /> })
                }}
            </DialogContent>
            <DialogFooter>
                <Button
                    label=t!("users.cancel")
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    on_click=Callback::new(move |()| set_target.set(None))
                />
                <Button
                    label=t!("users.save_password")
                    size=ButtonSize::Sm
                    disabled=Signal::derive(move || {
                        save.get().is_saving() || password.get().is_empty()
                    })
                    busy=Signal::derive(move || save.get().is_saving())
                    on_click=Callback::new(move |()| submit())
                />
            </DialogFooter>
        </Dialog>
    }
}

/// Remove one account, after a confirmation that names it.
#[component]
fn DeleteDialog(
    target: ReadSignal<Option<AccountTarget>>,
    set_target: WriteSignal<Option<AccountTarget>>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (save, set_save) = signal(Save::Idle);

    let remove = move || {
        let Some(account) = target.get_untracked() else {
            return;
        };
        if save.get().is_saving() {
            return;
        }
        let path = account.path();
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
            open=Signal::derive(move || target.get().is_some())
            on_open_change=Callback::new(move |next: bool| {
                if !next {
                    set_target.set(None);
                }
            })
        >
            <DialogHeader>
                <div class="space-y-1">
                    <DialogTitle>{t!("users.delete")}</DialogTitle>
                    <DialogDescription>
                        {move || {
                            target
                                .get()
                                .map(|account| t!("users.delete_confirm", name => account.username))
                                .unwrap_or_default()
                        }}
                    </DialogDescription>
                </div>
                <DialogDismiss />
            </DialogHeader>
            <DialogContent class="space-y-3">
                // The store refuses to remove the last admin; that answer arrives here rather than
                // being pre-judged, because only the server knows how many admins are left.
                <Alert variant=AlertVariant::Warning>
                    <AlertTitle>{t!("users.delete_warning_title")}</AlertTitle>
                    <AlertDescription>{t!("users.delete_warning_body")}</AlertDescription>
                </Alert>
                {move || {
                    save.get().message().map(|message| view! { <ErrorNote message=message /> })
                }}
            </DialogContent>
            <DialogFooter>
                <Button
                    label=t!("users.cancel")
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    on_click=Callback::new(move |()| set_target.set(None))
                />
                <Button
                    label=t!("users.confirm_delete_action")
                    variant=ButtonVariant::Destructive
                    size=ButtonSize::Sm
                    disabled=Signal::derive(move || save.get().is_saving())
                    busy=Signal::derive(move || save.get().is_saving())
                    on_click=Callback::new(move |()| remove())
                />
            </DialogFooter>
        </Dialog>
    }
}
