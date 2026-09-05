//! Outbound proxy pools, and the relays that can be deployed to fill them.
//!
//! `/api/proxy-pools` and `/api/proxy-pools/{id}` are routed to `nullrouter-state`, so everything
//! on this panel except the two noted below actually persists. The state service enforces one rule
//! the client cannot: a pool bound to a provider connection refuses to be deleted, with a 409 that
//! carries the number of connections holding it. That count is the whole content of the refusal, so
//! writes go through [`crate::routes::write_reporting`] rather than a bare status.
//!
//! Two things here are not supported by the server and are shown as such rather than as buttons:
//!
//! * **Testing a pool.** `POST /api/proxy-pools/{id}/test` is the one pool path the gateway does
//!   *not* route to the state service -- the `/test` tail keeps it in `nullrouter-api`, which
//!   answers 501 `{"unsupported": true}` unconditionally. A test button would fail every time.
//! * **`testStatus`.** The field is stored and returned, but nothing ever moves it off `unknown`
//!   while the test route is a stub, so it is rendered as the literal state rather than as a health
//!   verdict the router does not have.
//!
//! The list is fetched with `?includeUsage=true`, which adds `boundConnectionCount` to each row.
//! That is what lets the panel say *why* a delete will be refused before it is attempted.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api::{Hydrate, Method, Save, encode, load, request_detailed, submit_reporting};
use crate::routes::types::timestamp_label;
use crate::routes::{PageHeader, Panel};

/// `GET /api/proxy-pools?includeUsage=true`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PoolsList {
    #[serde(default)]
    proxy_pools: Vec<PoolRow>,
}

/// One stored pool.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PoolRow {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    proxy_url: String,
    #[serde(default)]
    no_proxy: String,
    #[serde(rename = "type", default)]
    proxy_type: String,
    #[serde(default)]
    is_active: bool,
    #[serde(default)]
    strict_proxy: bool,
    /// Always `unknown` while the test route is a stub.
    #[serde(default)]
    test_status: String,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    updated_at: String,
    /// Only present with `?includeUsage=true`; absent reads as nothing bound.
    #[serde(default)]
    bound_connection_count: u64,
}
/// A `POST /api/proxy-pools` body.
///
/// `type` is normalised server-side to one of `http`, `vercel`, `cloudflare`, `deno`, and anything
/// else silently becomes `http` -- so the panel offers exactly those four rather than a free-text
/// field whose contents would be quietly replaced.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatePool<'a> {
    name: &'a str,
    proxy_url: &'a str,
    no_proxy: &'a str,
    #[serde(rename = "type")]
    proxy_type: &'a str,
    strict_proxy: bool,
}

/// A `PUT /api/proxy-pools/{id}` body. Every field is optional server-side, so a single-key write
/// leaves the rest of the pool untouched.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdatePool {
    is_active: bool,
}

/// The four proxy types the store will keep.
const POOL_TYPES: [&str; 4] = ["http", "vercel", "cloudflare", "deno"];

#[component]
pub fn ProxyPools() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (list, set_list) = signal(Hydrate::<PoolsList>::Loading);
    let reload = move || {
        set_list.set(Hydrate::Loading);
        load("/api/proxy-pools?includeUsage=true", set_list);
    };
    reload();

    view! {
        <PageHeader
            title=locale.get("nav.proxy_pools").to_owned()
            description=locale.get("pools.description").to_owned()
        />
        <CreateForm reload=reload />
        <Panel
            state=list
            on_retry=Callback::new(move |()| reload())
            children=move |data: PoolsList| view! { <PoolTable rows=data.proxy_pools reload=reload /> }
        />
        <p class="mt-3 text-sm text-muted-foreground">
            {locale.get("pools.test_unsupported").to_owned()}
        </p>
        <DeploySection reload=reload />
    }
}
/// Create a pool by hand, for a proxy that already exists somewhere.
#[component]
fn CreateForm(reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (name, set_name) = signal(String::new());
    let (url, set_url) = signal(String::new());
    let (no_proxy, set_no_proxy) = signal(String::new());
    let (kind, set_kind) = signal("http".to_owned());
    let (strict, set_strict) = signal(false);
    let (save, set_save) = signal(Save::Idle);

    let incomplete = move || {
        save.get().is_saving() || name.get().trim().is_empty() || url.get().trim().is_empty()
    };

    // Owned up front: `Locale` is not `Copy`, so a `move` closure that read it would leave nothing
    // for the view below.
    let encode_failed = locale.get("pools.encode_failed").to_owned();

    let submit = move || {
        let (label, target, bypass) = (name.get(), url.get(), no_proxy.get());
        let (label, target) = (label.trim(), target.trim());
        if label.is_empty() || target.is_empty() {
            return;
        }
        let Ok(body) = encode(&CreatePool {
            name: label,
            proxy_url: target,
            no_proxy: bypass.trim(),
            proxy_type: &kind.get(),
            strict_proxy: strict.get(),
        }) else {
            set_save.set(Save::Refused(encode_failed.clone()));
            return;
        };

        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Post, "/api/proxy-pools", Some(&body)).await },
            move |_| {
                set_name.set(String::new());
                set_url.set(String::new());
                set_no_proxy.set(String::new());
                set_strict.set(false);
                reload();
            },
        );
    };

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4 mb-4">
            <p class="text-sm text-muted-foreground">{locale.get("pools.create_hint").to_owned()}</p>
            <div class="grid gap-3 sm:grid-cols-2">
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">{locale.get("pools.name").to_owned()}</span>
                    <input
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || name.get()
                        on:input=move |ev| set_name.set(event_target_value(&ev))
                        placeholder=locale.get("pools.name_placeholder").to_owned()
                    />
                </label>
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">{locale.get("pools.url").to_owned()}</span>
                    <input
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono"
                        prop:value=move || url.get()
                        on:input=move |ev| set_url.set(event_target_value(&ev))
                        placeholder=locale.get("pools.url_placeholder").to_owned()
                    />
                </label>
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("pools.no_proxy").to_owned()}
                    </span>
                    <input
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono"
                        prop:value=move || no_proxy.get()
                        on:input=move |ev| set_no_proxy.set(event_target_value(&ev))
                        placeholder=locale.get("pools.no_proxy_placeholder").to_owned()
                    />
                </label>
                <label class="space-y-1 text-sm">
                    <span class="text-muted-foreground">{locale.get("pools.type").to_owned()}</span>
                    <select
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || kind.get()
                        on:change=move |ev| set_kind.set(event_target_value(&ev))
                    >
                        {POOL_TYPES
                            .into_iter()
                            .map(|option| {
                                view! { <option value=option>{option}</option> }
                            })
                            .collect_view()}
                    </select>
                </label>
            </div>
            <label class="flex items-start gap-2 text-sm">
                <input
                    type="checkbox"
                    class="size-4 mt-0.5"
                    prop:checked=move || strict.get()
                    on:change=move |ev| set_strict.set(event_target_checked(&ev))
                />
                <span>
                    {locale.get("pools.strict").to_owned()}
                    <span class="block text-muted-foreground">
                        {locale.get("pools.strict_hint").to_owned()}
                    </span>
                </span>
            </label>
            <button
                type="button"
                class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                disabled=incomplete
                on:click=move |_| submit()
            >
                {locale.get("pools.create").to_owned()}
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
fn PoolTable(
    rows: Vec<PoolRow>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">{locale.get("pools.empty").to_owned()}</p>
        }
        .into_any();
    }
    view! {
        <div class="rounded-lg border border-border overflow-x-auto">
            <table class="w-full text-sm">
                <thead class="bg-muted/50 text-muted-foreground">
                    <tr>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("pools.col_name").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("pools.col_url").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("pools.col_type").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("pools.col_status").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("pools.col_bound").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("pools.col_updated").to_owned()}
                        </th>
                        <th class="px-3 py-2"></th>
                    </tr>
                </thead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| view! { <PoolLine row=row reload=reload /> })
                        .collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}
/// One pool, with the two writes the store accepts for it.
///
/// Delete is armed by a first click rather than fired by it. The 409 makes the dangerous case
/// (a pool a connection depends on) safe, but an unbound pool deletes immediately and there is no
/// undo, so the bound count is shown next to the confirm.
#[component]
fn PoolLine(row: PoolRow, reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (save, set_save) = signal(Save::Idle);
    let (armed, set_armed) = signal(false);

    let active = row.is_active;
    let bound = row.bound_connection_count;
    // Stored rather than captured: delete is invoked from inside a reactive closure, which rebuilds
    // its handler on every run and so needs a `Copy` closure.
    let id = StoredValue::new(row.id.clone());
    let encode_failed = StoredValue::new(locale.get("pools.encode_failed").to_owned());

    // Owned before the delete control's reactive closure would move the non-`Copy` locale.
    let label_delete = locale.get("pools.delete").to_owned();
    let label_confirm_delete = locale.get("pools.confirm_delete").to_owned();
    let label_cancel = locale.get("pools.cancel").to_owned();

    let toggle = move || {
        let path = format!("/api/proxy-pools/{}", id.get_value());
        let Ok(body) = encode(&UpdatePool { is_active: !active }) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
            return;
        };
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Put, &path, Some(&body)).await },
            move |_| reload(),
        );
    };

    // The 409 this can return carries the count of connections still bound to the pool, and
    // `submit_reporting` keeps that sentence rather than folding it into a status.
    let remove = move || {
        let path = format!("/api/proxy-pools/{}", id.get_value());
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Delete, &path, None).await },
            move |_| {
                set_armed.set(false);
                reload();
            },
        );
    };

    view! {
        <tr class="border-t border-border align-top">
            <td class="px-3 py-2">
                <div class="space-y-0.5">
                    <span>{row.name}</span>
                    {(!row.no_proxy.is_empty())
                        .then(|| {
                            view! {
                                <p class="font-mono text-xs text-muted-foreground">
                                    {format!("{}: {}", locale.get("pools.no_proxy"), row.no_proxy)}
                                </p>
                            }
                        })}
                    {row.strict_proxy
                        .then(|| {
                            view! {
                                <p class="text-xs text-muted-foreground">
                                    {locale.get("pools.strict").to_owned()}
                                </p>
                            }
                        })}
                </div>
            </td>
            <td class="px-3 py-2 font-mono text-xs break-all">{row.proxy_url}</td>
            <td class="px-3 py-2 font-mono text-xs text-muted-foreground">{row.proxy_type}</td>
            <td class="px-3 py-2">
                <div class="flex items-center gap-2">
                    <span class=if active {
                        "size-1.5 rounded-full bg-success"
                    } else {
                        "size-1.5 rounded-full bg-muted-foreground/40"
                    } />
                    <span>
                        {if active {
                            locale.get("state.enabled").to_owned()
                        } else {
                            locale.get("state.disabled").to_owned()
                        }}
                    </span>
                </div>
                <p class="font-mono text-xs text-muted-foreground">{row.test_status}</p>
                {row.last_error
                    .filter(|message| !message.is_empty())
                    .map(|message| {
                        view! { <p class="text-xs text-destructive">{message}</p> }
                    })}
            </td>
            <td class="px-3 py-2 tabular-nums">{bound.to_string()}</td>
            <td class="px-3 py-2 text-xs text-muted-foreground">
                {timestamp_label(&row.updated_at)}
            </td>
            <td class="px-3 py-2 text-right">
                <div class="flex flex-col items-end gap-1.5">
                    <button
                        type="button"
                        class="text-sm underline-offset-4 hover:underline disabled:opacity-50"
                        disabled=move || save.get().is_saving()
                        on:click=move |_| toggle()
                    >
                        {if active {
                            locale.get("pools.disable").to_owned()
                        } else {
                            locale.get("pools.enable").to_owned()
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
                                        disabled=move || save.get().is_saving()
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
                    // Named before the delete is attempted: the store refuses while anything is
                    // bound, and the count is the reason.
                    {(bound > 0)
                        .then(|| {
                            view! {
                                <p class="text-xs text-muted-foreground text-right">
                                    {locale.fmt("pools.bound_warning", &[("count", &bound.to_string())])}
                                </p>
                            }
                        })}
                    {move || {
                        save.get()
                            .message()
                            .map(|message| {
                                view! {
                                    <p class="text-xs text-destructive text-right">{message}</p>
                                }
                            })
                    }}
                </div>
            </td>
        </tr>
    }
}
/// A relay platform, and the credentials its deploy needs.
///
/// These three routes are the only ones on this panel that leave the machine: each authenticates to
/// a third-party API and creates a real project in the operator's account. `second` is `None` for
/// Vercel because a Vercel token is the whole credential.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Platform {
    Cloudflare,
    Deno,
    Vercel,
}

impl Platform {
    const ALL: [Self; 3] = [Self::Cloudflare, Self::Deno, Self::Vercel];

    const fn path(self) -> &'static str {
        match self {
            Self::Cloudflare => "/api/proxy-pools/cloudflare-deploy",
            Self::Deno => "/api/proxy-pools/deno-deploy",
            Self::Vercel => "/api/proxy-pools/vercel-deploy",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Cloudflare => "pools.deploy_cloudflare",
            Self::Deno => "pools.deploy_deno",
            Self::Vercel => "pools.deploy_vercel",
        }
    }

    /// The first field's label key. Not a secret for Cloudflare and Deno; the token for Vercel.
    const fn first_label(self) -> &'static str {
        match self {
            Self::Cloudflare => "pools.cf_account",
            Self::Deno => "pools.deno_org",
            Self::Vercel => "pools.vercel_token",
        }
    }

    const fn first_is_secret(self) -> bool {
        matches!(self, Self::Vercel)
    }

    /// The second field's label key, when the platform needs one.
    const fn second_label(self) -> Option<&'static str> {
        match self {
            Self::Cloudflare => Some("pools.cf_token"),
            Self::Deno => Some("pools.deno_token"),
            Self::Vercel => None,
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudflareDeploy<'a> {
    account_id: &'a str,
    api_token: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_name: Option<&'a str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DenoDeploy<'a> {
    org_domain: &'a str,
    deno_token: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_name: Option<&'a str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VercelDeploy<'a> {
    vercel_token: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_name: Option<&'a str>,
}

/// `{"proxyPool": {...}, "deployUrl": "..."}` on success.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeployResult {
    #[serde(default)]
    deploy_url: String,
}

/// Build the platform's body. `project_name` is omitted when blank so the server picks its default.
fn deploy_body(
    platform: Platform,
    first: &str,
    second: &str,
    project: Option<&str>,
) -> Result<String, crate::api::ApiError> {
    match platform {
        Platform::Cloudflare => encode(&CloudflareDeploy {
            account_id: first,
            api_token: second,
            project_name: project,
        }),
        Platform::Deno => encode(&DenoDeploy {
            org_domain: first,
            deno_token: second,
            project_name: project,
        }),
        Platform::Vercel => encode(&VercelDeploy {
            vercel_token: first,
            project_name: project,
        }),
    }
}
#[component]
fn DeploySection(reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <section class="mt-6 space-y-3">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("pools.deploy").to_owned()}
            </h2>
            <p class="text-sm text-muted-foreground">{locale.get("pools.deploy_hint").to_owned()}</p>
            <div class="grid gap-4 lg:grid-cols-3">
                {Platform::ALL
                    .into_iter()
                    .map(|platform| view! { <DeployCard platform=platform reload=reload /> })
                    .collect_view()}
            </div>
        </section>
    }
}

/// One platform's deploy form.
#[component]
fn DeployCard(
    platform: Platform,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (first, set_first) = signal(String::new());
    let (second, set_second) = signal(String::new());
    let (project, set_project) = signal(String::new());
    let (save, set_save) = signal(Save::Idle);
    let (deployed, set_deployed) = signal(None::<String>);

    let needs_second = platform.second_label().is_some();
    let incomplete = move || {
        save.get().is_saving()
            || first.get().trim().is_empty()
            || (needs_second && second.get().trim().is_empty())
    };
    let encode_failed = locale.get("pools.encode_failed").to_owned();

    let send = move || {
        let (one, two, name) = (first.get(), second.get(), project.get());
        let trimmed_project = name.trim();
        let Ok(body) = deploy_body(
            platform,
            one.trim(),
            two.trim(),
            (!trimmed_project.is_empty()).then_some(trimmed_project),
        ) else {
            set_save.set(Save::Refused(encode_failed.clone()));
            return;
        };

        set_deployed.set(None);
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Post, platform.path(), Some(&body)).await },
            move |response| {
                // Credentials are cleared once they have been used: they are not needed again, and
                // a token left in a live input is one screenshot away from being shared.
                set_first.set(String::new());
                set_second.set(String::new());
                let url = crate::api::decode::<DeployResult>(&response)
                    .map(|result| result.deploy_url)
                    .unwrap_or_default();
                set_deployed.set(Some(url));
                reload();
            },
        );
    };

    let first_type = if platform.first_is_secret() {
        "password"
    } else {
        "text"
    };
    // Both labels are owned before the closure below borrows the locale.
    let deploying = locale.get("pools.deploying").to_owned();
    let deploy_run = locale.get("pools.deploy_run").to_owned();

    view! {
        <div class="rounded-lg border border-border bg-card p-5 space-y-3">
            <h3 class="text-sm font-medium">{locale.get(platform.title()).to_owned()}</h3>
            <label class="block space-y-1 text-sm">
                <span class="text-muted-foreground">
                    {locale.get(platform.first_label()).to_owned()}
                </span>
                <input
                    type=first_type
                    autocomplete="off"
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                    prop:value=move || first.get()
                    on:input=move |ev| set_first.set(event_target_value(&ev))
                />
            </label>
            {platform
                .second_label()
                .map(|key| {
                    view! {
                        <label class="block space-y-1 text-sm">
                            <span class="text-muted-foreground">{locale.get(key).to_owned()}</span>
                            <input
                                type="password"
                                autocomplete="off"
                                class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                                prop:value=move || second.get()
                                on:input=move |ev| set_second.set(event_target_value(&ev))
                            />
                        </label>
                    }
                })}
            <label class="block space-y-1 text-sm">
                <span class="text-muted-foreground">
                    {locale.get("pools.project_name").to_owned()}
                </span>
                <input
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                    prop:value=move || project.get()
                    on:input=move |ev| set_project.set(event_target_value(&ev))
                    placeholder=locale.get("pools.project_placeholder").to_owned()
                />
            </label>
            <button
                type="button"
                class="w-full rounded-md bg-primary px-3 py-2 text-sm font-medium \
                       text-primary-foreground disabled:opacity-50"
                disabled=incomplete
                on:click=move |_| send()
            >
                {move || {
                    if save.get().is_saving() {
                        deploying.clone()
                    } else {
                        deploy_run.clone()
                    }
                }}
            </button>
            {move || {
                save.get()
                    .message()
                    .map(|message| view! { <p class="text-sm text-destructive">{message}</p> })
            }}
            {move || {
                deployed
                    .get()
                    .map(|url| {
                        let label = locale.get("pools.deployed").to_owned();
                        if url.is_empty() {
                            // A 201 with no `deployUrl` still created the pool; saying so beats
                            // rendering an empty link.
                            return view! { <p class="text-sm">{label}</p> }.into_any();
                        }
                        let href = url.clone();
                        view! {
                            <div class="space-y-1">
                                <p class="text-sm">{label}</p>
                                <a
                                    href=href
                                    target="_blank"
                                    rel="noreferrer noopener"
                                    class="block break-all font-mono text-xs underline-offset-4 hover:underline"
                                >
                                    {url}
                                </a>
                            </div>
                        }
                            .into_any()
                    })
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{DeployResult, Platform, PoolsList, deploy_body};

    /// `GET /api/proxy-pools?includeUsage=true` with one pool, captured from the running router.
    const LIVE_LIST: &str = r#"{"proxyPools":[{
        "createdAt":"unix-ms:1788527313109","id":"proxy_pool_1788527313109_1","isActive":true,
        "lastError":null,"lastTestedAt":null,"name":"probe","noProxy":"",
        "proxyUrl":"http://127.0.0.1:9","strictProxy":false,"testStatus":"unknown",
        "type":"http","updatedAt":"unix-ms:1788527313109","boundConnectionCount":2
    }]}"#;

    #[test]
    fn the_live_list_decodes_with_its_usage_count() {
        let parsed: PoolsList = serde_json::from_str(LIVE_LIST).expect("must decode");
        let row = parsed.proxy_pools.first().expect("one pool");
        assert_eq!(row.id, "proxy_pool_1788527313109_1");
        assert_eq!(row.proxy_type, "http", "`type` is renamed, not camelCased");
        assert!(row.is_active);
        assert_eq!(row.test_status, "unknown");
        assert_eq!(row.bound_connection_count, 2);
        assert!(row.last_error.is_none());
    }

    #[test]
    fn a_list_without_usage_reads_as_nothing_bound() {
        // `boundConnectionCount` only appears with `?includeUsage=true`; its absence must not
        // deserialize into a number that would gate the delete warning wrongly.
        let body = r#"{"proxyPools":[{"id":"p1","name":"n","proxyUrl":"http://x","type":"http"}]}"#;
        let parsed: PoolsList = serde_json::from_str(body).expect("must decode");
        let row = parsed.proxy_pools.first().expect("one pool");
        assert_eq!(row.bound_connection_count, 0);
    }

    #[test]
    fn an_empty_list_is_not_a_failure() {
        let parsed: PoolsList = serde_json::from_str(r#"{"proxyPools":[]}"#).expect("must decode");
        assert!(parsed.proxy_pools.is_empty());
    }
    #[test]
    fn each_platform_sends_the_field_names_its_handler_reads() {
        // The handlers read `accountId`/`apiToken`, `orgDomain`/`denoToken` and `vercelToken`. A
        // wrong name here deserializes to `None` server-side and comes back as "required", so the
        // exact spelling is pinned.
        let cloudflare = deploy_body(Platform::Cloudflare, "acct", "token", None).expect("encodes");
        assert!(cloudflare.contains("\"accountId\":\"acct\""));
        assert!(cloudflare.contains("\"apiToken\":\"token\""));

        let deno = deploy_body(Platform::Deno, "org.deno.dev", "token", None).expect("encodes");
        assert!(deno.contains("\"orgDomain\":\"org.deno.dev\""));
        assert!(deno.contains("\"denoToken\":\"token\""));

        let vercel = deploy_body(Platform::Vercel, "token", "", None).expect("encodes");
        assert!(vercel.contains("\"vercelToken\":\"token\""));
        // Vercel takes one credential; the unused second field must not be sent.
        assert!(!vercel.contains("apiToken"));
    }

    #[test]
    fn a_blank_project_name_is_omitted_rather_than_sent_empty() {
        // Sent as "", the server's own validator rejects it; omitted, it picks a default.
        let body = deploy_body(Platform::Vercel, "token", "", None).expect("encodes");
        assert!(!body.contains("projectName"));

        let named = deploy_body(Platform::Vercel, "token", "", Some("relay")).expect("encodes");
        assert!(named.contains("\"projectName\":\"relay\""));
    }

    #[test]
    fn a_deploy_reports_the_url_it_created() {
        let body = r#"{"proxyPool":{"id":"p1"},"deployUrl":"https://relay.workers.dev"}"#;
        let parsed: DeployResult = serde_json::from_str(body).expect("must decode");
        assert_eq!(parsed.deploy_url, "https://relay.workers.dev");
    }

    #[test]
    fn every_platform_has_a_distinct_route() {
        for (index, platform) in Platform::ALL.into_iter().enumerate() {
            assert!(platform.path().starts_with("/api/proxy-pools/"));
            let duplicate = Platform::ALL
                .into_iter()
                .skip(index + 1)
                .find(|other| other.path() == platform.path());
            assert!(duplicate.is_none(), "{:?} is listed twice", platform.path());
        }
    }
}
