//! Cloudflare Quick Tunnels and Tailscale Funnel: what this machine exposes, and the controls
//! that change it.
//!
//! One thing about this panel is not visible from its contents: every route under `/api/tunnel` is
//! loopback-only at the gateway. Opened *through* the tunnel it is describing, every read here
//! returns 403, and `ApiError`'s generic "This action is not permitted." gives no hint that the fix
//! is to reopen the dashboard on the host. So a 403 gets its own explanation here rather than the
//! shared one.
//!
//! Enabling is two-step. A quick tunnel publishes this machine to the public internet from a single
//! click, and the confirm step names that consequence before it happens. Disabling is one click:
//! reducing exposure needs no ceremony.
//!
//! `POST /api/tunnel/operations/{id}` is deliberately not wired -- see [`OperationsCard`].

use leptos::prelude::*;
use serde::Deserialize;

use crate::api::{ApiError, Hydrate, Save, encode, load};
use crate::routes::{PageHeader, Panel};

/// `GET /api/tunnel/status`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TunnelStatus {
    #[serde(default)]
    tunnel: CloudflareState,
    #[serde(default)]
    tailscale: TailscaleState,
    #[serde(default)]
    download: DownloadState,
}

/// The Cloudflare half. `state` is the supervisor's: `stopped`, `starting`, `running`,
/// `stopping`, `backoff`, `failed`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudflareState {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    url: String,
    #[serde(default)]
    running: bool,
    #[serde(default)]
    state: String,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    restarts: u32,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    installed: bool,
}
/// The Tailscale half.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TailscaleState {
    #[serde(default)]
    installed: bool,
    #[serde(default)]
    logged_in: bool,
    #[serde(default)]
    daemon_running: bool,
    #[serde(default)]
    url: String,
    #[serde(default)]
    funnel_active: bool,
    #[serde(default)]
    state: String,
    /// Where a pending login has to be completed, in a browser.
    #[serde(default)]
    auth_url: Option<String>,
}

/// Always "not downloading": this router never fetches a tunnel binary. The message says why, so
/// the panel shows a reason instead of an idle progress bar.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadState {
    #[serde(default)]
    in_progress: bool,
    #[serde(default)]
    message: String,
}

/// `GET /api/tunnel/tailscale-check`.
///
/// `brew_available` and `has_cached_password` are always false on this build and are shown anyway:
/// they are how the panel reports that nothing here installs software or holds a sudo password.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TailscaleCheck {
    #[serde(default)]
    installed: bool,
    #[serde(default)]
    logged_in: bool,
    #[serde(default)]
    platform: String,
    #[serde(default)]
    brew_available: bool,
    #[serde(default)]
    daemon_running: bool,
    #[serde(default)]
    custom_daemon_running: bool,
    #[serde(default)]
    system_daemon_running: bool,
    #[serde(default)]
    has_cached_password: bool,
    #[serde(default)]
    daemon_installed: bool,
}
/// `GET /api/tunnel/operations` -- the catalog, plus which binaries back it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Operations {
    #[serde(default)]
    operations: Vec<OperationRow>,
    #[serde(default)]
    tools: Vec<ToolRow>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationRow {
    #[serde(default)]
    id: String,
    #[serde(default)]
    about: String,
    #[serde(default)]
    tool: String,
    /// `read` or `mutate`.
    #[serde(default)]
    effect: String,
    /// `oneShot` or `supervised`.
    #[serde(default)]
    mode: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    params: Vec<ParamRow>,
    #[serde(default)]
    available: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParamRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    secret: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolRow {
    #[serde(default)]
    id: String,
    #[serde(default)]
    installed: bool,
    #[serde(default)]
    path: Option<String>,
}
/// What every mutation on this panel answers with.
///
/// `tunnelUrl`, `authUrl` and `enableUrl` are each omitted rather than sent empty, so `Option` is
/// the shape and absence is not read as a blank URL.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MutationResult {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    tunnel_url: Option<String>,
    #[serde(default)]
    auth_url: Option<String>,
    /// Set when the tailnet has Funnel switched off and an admin has to allow it.
    #[serde(default)]
    enable_url: Option<String>,
    #[serde(default)]
    needs_login: bool,
    #[serde(default)]
    message: String,
}

/// A `{"port": u16}` body. Both enable routes accept it; both default when it is absent.
#[derive(Debug, serde::Serialize)]
struct PortBody {}

/// A `{"token": "..."}` body for a named, remotely-managed tunnel.
#[derive(Debug, serde::Serialize)]
struct TokenBody<'a> {
    token: &'a str,
}

/// Run one mutation, keeping the server's own sentence whichever way it went.
///
/// Not [`crate::api::submit_reporting`]: that reads a refusal's wording from an `error` field, and
/// these routes put theirs in `message` -- every one of them answers with a [`MutationResult`],
/// refusals included, so a 502 carrying "cloudflared started but never announced a tunnel URL"
/// would otherwise be flattened to "The router returned an error."
///
/// `Save` is left to report only the transport, and `result` holds the server's answer. Status is
/// refetched either way: a failed enable can still have changed what the supervisor is doing.
fn run(
    path: &'static str,
    body: String,
    save: WriteSignal<Save>,
    result: WriteSignal<Option<MutationResult>>,
    reload: impl Fn() + Copy + 'static,
) {
    result.set(None);
    save.set(Save::Saving);
    leptos::task::spawn_local(async move {
        match crate::api::request_detailed(crate::api::Method::Post, path, Some(&body)).await {
            Ok(response) => {
                let mut answer =
                    crate::api::decode::<MutationResult>(&response.body).unwrap_or_default();
                // An unreadable or empty body still has a status worth reporting; without this the
                // panel would show a blank line where the explanation should be.
                if answer.message.is_empty() {
                    ApiError::Status(response.status)
                        .message()
                        .clone_into(&mut answer.message);
                }
                save.set(Save::Saved);
                result.set(Some(answer));
                reload();
            }
            Err(error) => save.set(Save::Failed(error)),
        }
    });
}

#[component]
pub fn Tunnel() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (status, set_status) = signal(Hydrate::<TunnelStatus>::Loading);
    let (check, set_check) = signal(Hydrate::<TailscaleCheck>::Loading);
    let (operations, set_operations) = signal(Hydrate::<Operations>::Loading);

    let reload = move || {
        set_status.set(Hydrate::Loading);
        load("/api/tunnel/status", set_status);
    };
    let reload_all = move || {
        reload();
        set_check.set(Hydrate::Loading);
        set_operations.set(Hydrate::Loading);
        load("/api/tunnel/tailscale-check", set_check);
        load("/api/tunnel/operations", set_operations);
    };
    reload_all();

    let host_only = locale.get("tunnel.host_only").to_owned();

    view! {
        <PageHeader
            title=locale.get("nav.tunnel").to_owned()
            description=locale.get("tunnel.description").to_owned()
        />

        // A 403 here means the request did not come from the host. That is the one failure on this
        // panel whose remedy is not guessable from the generic message, so it gets named.
        {move || {
            (status.get().failure() == Some(ApiError::Status(403)))
                .then(|| {
                    view! {
                        <p class="mb-4 rounded-lg border border-warning/40 bg-warning/5 p-4 text-sm">
                            {host_only.clone()}
                        </p>
                    }
                })
        }}

        <Panel
            state=status
            on_retry=Callback::new(move |()| reload())
            children=move |data: TunnelStatus| {
                view! {
                    <div class="grid gap-4 lg:grid-cols-2">
                        <CloudflareCard state=data.tunnel reload=reload />
                        <TailscaleCard state=data.tailscale reload=reload />
                    </div>
                    <InstallNote download=data.download />
                }
            }
        />

        <div class="mt-4 grid gap-4">
            <Panel
                state=check
                on_retry=Callback::new(move |()| reload_all())
                children=|data: TailscaleCheck| view! { <EnvironmentCard check=data /> }
            />
            <Panel
                state=operations
                on_retry=Callback::new(move |()| reload_all())
                children=|data: Operations| view! { <OperationsCard data=data /> }
            />
        </div>
    }
}
/// Cloudflare: the quick tunnel, the named tunnel, and the supervisor's view of the child.
#[component]
fn CloudflareCard(
    state: CloudflareState,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (save, set_save) = signal(Save::Idle);
    let (result, set_result) = signal(None::<MutationResult>);
    let (token, set_token) = signal(String::new());

    let installed = state.installed;
    let running = state.running;
    let busy = Signal::derive(move || save.get().is_saving());

    let send =
        move |path: &'static str, body: String| run(path, body, set_save, set_result, reload);
    let port_body = move || encode(&PortBody {}).unwrap_or_else(|_| "{}".to_owned());

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <div class="flex items-center justify-between gap-3">
                <h2 class="text-sm font-medium text-muted-foreground">
                    {locale.get("tunnel.cloudflare").to_owned()}
                </h2>
                <StateBadge state=state.state.clone() on=state.enabled />
            </div>

            <dl class="space-y-2.5 text-sm">
                <Field label=locale.get("tunnel.installed").to_owned() value=yes_no(installed) />
                <Field label=locale.get("tunnel.running").to_owned() value=yes_no(running) />
                <Field
                    label=locale.get("tunnel.restarts").to_owned()
                    value=state.restarts.to_string()
                />
                {state
                    .pid
                    .map(|pid| {
                        view! {
                            <Field label=locale.get("tunnel.pid").to_owned() value=pid.to_string() />
                        }
                    })}
            </dl>

            {(!state.url.is_empty())
                .then(|| {
                    view! { <PublicUrl label=locale.get("tunnel.url").to_owned() url=state.url.clone() /> }
                })}
            {state
                .last_error
                .clone()
                .filter(|error| !error.is_empty())
                .map(|error| {
                    view! {
                        <p class="text-sm text-destructive">
                            {format!("{}: {error}", locale.get("tunnel.last_error"))}
                        </p>
                    }
                })}

            {if installed {
                view! {
                    <div class="space-y-3 border-t border-border pt-4">
                        <Confirm
                            label=locale.get("tunnel.enable_quick").to_owned()
                            warning=locale.get("tunnel.public_warning").to_owned()
                            disabled=busy
                            on_confirm=Callback::new(move |()| {
                                send("/api/tunnel/enable", port_body());
                            })
                        />
                        {running
                            .then(|| {
                                view! {
                                    <button
                                        type="button"
                                        class="w-full rounded-md border border-border px-3 py-2 text-sm \
                                               font-medium transition-colors hover:bg-accent disabled:opacity-50"
                                        disabled=move || busy.get()
                                        on:click=move |_| send("/api/tunnel/disable", "{}".to_owned())
                                    >
                                        {locale.get("tunnel.disable").to_owned()}
                                    </button>
                                }
                            })}
                        <NamedTunnel token=token set_token=set_token busy=busy send=send />
                    </div>
                }
                    .into_any()
            } else {
                view! {
                    <p class="border-t border-border pt-4 text-sm text-muted-foreground">
                        {locale.get("tunnel.cloudflared_missing").to_owned()}
                    </p>
                }
                    .into_any()
            }}
            <Outcome save=save result=result />
        </section>
    }
}
/// A named, remotely-managed tunnel.
///
/// The token is a Cloudflare credential, so the field is a password input and the value is never
/// echoed back into the DOM after it is sent.
#[component]
fn NamedTunnel(
    token: ReadSignal<String>,
    set_token: WriteSignal<String>,
    busy: Signal<bool>,
    send: impl Fn(&'static str, String) + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let empty = Signal::derive(move || busy.get() || token.get().trim().is_empty());

    view! {
        <div class="space-y-2">
            <label class="block space-y-1 text-sm">
                <span class="text-muted-foreground">
                    {locale.get("tunnel.named_token").to_owned()}
                </span>
                <input
                    type="password"
                    autocomplete="off"
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                    prop:value=move || token.get()
                    on:input=move |ev| set_token.set(event_target_value(&ev))
                    placeholder=locale.get("tunnel.named_placeholder").to_owned()
                />
            </label>
            <Confirm
                label=locale.get("tunnel.enable_named").to_owned()
                warning=locale.get("tunnel.public_warning").to_owned()
                disabled=empty
                on_confirm=Callback::new(move |()| {
                    let value = token.get();
                    let trimmed = value.trim();
                    if trimmed.is_empty() {
                        return;
                    }
                    let Ok(body) = encode(&TokenBody { token: trimmed }) else {
                        return;
                    };
                    set_token.set(String::new());
                    send("/api/tunnel/named/enable", body);
                })
            />
        </div>
    }
}
/// Tailscale: the daemon, the login, and Funnel.
#[component]
fn TailscaleCard(
    state: TailscaleState,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (save, set_save) = signal(Save::Idle);
    let (result, set_result) = signal(None::<MutationResult>);

    let installed = state.installed;
    let funnel_active = state.funnel_active;
    let busy = Signal::derive(move || save.get().is_saving());

    let send = move |path: &'static str| run(path, "{}".to_owned(), set_save, set_result, reload);

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <div class="flex items-center justify-between gap-3">
                <h2 class="text-sm font-medium text-muted-foreground">
                    {locale.get("tunnel.tailscale").to_owned()}
                </h2>
                <StateBadge state=state.state.clone() on=funnel_active />
            </div>

            <dl class="space-y-2.5 text-sm">
                <Field label=locale.get("tunnel.installed").to_owned() value=yes_no(installed) />
                <Field
                    label=locale.get("tunnel.daemon_running").to_owned()
                    value=yes_no(state.daemon_running)
                />
                <Field
                    label=locale.get("tunnel.logged_in").to_owned()
                    value=yes_no(state.logged_in)
                />
                <Field
                    label=locale.get("tunnel.funnel_active").to_owned()
                    value=yes_no(funnel_active)
                />
            </dl>

            {(!state.url.is_empty())
                .then(|| {
                    view! { <PublicUrl label=locale.get("tunnel.url").to_owned() url=state.url.clone() /> }
                })}
            // A pending login can only be finished in a browser, so the URL is the control.
            {state
                .auth_url
                .clone()
                .filter(|url| !url.is_empty())
                .map(|url| {
                    view! {
                        <a
                            href=url
                            target="_blank"
                            rel="noreferrer noopener"
                            class="block rounded-md border border-warning/40 bg-warning/5 px-3 py-2 \
                                   text-sm font-medium underline-offset-4 hover:underline"
                        >
                            {locale.get("tunnel.finish_login").to_owned()}
                        </a>
                    }
                })}

            {if installed {
                view! {
                    <div class="space-y-3 border-t border-border pt-4">
                        <Confirm
                            label=locale.get("tunnel.enable_funnel").to_owned()
                            warning=locale.get("tunnel.public_warning").to_owned()
                            disabled=busy
                            on_confirm=Callback::new(move |()| send("/api/tunnel/tailscale-enable"))
                        />
                        {funnel_active
                            .then(|| {
                                view! {
                                    <button
                                        type="button"
                                        class="w-full rounded-md border border-border px-3 py-2 text-sm \
                                               font-medium transition-colors hover:bg-accent disabled:opacity-50"
                                        disabled=move || busy.get()
                                        on:click=move |_| send("/api/tunnel/tailscale-disable")
                                    >
                                        {locale.get("tunnel.withdraw_funnel").to_owned()}
                                    </button>
                                }
                            })}
                    </div>
                }
                    .into_any()
            } else {
                view! {
                    <p class="border-t border-border pt-4 text-sm text-muted-foreground">
                        {locale.get("tunnel.tailscale_missing").to_owned()}
                    </p>
                }
                    .into_any()
            }}
            <Outcome save=save result=result />
        </section>
    }
}
/// A two-step control for an action that publishes this machine.
///
/// The first click only arms it; the second sends. The intermediate state is where the consequence
/// is spelled out, because "Enable" on its own does not say that the result is a public URL into
/// this host.
#[component]
fn Confirm(
    label: String,
    warning: String,
    #[prop(into)] disabled: Signal<bool>,
    on_confirm: Callback<()>,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (armed, set_armed) = signal(false);
    let confirm = locale.get("tunnel.confirm").to_owned();
    let cancel = locale.get("tunnel.cancel").to_owned();

    view! {
        {move || {
            if armed.get() {
                let (confirm, cancel) = (confirm.clone(), cancel.clone());
                view! {
                    <div class="space-y-2 rounded-md border border-warning/40 bg-warning/5 p-3 \
                                animate-in fade-in duration-150">
                        <p class="text-sm">{warning.clone()}</p>
                        <div class="flex gap-2">
                            <button
                                type="button"
                                class="flex-1 rounded-md bg-destructive px-3 py-2 text-sm font-medium \
                                       text-destructive-foreground disabled:opacity-50"
                                disabled=move || disabled.get()
                                on:click=move |_| {
                                    set_armed.set(false);
                                    on_confirm.run(());
                                }
                            >
                                {confirm}
                            </button>
                            <button
                                type="button"
                                class="flex-1 rounded-md border border-border px-3 py-2 text-sm \
                                       font-medium transition-colors hover:bg-accent"
                                on:click=move |_| set_armed.set(false)
                            >
                                {cancel}
                            </button>
                        </div>
                    </div>
                }
                    .into_any()
            } else {
                view! {
                    <button
                        type="button"
                        class="w-full rounded-md bg-primary px-3 py-2 text-sm font-medium \
                               text-primary-foreground disabled:opacity-50"
                        disabled=move || disabled.get()
                        on:click=move |_| set_armed.set(true)
                    >
                        {label.clone()}
                    </button>
                }
                    .into_any()
            }
        }}
    }
}
/// How the last mutation ended: the transport failure, or the server's own answer.
///
/// A `success: false` body is rendered as a failure even though the request itself was a 200, which
/// is the case a plain `Save::Saved` would report as a win.
#[component]
fn Outcome(save: ReadSignal<Save>, result: ReadSignal<Option<MutationResult>>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let enable_funnel = locale.get("tunnel.allow_funnel").to_owned();

    view! {
        {move || {
            save.get()
                .message()
                .map(|message| {
                    view! { <p class="text-sm text-destructive">{message}</p> }
                })
        }}
        {move || {
            result
                .get()
                .map(|outcome| {
                    let class = if outcome.success {
                        "text-sm text-foreground"
                    } else {
                        "text-sm text-destructive"
                    };
                    let url = outcome
                        .tunnel_url
                        .or(outcome.auth_url)
                        .filter(|url| !url.is_empty());
                    let allow = outcome.enable_url.filter(|url| !url.is_empty());
                    // Rendered rather than inferred from the presence of `auth_url`: the server can
                    // report that a sign-in is needed without yet having a URL to send you to, and a
                    // panel that only watches the URL shows nothing at all in that window.
                    let needs_login = outcome.needs_login;
                    let enable_funnel = enable_funnel.clone();
                    view! {
                        <div class="space-y-1.5">
                            <p class=class>{outcome.message}</p>
                            {if needs_login {
                                view! {
                                    <p class="text-xs text-muted-foreground">
                                        {locale.get("tunnel.needs_login").to_owned()}
                                    </p>
                                }
                                    .into_any()
                            } else {
                                ().into_any()
                            }}
                            {url
                                .map(|url| {
                                    let href = url.clone();
                                    view! {
                                        <a
                                            href=href
                                            target="_blank"
                                            rel="noreferrer noopener"
                                            class="block break-all font-mono text-xs underline-offset-4 hover:underline"
                                        >
                                            {url}
                                        </a>
                                    }
                                })}
                            {allow
                                .map(|url| {
                                    view! {
                                        <a
                                            href=url
                                            target="_blank"
                                            rel="noreferrer noopener"
                                            class="block text-sm font-medium underline-offset-4 hover:underline"
                                        >
                                            {enable_funnel}
                                        </a>
                                    }
                                })}
                        </div>
                    }
                })
        }}
    }
}
/// A live public URL, marked as one.
#[component]
fn PublicUrl(label: String, url: String) -> impl IntoView {
    let href = url.clone();
    view! {
        <div class="rounded-md border border-success/40 bg-success/5 p-3 space-y-1">
            <p class="text-xs font-medium text-muted-foreground">{label}</p>
            <a
                href=href
                target="_blank"
                rel="noreferrer noopener"
                class="block break-all font-mono text-sm underline-offset-4 hover:underline"
            >
                {url}
            </a>
        </div>
    }
}

/// One label/value pair.
#[component]
fn Field(label: String, value: String) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between gap-4">
            <dt class="text-muted-foreground truncate">{label}</dt>
            <dd class="shrink-0">{value}</dd>
        </div>
    }
}

/// The supervisor's state, with a dot whose colour tracks whether anything is exposed.
#[component]
fn StateBadge(state: String, on: bool) -> impl IntoView {
    let label = if state.is_empty() {
        "unknown".to_owned()
    } else {
        state
    };
    view! {
        <span class="flex items-center gap-2 shrink-0 text-xs">
            <span class=if on {
                "size-1.5 rounded-full bg-success"
            } else {
                "size-1.5 rounded-full bg-muted-foreground/40"
            } />
            <span class="font-mono text-muted-foreground">{label}</span>
        </span>
    }
}

/// Why no binary will be downloaded. Shown always, because it is the answer to "why is there no
/// install button".
#[component]
fn InstallNote(download: DownloadState) -> impl IntoView {
    if download.message.is_empty() {
        return ().into_any();
    }
    let locale = crate::i18n::use_locale();
    // `in_progress` is false on this build, which never fetches a binary. Rendered rather than
    // ignored because a server that starts reporting a download must not do so into a panel that
    // silently drops the flag.
    let downloading = download.in_progress;
    view! {
        <p class="mt-4 rounded-lg border border-border bg-card p-4 text-sm text-muted-foreground">
            {if downloading {
                view! {
                    <span class="mr-2 font-medium text-foreground">
                        {locale.get("tunnel.downloading").to_owned()}
                    </span>
                }
                    .into_any()
            } else {
                ().into_any()
            }} {download.message}
        </p>
    }
    .into_any()
}
/// What `tailscale-check` found on this host.
#[component]
fn EnvironmentCard(check: TailscaleCheck) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let platform = if check.platform.is_empty() {
        locale.get("state.unknown").to_owned()
    } else {
        check.platform
    };

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("tunnel.environment").to_owned()}
            </h2>
            <dl class="grid gap-2.5 text-sm sm:grid-cols-2">
                <Field label=locale.get("tunnel.platform").to_owned() value=platform />
                <Field
                    label=locale.get("tunnel.cli_installed").to_owned()
                    value=yes_no(check.installed)
                />
                <Field
                    label=locale.get("tunnel.daemon_installed").to_owned()
                    value=yes_no(check.daemon_installed)
                />
                <Field
                    label=locale.get("tunnel.daemon_running").to_owned()
                    value=yes_no(check.daemon_running)
                />
                <Field
                    label=locale.get("tunnel.custom_daemon").to_owned()
                    value=yes_no(check.custom_daemon_running)
                />
                <Field
                    label=locale.get("tunnel.system_daemon").to_owned()
                    value=yes_no(check.system_daemon_running)
                />
                <Field
                    label=locale.get("tunnel.logged_in").to_owned()
                    value=yes_no(check.logged_in)
                />
                <Field
                    label=locale.get("tunnel.brew_available").to_owned()
                    value=yes_no(check.brew_available)
                />
                <Field
                    label=locale.get("tunnel.cached_password").to_owned()
                    value=yes_no(check.has_cached_password)
                />
            </dl>
        </section>
    }
}
/// The operation catalog, as inventory.
///
/// Read-only on purpose. `POST /api/tunnel/operations/{id}` exists and works, but running one from
/// here would be a control whose side effects its own label does not describe: every Tailscale
/// operation except `tailscale.version` calls `ensure_daemon()` first, so a row marked
/// `effect: read` -- `tailscale.status`, say -- starts a daemon as a precondition. A button labelled
/// with a read that starts a process is the kind of quiet mismatch this dashboard is meant not to
/// ship, and the operations worth reaching are already the named controls above.
#[component]
fn OperationsCard(data: Operations) -> impl IntoView {
    let locale = crate::i18n::use_locale();

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("tunnel.operations").to_owned()}
            </h2>
            <p class="text-sm text-muted-foreground">
                {locale.get("tunnel.operations_hint").to_owned()}
            </p>

            <ul class="flex flex-wrap gap-2">
                {data
                    .tools
                    .into_iter()
                    .map(|tool| {
                        let detail = tool.path.filter(|path| !path.is_empty());
                        view! {
                            <li class="flex items-center gap-2 rounded-md border border-border px-3 py-1.5 text-xs">
                                <span class=if tool.installed {
                                    "size-1.5 rounded-full bg-success"
                                } else {
                                    "size-1.5 rounded-full bg-muted-foreground/40"
                                } />
                                <span class="font-medium">{tool.id}</span>
                                {detail
                                    .map(|path| {
                                        view! {
                                            <code class="text-muted-foreground truncate max-w-56">{path}</code>
                                        }
                                    })}
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
            <OperationsTable rows=data.operations />
        </section>
    }
}
#[component]
fn OperationsTable(rows: Vec<OperationRow>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">{locale.get("tunnel.no_operations").to_owned()}</p>
        }
        .into_any();
    }
    view! {
        <div class="rounded-lg border border-border overflow-x-auto">
            <table class="w-full text-sm">
                <thead class="bg-muted/50 text-muted-foreground">
                    <tr>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("tunnel.col_operation").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("tunnel.col_tool").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("tunnel.col_effect").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("tunnel.col_mode").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("tunnel.col_params").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("tunnel.col_timeout").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("tunnel.col_available").to_owned()}
                        </th>
                    </tr>
                </thead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| view! { <OperationLine row=row /> })
                        .collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}
#[component]
fn OperationLine(row: OperationRow) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    // A secret parameter is marked as one: it is the difference between a port and a Cloudflare
    // token, and it decides whether a value can be pasted anywhere it might be logged.
    let params = row
        .params
        .iter()
        .map(|param| {
            let mut name = param.name.clone();
            if param.required {
                name.push('*');
            }
            if param.secret {
                name.push_str(" (secret)");
            }
            name
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mutating = row.effect == "mutate";

    view! {
        <tr class="border-t border-border">
            <td class="px-3 py-2">
                <div class="space-y-0.5">
                    <code class="text-xs">{row.id}</code>
                    <p class="text-xs text-muted-foreground">{row.about}</p>
                </div>
            </td>
            <td class="px-3 py-2 font-mono text-xs text-muted-foreground">{row.tool}</td>
            <td class="px-3 py-2">
                <span class=if mutating {
                    "rounded px-1.5 py-0.5 text-xs bg-warning/15 text-foreground"
                } else {
                    "rounded px-1.5 py-0.5 text-xs bg-muted text-muted-foreground"
                }>{row.effect}</span>
            </td>
            <td class="px-3 py-2 font-mono text-xs text-muted-foreground">{row.mode}</td>
            <td class="px-3 py-2 text-xs text-muted-foreground">{params}</td>
            <td class="px-3 py-2 font-mono text-xs text-muted-foreground">
                {row.timeout_ms.map_or_else(|| "-".to_owned(), |ms| format!("{ms} ms"))}
            </td>
            <td class="px-3 py-2">
                {if row.available {
                    locale.get("state.yes").to_owned()
                } else {
                    locale.get("state.no").to_owned()
                }}
            </td>
        </tr>
    }
}

/// A boolean as a word, through the locale.
fn yes_no(value: bool) -> String {
    let locale = crate::i18n::use_locale();
    if value {
        locale.get("state.yes").to_owned()
    } else {
        locale.get("state.no").to_owned()
    }
}
#[cfg(test)]
mod tests {
    use super::{MutationResult, Operations, TailscaleCheck, TunnelStatus};

    /// `GET /api/tunnel/status`, captured from the running router.
    const LIVE_STATUS: &str = r#"{
        "tunnel": {
            "enabled": false, "url": "", "running": false, "state": "stopped",
            "pid": null, "restarts": 0, "lastError": null, "installed": true
        },
        "tailscale": {
            "installed": false, "loggedIn": false, "daemonRunning": false, "url": "",
            "funnelActive": false, "state": "stopped", "authUrl": null
        },
        "download": {
            "inProgress": false,
            "message": "nullrouter never downloads tunnel binaries; install cloudflared or tailscale yourself"
        }
    }"#;

    /// `GET /api/tunnel/tailscale-check`, captured from the running router.
    const LIVE_CHECK: &str = r#"{
        "installed": false, "loggedIn": false, "platform": "linux", "brewAvailable": false,
        "daemonRunning": false, "customDaemonRunning": false, "systemDaemonRunning": false,
        "hasCachedPassword": false, "daemonInstalled": false
    }"#;

    #[test]
    fn the_live_status_decodes_into_its_three_halves() {
        let parsed: TunnelStatus = serde_json::from_str(LIVE_STATUS).expect("must decode");
        assert!(parsed.tunnel.installed, "cloudflared is installed here");
        assert_eq!(parsed.tunnel.state, "stopped");
        assert_eq!(parsed.tunnel.pid, None);
        assert!(!parsed.tailscale.installed);
        assert!(parsed.tailscale.auth_url.is_none());
        assert!(!parsed.download.in_progress);
        assert!(parsed.download.message.contains("never downloads"));
    }

    #[test]
    fn a_null_pid_is_absence_rather_than_zero() {
        // Rendered, a zero pid would name a process that does not exist.
        let parsed: TunnelStatus = serde_json::from_str(LIVE_STATUS).expect("must decode");
        assert!(parsed.tunnel.pid.is_none());
    }

    #[test]
    fn the_live_check_decodes_with_its_platform() {
        let parsed: TailscaleCheck = serde_json::from_str(LIVE_CHECK).expect("must decode");
        assert_eq!(parsed.platform, "linux");
        assert!(!parsed.daemon_installed);
    }
    #[test]
    fn the_operations_catalog_decodes_with_its_params_and_availability() {
        // Trimmed from the live body. `available` is the field that decides whether a row is
        // reachable at all, and `secret` decides how its value may be handled.
        let body = r#"{
            "operations": [
                {
                    "id": "cloudflared.tunnel.run",
                    "about": "Run a named, remotely-managed tunnel",
                    "tool": "cloudflared", "effect": "mutate", "mode": "supervised",
                    "params": [{"name":"token","about":"token","required":true,"secret":true}],
                    "available": true
                },
                {
                    "id": "tailscale.version", "about": "Report the version",
                    "tool": "tailscale", "effect": "read", "mode": "oneShot",
                    "timeoutMs": 5000, "params": [], "available": false
                }
            ],
            "tools": [
                {"id": "cloudflared", "installed": true, "path": "/usr/local/bin/cloudflared"},
                {"id": "tailscale", "installed": false}
            ]
        }"#;
        let parsed: Operations = serde_json::from_str(body).expect("must decode");
        assert_eq!(parsed.operations.len(), 2);
        let named = parsed.operations.first().expect("first row");
        assert_eq!(named.effect, "mutate");
        assert!(named.params.first().is_some_and(|param| param.secret));
        assert!(parsed.operations.get(1).is_some_and(|row| !row.available));
        // A tool with no path still decodes; absence must not read as an empty install path.
        assert!(parsed.tools.get(1).is_some_and(|tool| tool.path.is_none()));
    }

    #[test]
    fn a_mutation_reports_the_url_it_established() {
        let body = r#"{"success": true, "tunnelUrl": "https://x.trycloudflare.com",
                       "message": "tunnel is up at https://x.trycloudflare.com"}"#;
        let parsed: MutationResult = serde_json::from_str(body).expect("must decode");
        assert!(parsed.success);
        assert_eq!(
            parsed.tunnel_url.as_deref(),
            Some("https://x.trycloudflare.com")
        );
        assert!(!parsed.needs_login);
    }

    #[test]
    fn a_pending_login_is_not_read_as_a_success() {
        // The server omits `tunnelUrl` and sets `needsLogin`; a panel that only checked for a 200
        // would report a working funnel here.
        let body = r#"{"success": false, "needsLogin": true,
                       "authUrl": "https://login.tailscale.com/a/abc",
                       "message": "log in to continue"}"#;
        let parsed: MutationResult = serde_json::from_str(body).expect("must decode");
        assert!(!parsed.success);
        assert!(parsed.needs_login);
        assert!(parsed.tunnel_url.is_none());
        assert_eq!(
            parsed.auth_url.as_deref(),
            Some("https://login.tailscale.com/a/abc")
        );
    }
}
