//! Managed API keys used when the router requires one.

use leptos::prelude::*;

use crate::api::{Hydrate, Save, decode, encode, load, post, put, submit};
use crate::routes::types::{ApiKeyRow, CreateKeyBody, CreatedKey, KeysList, UpdateKeyBody};
use crate::routes::{PageHeader, Panel};

#[component]
pub fn Keys() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (keys, set_keys) = signal(Hydrate::<KeysList>::Loading);
    let (save, set_save) = signal(Save::Idle);
    let (name, set_name) = signal(String::new());
    let (created_secret, set_created_secret) = signal(None::<String>);

    let reload = move || {
        set_keys.set(Hydrate::Loading);
        load("/api/keys", set_keys);
    };
    reload();

    view! {
        <PageHeader
            title=locale.get("nav.keys").to_owned()
            description=locale.get("keys.description").to_owned()
        />
        <div class="rounded-lg border border-border bg-card p-5 space-y-4 mb-4">
            <p class="text-sm text-muted-foreground">{locale.get("keys.create_hint").to_owned()}</p>
            <div class="flex flex-col sm:flex-row gap-2">
                <input
                    class="flex-1 rounded-md border border-input bg-background px-3 py-2 text-sm"
                    prop:value=move || name.get()
                    on:input=move |ev| set_name.set(event_target_value(&ev))
                    placeholder=locale.get("keys.name_placeholder").to_owned()
                />
                <button
                    type="button"
                    class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                    disabled=move || save.get().is_saving() || name.get().trim().is_empty()
                    on:click=move |_| {
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
                                if let Ok(created) = decode::<CreatedKey>(&response) {
                                    set_created_secret.set(Some(created.key.key));
                                    set_name.set(String::new());
                                    reload();
                                }
                            },
                        );
                    }
                >
                    {locale.get("keys.create").to_owned()}
                </button>
            </div>
            {move || {
                save.get().failure().map(|error| {
                    view! { <p class="text-sm text-destructive">{error.message()}</p> }
                })
            }}
            {move || {
                created_secret.get().map(|secret| {
                    view! {
                        <p class="text-sm">
                            {locale.get("keys.created_once").to_owned()}
                            <code class="ml-2 font-mono break-all">{secret}</code>
                        </p>
                    }
                })
            }}
        </div>
        <Panel
            state=keys
            on_retry=Callback::new(move |()| reload())
            children=move |data: KeysList| view! { <KeysTable rows=data.keys reload=reload /> }
        />
    }
}

#[component]
fn KeysTable(
    rows: Vec<ApiKeyRow>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">{locale.get("keys.empty").to_owned()}</p>
        }
        .into_any();
    }
    view! {
        <div class="rounded-lg border border-border overflow-x-auto">
            <table class="w-full text-sm">
                <thead class="bg-muted/50 text-muted-foreground">
                    <tr>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("keys.col_name").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("keys.col_id").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("keys.col_status").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("keys.col_created").to_owned()}
                        </th>
                        <th class="px-3 py-2"></th>
                    </tr>
                </thead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| view! { <KeyRow row=row reload=reload /> })
                        .collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

#[component]
fn KeyRow(row: ApiKeyRow, reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (save, set_save) = signal(Save::Idle);
    let active = row.is_active;
    let id = row.id.clone();
    view! {
        <tr class="border-t border-border">
            <td class="px-3 py-2">{row.name}</td>
            <td class="px-3 py-2 font-mono text-xs text-muted-foreground">{row.id.clone()}</td>
            <td class="px-3 py-2">
                {if active {
                    locale.get("state.enabled").to_owned()
                } else {
                    locale.get("state.disabled").to_owned()
                }}
            </td>
            <td class="px-3 py-2 text-muted-foreground">{row.created_at}</td>
            <td class="px-3 py-2 text-right">
                <button
                    type="button"
                    class="text-sm underline-offset-4 hover:underline"
                    disabled=move || save.get().is_saving()
                    on:click=move |_| {
                        let path = format!("/api/keys/{id}");
                        let Ok(body) = encode(&UpdateKeyBody { is_active: !active }) else {
                            return;
                        };
                        submit(
                            set_save,
                            move || async move { put(&path, &body).await },
                            move |_| reload(),
                        );
                    }
                >
                    {if active {
                        locale.get("keys.disable").to_owned()
                    } else {
                        locale.get("keys.enable").to_owned()
                    }}
                </button>
            </td>
        </tr>
    }
}
