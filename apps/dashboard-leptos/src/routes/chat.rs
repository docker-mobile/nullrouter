//! Interactive chat playground for testing models and providers.
//!
//! Maps `/api/dashboard/chat/completions` to a reactive Leptos WASM UI supporting multi-turn
//! conversations, system prompts, model switching, and real-time streaming feedback.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api::{Hydrate, Method, load};
use crate::routes::PageHeader;
use crate::routes::types::ModelsList;
use crate::routes::write_reporting;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub is_streaming: bool,
    pub reasoning: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct DashboardChatCompletionRequest {
    model: String,
    messages: Vec<WireMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Clone, Debug, Serialize)]
struct WireMessage {
    role: String,
    content: String,
}

fn parse_chat_response(raw: &str) -> (String, Option<String>, Option<String>) {
    let text = raw.trim();
    if text.is_empty() {
        return (
            String::new(),
            None,
            Some("Empty response received from router".to_owned()),
        );
    }

    if text.contains("data: ") {
        let mut accumulated_content = String::new();
        let mut accumulated_reasoning = String::new();
        let mut err = None;
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("data: ") {
                let rest = rest.trim();
                if rest == "[DONE]" {
                    continue;
                }
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(rest) {
                    if let Some(content) = val
                        .pointer("/choices/0/delta/content")
                        .and_then(serde_json::Value::as_str)
                    {
                        accumulated_content.push_str(content);
                    } else if let Some(reasoning) = val
                        .pointer("/choices/0/delta/reasoning_content")
                        .or_else(|| val.pointer("/choices/0/delta/reasoning"))
                        .and_then(serde_json::Value::as_str)
                    {
                        accumulated_reasoning.push_str(reasoning);
                    } else if let Some(msg) = val
                        .pointer("/error/message")
                        .and_then(serde_json::Value::as_str)
                    {
                        err = Some(msg.to_owned());
                    }
                }
            }
        }
        if accumulated_reasoning.is_empty() {
            if !accumulated_content.is_empty() {
                return (accumulated_content, None, err);
            }
        } else {
            let reasoning = accumulated_reasoning.trim().to_owned();
            if accumulated_content.is_empty() {
                return (String::new(), Some(reasoning), err);
            }
            return (accumulated_content, Some(reasoning), err);
        }
        if let Some(e) = err {
            return (String::new(), None, Some(e));
        }
    }

    if let Ok(val) = serde_json::from_str::<serde_json::Value>(text) {
        let content = val
            .pointer("/choices/0/message/content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let reasoning = val
            .pointer("/choices/0/message/reasoning_content")
            .or_else(|| val.pointer("/choices/0/message/reasoning"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !reasoning.is_empty() {
            let r = reasoning.trim().to_owned();
            if content.is_empty() {
                return (String::new(), Some(r), None);
            }
            return (content.to_owned(), Some(r), None);
        }
        if !content.is_empty() {
            return (content.to_owned(), None, None);
        }
        if let Some(msg) = val
            .pointer("/error/message")
            .and_then(serde_json::Value::as_str)
        {
            return (String::new(), None, Some(msg.to_owned()));
        }
    }

    (text.to_owned(), None, None)
}

fn append_turn(set_messages: WriteSignal<Vec<ChatMessage>>, user_content: String) -> String {
    let mut asst_id = String::new();
    set_messages.update(|msgs| {
        let u_id = format!("u_{}", msgs.len());
        let a_id = format!("a_{}", msgs.len() + 1);
        asst_id.clone_from(&a_id);
        msgs.push(ChatMessage {
            id: u_id,
            role: "user".to_owned(),
            content: user_content,
            is_streaming: false,
            reasoning: None,
            error: None,
        });
        msgs.push(ChatMessage {
            id: a_id,
            role: "assistant".to_owned(),
            content: String::new(),
            is_streaming: true,
            reasoning: None,
            error: None,
        });
    });
    asst_id
}

fn build_chat_request(
    model: String,
    stream: bool,
    temp_str: &str,
    system_prompt: &str,
    messages: &[ChatMessage],
) -> DashboardChatCompletionRequest {
    let mut wire_messages = Vec::new();
    let sys = system_prompt.trim();
    if !sys.is_empty() {
        wire_messages.push(WireMessage {
            role: "system".to_owned(),
            content: sys.to_owned(),
        });
    }

    for msg in messages {
        if !msg.is_streaming && msg.error.is_none() && !msg.content.is_empty() {
            wire_messages.push(WireMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
            });
        }
    }

    DashboardChatCompletionRequest {
        model,
        messages: wire_messages,
        stream,
        temperature: temp_str.parse::<f32>().ok(),
    }
}

#[component]
fn ConversationBox(
    messages: ReadSignal<Vec<ChatMessage>>,
    on_send_suggestion: Callback<String>,
    input_text: ReadSignal<String>,
    set_input_text: WriteSignal<String>,
    is_generating: ReadSignal<bool>,
    on_send: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="rounded-lg border border-border bg-card min-h-[420px] max-h-[640px] flex flex-col justify-between overflow-hidden shadow-sm">
            <div class="p-4 space-y-4 overflow-y-auto flex-1">
                {move || {
                    let msgs = messages.get();
                    if msgs.is_empty() {
                        view! { <EmptyState on_select=on_send_suggestion /> }.into_any()
                    } else {
                        view! { <MessageList messages=msgs /> }.into_any()
                    }
                }}
            </div>
            <ChatInput
                input_text=input_text
                set_input_text=set_input_text
                is_generating=is_generating
                on_send=on_send
            />
        </div>
    }
}

#[component]
fn ChatHeader(
    label_title: String,
    label_desc: String,
    label_params: String,
    label_clear: String,
    set_show_params: WriteSignal<bool>,
    on_clear: Callback<()>,
) -> impl IntoView {
    view! {
        <PageHeader
            title=label_title
            description=label_desc
        >
            <div class="flex items-center gap-2">
                <button
                    type="button"
                    class="px-3 py-1.5 rounded-md border border-border text-xs font-medium hover:bg-accent transition-colors cursor-pointer"
                    on:click=move |_| set_show_params.update(|v| *v = !*v)
                >
                    {label_params}
                </button>
                <button
                    type="button"
                    class="px-3 py-1.5 rounded-md border border-border text-xs font-medium hover:bg-destructive/10 hover:text-destructive transition-colors cursor-pointer"
                    on:click=move |_| on_clear.run(())
                >
                    {label_clear}
                </button>
            </div>
        </PageHeader>
    }
}

#[component]
pub fn Chat() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let label_title = locale.get("chat.title").to_owned();
    let label_desc = locale.get("chat.description").to_owned();
    let label_params = locale.get("chat.parameters").to_owned();
    let label_clear = locale.get("chat.clear").to_owned();

    let (catalogue, set_catalogue) = signal(Hydrate::<ModelsList>::Loading);
    load("/api/models", set_catalogue);

    let (messages, set_messages) = signal(Vec::<ChatMessage>::new());
    let (input_text, set_input_text) = signal(String::new());
    let (selected_model, set_selected_model) = signal(String::from("openai/gpt-5"));
    let (system_prompt, set_system_prompt) = signal(String::from(
        "You are a helpful, fast, and precise AI assistant powered by nullrouter.",
    ));
    let (temperature, set_temperature) = signal(String::from("0.7"));
    let (stream_enabled, set_stream_enabled) = signal(true);
    let (is_generating, set_is_generating) = signal(false);
    let (error_banner, set_error_banner) = signal(Option::<String>::None);
    let (metrics, set_metrics) = signal(Option::<(u64, usize)>::None);
    let (show_params, set_show_params) = signal(false);

    let send_message = move |text: String| {
        let trimmed = text.trim().to_owned();
        if trimmed.is_empty() || is_generating.get() {
            return;
        }

        let assistant_msg_id = append_turn(set_messages, trimmed);

        set_input_text.set(String::new());
        set_is_generating.set(true);
        set_error_banner.set(None);

        let cur_msgs = messages.get();
        let temp_str = temperature.get();
        let sys_str = system_prompt.get();
        let req = build_chat_request(
            selected_model.get(),
            stream_enabled.get(),
            &temp_str,
            &sys_str,
            &cur_msgs,
        );

        let payload_str = serde_json::to_string(&req).unwrap_or_default();
        leptos::task::spawn_local(dispatch_chat_turn(
            payload_str,
            assistant_msg_id,
            set_is_generating,
            set_metrics,
            set_messages,
            set_error_banner,
        ));
    };

    let on_send_suggestion = Callback::new(move |s: String| send_message(s));
    let on_submit_input = Callback::new(move |()| send_message(input_text.get()));

    let clear_conversation = move || {
        set_messages.set(Vec::new());
        set_metrics.set(None);
        set_error_banner.set(None);
    };

    view! {
        <div class="max-w-5xl mx-auto space-y-4">
            <ChatHeader
                label_title=label_title
                label_desc=label_desc
                label_params=label_params
                label_clear=label_clear
                set_show_params=set_show_params
                on_clear=Callback::new(move |()| clear_conversation())
            />

            {move || show_params.get().then(|| view! {
                <ParametersDrawer
                    system_prompt=system_prompt
                    set_system_prompt=set_system_prompt
                    temperature=temperature
                    set_temperature=set_temperature
                    stream_enabled=stream_enabled
                    set_stream_enabled=set_stream_enabled
                />
            })}

            <ModelBar
                catalogue=catalogue
                selected_model=selected_model
                set_selected_model=set_selected_model
                metrics=metrics
            />

            {move || error_banner.get().map(|err| view! {
                <div class="p-3 rounded-md bg-destructive/10 border border-destructive/20 text-destructive text-xs font-medium">
                    {err}
                </div>
            })}

            <ConversationBox
                messages=messages
                on_send_suggestion=on_send_suggestion
                input_text=input_text
                set_input_text=set_input_text
                is_generating=is_generating
                on_send=on_submit_input
            />
        </div>
    }
}

async fn dispatch_chat_turn(
    payload: String,
    assistant_msg_id: String,
    set_is_generating: WriteSignal<bool>,
    set_metrics: WriteSignal<Option<(u64, usize)>>,
    set_messages: WriteSignal<Vec<ChatMessage>>,
    set_error_banner: WriteSignal<Option<String>>,
) {
    let start_time = web_time_millis();
    let result = write_reporting(
        Method::Post,
        "/api/dashboard/chat/completions",
        Some(&payload),
    )
    .await;

    let elapsed_ms = web_time_millis().saturating_sub(start_time);
    set_is_generating.set(false);

    match result {
        Ok(raw) => {
            let (content, reasoning, err) = parse_chat_response(&raw);
            let words = content.split_whitespace().count();
            set_metrics.set(Some((elapsed_ms, words)));

            set_messages.update(|msgs| {
                if let Some(target) = msgs.iter_mut().find(|m| m.id == assistant_msg_id) {
                    target.is_streaming = false;
                    target.reasoning = reasoning;
                    if let Some(e) = err {
                        target.error = Some(e);
                    } else {
                        target.content = content;
                    }
                }
            });
        }
        Err(err_msg) => {
            let err_display = if err_msg.is_empty() {
                "Failed to communicate with provider".to_owned()
            } else {
                err_msg
            };
            set_error_banner.set(Some(err_display.clone()));
            set_messages.update(|msgs| {
                if let Some(target) = msgs.iter_mut().find(|m| m.id == assistant_msg_id) {
                    target.is_streaming = false;
                    target.error = Some(err_display);
                }
            });
        }
    }
}

#[component]
fn ParametersDrawer(
    system_prompt: ReadSignal<String>,
    set_system_prompt: WriteSignal<String>,
    temperature: ReadSignal<String>,
    set_temperature: WriteSignal<String>,
    stream_enabled: ReadSignal<bool>,
    set_stream_enabled: WriteSignal<bool>,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let label_title = locale.get("chat.parameters").to_owned();
    let label_prompt = locale.get("chat.system_prompt").to_owned();
    let placeholder_prompt = locale.get("chat.system_placeholder").to_owned();
    let label_temp = locale.get("chat.temperature").to_owned();
    let label_stream = locale.get("chat.stream").to_owned();

    view! {
        <section class="rounded-lg border border-border bg-card p-4 space-y-3">
            <h2 class="text-sm font-semibold">{label_title}</h2>
            <div class="grid gap-3 sm:grid-cols-2">
                <label class="space-y-1 text-xs">
                    <span class="text-muted-foreground font-medium">{label_prompt}</span>
                    <textarea
                        class="w-full rounded-md border border-input bg-background p-2 text-xs h-20 resize-none font-mono"
                        prop:value=move || system_prompt.get()
                        on:input=move |ev| set_system_prompt.set(event_target_value(&ev))
                        placeholder=placeholder_prompt
                    />
                </label>
                <div class="space-y-3">
                    <label class="space-y-1 text-xs block">
                        <div class="flex justify-between">
                            <span class="text-muted-foreground font-medium">{label_temp}</span>
                            <span class="font-mono">{move || temperature.get()}</span>
                        </div>
                        <input
                            type="range"
                            min="0"
                            max="2"
                            step="0.1"
                            class="w-full"
                            prop:value=move || temperature.get()
                            on:input=move |ev| set_temperature.set(event_target_value(&ev))
                        />
                    </label>
                    <label class="flex items-center gap-2 text-xs cursor-pointer select-none">
                        <input
                            type="checkbox"
                            class="rounded border-input text-primary"
                            prop:checked=move || stream_enabled.get()
                            on:change=move |ev| set_stream_enabled.set(event_target_checked(&ev))
                        />
                        <span>{label_stream}</span>
                    </label>
                </div>
            </div>
        </section>
    }
}

#[component]
fn ModelBar(
    catalogue: ReadSignal<Hydrate<ModelsList>>,
    selected_model: ReadSignal<String>,
    set_selected_model: WriteSignal<String>,
    metrics: ReadSignal<Option<(u64, usize)>>,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let label_model = locale.get("chat.model").to_owned();
    let label_latency = locale.get("chat.latency").to_owned();
    let label_tokens = locale.get("chat.tokens").to_owned();

    view! {
        <div class="flex flex-wrap items-center justify-between gap-3 p-3 rounded-lg border border-border bg-card/60">
            <div class="flex items-center gap-2 min-w-0 flex-1">
                <span class="text-xs font-semibold text-muted-foreground shrink-0">{label_model}:</span>
                {move || match catalogue.get() {
                    Hydrate::Ready(data) if !data.models.is_empty() => {
                        let models = data.models;
                        view! {
                            <select
                                class="rounded-md border border-input bg-background px-2.5 py-1 text-xs font-mono max-w-sm truncate"
                                prop:value=move || selected_model.get()
                                on:change=move |ev| set_selected_model.set(event_target_value(&ev))
                            >
                                {models.into_iter().map(|m| {
                                    let name = if m.full_model.is_empty() {
                                        format!("{}/{}", m.provider, m.model)
                                    } else {
                                        m.full_model
                                    };
                                    let val = name.clone();
                                    view! { <option value=val>{name}</option> }
                                }).collect::<Vec<_>>()}
                            </select>
                        }.into_any()
                    }
                    _ => view! {
                        <input
                            type="text"
                            class="rounded-md border border-input bg-background px-2.5 py-1 text-xs font-mono w-64"
                            prop:value=move || selected_model.get()
                            on:input=move |ev| set_selected_model.set(event_target_value(&ev))
                            placeholder="provider/model"
                        />
                    }.into_any()
                }}
            </div>

            {move || metrics.get().map(|(ms, words)| {
                view! {
                    <div class="flex items-center gap-3 text-xs text-muted-foreground font-mono">
                        <span>{label_latency.clone()}: {ms}ms</span>
                        <span>{label_tokens.clone()}: ~{words}</span>
                    </div>
                }
            })}
        </div>
    }
}

#[component]
fn EmptyState(on_select: Callback<String>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let title = locale.get("chat.empty_title").to_owned();
    let hint = locale.get("chat.empty_hint").to_owned();
    let sug1 = locale.get("chat.suggestion_1").to_owned();
    let sug2 = locale.get("chat.suggestion_2").to_owned();
    let sug3 = locale.get("chat.suggestion_3").to_owned();

    let s1_click = {
        let text = sug1.clone();
        move |_| on_select.run(text.clone())
    };
    let s2_click = {
        let text = sug2.clone();
        move |_| on_select.run(text.clone())
    };
    let s3_click = {
        let text = sug3.clone();
        move |_| on_select.run(text.clone())
    };

    view! {
        <div class="py-16 text-center space-y-3">
            <div class="size-12 rounded-full bg-primary/10 text-primary mx-auto flex items-center justify-center">
                <svg class="size-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
                </svg>
            </div>
            <h3 class="font-semibold text-sm">{title}</h3>
            <p class="text-xs text-muted-foreground max-w-sm mx-auto">{hint}</p>
            <div class="pt-4 flex flex-wrap justify-center gap-2 max-w-xl mx-auto">
                <button
                    type="button"
                    class="px-3 py-1.5 rounded-full border border-border bg-background text-xs hover:border-primary transition-colors text-left cursor-pointer"
                    on:click=s1_click
                >
                    {sug1}
                </button>
                <button
                    type="button"
                    class="px-3 py-1.5 rounded-full border border-border bg-background text-xs hover:border-primary transition-colors text-left cursor-pointer"
                    on:click=s2_click
                >
                    {sug2}
                </button>
                <button
                    type="button"
                    class="px-3 py-1.5 rounded-full border border-border bg-background text-xs hover:border-primary transition-colors text-left cursor-pointer"
                    on:click=s3_click
                >
                    {sug3}
                </button>
            </div>
        </div>
    }
}

#[component]
fn MessageList(messages: Vec<ChatMessage>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let label_user = locale.get("chat.user_label").to_owned();
    let label_asst = locale.get("chat.assistant_label").to_owned();
    let label_copy = locale.get("chat.copy").to_owned();
    let label_streaming = locale.get("chat.streaming").to_owned();
    let reasoning_label = locale.get("chat.reasoning");
    let reasoning_label = if reasoning_label.is_empty() {
        "Reasoning".to_owned()
    } else {
        reasoning_label.to_owned()
    };

    view! {
        <div class="space-y-4">
            {messages.into_iter().map(|msg| {
                let is_user = msg.role == "user";
                let role_label = if is_user { label_user.clone() } else { label_asst.clone() };
                let copy_label = label_copy.clone();
                let streaming_label = label_streaming.clone();
                let content = msg.content;

                view! {
                    <div class=format!("flex gap-3 {}", if is_user { "justify-end" } else { "justify-start" })>
                        <div class=format!(
                            "max-w-[80%] rounded-lg p-3.5 text-sm {}",
                            if is_user {
                                "bg-primary text-primary-foreground"
                            } else {
                                "bg-muted border border-border text-foreground"
                            }
                        )>
                            <div class="flex items-center justify-between gap-2 mb-1 opacity-80 text-[11px]">
                                <span class="font-semibold">{role_label}</span>
                                {(!is_user && !content.is_empty()).then(|| {
                                    let copy_text = content.clone();
                                    view! {
                                        <button
                                            type="button"
                                            class="hover:underline cursor-pointer opacity-75 hover:opacity-100"
                                            on:click=move |_| copy_to_clipboard(&copy_text)
                                        >
                                            {copy_label}
                                        </button>
                                    }
                                })}
                            </div>

                            {if msg.is_streaming {
                                view! {
                                    <div class="flex items-center gap-2 py-1 text-muted-foreground text-xs italic">
                                        <span class="size-2 rounded-full bg-primary animate-ping" />
                                        <span>{streaming_label}</span>
                                    </div>
                                }.into_any()
                            } else if let Some(err) = msg.error {
                                view! {
                                    <div class="text-destructive font-mono text-xs whitespace-pre-wrap">
                                        {err}
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="space-y-2">
                                        {msg.reasoning.as_ref().filter(|r| !r.is_empty()).map(|r| {
                                            let reasoning_text = r.clone();
                                            view! {
                                                <details class="group rounded-md bg-muted/50 border border-border/50 overflow-hidden">
                                                    <summary class="cursor-pointer select-none px-3 py-1.5 text-[11px] font-medium text-muted-foreground hover:bg-muted transition-colors flex items-center gap-1.5">
                                                        <svg class="size-3 transition-transform group-open:rotate-90" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                                                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
                                                        </svg>
                                                        {reasoning_label.clone()}
                                                    </summary>
                                                    <div class="px-3 pb-2 pt-1 text-[11px] text-muted-foreground/80 whitespace-pre-wrap italic border-t border-border/30">
                                                        {reasoning_text}
                                                    </div>
                                                </details>
                                            }
                                        })}
                                        <div class="whitespace-pre-wrap leading-relaxed">
                                            {content}
                                        </div>
                                    </div>
                                }.into_any()
                            }}
                        </div>
                    </div>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}

#[component]
fn ChatInput(
    input_text: ReadSignal<String>,
    set_input_text: WriteSignal<String>,
    is_generating: ReadSignal<bool>,
    on_send: Callback<()>,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let placeholder = locale.get("chat.input_placeholder").to_owned();
    let label_send = locale.get("chat.send").to_owned();

    let on_submit = move || on_send.run(());

    view! {
        <div class="p-3 border-t border-border bg-card/80">
            <form
                class="flex gap-2 items-end"
                on:submit=move |ev| {
                    ev.prevent_default();
                    on_submit();
                }
            >
                <textarea
                    class="flex-1 rounded-md border border-input bg-background px-3 py-2 text-sm resize-none focus:outline-none focus:ring-1 focus:ring-ring min-h-[44px] max-h-32 leading-tight"
                    rows="1"
                    placeholder=placeholder
                    prop:value=move || input_text.get()
                    on:input=move |ev| set_input_text.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" && !ev.shift_key() {
                            ev.prevent_default();
                            on_submit();
                        }
                    }
                />
                <button
                    type="submit"
                    disabled=move || input_text.get().trim().is_empty() || is_generating.get()
                    class="px-4 py-2.5 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed transition-colors shrink-0 flex items-center gap-1.5 cursor-pointer"
                >
                    <span>{label_send}</span>
                    <svg class="size-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M14 5l7 7m0 0l-7 7m7-7H3" />
                    </svg>
                </button>
            </form>
        </div>
    }
}

fn web_time_millis() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
    }
}

fn copy_to_clipboard(_text: &str) {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let _ = window.navigator().clipboard().write_text(_text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_chat_response_handles_sse_stream() {
        let stream_data = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"}}]}\ndata: {\"choices\":[{\"delta\":{\"content\":\"world!\"}}]}\ndata: [DONE]\n";
        let (content, reasoning, err) = parse_chat_response(stream_data);
        assert_eq!(content, "Hello world!");
        assert!(reasoning.is_none());
        assert!(err.is_none());
    }

    #[test]
    fn parse_chat_response_handles_json_completion() {
        let json_data = r#"{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","content":"Testing 123"}}]}"#;
        let (content, reasoning, err) = parse_chat_response(json_data);
        assert_eq!(content, "Testing 123");
        assert!(reasoning.is_none());
        assert!(err.is_none());
    }

    #[test]
    fn parse_chat_response_handles_error_envelope() {
        let err_data = r#"{"error":{"message":"Rate limit reached for provider"}}"#;
        let (content, reasoning, err) = parse_chat_response(err_data);
        assert!(content.is_empty());
        assert!(reasoning.is_none());
        assert_eq!(err.as_deref(), Some("Rate limit reached for provider"));
    }

    #[test]
    fn parse_chat_response_handles_empty_response() {
        let (content, reasoning, err) = parse_chat_response("   ");
        assert!(content.is_empty());
        assert!(reasoning.is_none());
        assert!(err.is_some());
    }

    #[test]
    fn parse_chat_response_falls_back_to_raw_text() {
        let raw = "Just raw text response from backend";
        let (content, reasoning, err) = parse_chat_response(raw);
        assert_eq!(content, raw);
        assert!(reasoning.is_none());
        assert!(err.is_none());
    }
}
