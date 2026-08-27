//! The Console Log panel.
//!
//! This panel rendered `console_log_dashboard_state()`: an empty fixture whose
//! own pills said the stream was unwired, beside a Clear button that was disabled
//! because nothing had been connected to it. Both endpoints existed the whole
//! time.
//!
//! It now subscribes to `GET /api/translator/console-logs/stream` and appends each
//! frame as it arrives, falling back to `GET /api/translator/console-logs` for the
//! initial fill. When the stream drops, the panel says the feed is disconnected
//! and marks what is on screen as not current — it does not keep presenting old
//! lines as live output.
//!
//! Derivations live in [`crate::dashboard::console_log_live`]; this file is markup
//! and wiring.

use crate::api::{ApiError, Hydrate};
use crate::dashboard::console_log_live::{
    ClearOutcome, HISTORY_PATH, LogBuffer, LogLevel, LogLine, STREAM_PATH, StreamState,
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
/// The fixture version listed "EventSource stream unwired in this WASM slice" and
/// "Clear endpoint unwired". Both are gone: the stream is subscribed to and Clear
/// performs a `DELETE`, so rendering either would be a false statement about this
/// build.
static VISIBLE_CONTRACT: &[&str] = &[
    "nr-console-log-panel",
    "nr-console-log-viewport",
    "nr-console-level-legend",
    "nr-console-endpoint-list",
    "Console Log",
    "Clear",
    "No console logs yet.",
    "Disconnected",
    "Connected",
    "No live capture",
    "/api/translator/console-logs",
    "/api/translator/console-logs/stream",
    "Newest 200 lines retained",
    "0 retained",
    "200 max",
    "LOG",
    "INFO",
    "WARN",
    "ERROR",
    "DEBUG",
    "nr-console-level-log",
    "nr-console-level-info",
    "nr-console-level-warn",
    "nr-console-level-error",
    "nr-console-level-debug",
];

pub fn console_log_visible_contract() -> &'static [&'static str] {
    VISIBLE_CONTRACT
}

/// Everything the panel's subtrees read.
#[derive(Clone, Copy)]
struct ConsoleSignals {
    buffer: RwSignal<LogBuffer>,
    stream: RwSignal<StreamState>,
    /// The initial fill from the history endpoint, which is the only part of the
    /// panel that can be in a "failed to load" state.
    history: RwSignal<Hydrate<usize>>,
    clearing: RwSignal<bool>,
    notice: RwSignal<String>,
}

#[component]
pub(super) fn ConsoleLogPanel() -> impl IntoView {
    let signals = ConsoleSignals {
        buffer: RwSignal::new(LogBuffer::default()),
        stream: RwSignal::new(StreamState::default()),
        history: RwSignal::new(Hydrate::Loading),
        clearing: RwSignal::new(false),
        notice: RwSignal::new(String::new()),
    };

    fetch_history(signals);
    subscribe(signals);

    view! {
        <style>{TOOLS_LIVE_STYLES}</style>
        <div class="nr-panel-stack nr-console-log-panel">
            <article class="nr-card">
                <div class="nr-console-log-head">
                    <div>
                        <p class="nr-eyebrow">"Runtime stream"</p>
                        <h2>"Console Log"</h2>
                        <p>"Translator console output, streamed from the local router."</p>
                    </div>
                    <ClearAction signals />
                </div>
                <StreamBanner signals />
                <RetentionMeta signals />
                <LevelLegend />
                <LogViewport signals />
                <EndpointList signals />
            </article>
        </div>
    }
}

/// The Clear control and the result of the last clear.
#[component]
fn ClearAction(signals: ConsoleSignals) -> impl IntoView {
    let clearing = move || signals.clearing.get();

    view! {
        <div class="nr-console-log-actions">
            <button
                type="button"
                class="nr-button secondary small"
                title="Empty the router's console buffer with DELETE /api/translator/console-logs"
                aria-describedby="nr-console-clear-status"
                disabled=move || clearing()
                on:click=move |_| clear(signals)
            >
                "Clear"
            </button>
            <Show when=clearing>
                <span class="nr-spinner" aria-hidden="true"></span>
            </Show>
            <small id="nr-console-clear-status" role="status" aria-live="polite" aria-atomic="true">
                {move || signals.notice.get()}
            </small>
        </div>
    }
}

/// The stream's state, in words, with the two pills the panel has always shown.
#[component]
fn StreamBanner(signals: ConsoleSignals) -> impl IntoView {
    let stream = signals.stream;
    let stale = move || !stream.get().is_live();
    let capture_label = move || {
        if stream.get() == StreamState::Live {
            "Live capture"
        } else {
            "No live capture"
        }
    };
    let capture_class = move || {
        if stream.get() == StreamState::Live {
            "nr-status-pill is-connected"
        } else {
            "nr-status-pill is-degraded"
        }
    };

    view! {
        <div class="nr-console-log-stream" class:is-stale=stale>
            <div>
                <strong>"Stream status"</strong>
                <p class="nr-console-log-note">{move || stream.get().detail()}</p>
            </div>
            <div class="nr-console-log-meta" aria-label="Console log stream metadata">
                <span
                    class=move || format!("nr-status-pill {}", stream.get().class_name())
                    aria-label=move || format!("Console log stream: {}", stream.get().label())
                >
                    <span></span>
                    {move || stream.get().label()}
                </span>
                <span class=capture_class><span></span>{capture_label}</span>
            </div>
        </div>
    }
}

/// Retention counters, stated from what the buffer actually holds.
#[component]
fn RetentionMeta(signals: ConsoleSignals) -> impl IntoView {
    view! {
        <div class="nr-console-log-meta" aria-label="Console log retention metadata">
            <span class="nr-status-pill is-idle">
                <span></span>
                {move || signals.buffer.with(LogBuffer::retained_label)}
            </span>
            <span class="nr-status-pill is-idle">
                <span></span>
                {LogBuffer::max_label()}
            </span>
            <span class="nr-status-pill is-idle">
                <span></span>
                {move || signals.buffer.with(LogBuffer::trim_label)}
            </span>
        </div>
    }
}

#[component]
fn LevelLegend() -> impl IntoView {
    view! {
        <div class="nr-console-level-legend" aria-label="Console log level color semantics">
            {LogLevel::ALL
                .into_iter()
                .map(|level| {
                    view! {
                        <span class=format!("nr-console-level-chip {}", level.class_name())>
                            {level.label()}
                        </span>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// The log region.
///
/// `aria-live="polite"` is on the viewport itself, so an appended line is
/// announced without the whole panel being re-read.
#[component]
fn LogViewport(signals: ConsoleSignals) -> impl IntoView {
    view! {
        <pre
            class="nr-console-log-viewport"
            role="log"
            aria-label="Console log viewport"
            aria-live="polite"
            aria-busy=move || signals.history.get().is_loading().to_string()
        >
            {move || {
                let lines = signals.buffer.with(|buffer| buffer.lines().to_vec());
                if !lines.is_empty() {
                    return view! { <LogLines lines /> }.into_any();
                }
                match signals.history.get() {
                    Hydrate::Loading => {
                        view! {
                            <span class="nr-skeleton nr-skeleton-text" aria-hidden="true">"loading"</span>
                        }
                            .into_any()
                    }
                    Hydrate::Failed(error) => {
                        view! { <HistoryFailure error signals /> }.into_any()
                    }
                    Hydrate::Ready(_count) => {
                        view! { <span class="nr-console-log-empty">"No console logs yet."</span> }
                            .into_any()
                    }
                }
            }}
        </pre>
    }
}

#[component]
fn LogLines(lines: Vec<LogLine>) -> impl IntoView {
    view! {
        <code>
            <For
                each=move || lines.clone()
                key=|line| line.sequence
                children=|line| {
                let class_name = line.class_name();
                view! { <span class=class_name>{line.text}</span> }
            }
            />
        </code>
    }
}

/// The history fetch failed: say so, and offer a retry.
#[component]
fn HistoryFailure(error: ApiError, signals: ConsoleSignals) -> impl IntoView {
    view! {
        <span class="nr-tool-notice is-error" role="alert">
            <strong>"The console history could not be read"</strong>
            <span>{error.message()}</span>
            <button
                type="button"
                class="nr-button secondary small"
                on:click=move |_| {
                    signals.history.set(Hydrate::Loading);
                    fetch_history(signals);
                }
            >
                "Retry"
            </button>
        </span>
    }
}

/// The two endpoints this panel uses, and what each one last did.
#[component]
fn EndpointList(signals: ConsoleSignals) -> impl IntoView {
    let history_state = move || {
        signals.history.with(|state| match state {
            Hydrate::Loading => ("is-idle", "Reading".to_owned()),
            Hydrate::Failed(error) => ("is-degraded", error.message().to_owned()),
            Hydrate::Ready(count) => ("is-connected", format!("{count} lines read")),
        })
    };

    view! {
        <div class="nr-console-endpoint-list" aria-label="Console log endpoints in use">
            <div class="nr-console-endpoint-row">
                <span class="nr-console-endpoint-label">
                    <strong>"History"</strong>
                    <small>"GET/DELETE"</small>
                </span>
                <code>{HISTORY_PATH}</code>
                <span class=move || format!("nr-status-pill {}", history_state().0)>
                    <span></span>
                    {move || history_state().1}
                </span>
            </div>
            <div class="nr-console-endpoint-row">
                <span class="nr-console-endpoint-label">
                    <strong>"Stream"</strong>
                    <small>"GET"</small>
                </span>
                <code>{STREAM_PATH}</code>
                <span class=move || {
                    format!("nr-status-pill {}", signals.stream.get().class_name())
                }>
                    <span></span>
                    {move || signals.stream.get().label()}
                </span>
            </div>
        </div>
    }
}

/// Adopt a history fetch.
///
/// The stream's `init` frame may already have filled the buffer, in which case
/// the fetch only reports how many lines the endpoint held; overwriting would
/// discard the newer snapshot.
fn finish_history(signals: ConsoleSignals, outcome: Result<Vec<String>, ApiError>) {
    match outcome {
        Ok(lines) => {
            let count = lines.len();
            if signals.buffer.with(LogBuffer::is_empty) {
                signals.buffer.update(|buffer| buffer.replace(lines));
            }
            signals.history.set(Hydrate::Ready(count));
        }
        Err(error) => signals.history.set(Hydrate::Failed(error)),
    }
}

/// Adopt a clear.
fn finish_clear(signals: ConsoleSignals, outcome: ClearOutcome) {
    signals.clearing.set(false);
    if outcome.succeeded() {
        signals.buffer.update(LogBuffer::clear);
        signals.history.set(Hydrate::Ready(0));
    }
    signals.notice.set(outcome.message());
}

// ── requests ────────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
fn fetch_history(signals: ConsoleSignals) {
    wasm_bindgen_futures::spawn_local(async move {
        let outcome = crate::dashboard::console_log_live::load_history().await;
        finish_history(signals, outcome);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_history(signals: ConsoleSignals) {
    finish_history(signals, Err(ApiError::Environment));
}

#[cfg(target_arch = "wasm32")]
fn clear(signals: ConsoleSignals) {
    if signals.clearing.get_untracked() {
        return;
    }
    signals.clearing.set(true);
    signals.notice.set(String::from("Clearing…"));
    wasm_bindgen_futures::spawn_local(async move {
        let outcome = crate::dashboard::console_log_live::clear_history().await;
        finish_clear(signals, outcome);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn clear(signals: ConsoleSignals) {
    finish_clear(signals, ClearOutcome::Rejected(ApiError::Environment));
}

/// Subscribe to the console-log stream and append frames as they arrive.
///
/// The `EventSource` and its closures are parked in a thread-local so
/// [`on_cleanup`] — which takes a `Send + Sync` closure, and neither of those is
/// — can close them by token. Closing matters: an `EventSource` left open
/// reconnects forever after the panel is gone.
#[cfg(target_arch = "wasm32")]
fn subscribe(signals: ConsoleSignals) {
    use crate::dashboard::console_log_live::{
        CONNECTED_EVENT, CONSOLE_LOGS_EVENT, FrameKind, parse_connected_frame, parse_console_frame,
    };
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::{EventSource, MessageEvent};

    let Ok(source) = EventSource::new(STREAM_PATH) else {
        signals.stream.set(StreamState::Unavailable);
        return;
    };

    let on_frame = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Some(data) = event.data().as_string() else {
            return;
        };
        let Some(frame) = parse_console_frame(&data) else {
            // Not a frame this build understands; ignored rather than turned
            // into a blank line.
            return;
        };
        signals
            .stream
            .set(StreamState::from_capture(frame.live_capture));

        // An `init` frame is a snapshot, and the events service ends its response
        // after sending one — so the browser reconnects and sends another every
        // few seconds. An empty snapshot on reconnect must therefore not wipe
        // lines that are already held; only a snapshot that carries content
        // replaces what is on screen. A `clear`, which is an explicit
        // instruction, still empties the buffer.
        let replaces = !frame.lines.is_empty();
        signals.buffer.update(|buffer| match frame.kind {
            FrameKind::Init if replaces => buffer.replace(frame.lines),
            FrameKind::Init => {}
            FrameKind::Append => buffer.extend(frame.lines),
            FrameKind::Clear => buffer.clear(),
        });
        if frame.kind == FrameKind::Clear || (frame.kind == FrameKind::Init && replaces) {
            // The stream's snapshot is now the authority on what exists, so the
            // history row reports from it.
            let held = signals.buffer.with_untracked(LogBuffer::len);
            signals.history.set(Hydrate::Ready(held));
        }
    });

    let on_connected = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let connected = event
            .data()
            .as_string()
            .and_then(|data| parse_connected_frame(&data))
            .unwrap_or(false);
        if connected && !signals.stream.get_untracked().is_live() {
            // A `connected` frame proves the socket is open. Whether output is
            // being captured is only known once a `console_logs` frame arrives,
            // so this does not claim capture.
            signals.stream.set(StreamState::Connecting);
        }
    });

    let on_error = Closure::<dyn FnMut()>::new(move || {
        // The browser retries on its own; say the feed is disconnected rather
        // than leaving the lines on screen labelled as live.
        signals.stream.set(StreamState::Interrupted);
    });

    let named = source
        .add_event_listener_with_callback(CONSOLE_LOGS_EVENT, on_frame.as_ref().unchecked_ref())
        .is_ok();
    // The events service also emits the payload as a default `message` event,
    // which is what upstream's client reads; both are handled so either shape
    // fills the panel.
    let default = source
        .add_event_listener_with_callback("message", on_frame.as_ref().unchecked_ref())
        .is_ok();
    if !named && !default {
        source.close();
        signals.stream.set(StreamState::Unavailable);
        return;
    }
    // The `connected` frame is a nicety: it confirms the socket opened before any
    // log frame arrives. Failing to register it costs nothing, so it is not a
    // reason to abandon the subscription.
    drop(
        source.add_event_listener_with_callback(
            CONNECTED_EVENT,
            on_connected.as_ref().unchecked_ref(),
        ),
    );
    source.set_onerror(Some(on_error.as_ref().unchecked_ref()));

    let token = stream_registry::register(
        source,
        vec![
            on_frame.into_js_value(),
            on_connected.into_js_value(),
            on_error.into_js_value(),
        ],
    );
    on_cleanup(move || stream_registry::close(token));
}

/// Live subscriptions, parked so cleanup can reach them without capturing
/// browser handles in a `Send + Sync` closure.
#[cfg(target_arch = "wasm32")]
mod stream_registry {
    use std::cell::RefCell;

    use wasm_bindgen::JsValue;
    use web_sys::EventSource;

    /// One live subscription: the source, plus the closures that must outlive
    /// the call that registered them.
    struct Subscription {
        token: u64,
        source: EventSource,
        _closures: Vec<JsValue>,
    }

    thread_local! {
        static NEXT_TOKEN: RefCell<u64> = const { RefCell::new(0) };
        static SUBSCRIPTIONS: RefCell<Vec<Subscription>> = const { RefCell::new(Vec::new()) };
    }

    /// Park a subscription and return its cleanup token.
    pub(super) fn register(source: EventSource, closures: Vec<JsValue>) -> u64 {
        let token = NEXT_TOKEN.with_borrow_mut(|next| {
            *next = next.saturating_add(1);
            *next
        });
        SUBSCRIPTIONS.with_borrow_mut(|live| {
            live.push(Subscription {
                token,
                source,
                _closures: closures,
            });
        });
        token
    }

    /// Close and drop the subscription with this token.
    pub(super) fn close(token: u64) {
        let found = SUBSCRIPTIONS.with_borrow_mut(|live| {
            live.iter()
                .position(|entry| entry.token == token)
                .map(|index| live.swap_remove(index))
        });
        if let Some(entry) = found {
            entry.source.close();
        }
    }
}

/// Native builds have no `EventSource`, so there is no live feed to report.
#[cfg(not(target_arch = "wasm32"))]
fn subscribe(signals: ConsoleSignals) {
    signals.stream.set(StreamState::Unavailable);
}
