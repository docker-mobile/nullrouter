//! Console logs streamed over SSE.

use leptos::prelude::*;
use crate::api::sse::{self, Connection};

#[component]
pub fn Logs() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (lines, set_lines) = signal(Vec::<String>::new());
    let (connection, set_connection) = signal(Connection::default());

    Effect::new(move || {
        let _stream = sse::subscribe("/api/usage/logs", set_connection, move |text: String| {
            set_lines.update(|existing| {
                existing.push(text);
                if existing.len() > 200 {
                    existing.drain(0..50);
                }
            });
        });
    });

    view! {
        <crate::routes::PageHeader
            title=locale.get("nav.logs").to_owned()
            description=locale.get("logs.description").to_owned()
        >
            <ConnectionIndicator connection=connection />
        </crate::routes::PageHeader>

        <div class="rounded-lg border border-border bg-card font-mono text-xs overflow-hidden">
            <div class="max-h-[600px] overflow-y-auto p-4 space-y-1">
                {move || {
                    if lines.get().is_empty() {
                        view! {
                            <p class="text-muted-foreground italic">
                                {locale.get("logs.waiting").to_owned()}
                            </p>
                        }
                            .into_any()
                    } else {
                        lines
                            .get()
                            .into_iter()
                            .map(|line| view! { <pre class="text-foreground">{line}</pre> })
                            .collect_view()
                            .into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn ConnectionIndicator(connection: ReadSignal<Connection>) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2 text-xs">
            <span class=move || {
                if connection.get().is_live() {
                    "size-1.5 rounded-full bg-success animate-pulse"
                } else {
                    "size-1.5 rounded-full bg-muted-foreground/40"
                }
            } />
            <span class="text-muted-foreground">{move || connection.get().label().to_owned()}</span>
        </div>
    }
}
