//! The Translator Debug panel.
//!
//! This panel used to render `translator_dashboard_state()`: seven steps whose
//! bodies were compile-time strings — one of them literally
//! `{"status":"preview", …}` — with every action disabled. A user could read it
//! and believe they were looking at a translated payload.
//!
//! Every buffer here is now either loaded from `GET /api/translator/load`, typed
//! by the user, computed by `POST /api/translator/translate`, or returned by
//! `POST /api/translator/send`. Which of those it is shows on the step, and a call
//! that produces no body says so instead of leaving the previous content in place.
//!
//! Derivations live in [`crate::dashboard::translator_live`]; this file is markup
//! and wiring.

use crate::dashboard::translator_live::{
    LoadOutcome, NO_READING, SaveOutcome, SendOutcome, StepSource, TranslateOutcome, TranslateStep,
    Translation, TranslationMeta, TranslatorFile, format_json, merge_meta, save_body, send_body,
    translate_body,
};
use leptos::prelude::*;

/// Panel styles, shared verbatim with the actix host.
///
/// Inlined from the file the host serves at `/assets/dashboard/tools-live.css`,
/// because the CSR build links no stylesheet of its own.
const TOOLS_LIVE_STYLES: &str =
    include_str!("../../../../services/dashboard-actix/static/assets/dashboard/tools-live.css");

/// Strings this route renders, asserted by the CSR-only boundary tests.
///
/// Only entries the panel can actually put on screen belong here. The three
/// "preview"/"disabled" claims the fixture version carried are gone, because
/// nothing on this page is a preview any more.
const VISIBLE_CONTRACT: [&str; 44] = [
    "nr-translator-panel",
    "nr-translator-meta",
    "nr-translator-step",
    "nr-translator-code",
    "Translator Debug",
    "Replay request flow — matches log files",
    "Client Request",
    "1_req_client.json",
    "Source Body",
    "2_req_source.json",
    "OpenAI Intermediate",
    "3_req_openai.json",
    "Target Request",
    "4_req_target.json",
    "Provider Response",
    "5_res_provider.txt",
    "OpenAI Response",
    "6_res_openai.txt",
    "Client Response",
    "7_res_client.txt",
    "7_res_client.json",
    "Raw request from client",
    "source → openai",
    "openai → target + URL + headers",
    "json",
    "text",
    "expand_more",
    "chevron_right",
    "Load",
    "Copy",
    "Format",
    "→ OpenAI",
    "→ Target",
    "Send",
    "src:",
    "dst:",
    "provider:",
    "model:",
    "Filesystem",
    "Save",
    "Provider execution",
    "Persistence",
    "API default:",
    "Detect",
];

pub const fn translator_visible_contract() -> &'static [&'static str] {
    &VISIBLE_CONTRACT
}

/// One step's own state.
///
/// `buffer` is the editable content, `source` records where it came from, and
/// `status` is the last thing a request said about this step. They are per step so
/// a failure on one stage never blanks another.
#[derive(Clone, Copy)]
struct StepSignals {
    buffer: RwSignal<String>,
    source: RwSignal<StepSource>,
    status: RwSignal<StepStatus>,
    busy: RwSignal<bool>,
}

/// The last outcome reported on a step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct StepStatus {
    text: String,
    failed: bool,
}

impl StepStatus {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            failed: false,
        }
    }

    fn bad(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            failed: true,
        }
    }
}

/// Signals shared by every step.
#[derive(Clone, Copy)]
struct PanelSignals {
    meta: RwSignal<TranslationMeta>,
    /// Steps, in pipeline order. Fixed length, so a lookup by index is total.
    steps: StoredValue<[StepSignals; 7]>,
}

impl PanelSignals {
    /// The signals for one stage.
    fn step(self, file: TranslatorFile) -> StepSignals {
        let index = usize::from(file.index()).saturating_sub(1);
        self.steps.with_value(|steps| {
            steps
                .get(index)
                .copied()
                // Unreachable in practice: `index()` is 1..=7 and `steps` has 7
                // entries. Written as a fallback rather than an index so the
                // lookup cannot panic.
                .unwrap_or_else(new_step_signals)
        })
    }
}

fn new_step_signals() -> StepSignals {
    StepSignals {
        buffer: RwSignal::new(String::new()),
        source: RwSignal::new(StepSource::Empty),
        status: RwSignal::new(StepStatus::default()),
        busy: RwSignal::new(false),
    }
}

#[component]
pub(super) fn TranslatorPanel() -> impl IntoView {
    let expanded_step = RwSignal::new(Some(1_u8));
    let panel = PanelSignals {
        meta: RwSignal::new(TranslationMeta::default()),
        steps: StoredValue::new([
            new_step_signals(),
            new_step_signals(),
            new_step_signals(),
            new_step_signals(),
            new_step_signals(),
            new_step_signals(),
            new_step_signals(),
        ]),
    };

    view! {
        <style>{TOOLS_LIVE_STYLES}</style>
        <div class="nr-panel-stack nr-translator-panel">
            <article class="nr-card">
                <div class="nr-translator-head">
                    <div>
                        <p class="nr-eyebrow">"Translator"</p>
                        <h2>"Translator Debug"</h2>
                        <p>"Replay request flow — matches log files"</p>
                    </div>
                    <div class="nr-translator-meta" aria-label="Detected translation metadata">
                        <MetaBadges panel />
                    </div>
                </div>
                <p class="nr-tool-meta">
                    "Each step loads from logs/translator over GET /api/translator/load, saves with POST /api/translator/save, and translates with POST /api/translator/translate. Nothing is shown that a request did not return."
                </p>
                <div class="nr-translator-status" aria-label="Translator surfaces in use">
                    <CapabilityCard
                        label="Filesystem"
                        detail="Load and Save read and write logs/translator through the router."
                    />
                    <CapabilityCard
                        label="Persistence"
                        detail="Save reports what the router stored; it never claims a write the router refused."
                    />
                    <CapabilityCard
                        label="Provider execution"
                        detail="Send posts the target request to the provider and shows the response verbatim."
                    />
                    <CapabilityCard
                        label="Detect"
                        detail="Step 1 asks the router for the provider, model, and formats shown above."
                    />
                </div>
            </article>

            <div class="nr-translator-flow">
                {TranslatorFile::ALL
                    .into_iter()
                    .map(|file| view! { <StepCard file panel expanded_step /> })
                    .collect_view()}
            </div>
        </div>
    }
}

/// The four `src`/`dst`/`provider`/`model` badges, from the last detect call.
#[component]
fn MetaBadges(panel: PanelSignals) -> impl IntoView {
    view! {
        {move || {
            panel
                .meta
                .with(TranslationMeta::badges)
                .into_iter()
                .map(|(label, value)| {
                    let title = if value == NO_READING {
                        format!("{label} has not been reported by the router yet")
                    } else {
                        format!("{label} reported by the router: {value}")
                    };
                    view! {
                        <span class="nr-translator-badge" title=title>
                            <small>{label}":"</small>
                            {value}
                        </span>
                    }
                })
                .collect_view()
        }}
    }
}

#[component]
fn CapabilityCard(label: &'static str, detail: &'static str) -> impl IntoView {
    view! {
        <div class="nr-translator-capability">
            <strong>{label}</strong>
            <span>{detail}</span>
        </div>
    }
}

#[component]
fn StepCard(
    file: TranslatorFile,
    panel: PanelSignals,
    expanded_step: RwSignal<Option<u8>>,
) -> impl IntoView {
    let step_id = file.index();
    let signals = panel.step(file);
    let is_expanded = move || expanded_step.get() == Some(step_id);
    let toggle_step = move |_| {
        expanded_step.update(|selected| {
            *selected = match *selected {
                Some(current) if current == step_id => None,
                _ => Some(step_id),
            };
        });
    };

    view! {
        <article class="nr-translator-step">
            <div class="nr-translator-step-head">
                <button
                    type="button"
                    class="nr-translator-step-toggle"
                    aria-expanded=move || is_expanded().to_string()
                    aria-controls=file.editor_id()
                    on:click=toggle_step
                >
                    <span
                        class="nr-translator-step-icon"
                        aria-label=move || {
                            if is_expanded() { "expand_more" } else { "chevron_right" }
                        }
                    >
                        {move || if is_expanded() { "v" } else { ">" }}
                    </span>
                    <span class="nr-translator-step-index">{step_id}</span>
                    <span class="nr-translator-step-title">
                        <strong>{file.label()}</strong>
                        <span class="nr-translator-step-file">{file.file_name()}</span>
                        <span class="nr-translator-step-desc">{file.description()}</span>
                    </span>
                    <span class="nr-translator-language">{file.language()}</span>
                </button>
                <span
                    class=move || {
                        format!("nr-status-pill {}", signals.source.get().class_name())
                    }
                >
                    <span></span>
                    {move || signals.source.get().label()}
                </span>
            </div>
            <Show when=is_expanded>
                <StepBody file panel signals />
            </Show>
        </article>
    }
}

/// The editor, the actions, and the step's status line.
#[component]
fn StepBody(file: TranslatorFile, panel: PanelSignals, signals: StepSignals) -> impl IntoView {
    let editor_id = file.editor_id();
    let status_id = file.status_id();
    let busy = move || signals.busy.get();

    view! {
        <div class="nr-translator-step-body">
            <div class="nr-translator-code-head">
                <label for=editor_id.clone()>
                    <strong>{file.label()}" body"</strong>
                </label>
                <small>
                    {if file.is_json() {
                        "Edited here, then sent verbatim. Format pretty-prints it locally."
                    } else {
                        "Response text, shown exactly as it was received."
                    }}
                </small>
            </div>
            <textarea
                id=editor_id
                class="nr-translator-code nr-translator-editor"
                spellcheck="false"
                aria-describedby=status_id.clone()
                aria-busy=move || busy().to_string()
                prop:value=move || signals.buffer.get()
                disabled=busy
                on:input=move |event| {
                    signals.buffer.set(event_target_value(&event));
                    signals.source.set(StepSource::Edited);
                }
            ></textarea>
            {file
                .alternate_file()
                .map(|alternate| {
                    view! {
                        <span class="nr-translator-badge nr-translator-default-file">
                            <small>"API default:"</small>
                            {alternate}
                        </span>
                    }
                })}
            <StepActions file panel signals />
            <StepStatusLine signals status_id />
        </div>
    }
}

/// The status line for one step, announced politely.
#[component]
fn StepStatusLine(signals: StepSignals, status_id: String) -> impl IntoView {
    view! {
        <p
            id=status_id
            class=move || {
                if signals.status.with(|status| status.failed) {
                    "nr-translator-step-status is-failed"
                } else {
                    "nr-translator-step-status"
                }
            }
            role="status"
            aria-live="polite"
            aria-atomic="true"
        >
            {move || signals.status.with(|status| status.text.clone())}
        </p>
    }
}

#[component]
fn StepActions(file: TranslatorFile, panel: PanelSignals, signals: StepSignals) -> impl IntoView {
    let busy = move || signals.busy.get();
    let empty = move || signals.buffer.with(|buffer| buffer.trim().is_empty());

    view! {
        <div class="nr-translator-actions">
            <button
                type="button"
                class="nr-button secondary small"
                disabled=busy
                on:click=move |_| load_step(file, signals)
            >
                "Load"
            </button>
            <button
                type="button"
                class="nr-button secondary small"
                disabled=move || busy() || empty()
                on:click=move |_| copy_step(signals)
            >
                "Copy"
            </button>
            <Show when=move || file.is_json()>
                <button
                    type="button"
                    class="nr-button secondary small"
                    disabled=move || busy() || empty()
                    on:click=move |_| format_step(signals)
                >
                    "Format"
                </button>
            </Show>
            <button
                type="button"
                class="nr-button secondary small"
                disabled=move || busy() || empty()
                on:click=move |_| save_step(file, signals)
            >
                "Save"
            </button>
            {file
                .translate_step()
                .map(|step| {
                    view! {
                        <button
                            type="button"
                            class="nr-button primary small"
                            title=step.detail()
                            disabled=move || busy() || empty()
                            on:click=move |_| translate_step(step, panel, signals)
                        >
                            {step.label()}
                        </button>
                    }
                })}
            <Show when=move || file.is_sendable()>
                <button
                    type="button"
                    class="nr-button primary small"
                    title="Post this body to the provider and show its response."
                    disabled=move || busy() || empty()
                    on:click=move |_| send_step(panel, signals)
                >
                    "Send"
                </button>
            </Show>
        </div>
    }
}

/// Pretty-print the buffer, reporting a parse failure rather than replacing it.
fn format_step(signals: StepSignals) {
    let formatted = signals.buffer.with(|buffer| format_json(buffer));
    match formatted {
        Some(text) => {
            signals.buffer.set(text);
            signals.source.set(StepSource::Edited);
            signals.status.set(StepStatus::ok("Formatted."));
        }
        None => signals.status.set(StepStatus::bad(
            "This body is not valid JSON, so it was left unchanged.",
        )),
    }
}

/// Copy the buffer to the clipboard.
#[cfg(target_arch = "wasm32")]
fn copy_step(signals: StepSignals) {
    let text = signals.buffer.get_untracked();
    let Some(clipboard) = web_sys::window().map(|window| window.navigator().clipboard()) else {
        signals
            .status
            .set(StepStatus::bad("This browser exposes no clipboard."));
        return;
    };
    let promise = clipboard.write_text(&text);
    wasm_bindgen_futures::spawn_local(async move {
        let status = if wasm_bindgen_futures::JsFuture::from(promise).await.is_ok() {
            StepStatus::ok("Copied to the clipboard.")
        } else {
            StepStatus::bad("The browser refused the clipboard write.")
        };
        signals.status.set(status);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn copy_step(signals: StepSignals) {
    signals
        .status
        .set(StepStatus::bad("No clipboard outside a browser."));
}

/// Report the result of a load.
fn finish_load(signals: StepSignals, outcome: LoadOutcome) {
    signals.busy.set(false);
    match outcome {
        LoadOutcome::Loaded(content) => {
            let empty = content.trim().is_empty();
            signals.buffer.set(content);
            signals.source.set(StepSource::Loaded);
            signals.status.set(if empty {
                StepStatus::ok("Loaded. The file exists and is empty.")
            } else {
                StepStatus::ok("Loaded from logs/translator.")
            });
        }
        LoadOutcome::Missing(detail) => {
            // The buffer is deliberately left alone: replacing it with nothing
            // would discard an edit in progress over a file that never existed.
            signals
                .status
                .set(StepStatus::bad(format!("Not loaded. {detail}")));
        }
        LoadOutcome::Rejected(error) => {
            signals
                .status
                .set(StepStatus::bad(format!("Not loaded. {}", error.message())));
        }
    }
}

/// Report the result of a translate call.
fn finish_translate(
    step: TranslateStep,
    panel: PanelSignals,
    signals: StepSignals,
    outcome: TranslateOutcome,
) {
    signals.busy.set(false);
    let status = match outcome {
        TranslateOutcome::Translated(result) => settle_translation(step, panel, &result),
        TranslateOutcome::Refused(detail) => StepStatus::bad(format!("Not translated. {detail}")),
        TranslateOutcome::Rejected(error) => {
            StepStatus::bad(format!("Not translated. {}", error.message()))
        }
    };
    signals.status.set(status);
}

/// Adopt a translation: record its metadata, fill the step it feeds, and say
/// what happened.
///
/// A call that returned no body reports exactly that. It does not leave the
/// destination step holding older content while claiming a fresh translation.
fn settle_translation(
    step: TranslateStep,
    panel: PanelSignals,
    result: &Translation,
) -> StepStatus {
    panel
        .meta
        .update(|meta| *meta = merge_meta(meta, result.meta.clone()));

    let Some(file) = step.writes_into() else {
        // Step 1 reports metadata only.
        let known = panel
            .meta
            .with(|meta| meta.provider.is_some() || meta.source_format.is_some());
        return if known {
            StepStatus::ok("Detected. The badges above are from this call.")
        } else {
            StepStatus::bad("The router answered without naming a provider, model, or format.")
        };
    };

    let Some(body) = result.body.clone() else {
        return StepStatus::bad(Translation::empty_note());
    };

    let destination = panel.step(file);
    destination.buffer.set(body);
    destination.source.set(StepSource::Translated);
    destination
        .status
        .set(StepStatus::ok("Written by the router's translate call."));

    result.headers.clone().map_or_else(
        || StepStatus::ok(format!("Translated into step {}.", file.index())),
        |headers| {
            let lines = headers.lines().count();
            StepStatus::ok(format!(
                "Translated into step {}, with {lines} lines of request headers{}.",
                file.index(),
                result
                    .meta
                    .url
                    .as_ref()
                    .map_or_else(String::new, |url| format!(" for {url}"))
            ))
        },
    )
}

/// Report the result of a send, filling the provider-response step.
fn finish_send(panel: PanelSignals, signals: StepSignals, outcome: SendOutcome) {
    signals.busy.set(false);
    let message = outcome.message();
    if let SendOutcome::Answered(body) = outcome {
        let destination = panel.step(TranslatorFile::ProviderResponse);
        destination.buffer.set(body);
        destination.source.set(StepSource::Received);
        destination
            .status
            .set(StepStatus::ok("Received from the provider."));
        signals.status.set(StepStatus::ok(message));
    } else {
        signals.status.set(StepStatus::bad(message));
    }
}

/// Report the result of a save.
fn finish_save(signals: StepSignals, outcome: SaveOutcome) {
    signals.busy.set(false);
    let message = outcome.message();
    signals.status.set(if outcome.wrote_file() {
        StepStatus::ok(message)
    } else {
        StepStatus::bad(message)
    });
}

// ── requests ────────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
fn load_step(file: TranslatorFile, signals: StepSignals) {
    signals.busy.set(true);
    signals.status.set(StepStatus::ok("Loading…"));
    wasm_bindgen_futures::spawn_local(async move {
        let outcome = crate::dashboard::translator_live::load_file(file).await;
        finish_load(signals, outcome);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn load_step(_file: TranslatorFile, signals: StepSignals) {
    finish_load(
        signals,
        LoadOutcome::Rejected(crate::api::ApiError::Environment),
    );
}

#[cfg(target_arch = "wasm32")]
fn save_step(file: TranslatorFile, signals: StepSignals) {
    let body = match signals
        .buffer
        .with_untracked(|buffer| save_body(file, buffer))
    {
        Ok(body) => body,
        Err(error) => {
            signals.status.set(StepStatus::bad(error.message()));
            return;
        }
    };
    signals.busy.set(true);
    signals.status.set(StepStatus::ok("Saving…"));
    wasm_bindgen_futures::spawn_local(async move {
        let outcome = crate::dashboard::translator_live::save_file(body).await;
        finish_save(signals, outcome);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn save_step(file: TranslatorFile, signals: StepSignals) {
    if let Err(error) = signals
        .buffer
        .with_untracked(|buffer| save_body(file, buffer))
    {
        signals.status.set(StepStatus::bad(error.message()));
        return;
    }
    finish_save(
        signals,
        SaveOutcome::Rejected(crate::api::ApiError::Environment),
    );
}

#[cfg(target_arch = "wasm32")]
fn translate_step(step: TranslateStep, panel: PanelSignals, signals: StepSignals) {
    let Some(body) = build_translate_body(step, panel, signals) else {
        return;
    };
    signals.busy.set(true);
    signals.status.set(StepStatus::ok("Translating…"));
    wasm_bindgen_futures::spawn_local(async move {
        let outcome = crate::dashboard::translator_live::translate(body).await;
        finish_translate(step, panel, signals, outcome);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn translate_step(step: TranslateStep, panel: PanelSignals, signals: StepSignals) {
    if build_translate_body(step, panel, signals).is_some() {
        finish_translate(
            step,
            panel,
            signals,
            TranslateOutcome::Rejected(crate::api::ApiError::Environment),
        );
    }
}

/// Validate the buffer and build the translate body, reporting why not.
fn build_translate_body(
    step: TranslateStep,
    panel: PanelSignals,
    signals: StepSignals,
) -> Option<String> {
    let built = signals.buffer.with_untracked(|buffer| {
        panel
            .meta
            .with_untracked(|meta| translate_body(step, buffer, meta))
    });
    match built {
        Ok(body) => Some(body),
        Err(error) => {
            signals.status.set(StepStatus::bad(error.message()));
            None
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn send_step(panel: PanelSignals, signals: StepSignals) {
    let body = match build_send_body(panel, signals) {
        Some(body) => body,
        None => return,
    };
    signals.busy.set(true);
    signals.status.set(StepStatus::ok("Sending…"));
    wasm_bindgen_futures::spawn_local(async move {
        let outcome = crate::dashboard::translator_live::send(body).await;
        finish_send(panel, signals, outcome);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn send_step(panel: PanelSignals, signals: StepSignals) {
    if build_send_body(panel, signals).is_some() {
        finish_send(
            panel,
            signals,
            SendOutcome::Rejected(crate::api::ApiError::Environment),
        );
    }
}

/// Validate the buffer and build the send body, reporting why not.
fn build_send_body(panel: PanelSignals, signals: StepSignals) -> Option<String> {
    let built = signals
        .buffer
        .with_untracked(|buffer| panel.meta.with_untracked(|meta| send_body(buffer, meta)));
    match built {
        Ok(body) => Some(body),
        Err(error) => {
            signals.status.set(StepStatus::bad(error.message()));
            None
        }
    }
}
