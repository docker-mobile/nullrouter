//! Interactive chat playground for testing models and providers.
//!
//! Maps `/api/dashboard/chat/completions` to a reactive Leptos WASM UI supporting multi-turn
//! conversations, system prompts, model switching, and real-time streaming feedback.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api::{Hydrate, Method, load};
use crate::routes::types::ModelsList;
use crate::routes::write_reporting;
use crate::t;
use crate::ui::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardHeader, CardTitle, Checkbox, ErrorNote, Field, Icon, IconName, Input, PageBody, PageHeader,
    Spinner, Textarea,
};

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
        // A chat is the one place on this dashboard where a tall fixed pane is right: the transcript has
        // to scroll independently of the composer, and the composer has to stay at the bottom where the
        // eye expects it.
        <div class="flex h-[calc(100dvh-16rem)] min-h-[26rem] flex-col overflow-hidden rounded-xl border \
                    border-border bg-card shadow-xs">
            <div class="flex-1 space-y-4 overflow-y-auto p-4">
                {move || {
                    let msgs = messages.get();
                    if msgs.is_empty() {
                        view! { <ChatEmpty on_select=on_send_suggestion /> }.into_any()
                    } else {
                        view! { <MessageList messages=msgs /> }.into_any()
                    }
                }}
            </div>
            <ChatComposer
                input_text=input_text
                set_input_text=set_input_text
                is_generating=is_generating
                on_send=on_send
            />
        </div>
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the page wires seven signals into six panels and reads better in one place"
)]
#[component]
pub fn Chat() -> impl IntoView {
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
        <PageHeader title=t!("chat.title") description=t!("chat.description")>
            <Button
                label=t!("chat.parameters")
                variant=ButtonVariant::Outline
                size=ButtonSize::Sm
                on_click=Callback::new(move |()| set_show_params.update(|value| *value = !*value))
            />
            <Button
                label=t!("chat.clear")
                variant=ButtonVariant::Ghost
                size=ButtonSize::Sm
                on_click=Callback::new(move |()| clear_conversation())
            />
        </PageHeader>

        <PageBody>
            {move || {
                show_params
                    .get()
                    .then(|| {
                        view! {
                            <ParametersPanel
                                system_prompt=system_prompt
                                set_system_prompt=set_system_prompt
                                temperature=temperature
                                set_temperature=set_temperature
                                stream_enabled=stream_enabled
                                set_stream_enabled=set_stream_enabled
                            />
                        }
                    })
            }}

            <ModelBar
                catalogue=catalogue
                selected_model=selected_model
                set_selected_model=set_selected_model
                metrics=metrics
            />

            {move || {
                error_banner
                    .get()
                    .map(|message| {
                        view! {
                            <Alert variant=AlertVariant::Destructive>
                                <AlertDescription>
                                    <p class="whitespace-pre-wrap break-words">{message}</p>
                                </AlertDescription>
                            </Alert>
                        }
                    })
            }}

            <ConversationBox
                messages=messages
                on_send_suggestion=on_send_suggestion
                input_text=input_text
                set_input_text=set_input_text
                is_generating=is_generating
                on_send=on_submit_input
            />
        </PageBody>
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

/// The system prompt, temperature and streaming switch.
///
/// Collapsed by default and toggled from the header: these are set once for a session of testing, and
/// leaving them open costs the transcript height every time.
#[component]
fn ParametersPanel(
    system_prompt: ReadSignal<String>,
    set_system_prompt: WriteSignal<String>,
    temperature: ReadSignal<String>,
    set_temperature: WriteSignal<String>,
    stream_enabled: ReadSignal<bool>,
    set_stream_enabled: WriteSignal<bool>,
) -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <CardTitle>{t!("chat.parameters")}</CardTitle>
            </CardHeader>
            <CardContent>
                <div class="grid gap-3 sm:grid-cols-2">
                    <Field label=t!("chat.system_prompt")>
                        <Textarea
                            value=Signal::derive(move || system_prompt.get())
                            on_input=Callback::new(move |value| set_system_prompt.set(value))
                            placeholder=t!("chat.system_placeholder")
                            rows=3
                            class="font-mono text-xs"
                        />
                    </Field>
                    <div class="space-y-3">
                        <Field label=t!("chat.temperature")>
                            <div class="flex items-center gap-3">
                                <input
                                    type="range"
                                    min="0"
                                    max="2"
                                    step="0.1"
                                    class="h-1.5 w-full cursor-pointer appearance-none rounded-full \
                                           bg-muted accent-primary"
                                    prop:value=move || temperature.get()
                                    on:input=move |event| {
                                        set_temperature.set(event_target_value(&event));
                                    }
                                />
                                <span class="w-10 shrink-0 text-right font-mono text-xs tabular-nums">
                                    {move || temperature.get()}
                                </span>
                            </div>
                        </Field>
                        <Checkbox
                            checked=Signal::derive(move || stream_enabled.get())
                            on_change=Callback::new(move |next| set_stream_enabled.set(next))
                            label=t!("chat.stream")
                        />
                    </div>
                </div>
            </CardContent>
        </Card>
    }
}

/// The model picker, and the last turn's latency.
#[component]
fn ModelBar(
    catalogue: ReadSignal<Hydrate<ModelsList>>,
    selected_model: ReadSignal<String>,
    set_selected_model: WriteSignal<String>,
    metrics: ReadSignal<Option<(u64, usize)>>,
) -> impl IntoView {
    view! {
        <div class="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-border \
                    bg-card px-3 py-2 shadow-xs">
            <div class="flex min-w-0 flex-1 items-center gap-2">
                <Icon name=IconName::Box class="size-3.5 shrink-0 text-muted-foreground" />
                <span class="shrink-0 text-xs font-medium text-muted-foreground">
                    {t!("chat.model")}
                </span>
                {move || match catalogue.get() {
                    Hydrate::Ready(data) if !data.models.is_empty() => {
                        // The catalogue's own `full_model` when it has one, otherwise `provider/model`.
                        // The option's label and value are the same string, so it is built once.
                        let models = data
                            .models
                            .into_iter()
                            .map(|model| {
                                if model.full_model.is_empty() {
                                    format!("{}/{}", model.provider, model.model)
                                } else {
                                    model.full_model
                                }
                            })
                            .collect::<Vec<String>>();
                        view! {
                            <select
                                class="h-8 min-w-0 max-w-sm flex-1 rounded-md border border-input \
                                       bg-transparent px-2 font-mono text-xs outline-none \
                                       focus-visible:border-ring focus-visible:ring-[3px] \
                                       focus-visible:ring-ring/40 dark:bg-input/20"
                                prop:value=move || selected_model.get()
                                on:change=move |event| {
                                    set_selected_model.set(event_target_value(&event));
                                }
                            >
                                {models
                                    .into_iter()
                                    .map(|name| {
                                        let value = name.clone();
                                        view! { <option value=value>{name}</option> }
                                    })
                                    .collect_view()}
                            </select>
                        }
                            .into_any()
                    }
                    _ => {
                        // Falls back to a free-text field: the catalogue can be empty on a fresh install,
                        // and refusing to send a request without one would leave nothing to test against.
                        view! {
                            <Input
                                value=Signal::derive(move || selected_model.get())
                                on_input=Callback::new(move |value| set_selected_model.set(value))
                                placeholder="provider/model"
                                class="h-8 max-w-sm font-mono text-xs"
                                aria_label=t!("chat.model")
                            />
                        }
                            .into_any()
                    }
                }}
            </div>

            {move || {
                metrics
                    .get()
                    .map(|(ms, words)| {
                        view! {
                            <div class="flex shrink-0 items-center gap-3 font-mono text-xs text-muted-foreground">
                                <span>{t!("chat.latency")} " " {format!("{ms}ms")}</span>
                                <span>{t!("chat.tokens")} " ~" {words.to_string()}</span>
                            </div>
                        }
                    })
            }}
        </div>
    }
}

/// What the transcript shows before the first turn.
#[component]
fn ChatEmpty(on_select: Callback<String>) -> impl IntoView {
    let suggestions = [
        t!("chat.suggestion_1"),
        t!("chat.suggestion_2"),
        t!("chat.suggestion_3"),
    ];

    view! {
        <div class="flex h-full flex-col items-center justify-center gap-3 py-8 text-center">
            <div class="grid size-10 place-items-center rounded-full border border-border bg-muted/50 \
                        text-muted-foreground">
                <Icon name=IconName::MessageSquare class="size-5" />
            </div>
            <div class="space-y-1">
                <p class="text-sm font-medium">{t!("chat.empty_title")}</p>
                <p class="mx-auto max-w-sm text-xs leading-relaxed text-muted-foreground">
                    {t!("chat.empty_hint")}
                </p>
            </div>
            <div class="mt-2 flex max-w-xl flex-wrap justify-center gap-2">
                {suggestions
                    .into_iter()
                    .map(|suggestion| {
                        let text = suggestion.clone();
                        view! {
                            <button
                                type="button"
                                class="max-w-xs rounded-full border border-border bg-background px-3 \
                                       py-1.5 text-left text-xs transition-colors hover:bg-accent \
                                       focus-visible:ring-[3px] focus-visible:ring-ring/50 outline-none"
                                on:click=move |_| on_select.run(text.clone())
                            >
                                {suggestion}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
        </div>
    }
}

#[component]
fn MessageList(messages: Vec<ChatMessage>) -> impl IntoView {
    view! {
        <div class="space-y-4">
            {messages
                .into_iter()
                .map(|message| view! { <Message message=message /> })
                .collect_view()}
        </div>
    }
}

/// One turn.
///
/// The two roles are distinguished by alignment and surface rather than only by colour: a user turn is a
/// filled bubble on the right, an assistant turn is a plain block on the left. A dashboard that painted
/// both roles in colour-coded bubbles makes a long transcript read as a wall of paint.
#[component]
fn Message(message: ChatMessage) -> impl IntoView {
    let is_user = message.role == "user";
    let content = message.content;
    let reasoning = message.reasoning.filter(|text| !text.trim().is_empty());
    let error = message.error;
    let streaming = message.is_streaming;

    let role_label = if is_user {
        t!("chat.user_label")
    } else {
        t!("chat.assistant_label")
    };
    let copy_text = content.clone();

    view! {
        <div class=if is_user {
            "flex justify-end"
        } else {
            "flex justify-start"
        }>
            <div class=if is_user {
                "max-w-[85%] rounded-2xl rounded-br-sm bg-primary px-3.5 py-2.5 text-sm \
                 text-primary-foreground"
            } else {
                "max-w-[85%] space-y-2 rounded-2xl rounded-bl-sm border border-border bg-muted/40 \
                 px-3.5 py-2.5 text-sm"
            }>
                <div class="mb-1 flex items-center justify-between gap-3 opacity-70">
                    <span class="text-[0.6875rem] font-medium uppercase tracking-wide">
                        {role_label}
                    </span>
                    {(!is_user && !content.is_empty())
                        .then(|| {
                            let copy_text = copy_text.clone();
                            view! {
                                <button
                                    type="button"
                                    class="text-[0.6875rem] underline-offset-4 outline-none \
                                           hover:underline focus-visible:ring-[3px] \
                                           focus-visible:ring-ring/50"
                                    on:click=move |_| copy_to_clipboard(&copy_text)
                                >
                                    {t!("chat.copy")}
                                </button>
                            }
                        })}
                </div>

                {if streaming {
                    view! {
                        <div class="flex items-center gap-2 py-1 text-xs">
                            <Spinner label=t!("chat.streaming") />
                        </div>
                    }
                        .into_any()
                } else if let Some(error) = error {
                    view! { <ErrorNote message=error /> }.into_any()
                } else {
                    view! {
                        <div class="space-y-2">
                            {reasoning
                                .map(|text| {
                                    view! {
                                        <details class="group overflow-hidden rounded-md border \
                                                        border-border/60 bg-background/50">
                                            <summary class="flex cursor-pointer select-none items-center \
                                                            gap-1.5 px-2.5 py-1.5 text-[0.6875rem] \
                                                            font-medium uppercase tracking-wide \
                                                            text-muted-foreground">
                                                <Icon
                                                    name=IconName::ChevronRight
                                                    class="size-3 transition-transform group-open:rotate-90"
                                                />
                                                {t!("chat.reasoning")}
                                            </summary>
                                            <div class="whitespace-pre-wrap border-t border-border/60 \
                                                        px-2.5 py-2 text-xs italic leading-relaxed \
                                                        text-muted-foreground">
                                                {text}
                                            </div>
                                        </details>
                                    }
                                })}
                            <div class="whitespace-pre-wrap leading-relaxed">{content}</div>
                        </div>
                    }
                        .into_any()
                }}
            </div>
        </div>
    }
}

#[component]
fn ChatComposer(
    input_text: ReadSignal<String>,
    set_input_text: WriteSignal<String>,
    is_generating: ReadSignal<bool>,
    on_send: Callback<()>,
) -> impl IntoView {
    let on_submit = move || on_send.run(());

    view! {
        <div class="border-t border-border bg-muted/20 p-3">
            <form
                class="flex items-end gap-2"
                on:submit=move |event| {
                    event.prevent_default();
                    on_submit();
                }
            >
                <textarea
                    class="min-h-10 max-h-32 flex-1 resize-none rounded-md border border-input \
                           bg-transparent px-2.5 py-2 text-sm leading-relaxed outline-none \
                           placeholder:text-muted-foreground focus-visible:border-ring \
                           focus-visible:ring-[3px] focus-visible:ring-ring/40 dark:bg-input/20"
                    rows="1"
                    placeholder=t!("chat.input_placeholder")
                    prop:value=move || input_text.get()
                    on:input=move |event| set_input_text.set(event_target_value(&event))
                    // Enter sends; Shift+Enter is a newline. The reverse makes a multi-line prompt
                    // impossible to type, and a chat box that cannot hold a newline is not a chat box.
                    on:keydown=move |event| {
                        if event.key() == "Enter" && !event.shift_key() {
                            event.prevent_default();
                            on_submit();
                        }
                    }
                />
                <button
                    type="submit"
                    disabled=move || input_text.get().trim().is_empty() || is_generating.get()
                    class=crate::ui::button_class(ButtonVariant::Default, ButtonSize::Icon)
                    aria-label=t!("chat.send")
                >
                    <Icon name=IconName::Play class="size-4" />
                </button>
            </form>
            <p class="mt-1.5 text-[0.6875rem] text-muted-foreground">
                <crate::ui::Kbd key="Enter".to_owned() />
                " "
                {t!("chat.send_hint")}
            </p>
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

#[allow(clippy::missing_const_for_fn)]
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
