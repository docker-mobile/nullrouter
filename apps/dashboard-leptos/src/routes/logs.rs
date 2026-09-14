//! Router console logs streamed by the events service.

use leptos::prelude::*;
use serde::Deserialize;

use crate::api::sse::{self, Connection};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConsoleFrame {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    logs: Vec<serde_json::Value>,
    #[serde(default)]
    lines: Vec<serde_json::Value>,
}

#[component]
pub fn Logs() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (lines, set_lines) = signal(Vec::<String>::new());
    let (connection, set_connection) = signal(Connection::default());

    // Stored, not bound as `_stream`: Stream's Drop closes the EventSource, so a
    // discarded handle would open and immediately shut the socket.
    let stream = StoredValue::new(None::<sse::Stream>);
    Effect::new(move || {
        stream.set_value(sse::subscribe_named(
            "/api/translator/console-logs/stream",
            "console_logs",
            set_connection,
            move |frame: ConsoleFrame| {
                let incoming = if frame.lines.is_empty() {
                    frame.logs
                } else {
                    frame.lines
                };
                let parsed = incoming
                    .into_iter()
                    .map(|line| match line {
                        serde_json::Value::String(text) => text,
                        value => value.to_string(),
                    })
                    .collect::<Vec<_>>();
                if frame.kind == "clear" || frame.kind == "init" {
                    set_lines.set(parsed);
                } else if !parsed.is_empty() {
                    set_lines.update(|existing| {
                        existing.extend(parsed);
                        if existing.len() > 200 {
                            existing.drain(..existing.len() - 200);
                        }
                    });
                }
            },
        ));
    });

    view! {
        <crate::routes::PageHeader title=locale.get("nav.logs").to_owned()
            description=locale.get("logs.description").to_owned()>
            <ConnectionIndicator connection=connection />
        </crate::routes::PageHeader>
        <div class="rounded-lg border border-border bg-card font-mono text-xs overflow-hidden">
            <div class="max-h-[600px] overflow-y-auto p-4 space-y-1">
                {move || if lines.get().is_empty() {
                    view! { <p class="text-muted-foreground italic">{locale.get("logs.waiting").to_owned()}</p> }.into_any()
                } else {
                    lines.get().into_iter().map(|line| view! { <pre class="text-foreground whitespace-pre-wrap break-words">{line}</pre> }).collect_view().into_any()
                }}
            </div>
        </div>
    }
}

#[component]
fn ConnectionIndicator(connection: ReadSignal<Connection>) -> impl IntoView {
    view! { <div class="flex items-center gap-2 text-xs">
        <span class=move || if connection.get().is_live() { "size-1.5 rounded-full bg-success animate-pulse" } else { "size-1.5 rounded-full bg-muted-foreground/40" } />
        <span class="text-muted-foreground">{move || connection.get().label().to_owned()}</span>
    </div> }
}
