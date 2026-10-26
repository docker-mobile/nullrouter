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
//!
//! # Why a card per pool
//!
//! A pool carries six values an operator reads together -- URL, type, state, test status, bound
//! connections, last error -- and two of them are long strings. Squeezed into table cells those
//! wrapped into four lines each and the actions column became a stack of links. One card per pool
//! gives the URL the width of the card, keeps the state badge beside the name, and puts the write
//! actions in a footer where they do not compete with the values. The list is small (a handful of
//! outbound proxies), so a grid of cards is also the right density.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api::{Hydrate, Method, Save, encode, load, request_detailed, submit_reporting};
use crate::routes::types::timestamp_label;
use crate::routes::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Badge, BadgeVariant, Button, ButtonSize,
    ButtonVariant, Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle, Checkbox,
    Dialog, DialogContent, DialogDescription, DialogDismiss, DialogFooter, DialogHeader,
    DialogTitle, EmptyState, ErrorNote, Field, Icon, IconName, Input, Mono, PageBody, PageHeader,
    Panel, Select,
};
use crate::t;

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
    let (list, set_list) = signal(Hydrate::<PoolsList>::Loading);
    let (creating, set_creating) = signal(false);
    let reload = move || {
        set_list.set(Hydrate::Loading);
        load("/api/proxy-pools?includeUsage=true", set_list);
    };
    reload();

    view! {
        <PageHeader title=t!("nav.proxy_pools") description=t!("pools.description")>
            <Button
                label=t!("pools.create")
                size=ButtonSize::Sm
                on_click=Callback::new(move |()| set_creating.set(true))
            />
        </PageHeader>

        <PageBody>
            // Stated, not offered: the test route answers 501 on this build, so a pool's health is
            // whatever the store last recorded rather than something this panel can go and measure.
            <Alert variant=AlertVariant::Muted>
                <AlertTitle>{t!("pools.test_unsupported_title")}</AlertTitle>
                <AlertDescription>{t!("pools.test_unsupported")}</AlertDescription>
            </Alert>

            <Panel
                state=list
                on_retry=Callback::new(move |()| reload())
                children=move |data: PoolsList| {
                    view! { <PoolList rows=data.proxy_pools reload=reload set_creating=set_creating /> }
                }
            />

            <DeploySection reload=reload />
        </PageBody>

        <CreateDialog open=creating set_open=set_creating reload=reload />
    }
}

/// The pools, as a grid of cards, or the reason there are none.
#[component]
fn PoolList(
    rows: Vec<PoolRow>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
    set_creating: WriteSignal<bool>,
) -> impl IntoView {
    if rows.is_empty() {
        return view! {
            <Card>
                <EmptyState
                    title=t!("pools.empty")
                    description=t!("pools.empty_hint")
                    icon=IconName::Network
                >
                    <Button
                        label=t!("pools.create")
                        size=ButtonSize::Sm
                        variant=ButtonVariant::Outline
                        on_click=Callback::new(move |()| set_creating.set(true))
                    />
                </EmptyState>
            </Card>
        }
        .into_any();
    }

    view! {
        <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
            {rows
                .into_iter()
                .map(|row| view! { <PoolCard row=row reload=reload /> })
                .collect_view()}
        </div>
    }
    .into_any()
}

/// Create a pool by hand, for a proxy that already exists somewhere.
#[component]
fn CreateDialog(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (name, set_name) = signal(String::new());
    let (url, set_url) = signal(String::new());
    let (no_proxy, set_no_proxy) = signal(String::new());
    let (kind, set_kind) = signal("http".to_owned());
    let (strict, set_strict) = signal(false);
    let (save, set_save) = signal(Save::Idle);

    let incomplete = move || {
        save.get().is_saving() || name.get().trim().is_empty() || url.get().trim().is_empty()
    };

    let encode_failed = StoredValue::new(t!("pools.encode_failed"));

    // The four values the store keeps. Anything else is normalised to `http` server-side, so the
    // panel offers exactly these rather than a field whose contents would be replaced.
    let type_options = Signal::derive(|| {
        POOL_TYPES
            .into_iter()
            .map(|value| (value.to_owned(), value.to_owned()))
            .collect::<Vec<_>>()
    });

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
            set_save.set(Save::Refused(encode_failed.get_value()));
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
                    <DialogTitle>{t!("pools.create")}</DialogTitle>
                    <DialogDescription>{t!("pools.create_hint")}</DialogDescription>
                </div>
                <DialogDismiss />
            </DialogHeader>
            <DialogContent class="space-y-3">
                <div class="grid gap-3 sm:grid-cols-2">
                    <Field label=t!("pools.name") required=true>
                        <Input
                            value=Signal::derive(move || name.get())
                            on_input=Callback::new(move |value| set_name.set(value))
                            placeholder=t!("pools.name_placeholder")
                            autocomplete="off"
                        />
                    </Field>
                    <Field label=t!("pools.url") required=true>
                        <Input
                            value=Signal::derive(move || url.get())
                            on_input=Callback::new(move |value| set_url.set(value))
                            placeholder=t!("pools.url_placeholder")
                            autocomplete="off"
                            class="font-mono"
                        />
                    </Field>
                    <Field label=t!("pools.no_proxy")>
                        <Input
                            value=Signal::derive(move || no_proxy.get())
                            on_input=Callback::new(move |value| set_no_proxy.set(value))
                            placeholder=t!("pools.no_proxy_placeholder")
                            autocomplete="off"
                            class="font-mono"
                        />
                    </Field>
                    <Field label=t!("pools.type")>
                        <Select
                            value=Signal::derive(move || kind.get())
                            options=type_options
                            on_change=Callback::new(move |chosen| set_kind.set(chosen))
                            aria_label=t!("pools.type")
                        />
                    </Field>
                </div>
                <div class="space-y-1">
                    <Checkbox
                        checked=Signal::derive(move || strict.get())
                        on_change=Callback::new(move |next| set_strict.set(next))
                        label=t!("pools.strict")
                    />
                    <p class="text-xs leading-relaxed text-muted-foreground">
                        {t!("pools.strict_hint")}
                    </p>
                </div>
                {move || {
                    save.get().message().map(|message| view! { <ErrorNote message=message /> })
                }}
            </DialogContent>
            <DialogFooter>
                <Button
                    label=t!("pools.cancel")
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    on_click=Callback::new(move |()| set_open.set(false))
                />
                <Button
                    label=t!("pools.create")
                    size=ButtonSize::Sm
                    disabled=Signal::derive(incomplete)
                    busy=Signal::derive(move || save.get().is_saving())
                    on_click=Callback::new(move |()| submit())
                />
            </DialogFooter>
        </Dialog>
    }
}

/// One pool, with the two writes the store accepts for it.
///
/// Delete is armed by a first click rather than fired by it. The 409 makes the dangerous case
/// (a pool a connection depends on) safe, but an unbound pool deletes immediately and there is no
/// undo, so the bound count is shown next to the confirm.
#[component]
fn PoolCard(row: PoolRow, reload: impl Fn() + Copy + 'static + Send + Sync) -> impl IntoView {
    let (save, set_save) = signal(Save::Idle);
    let (armed, set_armed) = signal(false);
    let (testing, set_testing) = signal(false);
    let (test_feedback, set_test_feedback) = signal(Option::<String>::None);

    let active = row.is_active;
    let bound = row.bound_connection_count;
    // Stored rather than captured: the writes below are invoked from inside reactive closures, which
    // rebuild their handlers on every run and so need a `Copy` closure.
    let id = StoredValue::new(row.id.clone());
    let encode_failed = StoredValue::new(t!("pools.encode_failed"));

    let test_action = move || {
        set_testing.set(true);
        set_test_feedback.set(None);
        let path = format!("/api/proxy-pools/{}/test", id.get_value());
        submit_reporting(
            set_save,
            move || async move { request_detailed(Method::Post, &path, None).await },
            move |raw: String| {
                set_testing.set(false);
                let parsed: serde_json::Value =
                    serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
                let ok = parsed
                    .get("ok")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let ms = parsed
                    .get("elapsedMs")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                if ok {
                    set_test_feedback.set(Some(format!("✓ {ms}ms")));
                } else {
                    let err = parsed
                        .get("error")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("failed");
                    set_test_feedback.set(Some(format!("✗ {err}")));
                }
                reload();
            },
        );
    };

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

    let no_proxy = (!row.no_proxy.is_empty()).then(|| row.no_proxy.clone());
    let strict = row.strict_proxy;
    let last_error = row.last_error.clone().filter(|message| !message.is_empty());

    view! {
        <Card>
            <CardHeader>
                <div class="min-w-0 space-y-1">
                    <CardTitle class="truncate">{row.name.clone()}</CardTitle>
                    <CardDescription class="truncate font-mono">{row.proxy_url.clone()}</CardDescription>
                </div>
                <Badge variant=if active { BadgeVariant::Success } else { BadgeVariant::Secondary }>
                    {if active { t!("state.enabled") } else { t!("state.disabled") }}
                </Badge>
            </CardHeader>

            <CardContent class="space-y-2.5">
                <dl class="grid grid-cols-2 gap-x-3 gap-y-1.5 text-xs">
                    <div class="flex items-center gap-1.5">
                        <dt class="text-muted-foreground">{t!("pools.col_type")}</dt>
                        <dd class="font-mono text-foreground">{row.proxy_type.clone()}</dd>
                    </div>
                    <div class="flex items-center gap-1.5">
                        <dt class="text-muted-foreground">{t!("pools.col_bound")}</dt>
                        <dd class="font-mono tabular-nums text-foreground">
                            {bound.to_string()}
                        </dd>
                    </div>
                    <div class="col-span-2 flex items-center gap-1.5">
                        <dt class="text-muted-foreground">{t!("pools.test_status")}</dt>
                        <dd class="font-mono text-foreground">{row.test_status.clone()}</dd>
                    </div>
                    <div class="col-span-2 flex items-center gap-1.5">
                        <dt class="text-muted-foreground">{t!("pools.col_updated")}</dt>
                        <dd class="font-mono tabular-nums text-muted-foreground">
                            {timestamp_label(&row.updated_at)}
                        </dd>
                    </div>
                    {no_proxy
                        .map(|list| {
                            view! {
                                <div class="col-span-2 space-y-0.5">
                                    <dt class="text-muted-foreground">{t!("pools.no_proxy")}</dt>
                                    <dd>
                                        <Mono value=list class="text-xs text-muted-foreground" />
                                    </dd>
                                </div>
                            }
                        })}
                </dl>

                {strict
                    .then(|| {
                        view! {
                            <div class="flex items-center gap-1.5 text-xs text-foreground">
                                <Icon name=IconName::ShieldCheck class="size-3.5 text-muted-foreground" />
                                {t!("pools.strict")}
                            </div>
                        }
                    })}

                {last_error
                    .map(|message| view! { <ErrorNote message=message /> })}

                {move || test_feedback.get().map(|feedback| view! {
                    <p class="font-mono text-xs text-muted-foreground">{feedback}</p>
                })}

                // Named before the delete is attempted: the store refuses while anything is bound,
                // and the count in the list above is the reason. A paragraph rather than a badge --
                // the sentence is the whole point of the warning and a badge does not wrap.
                {(bound > 0)
                    .then(|| {
                        view! {
                            <p class="flex items-start gap-1.5 text-xs leading-relaxed text-warning-emphasis">
                                <Icon
                                    name=IconName::Alert
                                    class="mt-px size-3.5 shrink-0 text-warning"
                                />
                                <span>{t!("pools.bound_warning")}</span>
                            </p>
                        }
                    })}

                {move || {
                    save.get().message().map(|message| view! { <ErrorNote message=message /> })
                }}
            </CardContent>

            <CardFooter class="flex-wrap">
                <div class="flex flex-wrap items-center gap-2">
                    // Rebuilt inside a closure because `Button::label` is an `Option<String>`, not a
                    // signal: the "Testing…" wording has to be chosen on each render, not once at mount.
                    {move || {
                        view! {
                            <Button
                                label=if testing.get() {
                                    t!("pools.testing")
                                } else {
                                    t!("pools.test")
                                }
                                variant=ButtonVariant::Outline
                                size=ButtonSize::Sm
                                disabled=Signal::derive(move || {
                                    testing.get() || save.get().is_saving()
                                })
                                on_click=Callback::new(move |()| test_action())
                            />
                        }
                    }}
                    <Button
                        label=if active { t!("pools.disable") } else { t!("pools.enable") }
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        disabled=Signal::derive(move || save.get().is_saving())
                        on_click=Callback::new(move |()| toggle())
                    />
                </div>

                // The confirm replaces the delete rather than sitting beside it, so the destructive
                // control is never one stray click away from a pool that cannot be restored.
                {move || {
                    if armed.get() {
                        view! {
                            <div class="flex items-center gap-2">
                                <Button
                                    label=t!("pools.confirm_delete")
                                    variant=ButtonVariant::Destructive
                                    size=ButtonSize::Sm
                                    disabled=Signal::derive(move || save.get().is_saving())
                                    on_click=Callback::new(move |()| remove())
                                />
                                <Button
                                    label=t!("pools.cancel")
                                    variant=ButtonVariant::Ghost
                                    size=ButtonSize::Sm
                                    on_click=Callback::new(move |()| set_armed.set(false))
                                />
                            </div>
                        }
                            .into_any()
                    } else {
                        view! {
                            <Button
                                label=t!("pools.delete")
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::Sm
                                on_click=Callback::new(move |()| set_armed.set(true))
                            />
                        }
                            .into_any()
                    }
                }}
            </CardFooter>
        </Card>
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

    /// Test-only: see [`Platform::title_text`], which is the form `t!` can use.
    #[cfg(test)]
    const fn title(self) -> &'static str {
        match self {
            Self::Cloudflare => "pools.deploy_cloudflare",
            Self::Deno => "pools.deploy_deno",
            Self::Vercel => "pools.deploy_vercel",
        }
    }

    /// The first field's label key. Not a secret for Cloudflare and Deno; the token for Vercel.
    ///
    /// Test-only, for the same reason as [`Platform::title`].
    #[cfg(test)]
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

    /// The title as a literal, for `t!`.
    ///
    /// [`Platform::title`] returns the key as a `&str` -- which the tests and the doc comment above
    /// rely on -- but `t!` only accepts a literal, so the literal lives here as a second match over
    /// the same variants. Both spellings are pinned together by the test at the bottom.
    fn title_text(self) -> String {
        match self {
            Self::Cloudflare => t!("pools.deploy_cloudflare"),
            Self::Deno => t!("pools.deploy_deno"),
            Self::Vercel => t!("pools.deploy_vercel"),
        }
    }

    /// The first field's label, for the same reason as [`Platform::title_text`].
    fn first_label_text(self) -> String {
        match self {
            Self::Cloudflare => t!("pools.cf_account"),
            Self::Deno => t!("pools.deno_org"),
            Self::Vercel => t!("pools.vercel_token"),
        }
    }

    /// The second field's label, when the platform has one.
    fn second_label_text(self) -> Option<String> {
        match self {
            Self::Cloudflare => Some(t!("pools.cf_token")),
            Self::Deno => Some(t!("pools.deno_token")),
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
    view! {
        <div class="space-y-3">
            <div class="space-y-1">
                <p class="text-sm font-medium text-foreground">{t!("pools.deploy")}</p>
                <p class="text-xs leading-relaxed text-muted-foreground">
                    {t!("pools.deploy_hint")}
                </p>
            </div>
            <div class="grid gap-4 lg:grid-cols-3">
                {Platform::ALL
                    .into_iter()
                    .map(|platform| view! { <DeployCard platform=platform reload=reload /> })
                    .collect_view()}
            </div>
        </div>
    }
}

/// One platform's deploy form.
#[component]
fn DeployCard(
    platform: Platform,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
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
    let encode_failed = StoredValue::new(t!("pools.encode_failed"));

    let send = move || {
        let (one, two, name) = (first.get(), second.get(), project.get());
        let trimmed_project = name.trim();
        let Ok(body) = deploy_body(
            platform,
            one.trim(),
            two.trim(),
            (!trimmed_project.is_empty()).then_some(trimmed_project),
        ) else {
            set_save.set(Save::Refused(encode_failed.get_value()));
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
    let second_label = platform.second_label_text();
    let title = platform.title_text();
    let first_label = platform.first_label_text();

    view! {
        <Card>
            <CardHeader>
                <CardTitle>{title}</CardTitle>
                <Icon name=IconName::Globe class="size-4 text-muted-foreground" />
            </CardHeader>
            <CardContent class="space-y-3">
                <Field label=first_label>
                    <Input
                        input_type=first_type
                        value=Signal::derive(move || first.get())
                        on_input=Callback::new(move |value| set_first.set(value))
                        autocomplete="off"
                        class="font-mono"
                    />
                </Field>
                {second_label
                    .map(|label| {
                        view! {
                            <Field label=label>
                                <Input
                                    input_type="password"
                                    value=Signal::derive(move || second.get())
                                    on_input=Callback::new(move |value| set_second.set(value))
                                    autocomplete="off"
                                    class="font-mono"
                                />
                            </Field>
                        }
                    })}
                <Field label=t!("pools.project_name")>
                    <Input
                        value=Signal::derive(move || project.get())
                        on_input=Callback::new(move |value| set_project.set(value))
                        placeholder=t!("pools.project_placeholder")
                        autocomplete="off"
                        class="font-mono"
                    />
                </Field>

                {move || {
                    save.get().message().map(|message| view! { <ErrorNote message=message /> })
                }}

                {move || {
                    deployed
                        .get()
                        .map(|url| {
                            if url.is_empty() {
                                // A 201 with no `deployUrl` still created the pool; saying so beats
                                // rendering an empty link.
                                return view! {
                                    <p class="text-[0.8125rem] text-foreground">
                                        {t!("pools.deployed")}
                                    </p>
                                }
                                    .into_any();
                            }
                            let href = url.clone();
                            view! {
                                <div class="space-y-1">
                                    <p class="text-[0.8125rem] text-foreground">
                                        {t!("pools.deployed")}
                                    </p>
                                    <a
                                        href=href
                                        target="_blank"
                                        rel="noreferrer noopener"
                                        class="block break-all font-mono text-xs text-muted-foreground \
                                               underline-offset-4 hover:text-foreground hover:underline"
                                    >
                                        {url}
                                    </a>
                                </div>
                            }
                                .into_any()
                        })
                }}
            </CardContent>
            <CardFooter>
                // `label` is an `Option<String>`, so the in-flight wording is chosen per render
                // rather than passed as a signal.
                {move || {
                    view! {
                        <Button
                            label=if save.get().is_saving() {
                                t!("pools.deploying")
                            } else {
                                t!("pools.deploy_run")
                            }
                            class="w-full"
                            size=ButtonSize::Sm
                            disabled=Signal::derive(incomplete)
                            on_click=Callback::new(move |()| send())
                        />
                    }
                }}
            </CardFooter>
        </Card>
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

    /// The literal labels beside the key-returning ones.
    ///
    /// `t!` only accepts a literal, so [`Platform::title_text`] and its siblings re-state each key as
    /// a literal match instead of looking one up. That is two places to keep in step, and this test is
    /// what keeps them in step: the message each literal resolves to has to be the same one its key
    /// does, which fails the moment a key is renamed in only one of the two matches.
    #[test]
    fn the_literal_label_matches_the_key_it_replaces() {
        use leptos::prelude::*;

        // `t!` reads the locale from context, so a context has to exist for this test to exercise the
        // real path rather than a copy of it.
        let owner = Owner::new();
        owner.set();
        crate::i18n::provide_embedded_locale();
        let locale = crate::i18n::use_locale();

        for platform in Platform::ALL {
            assert_eq!(
                platform.title_text(),
                locale.get(platform.title()),
                "{platform:?} title"
            );
            assert_eq!(
                platform.first_label_text(),
                locale.get(platform.first_label()),
                "{platform:?} first field"
            );
            assert_eq!(
                platform.second_label_text(),
                platform
                    .second_label()
                    .map(|key| locale.get(key).to_owned()),
                "{platform:?} second field"
            );
        }
    }
}
