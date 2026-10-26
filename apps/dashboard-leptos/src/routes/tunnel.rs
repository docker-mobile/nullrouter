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
//!
//! # Layout
//!
//! The caution that both providers publish this dashboard comes first, above either provider, because
//! it applies to both and a warning inside a card is read as a warning about that card. Each provider
//! is a [`Card`] whose header carries its supervisor state and whose footer carries its controls, so
//! the read and the write on one provider are never confused with the other's. The two things this
//! page only reports -- the host environment and the operation catalog -- follow as single cards.

use leptos::prelude::*;
use serde::Deserialize;

use crate::api::{ApiError, Hydrate, Save, encode, load};
use crate::routes::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Badge, BadgeVariant, Button, ButtonVariant,
    Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle, ErrorNote, Field, Input,
    Mono, PageBody, PageHeader, Panel, StatusDot, StatusTone, Table, TableCell, TableEmpty,
    TableFrame, TableHead, TableHeaderCell, TableRow,
};
use crate::t;

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

    view! {
        <PageHeader title=t!("nav.tunnel") description=t!("tunnel.description") />

        <PageBody>
            // The caution sits above both providers, not inside one: a warning rendered inside the
            // Cloudflare card reads as a warning about Cloudflare, and `tunnel.public_warning` is
            // true of either provider.
            <Alert variant=AlertVariant::Warning>
                <AlertTitle>{t!("tunnel.exposure_title")}</AlertTitle>
                <AlertDescription>{t!("tunnel.public_warning")}</AlertDescription>
            </Alert>

            // A 403 here means the request did not come from the host. That is the one failure on this
            // panel whose remedy is not guessable from the generic message, so it gets named.
            {move || {
                (status.get().failure() == Some(ApiError::Status(403)))
                    .then(|| {
                        view! {
                            <Alert variant=AlertVariant::Warning>
                                <AlertTitle>{t!("tunnel.host_only_title")}</AlertTitle>
                                <AlertDescription>{t!("tunnel.host_only")}</AlertDescription>
                            </Alert>
                        }
                    })
            }}

            <Panel
                state=status
                on_retry=Callback::new(move |()| reload())
                children=move |data: TunnelStatus| {
                    view! {
                        <div class="space-y-4">
                            <div class="grid gap-4 lg:grid-cols-2">
                                <CloudflareCard state=data.tunnel reload=reload />
                                <TailscaleCard state=data.tailscale reload=reload />
                            </div>
                            <InstallNote download=data.download />
                        </div>
                    }
                }
            />

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
        </PageBody>
    }
}
/// Cloudflare: the quick tunnel, the named tunnel, and the supervisor's view of the child.
#[component]
fn CloudflareCard(
    state: CloudflareState,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (save, set_save) = signal(Save::Idle);
    let (result, set_result) = signal(None::<MutationResult>);
    let (token, set_token) = signal(String::new());

    let installed = state.installed;
    let running = state.running;
    let exposed = state.enabled;
    // Read out of the struct up front: `view!` moves whatever it names, and each field below is used
    // once in the tree but would otherwise be borrowed through `state` from inside a closure.
    let supervisor = state.state.clone();
    let url = state.url.clone();
    let last_error = state.last_error.clone().filter(|error| !error.is_empty());
    let restarts = state.restarts.to_string();
    let pid = state.pid;

    let busy = Signal::derive(move || save.get().is_saving());

    let send =
        move |path: &'static str, body: String| run(path, body, set_save, set_result, reload);
    let port_body = move || encode(&PortBody {}).unwrap_or_else(|_| "{}".to_owned());

    view! {
        <Card>
            <CardHeader>
                <div class="min-w-0">
                    <CardTitle>{t!("tunnel.cloudflare")}</CardTitle>
                </div>
                <StateBadge state=supervisor on=exposed />
            </CardHeader>

            <CardContent class="space-y-3">
                <dl class="space-y-2.5 text-sm">
                    <DetailFlag label=t!("tunnel.installed") on=installed />
                    <DetailFlag label=t!("tunnel.running") on=running />
                    <Detail label=t!("tunnel.restarts") value=restarts />
                    {pid
                        .map(|pid| {
                            view! { <Detail label=t!("tunnel.pid") value=pid.to_string() /> }
                        })}
                </dl>

                {(!url.is_empty())
                    .then(|| view! { <PublicUrl label=t!("tunnel.url") url=url /> })}
                {last_error
                    .map(|error| {
                        view! {
                            <ErrorNote
                                message=format!("{}: {error}", t!("tunnel.last_error"))
                            />
                        }
                    })}

                {(!installed)
                    .then(|| {
                        view! {
                            <Alert variant=AlertVariant::Muted>
                                <AlertDescription>
                                    {t!("tunnel.cloudflared_missing")}
                                </AlertDescription>
                            </Alert>
                        }
                    })}

                // The result of the last mutation is rendered here rather than inside the footer,
                // because the footer only exists while the binary is installed: a failure whose
                // refetch reports the binary as gone would otherwise be dropped on the floor.
                <Outcome save=save result=result />
            </CardContent>

            // The controls live in the footer so the card's read half and write half are visibly
            // separate. Both enable paths are two-step: the confirm names the consequence first.
            {installed
                .then(|| {
                    view! {
                        <CardFooter class="flex-col items-stretch gap-2.5">
                            <Confirm
                                label=t!("tunnel.enable_quick")
                                warning=t!("tunnel.public_warning")
                                disabled=busy
                                on_confirm=Callback::new(move |()| {
                                    send("/api/tunnel/enable", port_body());
                                })
                            />
                            {running
                                .then(|| {
                                    view! {
                                        <Button
                                            class="w-full"
                                            label=t!("tunnel.disable")
                                            variant=ButtonVariant::Outline
                                            disabled=busy
                                            on_click=Callback::new(move |()| {
                                                send("/api/tunnel/disable", "{}".to_owned());
                                            })
                                        />
                                    }
                                })}
                            <NamedTunnel
                                token=token
                                set_token=set_token
                                busy=busy
                                send=send
                            />
                        </CardFooter>
                    }
                })}
        </Card>
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
    let empty = Signal::derive(move || busy.get() || token.get().trim().is_empty());

    view! {
        <div class="w-full space-y-2">
            <Field label=t!("tunnel.named_token")>
                <Input
                    input_type="password"
                    autocomplete="off"
                    value=Signal::derive(move || token.get())
                    on_input=Callback::new(move |value| set_token.set(value))
                    placeholder=t!("tunnel.named_placeholder")
                    class="font-mono text-xs"
                />
            </Field>
            <Confirm
                label=t!("tunnel.enable_named")
                warning=t!("tunnel.public_warning")
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
    let (save, set_save) = signal(Save::Idle);
    let (result, set_result) = signal(None::<MutationResult>);

    let installed = state.installed;
    let funnel_active = state.funnel_active;
    let daemon_running = state.daemon_running;
    let logged_in = state.logged_in;
    let supervisor = state.state.clone();
    let url = state.url.clone();
    let auth_url = state.auth_url.filter(|url| !url.is_empty());

    let busy = Signal::derive(move || save.get().is_saving());

    let send = move |path: &'static str| run(path, "{}".to_owned(), set_save, set_result, reload);

    view! {
        <Card>
            <CardHeader>
                <div class="min-w-0">
                    <CardTitle>{t!("tunnel.tailscale")}</CardTitle>
                </div>
                <StateBadge state=supervisor on=funnel_active />
            </CardHeader>

            <CardContent class="space-y-3">
                <dl class="space-y-2.5 text-sm">
                    <DetailFlag label=t!("tunnel.installed") on=installed />
                    <DetailFlag label=t!("tunnel.daemon_running") on=daemon_running />
                    <DetailFlag label=t!("tunnel.logged_in") on=logged_in />
                    <DetailFlag label=t!("tunnel.funnel_active") on=funnel_active />
                </dl>

                {(!url.is_empty())
                    .then(|| view! { <PublicUrl label=t!("tunnel.url") url=url /> })}
                // A pending login can only be finished in a browser, so the URL is the control -- and
                // it is a warning rather than a button because clicking it leaves this page.
                {auth_url
                    .map(|url| {
                        view! {
                            <Alert variant=AlertVariant::Warning>
                                <AlertTitle>{t!("tunnel.needs_login")}</AlertTitle>
                                <AlertDescription>
                                    <a
                                        href=url
                                        target="_blank"
                                        rel="noreferrer noopener"
                                        class="inline-flex items-center gap-1 font-medium text-foreground underline underline-offset-4"
                                    >
                                        {t!("tunnel.finish_login")}
                                    </a>
                                </AlertDescription>
                            </Alert>
                        }
                    })}

                {(!installed)
                    .then(|| {
                        view! {
                            <Alert variant=AlertVariant::Muted>
                                <AlertDescription>{t!("tunnel.tailscale_missing")}</AlertDescription>
                            </Alert>
                        }
                    })}

                // Outside the footer for the same reason as the Cloudflare card: the controls only
                // exist while the binary is installed, and a failure must outlive that.
                <Outcome save=save result=result />
            </CardContent>

            {installed
                .then(|| {
                    view! {
                        <CardFooter class="flex-col items-stretch gap-2.5">
                            <Confirm
                                label=t!("tunnel.enable_funnel")
                                warning=t!("tunnel.public_warning")
                                disabled=busy
                                on_confirm=Callback::new(move |()| send("/api/tunnel/tailscale-enable"))
                            />
                            {funnel_active
                                .then(|| {
                                    view! {
                                        <Button
                                            class="w-full"
                                            label=t!("tunnel.withdraw_funnel")
                                            variant=ButtonVariant::Outline
                                            disabled=busy
                                            on_click=Callback::new(move |()| {
                                                send("/api/tunnel/tailscale-disable");
                                            })
                                        />
                                    }
                                })}
                        </CardFooter>
                    }
                })}
        </Card>
    }
}
/// A two-step control for an action that publishes this machine.
///
/// The first click only arms it; the second sends. The intermediate state is where the consequence
/// is spelled out, because "Enable" on its own does not say that the result is a public URL into
/// this host. Arming renders an [`Alert`] rather than a recoloured button: the sentence has to be
/// read, and a button that changes colour is a button people click twice.
#[component]
fn Confirm(
    label: String,
    warning: String,
    #[prop(into)] disabled: Signal<bool>,
    on_confirm: Callback<()>,
) -> impl IntoView {
    let (armed, set_armed) = signal(false);

    view! {
        {move || {
            if armed.get() {
                // Cloned per render: the closure below is called again on every change, so it may not
                // move either string out of its own capture.
                let warning = warning.clone();
                view! {
                    <Alert variant=AlertVariant::Warning>
                        <AlertDescription>{warning}</AlertDescription>
                        <div class="mt-2 flex gap-2">
                            <Button
                                class="flex-1"
                                label=t!("tunnel.confirm")
                                variant=ButtonVariant::Destructive
                                disabled=disabled
                                on_click=Callback::new(move |()| {
                                    set_armed.set(false);
                                    on_confirm.run(());
                                })
                            />
                            <Button
                                class="flex-1"
                                label=t!("tunnel.cancel")
                                variant=ButtonVariant::Outline
                                on_click=Callback::new(move |()| set_armed.set(false))
                            />
                        </div>
                    </Alert>
                }
                    .into_any()
            } else {
                let label = label.clone();
                view! {
                    <Button
                        class="w-full"
                        label=label
                        disabled=disabled
                        on_click=Callback::new(move |()| set_armed.set(true))
                    />
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
    view! {
        {move || {
            save.get()
                .message()
                .map(|message| view! { <ErrorNote message=message /> })
        }}
        {move || {
            result
                .get()
                .map(|outcome| {
                    let url = outcome
                        .tunnel_url
                        .clone()
                        .or_else(|| outcome.auth_url.clone())
                        .filter(|url| !url.is_empty());
                    let allow = outcome.enable_url.clone().filter(|url| !url.is_empty());
                    // Rendered rather than inferred from the presence of `auth_url`: the server can
                    // report that a sign-in is needed without yet having a URL to send you to, and a
                    // panel that only watches the URL shows nothing at all in that window.
                    let needs_login = outcome.needs_login;
                    let message = outcome.message;
                    let success = outcome.success;
                    view! {
                        <div class="w-full space-y-1.5">
                            {if success {
                                view! {
                                    <p class="text-[0.8125rem] leading-relaxed text-foreground">
                                        {message}
                                    </p>
                                }
                                    .into_any()
                            } else {
                                view! { <ErrorNote message=message /> }.into_any()
                            }}
                            {needs_login
                                .then(|| {
                                    view! {
                                        <StatusDot
                                            label=t!("tunnel.needs_login")
                                            tone=StatusTone::Warning
                                        />
                                    }
                                })}
                            {url
                                .map(|url| {
                                    let href = url.clone();
                                    view! {
                                        <a
                                            href=href
                                            target="_blank"
                                            rel="noreferrer noopener"
                                            class="block underline-offset-4 hover:underline"
                                        >
                                            <Mono value=url class="text-xs" />
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
                                            class="block text-[0.8125rem] font-medium underline-offset-4 hover:underline"
                                        >
                                            {t!("tunnel.allow_funnel")}
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
///
/// An [`Alert`] rather than a green box: the fact worth noticing is that this URL leaves the machine,
/// and the alert's leading icon is what makes that visible at a glance instead of a border tint.
#[component]
fn PublicUrl(label: String, url: String) -> impl IntoView {
    let href = url.clone();
    view! {
        <Alert variant=AlertVariant::Info>
            <AlertTitle>{label}</AlertTitle>
            <AlertDescription>
                <a
                    href=href
                    target="_blank"
                    rel="noreferrer noopener"
                    class="underline-offset-4 hover:underline"
                >
                    <Mono value=url />
                </a>
            </AlertDescription>
        </Alert>
    }
}

/// One label/value pair, for a value that is text rather than a state.
///
/// Named `Detail` rather than `Field` because [`crate::ui::Field`] is the form control wrapper and the
/// two would otherwise shadow each other in this module.
#[component]
fn Detail(#[prop(into)] label: String, #[prop(into)] value: String) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between gap-4">
            <dt class="truncate text-muted-foreground">{label}</dt>
            <dd class="shrink-0">{value}</dd>
        </div>
    }
}

/// One label/value pair where the value is a yes/no state.
///
/// A [`StatusDot`] rather than the word alone: "Yes" beside "Installed" and "No" beside "Running" read
/// identically in a column of grey text, and the tone is what separates them.
#[component]
fn DetailFlag(#[prop(into)] label: String, on: bool) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between gap-4">
            <dt class="truncate text-muted-foreground">{label}</dt>
            <dd class="shrink-0">
                <StatusDot
                    label=yes_no(on)
                    tone=if on { StatusTone::Success } else { StatusTone::Muted }
                />
            </dd>
        </div>
    }
}

/// The supervisor's state as a dot plus its name.
///
/// The tone follows the supervisor's own word rather than the `enabled` flag alone: `failed` is a
/// failure and `backoff`/`starting`/`stopping` are in between, and a dot that only knew on/off would
/// paint a crash-looping tunnel the same colour as a healthy one.
#[component]
fn StateBadge(state: String, on: bool) -> impl IntoView {
    let tone = match state.as_str() {
        "failed" => StatusTone::Destructive,
        "backoff" | "starting" | "stopping" => StatusTone::Warning,
        _ if on => StatusTone::Success,
        _ => StatusTone::Muted,
    };
    let label = if state.is_empty() {
        t!("state.unknown")
    } else {
        state
    };
    view! {
        <div class="shrink-0">
            <StatusDot label=label tone=tone />
        </div>
    }
}

/// Why no binary will be downloaded. Shown always, because it is the answer to "why is there no
/// install button".
#[component]
fn InstallNote(download: DownloadState) -> impl IntoView {
    if download.message.is_empty() {
        return ().into_any();
    }
    // `in_progress` is false on this build, which never fetches a binary. Rendered rather than
    // ignored because a server that starts reporting a download must not do so into a panel that
    // silently drops the flag.
    let downloading = download.in_progress;
    view! {
        <Alert variant=AlertVariant::Muted>
            {downloading
                .then(|| view! { <AlertTitle>{t!("tunnel.downloading")}</AlertTitle> })}
            <AlertDescription>{download.message}</AlertDescription>
        </Alert>
    }
    .into_any()
}
/// What `tailscale-check` found on this host.
#[component]
fn EnvironmentCard(check: TailscaleCheck) -> impl IntoView {
    let platform = if check.platform.is_empty() {
        t!("state.unknown")
    } else {
        check.platform.clone()
    };

    view! {
        <Card>
            <CardHeader>
                <CardTitle>{t!("tunnel.environment")}</CardTitle>
            </CardHeader>
            <CardContent>
                <dl class="grid gap-2.5 text-sm sm:grid-cols-2">
                    <Detail label=t!("tunnel.platform") value=platform />
                    <DetailFlag label=t!("tunnel.cli_installed") on=check.installed />
                    <DetailFlag label=t!("tunnel.daemon_installed") on=check.daemon_installed />
                    <DetailFlag label=t!("tunnel.daemon_running") on=check.daemon_running />
                    <DetailFlag label=t!("tunnel.custom_daemon") on=check.custom_daemon_running />
                    <DetailFlag label=t!("tunnel.system_daemon") on=check.system_daemon_running />
                    <DetailFlag label=t!("tunnel.logged_in") on=check.logged_in />
                    <DetailFlag label=t!("tunnel.brew_available") on=check.brew_available />
                    <DetailFlag label=t!("tunnel.cached_password") on=check.has_cached_password />
                </dl>
            </CardContent>
        </Card>
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
    view! {
        <Card>
            <CardHeader>
                <CardTitle>{t!("tunnel.operations")}</CardTitle>
            </CardHeader>
            <CardContent class="space-y-3">
                <CardDescription>{t!("tunnel.operations_hint")}</CardDescription>

                // The binaries behind the catalog, as chips: an operation is only reachable when the
                // tool it names is installed, so the install state belongs beside the name.
                <ul class="flex flex-wrap gap-1.5">
                    {data
                        .tools
                        .into_iter()
                        .map(|tool| {
                            let detail = tool.path.filter(|path| !path.is_empty());
                            view! {
                                <li>
                                    <Badge
                                        variant=if tool.installed {
                                            BadgeVariant::Success
                                        } else {
                                            BadgeVariant::Secondary
                                        }
                                        class="gap-1.5 px-2 py-1"
                                    >
                                        <span class="font-mono">{tool.id}</span>
                                        {detail
                                            .map(|path| {
                                                view! {
                                                    <span class="max-w-56 truncate text-muted-foreground/80">
                                                        {path}
                                                    </span>
                                                }
                                            })}
                                    </Badge>
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>

                <OperationsTable rows=data.operations />
            </CardContent>
        </Card>
    }
}
#[component]
fn OperationsTable(rows: Vec<OperationRow>) -> impl IntoView {
    view! {
        <TableFrame>
            <Table>
                <TableHead>
                    <tr>
                        <TableHeaderCell>{t!("tunnel.col_operation")}</TableHeaderCell>
                        <TableHeaderCell>{t!("tunnel.col_tool")}</TableHeaderCell>
                        <TableHeaderCell>{t!("tunnel.col_effect")}</TableHeaderCell>
                        <TableHeaderCell>{t!("tunnel.col_mode")}</TableHeaderCell>
                        <TableHeaderCell>{t!("tunnel.col_params")}</TableHeaderCell>
                        <TableHeaderCell>{t!("tunnel.col_timeout")}</TableHeaderCell>
                        <TableHeaderCell>{t!("tunnel.col_available")}</TableHeaderCell>
                    </tr>
                </TableHead>
                {if rows.is_empty() {
                    view! { <TableEmpty message=t!("tunnel.no_operations") columns=7 /> }.into_any()
                } else {
                    view! {
                        <tbody>
                            {rows
                                .into_iter()
                                .map(|row| view! { <OperationLine row=row /> })
                                .collect_view()}
                        </tbody>
                    }
                        .into_any()
                }}
            </Table>
        </TableFrame>
    }
}
#[component]
fn OperationLine(row: OperationRow) -> impl IntoView {
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
    let timeout = row
        .timeout_ms
        .map_or_else(|| "—".to_owned(), |ms| format!("{ms} ms"));

    view! {
        <TableRow>
            <TableCell>
                <div class="space-y-0.5">
                    <Mono value=row.id class="text-xs" />
                    <p class="text-xs text-muted-foreground">{row.about}</p>
                </div>
            </TableCell>
            <TableCell>
                <Mono value=row.tool class="text-xs text-muted-foreground" />
            </TableCell>
            <TableCell>
                // `mutate` is the half of this table that can start or stop something, so it carries
                // the warning variant and a read stays neutral.
                <Badge
                    variant=if mutating { BadgeVariant::Warning } else { BadgeVariant::Secondary }
                >
                    {row.effect}
                </Badge>
            </TableCell>
            <TableCell>
                <Mono value=row.mode class="text-xs text-muted-foreground" />
            </TableCell>
            <TableCell class="text-xs text-muted-foreground">{params}</TableCell>
            <TableCell>
                <span class="whitespace-nowrap font-mono text-xs tabular-nums text-muted-foreground">
                    {timeout}
                </span>
            </TableCell>
            <TableCell>
                <Badge
                    variant=if row.available {
                        BadgeVariant::Success
                    } else {
                        BadgeVariant::Secondary
                    }
                >
                    {yes_no(row.available)}
                </Badge>
            </TableCell>
        </TableRow>
    }
}

/// A boolean as a word, through the locale.
fn yes_no(value: bool) -> String {
    if value {
        t!("state.yes")
    } else {
        t!("state.no")
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
