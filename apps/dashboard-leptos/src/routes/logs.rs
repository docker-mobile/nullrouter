//! Router console logs, streamed by the events service.
//!
//! # Layout
//!
//! A fixed-height terminal pane that scrolls itself, with the connection state in the header where it
//! cannot be scrolled away. The previous version put the indicator in the page header as a bare dot with
//! a colour and no word, so "live" and "reconnecting" were indistinguishable to anyone who could not
//! separate green from grey -- and a stream that had quietly died looked exactly like a quiet router.

use leptos::prelude::*;

use crate::t;
use serde::Deserialize;

use crate::api::sse::{self, Connection};
use crate::ui::{
    Button, ButtonSize, ButtonVariant, Card, ConnectionDot, PageBody, PageHeader, Spinner,
};

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

/// How many lines are kept. A console that grows without bound eventually stalls the tab.
const MAX_LINES: usize = 500;

#[allow(
    clippy::too_many_lines,
    reason = "subscription, toolbar and transcript are inseparable here"
)]
#[component]
pub fn Logs() -> impl IntoView {
    let (lines, set_lines) = signal(Vec::<String>::new());
    let (connection, set_connection) = signal(Connection::default());
    let (paused, set_paused) = signal(false);

    // Stored, not bound as `_stream`: `Stream`'s Drop closes the EventSource, so a discarded handle would
    // open and immediately shut the socket.
    let stream = StoredValue::new_local(None::<sse::Stream>);
    Effect::new(move |_| {
        stream.set_value(sse::subscribe_named(
            "/api/translator/console-logs/stream",
            "console_logs",
            set_connection,
            move |frame: ConsoleFrame| {
                // Pausing freezes what is on screen without unsubscribing: the stream keeps its place in
                // the server's buffer, so resuming continues from where it was rather than from a gap.
                if paused.get_untracked() {
                    return;
                }
                let incoming = if frame.lines.is_empty() {
                    frame.logs
                } else {
                    frame.lines
                };
                let lines = incoming
                    .into_iter()
                    .map(|line| match line {
                        serde_json::Value::String(text) => text,
                        value => value.to_string(),
                    })
                    .collect::<Vec<_>>();
                if frame.kind == "clear" || frame.kind == "init" {
                    set_lines.set(lines);
                } else if !lines.is_empty() {
                    set_lines.update(|existing| {
                        existing.extend(lines);
                        if existing.len() > MAX_LINES {
                            existing.drain(..existing.len() - MAX_LINES);
                        }
                    });
                }
            },
        ));
    });

    let heading = t!("nav.logs");
    let blurb = t!("logs.description");
    let waiting = t!("logs.waiting");
    let clear_label = t!("logs.clear");
    let pause_label = t!("logs.pause");
    let resume_label = t!("logs.resume");
    let live_label = Signal::derive(move || connection.get().label().to_owned());
    let live = Signal::derive(move || connection.get().is_live());

    view! {
        <PageHeader title=heading description=blurb>
            <ConnectionDot live=live label=live_label />
        </PageHeader>

        <PageBody>
            <Card>
                <div class="flex items-center justify-between gap-2 border-b border-border px-3 py-2">
                    <div class="flex items-center gap-1 text-xs text-muted-foreground">
                        <span class="font-mono">{move || format_count(lines.get().len())}</span>
                        <span>{t!("logs.lines")}</span>
                    </div>
                    <div class="flex items-center gap-1">
                        {move || {
                            let text = if paused.get() { resume_label.clone() } else { pause_label.clone() };
                            view! {
                                <Button
                                    label=text
                                    variant=ButtonVariant::Ghost
                                    size=ButtonSize::Sm
                                    on_click=Callback::new(move |()| set_paused.update(|value| *value = !*value))
                                />
                            }
                        }}
                        <Button
                            label=clear_label
                            variant=ButtonVariant::Ghost
                            size=ButtonSize::Sm
                            on_click=Callback::new(move |()| set_lines.set(Vec::new()))
                        />
                    </div>
                </div>
                <div class="min-h-[20rem] max-h-[65dvh] overflow-y-auto bg-muted/20 px-4 py-3">
                    {move || {
                        let current = lines.get();
                        if current.is_empty() {
                            view! {
                                <div class="grid h-full min-h-[18rem] place-items-center">
                                    <div class="flex items-center gap-2 text-sm text-muted-foreground">
                                        <Spinner />
                                        <span>{waiting.clone()}</span>
                                    </div>
                                </div>
                            }
                                .into_any()
                        } else {
                            view! {
                                <div class="space-y-0.5 font-mono text-xs leading-relaxed">
                                    {current
                                        .into_iter()
                                        .map(|line| {
                                            view! {
                                                <pre class="whitespace-pre-wrap break-words text-foreground/90">
                                                    {line}
                                                </pre>
                                            }
                                        })
                                        .collect_view()}
                                </div>
                            }
                                .into_any()
                        }
                    }}
                </div>
            </Card>
        </PageBody>
    }
}

/// Group digits without pulling in a formatting crate.
fn format_count(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::format_count;

    #[test]
    fn line_counts_are_grouped() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(500), "500");
        assert_eq!(format_count(1234), "1,234");
    }
}
