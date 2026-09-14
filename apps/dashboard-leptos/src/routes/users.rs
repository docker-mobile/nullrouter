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

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api::{Hydrate, Method, Save, encode, load, request_detailed, submit_reporting};
use crate::routes::types::{encode_query, timestamp_label};
use crate::routes::{PageHeader, Panel};

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

#[component]
pub fn Users() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (list, set_list) = signal(Hydrate::<UsersList>::Loading);
    let reload = move || {
        set_list.set(Hydrate::Loading);
        load("/api/users", set_list);
    };
    reload();

    view! {
        <PageHeader
            title=locale.get("nav.users").to_owned()
            description=locale.get("users.description").to_owned()
        />
        <Panel
            state=list
            on_retry=Callback::new(move |()| reload())
            children=move |data: UsersList| {
                view! { <Body data=data reload=reload /> }
            }
        />
    }
}

#[component]
fn Body(data: UsersList, reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <div class="space-y-4">
            {data
                .shared_password_active
                .then(|| {
                    view! {
                        <p class="rounded-md border border-warning/40 bg-warning/10 px-3 py-2 text-sm">
                            {locale.get("users.shared_password_notice").to_owned()}
                        </p>
                    }
                })}
            <CreateForm reload=reload />
            <UserTable rows=data.users reload=reload />
        </div>
    }
}

#[component]
fn CreateForm(reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (username, set_username) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (display, set_display) = signal(String::new());
    let (email, set_email) = signal(String::new());
    // Operator by default: it is the role most accounts need, and defaulting to admin would make the
    // safe choice the deliberate one.
    let (role, set_role) = signal("operator".to_owned());
    let (save, set_save) = signal(Save::Idle);
    let encode_failed = StoredValue::new(locale.get("users.encode_failed").to_owned());
    let label_create = locale.get("users.create").to_owned();

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
                reload();
            },
        );
    };

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("users.create").to_owned()}
            </h2>
            <div class="grid gap-3 sm:grid-cols-2">
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("users.field_username").to_owned()}
                    </span>
                    <input
                        type="text"
                        autocomplete="off"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono text-xs"
                        prop:value=move || username.get()
                        on:input=move |ev| set_username.set(event_target_value(&ev))
                        placeholder=locale.get("users.username_placeholder").to_owned()
                    />
                </label>
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("users.field_password").to_owned()}
                    </span>
                    <input
                        type="password"
                        autocomplete="new-password"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || password.get()
                        on:input=move |ev| set_password.set(event_target_value(&ev))
                        placeholder=locale.get("users.password_placeholder").to_owned()
                    />
                </label>
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("users.field_display_name").to_owned()}
                    </span>
                    <input
                        type="text"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || display.get()
                        on:input=move |ev| set_display.set(event_target_value(&ev))
                        placeholder=locale.get("users.display_name_placeholder").to_owned()
                    />
                </label>
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("users.field_email").to_owned()}
                    </span>
                    <input
                        type="email"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || email.get()
                        on:input=move |ev| set_email.set(event_target_value(&ev))
                        placeholder=locale.get("users.email_placeholder").to_owned()
                    />
                </label>
                <label class="space-y-1 text-sm sm:col-span-2">
                    <span class="text-muted-foreground">
                        {locale.get("users.field_role").to_owned()}
                    </span>
                    <select
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || role.get()
                        on:change=move |ev| set_role.set(event_target_value(&ev))
                    >
                        <option value="admin">{locale.get("users.role_admin").to_owned()}</option>
                        <option value="operator">
                            {locale.get("users.role_operator").to_owned()}
                        </option>
                        <option value="viewer">{locale.get("users.role_viewer").to_owned()}</option>
                    </select>
                </label>
            </div>
            <button
                type="button"
                class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                disabled=move || {
                    save.get().is_saving() || username.get().trim().is_empty()
                        || password.get().is_empty()
                }
                on:click=move |_| create()
            >
                {label_create}
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
fn UserTable(
    rows: Vec<UserRow>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">{locale.get("users.empty").to_owned()}</p>
        }
        .into_any();
    }
    view! {
        <div class="rounded-lg border border-border overflow-x-auto">
            <table class="w-full text-sm">
                <thead class="bg-muted/50 text-muted-foreground">
                    <tr>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("users.col_user").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("users.col_role").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("users.col_status").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("users.col_last_login").to_owned()}
                        </th>
                        <th class="px-3 py-2"></th>
                    </tr>
                </thead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| view! { <Row row=row reload=reload /> })
                        .collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

#[component]
fn Row(row: UserRow, reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (save, set_save) = signal(Save::Idle);
    let (new_password, set_new_password) = signal(String::new());

    let active = row.is_active;
    let path = StoredValue::new(format!("/api/users/{}", encode_query(&row.id)));
    let encode_failed = StoredValue::new(locale.get("users.encode_failed").to_owned());
    let label_toggle = if active {
        locale.get("users.disable").to_owned()
    } else {
        locale.get("users.enable").to_owned()
    };
    let label_delete = locale.get("users.delete").to_owned();
    let label_set_password = locale.get("users.set_password").to_owned();
    let last_login = row
        .last_login_at
        .as_deref()
        .map_or_else(|| locale.get("users.never").to_owned(), timestamp_label);
    let current_role = row.role.clone();

    let send = move |update: UpdateUser| {
        if save.get().is_saving() {
            return;
        }
        let Ok(encoded) = encode(&update) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };
        let target = path.get_value();
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Put, &target, Some(&encoded)).await },
            move |_| {
                set_new_password.set(String::new());
                reload();
            },
        );
    };

    let remove = move || {
        if save.get().is_saving() {
            return;
        }
        let target = path.get_value();
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Delete, &target, None).await },
            move |_| reload(),
        );
    };

    view! {
        <tr class="border-t border-border align-top">
            <td class="px-3 py-2">
                <span class="block">
                    {if row.display_name.is_empty() {
                        row.username.clone()
                    } else {
                        row.display_name.clone()
                    }}
                </span>
                <code class="block text-xs text-muted-foreground">{row.username}</code>
                {(!row.email.is_empty())
                    .then(|| {
                        view! {
                            <span class="block text-xs text-muted-foreground">{row.email}</span>
                        }
                    })}
            </td>
            <td class="px-3 py-2">
                <select
                    class="rounded-md border border-input bg-background px-2 py-1 text-xs"
                    on:change=move |ev| {
                        let chosen = event_target_value(&ev);
                        send(UpdateUser {
                            role: Some(chosen),
                            ..UpdateUser::default()
                        });
                    }
                >
                    <option value="admin" selected=current_role == "admin">
                        "admin"
                    </option>
                    <option value="operator" selected=current_role == "operator">
                        "operator"
                    </option>
                    <option value="viewer" selected=current_role == "viewer">
                        "viewer"
                    </option>
                </select>
            </td>
            <td class="px-3 py-2">
                {if active {
                    locale.get("state.enabled").to_owned()
                } else {
                    locale.get("state.disabled").to_owned()
                }}
                {move || {
                    save.get()
                        .message()
                        .map(|message| {
                            view! {
                                <p class="text-xs text-destructive whitespace-normal break-words">
                                    {message}
                                </p>
                            }
                        })
                }}
            </td>
            <td class="px-3 py-2 text-muted-foreground text-xs">{last_login}</td>
            <td class="px-3 py-2 text-right space-y-2">
                <div class="flex items-center justify-end gap-2">
                    <input
                        type="password"
                        autocomplete="new-password"
                        class="w-40 rounded-md border border-input bg-background px-2 py-1 text-xs"
                        prop:value=move || new_password.get()
                        on:input=move |ev| set_new_password.set(event_target_value(&ev))
                        placeholder=locale.get("users.new_password_placeholder").to_owned()
                    />
                    <button
                        type="button"
                        class="text-sm underline-offset-4 hover:underline disabled:opacity-50"
                        disabled=move || {
                            save.get().is_saving() || new_password.get().is_empty()
                        }
                        on:click=move |_| {
                            send(UpdateUser {
                                password: Some(new_password.get()),
                                ..UpdateUser::default()
                            });
                        }
                    >
                        {label_set_password}
                    </button>
                </div>
                <div class="flex items-center justify-end gap-3">
                    <button
                        type="button"
                        class="text-sm underline-offset-4 hover:underline disabled:opacity-50"
                        disabled=move || save.get().is_saving()
                        on:click=move |_| {
                            send(UpdateUser {
                                is_active: Some(!active),
                                ..UpdateUser::default()
                            });
                        }
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
                </div>
            </td>
        </tr>
    }
}
