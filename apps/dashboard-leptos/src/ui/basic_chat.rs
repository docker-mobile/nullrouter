//! The Basic Chat panel.
//!
//! This page used to render two hard-coded bubbles — one of them literally
//! saying replies "will stream here after /api/dashboard/chat/completions is
//! connected" — above a `disabled` textarea and a model menu built from a
//! contracts fixture. Provider execution has since landed, and the endpoint it
//! was waiting for forwards to it, so the composer sends for real.
//!
//! What changed, and why each part is the way it is:
//!
//! * The model menu is derived from `GET /api/providers`, so it lists models of
//!   providers a credential actually exists for. No connections means no menu and
//!   a boundary that says so, not a catalog of models the router could not reach.
//! * The transcript contains only turns that happened. A failed send becomes a
//!   visible error entry in sequence rather than a silent no-op.
//! * The composer is enabled exactly when a send would be valid, and its
//!   disabled state always has a stated reason.
//!
//! Derivations live in [`crate::dashboard::basic_chat_live`] so they stay
//! testable on the native target; this file is markup and wiring only.

use crate::api::{ApiError, Hydrate};
use crate::dashboard::basic_chat_live::{
    DraftError, ModelOption, ProviderModels, Turn, active_model_detail, active_model_label,
    default_model, load_connections, model_options, request_body, send_turn,
};
use leptos::prelude::*;

const BASIC_CHAT_STYLES: &str = r"
.nr-chat-card{min-height:620px;display:grid;grid-template-rows:auto auto minmax(280px,1fr) auto auto;gap:14px}
.nr-chat-toolbar,.nr-chat-actions,.nr-chat-trigger,.nr-chat-model-head,.nr-chat-model-top,.nr-chat-composer-actions,.nr-chat-composer-foot,.nr-chat-role{display:flex;align-items:center;gap:8px}
.nr-chat-toolbar,.nr-chat-model-head,.nr-chat-composer-foot{justify-content:space-between}
.nr-chat-trigger{min-width:min(100%,320px);border:1px solid var(--border-dark);border-radius:8px;background:var(--surface-dark-2);color:var(--text-main-dark);padding:10px 12px;text-align:left}
.nr-chat-trigger span:first-child,.nr-chat-copy{min-width:0;display:grid;gap:3px}
.nr-chat-trigger strong,.nr-chat-model-option strong{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.nr-chat-trigger small,.nr-chat-model-option small,.nr-chat-empty p,.nr-chat-note,.nr-chat-composer textarea::placeholder{color:var(--text-muted-dark)}
.nr-chat-chevron{margin-left:auto;color:var(--text-muted-dark)}
.nr-chat-model-menu,.nr-chat-composer,.nr-chat-boundary{border:1px solid var(--border-dark);border-radius:8px;background:var(--surface-dark-2)}
.nr-chat-model-menu{overflow:hidden}
.nr-chat-model-head{border-bottom:1px solid var(--border-dark);padding:10px 12px}
.nr-chat-model-list{display:grid;gap:8px;padding:10px}
.nr-chat-provider{display:grid;gap:8px;border:1px solid var(--border-dark);border-radius:8px;background:rgba(255,255,255,.025);padding:10px}
.nr-chat-model-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:8px}
.nr-chat-model-option{min-width:0;display:grid;gap:3px;border:1px solid var(--border-dark);border-radius:8px;background:var(--surface-dark);color:var(--text-main-dark);padding:10px;text-align:left}
.nr-chat-model-option.is-active{border-color:var(--brand);box-shadow:var(--shadow-warm)}
.nr-chat-main{min-height:0;display:grid;gap:12px;align-content:start;border:1px solid var(--border-dark);border-radius:8px;background:rgba(255,255,255,.025);padding:16px}
.nr-chat-empty{display:grid;place-items:center;min-height:220px;text-align:center}
.nr-chat-empty-inner{max-width:520px;display:grid;gap:10px;justify-items:center}
.nr-chat-empty-icon{width:52px;height:52px;display:grid;place-items:center;border:1px solid var(--border-dark);border-radius:8px;background:var(--surface-dark-2);color:color-mix(in srgb,var(--brand) 54%,white);font-weight:800}
.nr-chat-transcript{display:grid;gap:10px}
.nr-chat-message{max-width:min(88%,680px);display:grid;gap:6px;border:1px solid var(--border-dark);border-radius:8px;padding:12px}
.nr-chat-message.assistant{justify-self:start;background:var(--surface-dark)}
.nr-chat-message.user{justify-self:end;background:var(--surface-raised)}
.nr-chat-message.is-error{border-color:color-mix(in srgb,var(--warn) 55%,var(--border-dark));background:color-mix(in srgb,var(--warn) 8%,transparent)}
.nr-chat-message p{white-space:pre-wrap;overflow-wrap:anywhere;margin:0}
.nr-chat-role{color:var(--text-muted-dark);font-size:.76rem;font-weight:700;text-transform:uppercase}
.nr-chat-role small{font-weight:600;text-transform:none}
.nr-chat-pending{display:flex;align-items:center;gap:8px;color:var(--text-muted-dark);font-size:.86rem}
.nr-chat-boundary{display:grid;gap:4px;border-style:dashed;padding:12px;color:var(--text-muted-dark)}
.nr-chat-boundary strong{color:var(--text-main-dark)}
.nr-chat-composer{display:grid;gap:10px;padding:10px}
.nr-chat-composer textarea{width:100%;min-height:42px;max-height:25vh;resize:vertical;border:0;background:transparent;color:var(--text-main-dark);font:inherit;outline:0}
.nr-chat-composer-actions{justify-content:space-between}
.nr-chat-round-button{width:34px;height:34px;display:inline-grid;place-items:center;border:1px solid var(--border-dark);border-radius:999px;background:var(--surface-dark);color:var(--text-main-dark)}
.nr-chat-round-button:disabled{cursor:not-allowed;opacity:.55}
.nr-chat-note{text-align:center;font-size:.78rem}
@media (max-width:860px){.nr-chat-card{min-height:auto}.nr-chat-toolbar,.nr-chat-composer-actions{align-items:stretch;flex-direction:column}.nr-chat-actions,.nr-chat-trigger{width:100%}.nr-chat-actions .nr-button{flex:1}.nr-chat-model-grid{grid-template-columns:1fr}.nr-chat-message{max-width:100%}}
";

/// Everything the panel's subtrees read.
#[derive(Clone, Copy)]
struct ChatSignals {
    /// The model menu, derived from the configured connections.
    models: ReadSignal<Hydrate<Vec<ProviderModels>>>,
    set_models: WriteSignal<Hydrate<Vec<ProviderModels>>>,
    selected: RwSignal<Option<String>>,
    menu_open: RwSignal<bool>,
    transcript: RwSignal<Vec<Turn>>,
    draft: RwSignal<String>,
    /// `true` while a send is in flight.
    sending: RwSignal<bool>,
}

impl ChatSignals {
    /// Load the connection list and derive the menu from it.
    fn load(self) {
        self.set_models.set(Hydrate::Loading);
        let setter = self.set_models;
        let selected = self.selected;
        spawn(async move {
            match load_connections().await {
                Ok(connections) => {
                    let groups = model_options(&connections);
                    // Only pre-select when nothing is chosen, so a reload does
                    // not silently switch the model mid-conversation.
                    if selected.get_untracked().is_none() {
                        selected.set(default_model(&groups));
                    }
                    setter.set(Hydrate::Ready(groups));
                }
                Err(error) => setter.set(Hydrate::Failed(error)),
            }
        });
    }

    /// The groups currently available, or an empty slice.
    fn groups(self) -> Vec<ProviderModels> {
        self.models.get().ready().cloned().unwrap_or_default()
    }

    /// Why the composer cannot send right now, if it cannot.
    fn blocking_error(self) -> Option<DraftError> {
        if self.selected.get().is_none() {
            return Some(DraftError::NoModel);
        }
        if self.draft.get().trim().is_empty() {
            return Some(DraftError::Empty);
        }
        None
    }

    /// Send the draft as a new turn.
    fn send(self) {
        if self.sending.get_untracked() {
            return;
        }
        let Some(model) = self.selected.get_untracked() else {
            return;
        };
        let draft = self.draft.get_untracked();
        let history = self.transcript.get_untracked();
        let Ok(body) = request_body(&history, &draft, &model) else {
            return;
        };

        // The user's turn is appended before the request so the transcript
        // reflects what was actually sent, even if the reply never arrives.
        self.transcript
            .update(|turns| turns.push(Turn::user(draft.trim().to_owned())));
        self.draft.set(String::new());
        self.sending.set(true);

        let transcript = self.transcript;
        let sending = self.sending;
        spawn(async move {
            let outcome = send_turn(body).await;
            sending.set(false);
            transcript.update(|turns| turns.push(outcome.into_turn(model)));
        });
    }
}

/// Run a future on the browser's task queue.
///
/// Native builds have no queue and no `fetch` behind these calls, so the future
/// is dropped rather than driven, leaving the panel in the state that is true on
/// a target which cannot make the request.
#[cfg(target_arch = "wasm32")]
fn spawn<F>(task: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(task);
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn<F>(task: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    drop(task);
}

#[component]
pub(super) fn BasicChatPanel() -> impl IntoView {
    let (models, set_models) = signal(Hydrate::<Vec<ProviderModels>>::Loading);
    let signals = ChatSignals {
        models,
        set_models,
        selected: RwSignal::new(None),
        menu_open: RwSignal::new(false),
        transcript: RwSignal::new(Vec::new()),
        draft: RwSignal::new(String::new()),
        sending: RwSignal::new(false),
    };

    Effect::new(move |_| signals.load());

    view! {
        <style>{BASIC_CHAT_STYLES}</style>
        <div class="nr-panel-stack">
            <article class="nr-card nr-chat-card">
                <Toolbar signals />
                {move || match models.get() {
                    Hydrate::Loading => view! { <MenuSkeleton /> }.into_any(),
                    Hydrate::Failed(error) => view! { <MenuFailure error signals /> }.into_any(),
                    Hydrate::Ready(groups) if groups.is_empty() => {
                        view! { <ProviderBoundary /> }.into_any()
                    }
                    Hydrate::Ready(groups) => {
                        view! { <ModelMenu groups signals /> }.into_any()
                    }
                }}
                <Transcript signals />
                <Composer signals />
                <p class="nr-chat-note">
                    "Messages are sent to "
                    <code>"POST /api/dashboard/chat/completions"</code>
                    ", which routes through the configured provider. Replies are rendered when complete; this composer does not stream. "
                    "Nothing on this page is saved — the transcript is lost on reload."
                </p>
            </article>
        </div>
    }
}

#[component]
fn Toolbar(signals: ChatSignals) -> impl IntoView {
    let label = move || active_model_label(signals.selected.get().as_deref());
    let detail = move || active_model_detail(&signals.groups(), signals.selected.get().as_deref());
    let has_models = move || {
        signals
            .groups()
            .iter()
            .any(|group| !group.models.is_empty())
    };
    let has_turns = move || !signals.transcript.get().is_empty();

    view! {
        <div class="nr-chat-toolbar">
            <button
                type="button"
                class="nr-chat-trigger"
                aria-expanded=move || if signals.menu_open.get() { "true" } else { "false" }
                aria-label="Choose a model"
                disabled=move || !has_models()
                on:click=move |_| signals.menu_open.update(|open| *open = !*open)
            >
                <span class="nr-chat-copy">
                    <strong>{label}</strong>
                    <small>{detail}</small>
                </span>
                <span class="nr-chat-chevron" aria-hidden="true">"v"</span>
            </button>
            <div class="nr-chat-actions">
                <button
                    type="button"
                    class="nr-button secondary small"
                    disabled=move || !has_turns()
                    on:click=move |_| signals.transcript.set(Vec::new())
                >
                    "Clear transcript"
                </button>
            </div>
        </div>
    }
}

#[component]
fn MenuSkeleton() -> impl IntoView {
    view! {
        <div class="nr-chat-model-menu" aria-busy="true">
            <div class="nr-chat-model-head">
                <span class="nr-visually-hidden">"Loading connected providers."</span>
                <span class="nr-skeleton nr-skeleton-text-short">"—"</span>
                <span class="nr-spinner" aria-hidden="true"></span>
            </div>
            <div class="nr-chat-model-list">
                <div class="nr-skeleton nr-skeleton-row"></div>
                <div class="nr-skeleton nr-skeleton-row"></div>
            </div>
        </div>
    }
}

#[component]
fn MenuFailure(error: ApiError, signals: ChatSignals) -> impl IntoView {
    view! {
        <div class="nr-chat-boundary" role="alert">
            <strong>"Connected providers could not be read"</strong>
            <span>{error.message()}</span>
            <span>"Without the connection list there is no way to know which models this router can reach, so none are offered."</span>
            <p>
                <button
                    type="button"
                    class="nr-button secondary small"
                    on:click=move |_| signals.load()
                >
                    "Try again"
                </button>
            </p>
        </div>
    }
}

#[component]
fn ProviderBoundary() -> impl IntoView {
    view! {
        <div class="nr-chat-boundary">
            <strong>"No provider connections yet"</strong>
            <span>"This router holds no active provider connection, so there is no model to send to. Add one on the Providers page."</span>
            <span>"No model"</span>
        </div>
    }
}

#[component]
fn ModelMenu(groups: Vec<ProviderModels>, signals: ChatSignals) -> impl IntoView {
    let count = groups.len();
    let summary = format!(
        "{count} connected {}",
        if count == 1 { "provider" } else { "providers" }
    );

    view! {
        <Show when=move || signals.menu_open.get()>
            <div class="nr-chat-model-menu nr-anim-rise">
                <div class="nr-chat-model-head">
                    <span>
                        <strong>"Models"</strong>
                        <small>"Only from connected providers"</small>
                    </span>
                    <span class="nr-status-pill is-connected"><span></span>{summary.clone()}</span>
                </div>
                <div class="nr-chat-model-list nr-stagger">
                    <For
                        each={
                            let groups = groups.clone();
                            move || groups.clone()
                        }
                        key=|group| group.provider_id.clone()
                        children=move |group| view! { <ProviderModelGroup group signals /> }
                    />
                </div>
            </div>
        </Show>
    }
}

#[component]
fn ProviderModelGroup(group: ProviderModels, signals: ChatSignals) -> impl IntoView {
    let count = group.models.len().to_string();
    let models = group.models.clone();

    view! {
        <section class="nr-chat-provider">
            <div class="nr-chat-model-top">
                <strong>{group.provider_name}</strong>
                <span class="nr-status-pill is-idle"><span></span>{count}</span>
            </div>
            <div class="nr-chat-model-grid">
                <For
                    each=move || models.clone()
                    key=|model| model.request_model.clone()
                    children=move |model| view! { <ModelOptionButton model signals /> }
                />
            </div>
        </section>
    }
}

#[component]
fn ModelOptionButton(model: ModelOption, signals: ChatSignals) -> impl IntoView {
    let request_model = model.request_model.clone();
    let is_active = {
        let request_model = request_model.clone();
        move || signals.selected.get().as_deref() == Some(request_model.as_str())
    };
    let select = {
        let request_model = request_model.clone();
        move |_| {
            signals.selected.set(Some(request_model.clone()));
            signals.menu_open.set(false);
        }
    };
    // Read before the fields are moved into the view.
    let detail = model.detail();
    let model_id = model.model_id;

    view! {
        <button
            type="button"
            class="nr-chat-model-option"
            class:is-active=is_active.clone()
            aria-pressed=move || if is_active() { "true" } else { "false" }
            on:click=select
        >
            <strong>{model_id}</strong>
            <small>{request_model}</small>
            <small>{detail}</small>
        </button>
    }
}

#[component]
fn Transcript(signals: ChatSignals) -> impl IntoView {
    let is_empty = move || signals.transcript.get().is_empty();
    // Indexed outside the `view!` macro: a turbofish inside it is parsed as the
    // start of an element tag.
    let entries = move || -> Vec<(usize, Turn)> {
        signals.transcript.get().into_iter().enumerate().collect()
    };

    view! {
        <div class="nr-chat-main">
            <Show when=is_empty>
                <div class="nr-chat-empty">
                    <div class="nr-chat-empty-inner">
                        <span class="nr-chat-empty-icon" aria-hidden="true">"chat"</span>
                        <h2>"Start a conversation"</h2>
                        <p>"Pick a model from a connected provider and send a message. The reply comes back from that provider through this router."</p>
                    </div>
                </div>
            </Show>
            // `aria-live` on the log itself, so each appended turn is announced
            // as it arrives rather than only on focus.
            <div
                class="nr-chat-transcript"
                role="log"
                aria-live="polite"
                aria-label="Chat transcript"
            >
                <For
                    each=entries
                    key=|entry| entry.1.key(entry.0)
                    children=|entry| {
                        let turn = entry.1;
                        view! { <Bubble turn /> }
                    }
                />
                <Show when=move || signals.sending.get()>
                    <div class="nr-chat-message assistant">
                        <span class="nr-chat-role">"Assistant"</span>
                        <span class="nr-chat-pending">
                            <span class="nr-spinner" aria-hidden="true"></span>
                            "Waiting for the provider…"
                        </span>
                    </div>
                </Show>
            </div>
        </div>
    }
}

#[component]
fn Bubble(turn: Turn) -> impl IntoView {
    let class_name = format!(
        "nr-chat-message {}{}",
        turn.role.class_name(),
        if turn.is_error { " is-error" } else { "" }
    );

    view! {
        <div class=class_name>
            <span class="nr-chat-role">
                {if turn.is_error { "Not delivered" } else { turn.role.label() }}
                {turn.model.map(|model| view! { " " <small>{model}</small> })}
            </span>
            <p>{turn.text}</p>
        </div>
    }
}

#[component]
fn Composer(signals: ChatSignals) -> impl IntoView {
    let blocked = move || signals.blocking_error();
    let can_send = move || blocked().is_none() && !signals.sending.get();
    let send_hint = move || {
        blocked().map_or_else(
            || String::from("Send message"),
            |error| error.message().to_owned(),
        )
    };

    view! {
        <div class="nr-chat-composer">
            <label class="nr-visually-hidden" for="nr-chat-input">"Message"</label>
            <textarea
                id="nr-chat-input"
                placeholder="Message AI"
                rows="1"
                prop:value=move || signals.draft.get()
                disabled=move || signals.sending.get()
                on:input=move |event| signals.draft.set(event_target_value(&event))
                on:keydown=move |event: web_sys::KeyboardEvent| {
                    // Enter sends, Shift+Enter inserts a newline: the convention
                    // every chat composer this replaces already used.
                    if event.key() == "Enter" && !event.shift_key() {
                        event.prevent_default();
                        signals.send();
                    }
                }
            ></textarea>
            <div class="nr-chat-composer-actions">
                <div class="nr-chat-composer-foot">
                    <small>{move || active_model_label(signals.selected.get().as_deref())}</small>
                </div>
                <div class="nr-chat-composer-foot">
                    <span aria-live="polite" class="nr-chat-note">
                        {move || blocked().map(DraftError::message)}
                    </span>
                    <button
                        type="button"
                        class="nr-chat-round-button"
                        aria-label=send_hint
                        title=send_hint
                        disabled=move || !can_send()
                        on:click=move |_| signals.send()
                    >
                        {move || if signals.sending.get() {
                            view! { <span class="nr-spinner" aria-hidden="true"></span> }.into_any()
                        } else {
                            view! { "up" }.into_any()
                        }}
                    </button>
                </div>
            </div>
        </div>
    }
}
