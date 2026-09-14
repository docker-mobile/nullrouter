//! The translation inspector: what each stage of a request actually looks like on the wire.
//!
//! Eight panes, one per stage, held server-side so a capture survives a reload. The pane names are a
//! closed set the router enforces -- `/api/translator/load` answers `400` for anything else -- so
//! they are offered as a fixed list rather than a text field, which turns a typo from a refused
//! request into something that cannot be expressed.
//!
//! The step buttons run the real engine and write their output into the pane it belongs in, so the
//! panes stay a record of one request rather than eight independently edited scratchpads.

use std::collections::BTreeMap;

use leptos::prelude::*;
use serde_json::Value;

use crate::api::{Method, Save};
use crate::routes::{PageHeader, write_reporting};

/// The only file names `/api/translator/load` and `/save` accept.
///
/// Enforced server-side in `services/api-actix/src/translator.rs`; duplicated here because the
/// selectable set has to come from somewhere, and a free-text field would let a user ask for a name
/// that can only be refused.
const PANES: [&str; 8] = [
    "1_req_client.json",
    "2_req_source.json",
    "3_req_openai.json",
    "4_req_target.json",
    "5_res_provider.txt",
    "6_res_openai.txt",
    "7_res_client.txt",
    "7_res_client.json",
];

/// The pane a request starts in, and the one the panel opens on.
const CLIENT_REQUEST: &str = "1_req_client.json";
/// Where step 2 writes the OpenAI intermediate, and where step 3 reads it from.
const OPENAI_REQUEST: &str = "3_req_openai.json";
/// Where step 3 writes the provider's own body.
const TARGET_REQUEST: &str = "4_req_target.json";
/// The provider's raw response chunks, which step 5 reads.
const PROVIDER_RESPONSE: &str = "5_res_provider.txt";
/// Where step 5 writes the OpenAI form of the response.
const OPENAI_RESPONSE: &str = "6_res_openai.txt";
/// Where step 5 writes the client form of the response.
const CLIENT_RESPONSE: &str = "7_res_client.json";

/// One line of feedback, and whether it reports success.
///
/// The text is always the server's own, or a local parse failure named as such. Nothing here
/// paraphrases a refusal into a friendlier sentence: an inspector whose error messages are not the
/// ones the router produced is an inspector that cannot be used to diagnose the router.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Notice {
    ok: bool,
    text: String,
}

/// What step 1 resolved the request to.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Identity {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    source_format: String,
    #[serde(default)]
    target_format: String,
}

/// What step 3 resolved the outbound call to.
///
/// `url` and `headers` are absent when no connection for the provider is active, which is a
/// different thing from an empty URL and is why they are `Option`. `headers` arrive already redacted
/// by the runtime -- see `is_secret_header` there -- so this panel renders them as given.
// No `Eq`: `serde_json::Value` is only `PartialEq`, because it holds floats.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wire {
    #[serde(default)]
    body: Value,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    tool_name_map: BTreeMap<String, String>,
    #[serde(default)]
    connection_error: Option<String>,
}

/// Step 2's result: the request as the OpenAI intermediate.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
struct Translated {
    #[serde(default)]
    body: Value,
}

/// Step 5's result: the provider's chunks in both the intermediate and the client's format.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    #[serde(default)]
    openai: Vec<Value>,
    #[serde(default)]
    client: Vec<Value>,
}

/// The envelope every step answers in.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
struct StepReply<T> {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    result: T,
}

/// `/api/translator/load`'s answer.
///
/// `success: false` with `error: "File not found"` arrives as a `200`, because a pane that has not
/// been captured yet is an answer rather than a failure. Kept distinct from the `503` the route
/// returns when the state service is down, which is a failure and reads as one.
#[derive(Clone, Debug, Default, serde::Deserialize)]
struct LoadReply {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

const LOAD: &str = "/api/translator/load";
const SAVE: &str = "/api/translator/save";
const TRANSLATE: &str = "/api/translator/translate";

/// Run one inspector step, then hand the decoded result on.
///
/// `apply` runs only for a `200` whose `success` is true and whose result decoded. Anything else is
/// reported instead, so no pane is ever filled from a body this panel did not understand.
fn run_step<T, S>(
    payload: String,
    busy: WriteSignal<Save>,
    notice: WriteSignal<Option<Notice>>,
    undecodable: String,
    apply: S,
) where
    T: serde::de::DeserializeOwned + Default + 'static,
    S: FnOnce(T) + 'static,
{
    busy.set(Save::Saving);
    notice.set(None);
    leptos::task::spawn_local(async move {
        match write_reporting(Method::Post, TRANSLATE, Some(&payload)).await {
            Ok(body) => match serde_json::from_str::<StepReply<T>>(&body) {
                Ok(reply) if reply.success => {
                    busy.set(Save::Saved);
                    apply(reply.result);
                }
                _unusable => {
                    busy.set(Save::Idle);
                    notice.set(Some(Notice {
                        ok: false,
                        text: undecodable,
                    }));
                }
            },
            Err(message) => {
                busy.set(Save::Idle);
                notice.set(Some(Notice {
                    ok: false,
                    text: message,
                }));
            }
        }
    });
}

/// Read the provider-response pane as the chunk list step 5 needs.
///
/// Two shapes, because a capture arrives in either: a JSON array, or one JSON object per line with
/// the `data:` prefix SSE framing adds. The `[DONE]` sentinel is not JSON and is skipped rather than
/// reported as a broken line, which is what makes a pasted stream work unedited.
fn parse_chunks(text: &str) -> Option<Vec<Value>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('[') {
        return serde_json::from_str::<Vec<Value>>(trimmed).ok();
    }

    let mut chunks = Vec::new();
    for raw in trimmed.lines() {
        let line = raw.trim();
        let line = line.strip_prefix("data:").unwrap_or(line).trim();
        if line.is_empty() || line == "[DONE]" {
            continue;
        }
        chunks.push(serde_json::from_str::<Value>(line).ok()?);
    }
    (!chunks.is_empty()).then_some(chunks)
}

/// A value as the text a pane holds.
///
/// Pretty-printed: these panes exist to be read, and a translated body on one line is the thing this
/// panel is meant to make legible.
fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// Parse a pane as the JSON body a step takes.
fn pane_body(text: &str) -> Option<Value> {
    serde_json::from_str::<Value>(text.trim()).ok()
}

#[component]
// Five stage actions, each with its own request payload, its own response type, and its own
// success handler that writes a different pane and carries different fields forward. A generic
// helper over them would take the payload builder, the response type, and the handler as
// parameters, which relocates the length into a signature rather than removing it -- and separating
// a stage from the pane it writes is what makes this panel hard to follow. `run_step` already
// factors out the part that genuinely repeats.
#[expect(clippy::too_many_lines, reason = "five stages that do not generalise")]
pub fn Translator() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    // Every pane in one map rather than one signal each, because the steps write panes other than
    // the one on screen: step 2 reads pane 1 and fills pane 3.
    let panes = RwSignal::new(BTreeMap::<&'static str, String>::new());
    let selected = RwSignal::new(CLIENT_REQUEST);
    let (io, set_io) = signal(Save::Idle);
    let (busy, set_busy) = signal(Save::Idle);
    let (io_notice, set_io_notice) = signal(Option::<Notice>::None);
    let (step_notice, set_step_notice) = signal(Option::<Notice>::None);
    let (identity, set_identity) = signal(Option::<Identity>::None);
    let (wire, set_wire) = signal(Option::<Wire>::None);
    let (provider, set_provider) = signal(String::new());
    let (model, set_model) = signal(String::new());

    let read_pane =
        move |name: &'static str| panes.with(|map| map.get(name).cloned().unwrap_or_default());
    let write_pane = move |name: &'static str, text: String| {
        panes.update(|map| {
            map.insert(name, text);
        });
    };

    // Owned up front: these are needed inside async blocks, where a borrow of the locale cannot go.
    let msg_saved = locale.get("translator.saved").to_owned();
    let msg_unreadable = locale.get("translator.unreadable").to_owned();
    let msg_invalid = locale.get("translator.invalid_json").to_owned();
    let msg_need_provider = locale.get("translator.need_provider").to_owned();
    let msg_no_chunks = locale.get("translator.no_chunks").to_owned();

    // Cloned out here rather than inside: a `move` closure captures the whole `String`, so cloning
    // within the body still moves the original in and leaves the four later closures nothing to
    // clone from.
    let load_unreadable = msg_unreadable.clone();
    let load_pane = move || {
        let file = selected.get();
        let unreadable = load_unreadable.clone();
        set_io.set(Save::Saving);
        set_io_notice.set(None);
        leptos::task::spawn_local(async move {
            let path = format!("{LOAD}?file={file}");
            let outcome = match write_reporting(Method::Get, &path, None).await {
                Ok(body) => match serde_json::from_str::<LoadReply>(&body) {
                    Ok(reply) if reply.success => {
                        write_pane(file, reply.content.unwrap_or_default());
                        Ok(())
                    }
                    // A `200` with `success: false` is "not captured yet", and the route's own
                    // wording says so. Reported, not written: an empty pane presented as loaded
                    // content would read as a request that translated to nothing.
                    Ok(reply) => Err(reply.error.unwrap_or_else(|| unreadable.clone())),
                    Err(_undecodable) => Err(unreadable),
                },
                Err(message) => Err(message),
            };
            match outcome {
                Ok(()) => set_io.set(Save::Saved),
                Err(text) => {
                    set_io.set(Save::Idle);
                    set_io_notice.set(Some(Notice { ok: false, text }));
                }
            }
        });
    };

    let save_pane = move || {
        let file = selected.get();
        let saved = msg_saved.clone();
        let Ok(payload) = serde_json::to_string(&serde_json::json!({
            "file": file,
            "content": read_pane(file),
        })) else {
            return;
        };
        set_io.set(Save::Saving);
        set_io_notice.set(None);
        leptos::task::spawn_local(async move {
            match write_reporting(Method::Post, SAVE, Some(&payload)).await {
                Ok(_body) => {
                    set_io.set(Save::Saved);
                    set_io_notice.set(Some(Notice {
                        ok: true,
                        text: saved,
                    }));
                }
                Err(text) => {
                    set_io.set(Save::Idle);
                    set_io_notice.set(Some(Notice { ok: false, text }));
                }
            }
        });
    };

    // Report a local problem -- a pane that is not JSON, a missing provider -- without a request.
    let refuse_locally = move |text: String| {
        set_busy.set(Save::Idle);
        set_step_notice.set(Some(Notice { ok: false, text }));
    };

    let run_identify = {
        let invalid = msg_invalid.clone();
        let unreadable = msg_unreadable.clone();
        move || {
            let Some(body) = pane_body(&read_pane(CLIENT_REQUEST)) else {
                refuse_locally(invalid.clone());
                return;
            };
            let Ok(payload) = serde_json::to_string(&serde_json::json!({
                "step": 1, "body": body,
            })) else {
                return;
            };
            run_step::<Identity, _>(
                payload,
                set_busy,
                set_step_notice,
                unreadable.clone(),
                move |result| {
                    // Carried into the provider and model fields because steps 3 and 5 need both,
                    // and re-typing what step 1 just resolved is where a mismatch gets introduced.
                    set_provider.set(result.provider.clone());
                    set_model.set(result.model.clone());
                    set_identity.set(Some(result));
                },
            );
        }
    };

    let to_openai = {
        let invalid = msg_invalid.clone();
        let unreadable = msg_unreadable.clone();
        move || {
            let Some(body) = pane_body(&read_pane(CLIENT_REQUEST)) else {
                refuse_locally(invalid.clone());
                return;
            };
            let Ok(payload) = serde_json::to_string(&serde_json::json!({
                "step": 2, "body": body,
            })) else {
                return;
            };
            run_step::<Translated, _>(
                payload,
                set_busy,
                set_step_notice,
                unreadable.clone(),
                move |result| {
                    write_pane(OPENAI_REQUEST, pretty(&result.body));
                    // Move to the pane just written, so the output of a step is what is on screen.
                    selected.set(OPENAI_REQUEST);
                },
            );
        }
    };

    let to_target = {
        let invalid = msg_invalid;
        let unreadable = msg_unreadable.clone();
        let need_provider = msg_need_provider.clone();
        move || {
            let (chosen, named) = (provider.get(), model.get());
            if chosen.trim().is_empty() || named.trim().is_empty() {
                refuse_locally(need_provider.clone());
                return;
            }
            let Some(body) = pane_body(&read_pane(OPENAI_REQUEST)) else {
                refuse_locally(invalid.clone());
                return;
            };
            let Ok(payload) = serde_json::to_string(&serde_json::json!({
                "step": 3, "body": body, "provider": chosen, "model": named,
            })) else {
                return;
            };
            run_step::<Wire, _>(
                payload,
                set_busy,
                set_step_notice,
                unreadable.clone(),
                move |result| {
                    write_pane(TARGET_REQUEST, pretty(&result.body));
                    selected.set(TARGET_REQUEST);
                    set_wire.set(Some(result));
                },
            );
        }
    };

    let to_client = {
        let unreadable = msg_unreadable;
        let need_provider = msg_need_provider;
        let no_chunks = msg_no_chunks;
        move || {
            let chosen = provider.get();
            if chosen.trim().is_empty() {
                refuse_locally(need_provider.clone());
                return;
            }
            let Some(chunks) = parse_chunks(&read_pane(PROVIDER_RESPONSE)) else {
                refuse_locally(no_chunks.clone());
                return;
            };
            // The client's format is whatever step 1 detected. Sent under `body` because the API
            // service requires that key at every step, and null there means the engine's own default
            // rather than a format guessed here.
            let source = identity
                .get()
                .map(|resolved| resolved.source_format)
                .filter(|format| !format.is_empty());
            let Ok(payload) = serde_json::to_string(&serde_json::json!({
                "step": 5,
                "provider": chosen,
                "chunks": chunks,
                "body": { "sourceFormat": source },
            })) else {
                return;
            };
            run_step::<Response, _>(
                payload,
                set_busy,
                set_step_notice,
                unreadable.clone(),
                move |result| {
                    write_pane(OPENAI_RESPONSE, pretty(&Value::Array(result.openai)));
                    write_pane(CLIENT_RESPONSE, pretty(&Value::Array(result.client)));
                    selected.set(CLIENT_RESPONSE);
                },
            );
        }
    };

    view! {
        <PageHeader
            title=locale.get("nav.translator").to_owned()
            description=locale.get("translator.description").to_owned()
        />

        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("translator.panes").to_owned()}
            </h2>
            <p class="text-sm text-muted-foreground">
                {locale.get("translator.pane_hint").to_owned()}
            </p>
            <div class="flex flex-wrap gap-2">
                {PANES
                    .into_iter()
                    .map(|name| view! { <PaneTab name=name selected=selected /> })
                    .collect_view()}
            </div>
            <textarea
                class="w-full h-64 rounded-md border border-input bg-background px-3 py-2 \
                       font-mono text-xs"
                spellcheck="false"
                autocomplete="off"
                prop:value=move || read_pane(selected.get())
                on:input=move |ev| write_pane(selected.get(), event_target_value(&ev))
            />
            <div class="flex flex-wrap items-center gap-2">
                <button
                    type="button"
                    class="rounded-md border border-border px-3 py-2 text-sm font-medium \
                           transition-colors hover:bg-accent disabled:opacity-50"
                    disabled=move || io.get().is_saving()
                    on:click=move |_| load_pane()
                >
                    {locale.get("translator.load").to_owned()}
                </button>
                <button
                    type="button"
                    class="rounded-md bg-primary px-3 py-2 text-sm font-medium \
                           text-primary-foreground disabled:opacity-50"
                    disabled=move || io.get().is_saving()
                    on:click=move |_| save_pane()
                >
                    {locale.get("translator.save").to_owned()}
                </button>
            </div>
            <NoticeLine notice=io_notice />
        </section>

        <section class="mt-4 rounded-lg border border-border bg-card p-5 space-y-4">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("translator.steps").to_owned()}
            </h2>
            <p class="text-sm text-muted-foreground">
                {locale.get("translator.step_hint").to_owned()}
            </p>

            <div class="grid gap-3 sm:grid-cols-2">
                <label class="block space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("translator.provider").to_owned()}
                    </span>
                    <input
                        type="text"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 \
                               font-mono text-xs"
                        prop:value=move || provider.get()
                        on:input=move |ev| set_provider.set(event_target_value(&ev))
                    />
                </label>
                <label class="block space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("translator.model").to_owned()}
                    </span>
                    <input
                        type="text"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 \
                               font-mono text-xs"
                        prop:value=move || model.get()
                        on:input=move |ev| set_model.set(event_target_value(&ev))
                    />
                </label>
            </div>

            <div class="flex flex-wrap gap-2">
                <StepButton
                    label=locale.get("translator.identify").to_owned()
                    busy=busy
                    on_run=Callback::new(move |()| run_identify())
                />
                <StepButton
                    label=locale.get("translator.to_openai").to_owned()
                    busy=busy
                    on_run=Callback::new(move |()| to_openai())
                />
                <StepButton
                    label=locale.get("translator.to_target").to_owned()
                    busy=busy
                    on_run=Callback::new(move |()| to_target())
                />
                <StepButton
                    label=locale.get("translator.to_client").to_owned()
                    busy=busy
                    on_run=Callback::new(move |()| to_client())
                />
            </div>

            <NoticeLine notice=step_notice />
            {move || identity.get().map(|resolved| view! { <IdentityView resolved=resolved /> })}
            {move || wire.get().map(|resolved| view! { <WireView resolved=resolved /> })}

            // Documented rather than offered as a button. `/api/translator/send` answers 501
            // unconditionally in this port, and a control that cannot ever succeed is the same false
            // promise as a retry offered on a 404.
            <p class="text-xs text-muted-foreground">
                {locale.get("translator.send_unsupported").to_owned()}
            </p>
        </section>
    }
}

/// One pane in the selector. The name is the label: it is what the router calls the file.
#[component]
fn PaneTab(name: &'static str, selected: RwSignal<&'static str>) -> impl IntoView {
    view! {
        <button
            type="button"
            class=move || {
                let state = if selected.get() == name {
                    "border-primary bg-accent"
                } else {
                    "border-border hover:bg-accent/60"
                };
                format!(
                    "{state} rounded-md border px-2.5 py-1.5 font-mono text-xs transition-colors"
                )
            }
            aria-pressed=move || (selected.get() == name).to_string()
            on:click=move |_| selected.set(name)
        >
            {name}
        </button>
    }
}

#[component]
fn StepButton(label: String, busy: ReadSignal<Save>, on_run: Callback<()>) -> impl IntoView {
    view! {
        <button
            type="button"
            class="rounded-md border border-border px-3 py-2 text-sm font-medium \
                   transition-colors hover:bg-accent disabled:opacity-50"
            disabled=move || busy.get().is_saving()
            on:click=move |_| on_run.run(())
        >
            {label}
        </button>
    }
}

/// The last thing an action reported, in the server's own words.
#[component]
fn NoticeLine(notice: ReadSignal<Option<Notice>>) -> impl IntoView {
    view! {
        {move || {
            notice
                .get()
                .map(|shown| {
                    let class = if shown.ok {
                        "text-sm text-foreground"
                    } else {
                        "text-sm text-destructive"
                    };
                    let role = if shown.ok { "status" } else { "alert" };
                    view! {
                        <p class=class role=role>
                            {shown.text}
                        </p>
                    }
                })
        }}
    }
}

#[component]
fn Field(label: String, value: String) -> impl IntoView {
    view! {
        <div class="min-w-0">
            <dt class="text-xs text-muted-foreground">{label}</dt>
            <dd class="font-mono text-xs break-all">{value}</dd>
        </div>
    }
}

#[component]
fn IdentityView(resolved: Identity) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <dl class="grid gap-3 sm:grid-cols-4 rounded-md border border-border p-3">
            <Field label=locale.get("translator.provider").to_owned() value=resolved.provider />
            <Field label=locale.get("translator.model").to_owned() value=resolved.model />
            <Field
                label=locale.get("translator.source_format").to_owned()
                value=resolved.source_format
            />
            <Field
                label=locale.get("translator.target_format").to_owned()
                value=resolved.target_format
            />
        </dl>
    }
}

/// Step 3's outbound call: where the request would go, and what it would carry.
#[component]
fn WireView(resolved: Wire) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let headers = resolved.headers.unwrap_or_default();
    let tools = resolved.tool_name_map;

    view! {
        <div class="space-y-3 rounded-md border border-border p-3">
            {resolved
                .connection_error
                .map(|error| {
                    view! {
                        // The router's sentence, not a rephrasing: it names the provider it could
                        // not find an active connection for.
                        <p class="text-sm text-destructive" role="alert">
                            {error}
                        </p>
                    }
                })}
            {resolved
                .url
                .map(|url| {
                    view! {
                        <dl>
                            <Field label=locale.get("translator.url").to_owned() value=url />
                        </dl>
                    }
                })}
            {(!headers.is_empty())
                .then(|| {
                    view! {
                        <div class="space-y-1">
                            <p class="text-xs text-muted-foreground">
                                {locale.get("translator.headers").to_owned()}
                            </p>
                            <dl class="grid gap-2 sm:grid-cols-2">
                                {headers
                                    .into_iter()
                                    .map(|(name, value)| {
                                        view! { <Field label=name value=value /> }
                                    })
                                    .collect_view()}
                            </dl>
                        </div>
                    }
                })}
            {(!tools.is_empty())
                .then(|| {
                    view! {
                        <div class="space-y-1">
                            <p class="text-xs text-muted-foreground">
                                {locale.get("translator.tool_map").to_owned()}
                            </p>
                            <dl class="grid gap-2 sm:grid-cols-2">
                                {tools
                                    .into_iter()
                                    .map(|(from, to)| view! { <Field label=from value=to /> })
                                    .collect_view()}
                            </dl>
                        </div>
                    }
                })}
            <p class="text-xs text-muted-foreground">
                {locale.get("translator.redacted_note").to_owned()}
            </p>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CLIENT_REQUEST, CLIENT_RESPONSE, Identity, LoadReply, OPENAI_REQUEST, OPENAI_RESPONSE,
        PANES, PROVIDER_RESPONSE, Response, StepReply, TARGET_REQUEST, Wire, parse_chunks,
    };

    /// The list `services/api-actix/src/translator.rs` enforces, in its order.
    ///
    /// Pinned because it is the whole selectable set: a name that drifted out of agreement with the
    /// server would present the user a pane that can only ever answer `400`.
    const SERVER_ALLOWED: [&str; 8] = [
        "1_req_client.json",
        "2_req_source.json",
        "3_req_openai.json",
        "4_req_target.json",
        "5_res_provider.txt",
        "6_res_openai.txt",
        "7_res_client.txt",
        "7_res_client.json",
    ];

    #[test]
    fn the_offered_panes_are_exactly_the_ones_the_router_accepts() {
        assert_eq!(PANES, SERVER_ALLOWED);
    }

    #[test]
    fn every_pane_a_step_reads_or_writes_is_one_of_the_offered_panes() {
        for name in [
            CLIENT_REQUEST,
            OPENAI_REQUEST,
            TARGET_REQUEST,
            PROVIDER_RESPONSE,
            OPENAI_RESPONSE,
            CLIENT_RESPONSE,
        ] {
            assert!(PANES.contains(&name), "{name} is not a selectable pane");
        }
    }

    #[test]
    fn a_captured_stream_parses_without_being_edited_first() {
        let sse = "data: {\"a\":1}\n\ndata: {\"a\":2}\ndata: [DONE]\n";
        let chunks = parse_chunks(sse).unwrap_or_default();
        assert_eq!(chunks.len(), 2, "the DONE sentinel is not a chunk");
    }

    #[test]
    fn a_json_array_parses_too() {
        let chunks = parse_chunks(" [ {\"a\":1}, {\"a\":2} ] ").unwrap_or_default();
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn bare_json_lines_parse_without_the_sse_prefix() {
        let chunks = parse_chunks("{\"a\":1}\n{\"a\":2}").unwrap_or_default();
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn nothing_usable_is_reported_rather_than_sent_as_an_empty_list() {
        // An empty `chunks` would come back as an empty translation, which reads as "this response
        // translates to nothing" rather than "there was nothing to translate".
        assert!(parse_chunks("").is_none());
        assert!(parse_chunks("   \n  ").is_none());
        assert!(parse_chunks("data: [DONE]").is_none());
        assert!(parse_chunks("not json").is_none());
        assert!(parse_chunks("{\"a\":1}\nnot json").is_none());
        assert!(parse_chunks("[{\"a\":1},").is_none());
    }

    #[test]
    fn a_pane_that_was_never_captured_does_not_read_as_loaded_content() {
        let body = r#"{"success":false,"content":null,"error":"File not found"}"#;
        let reply: LoadReply = serde_json::from_str(body).expect("decodes");
        assert!(!reply.success);
        assert!(reply.content.is_none());
        assert_eq!(reply.error.as_deref(), Some("File not found"));
    }

    #[test]
    fn the_live_step_one_reply_decodes() {
        let body = r#"{"result":{"model":"gpt-5","provider":"openai",
            "sourceFormat":"openai","targetFormat":"openai"},"success":true}"#;
        let reply: StepReply<Identity> = serde_json::from_str(body).expect("decodes");
        assert!(reply.success);
        assert_eq!(reply.result.provider, "openai");
        assert_eq!(reply.result.model, "gpt-5");
        assert_eq!(reply.result.target_format, "openai");
    }

    #[test]
    fn the_live_step_three_reply_keeps_a_missing_connection_distinct_from_an_empty_url() {
        let body = r#"{"result":{"body":{"model":"gpt-5"},
            "connectionError":"No active connection for provider: openai",
            "headers":null,"toolNameMap":{},"url":null},"success":true}"#;
        let reply: StepReply<Wire> = serde_json::from_str(body).expect("decodes");
        assert!(reply.success);
        // `None`, not `Some("")`: there is no URL, which is a different claim from an empty one.
        assert!(reply.result.url.is_none());
        assert!(reply.result.headers.is_none());
        assert!(reply.result.tool_name_map.is_empty());
        assert_eq!(
            reply.result.connection_error.as_deref(),
            Some("No active connection for provider: openai")
        );
    }

    #[test]
    fn the_live_step_five_reply_decodes_both_forms() {
        let body = r#"{"result":{"client":[{"object":"chat.completion.chunk"}],
            "openai":[{"object":"chat.completion.chunk"}],
            "sourceFormat":"openai","targetFormat":"claude"},"success":true}"#;
        let reply: StepReply<Response> = serde_json::from_str(body).expect("decodes");
        assert_eq!(reply.result.openai.len(), 1);
        assert_eq!(reply.result.client.len(), 1);
    }

    #[test]
    fn a_step_that_answered_two_hundred_without_success_is_not_treated_as_one_that_worked() {
        let body = r#"{"success":false,"error":"chunks are required for a response step"}"#;
        let reply: StepReply<Response> = serde_json::from_str(body).expect("decodes");
        assert!(!reply.success);
    }
}
