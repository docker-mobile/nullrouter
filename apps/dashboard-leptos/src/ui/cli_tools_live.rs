//! The CLI Tools list and one tool's config page.
//!
//! Both routes previously rendered `cli_tools()` / `cli_tool_detail_state()` —
//! eight fixture tiles captioned "Unchecked", and a detail page of disabled
//! "preview" controls. The list is now whatever
//! `GET /api/cli-tools/all-statuses` returns, a tool the router did not find says
//! so, and the config form performs a real `POST`.
//!
//! Derivations live in [`crate::dashboard::cli_tools_live`] so they stay testable
//! on the native target; this file is markup and wiring.

use crate::api::{ApiError, Hydrate, Save};
use crate::dashboard::cli_tools_live::{
    ApplyOutcome, Detection, McpRegistry, ToolConfigDraft, ToolEntry, ToolList, ToolStatus,
    settings_path, tool_display_name,
};
use leptos::prelude::*;

/// Panel styles, shared verbatim with the actix host.
///
/// The CSR build links no stylesheet of its own, so the file the host serves from
/// `/assets/dashboard/tools-live.css` is inlined here. One source, two delivery
/// paths — the alternative was a second copy that would drift.
const TOOLS_LIVE_STYLES: &str =
    include_str!("../../../../services/dashboard-actix/static/assets/dashboard/tools-live.css");

/// Base URL a tool should be pointed at.
///
/// Prefilled from the page's own origin rather than a hardcoded port, so the
/// value is one this browser can actually reach. It is an editable default, not
/// an assertion about the tool's current config.
#[cfg(target_arch = "wasm32")]
fn default_base_url() -> String {
    web_sys::window()
        .map(|window| window.location())
        .and_then(|location| location.origin().ok())
        .filter(|origin| !origin.is_empty() && origin != "null")
        .map_or_else(String::new, |origin| format!("{origin}/v1"))
}

#[cfg(not(target_arch = "wasm32"))]
fn default_base_url() -> String {
    String::new()
}

#[component]
pub(super) fn CliToolsPanel() -> impl IntoView {
    let (tools, set_tools) = signal(Hydrate::<ToolList>::Loading);
    let (registry, set_registry) = signal(Hydrate::<McpRegistry>::Loading);
    let reload = move || {
        set_tools.set(Hydrate::Loading);
        fetch_tools(set_tools);
    };

    fetch_tools(set_tools);
    fetch_registry(set_registry);

    view! {
        <style>{TOOLS_LIVE_STYLES}</style>
        <div class="nr-panel-stack">
            <article class="nr-card">
                <div class="nr-card-head between">
                    <div>
                        <h2><span class="nr-card-icon">"cli"</span>"CLI Tools"</h2>
                        <p>
                            "Read from the local router over GET /api/cli-tools/all-statuses. A tool is only listed if the router reported on it, and only marked detected if the router found it."
                        </p>
                    </div>
                    <ToolsPill tools />
                </div>
                <p class="nr-tool-meta" role="status" aria-live="polite">
                    {move || match tools.get() {
                        Hydrate::Loading => "Checking which tools are installed…".to_owned(),
                        Hydrate::Failed(error) => error.message().to_owned(),
                        Hydrate::Ready(list) => list.summary(),
                    }}
                </p>
                {move || match tools.get() {
                    Hydrate::Loading => view! { <ToolsSkeleton /> }.into_any(),
                    Hydrate::Failed(error) => {
                        view! {
                            <ToolsFailure
                                error
                                heading="The CLI tool list could not be loaded"
                                on_retry=Callback::new(move |()| reload())
                            />
                        }
                            .into_any()
                    }
                    Hydrate::Ready(list) => {
                        if list.is_empty() {
                            view! {
                                <div class="nr-tool-notice">
                                    <strong>"No CLI tools reported"</strong>
                                    <span>
                                        "The router answered and named no tools, so there is nothing to configure from here."
                                    </span>
                                </div>
                            }
                                .into_any()
                        } else {
                            let tools = list.tools().to_vec();
                            view! { <ToolGrid tools /> }.into_any()
                        }
                    }
                }}
            </article>
            <McpRegistryCard registry />
        </div>
    }
}

/// The list's own status pill.
#[component]
fn ToolsPill(tools: ReadSignal<Hydrate<ToolList>>) -> impl IntoView {
    let tone = move || {
        tools.with(|state| match state {
            Hydrate::Loading => "is-idle",
            Hydrate::Ready(_) => "is-connected",
            Hydrate::Failed(_) => "is-degraded",
        })
    };
    let label = move || {
        tools.with(|state| match state {
            Hydrate::Loading => "Checking".to_owned(),
            Hydrate::Failed(_) => "Unavailable".to_owned(),
            Hydrate::Ready(list) => format!("{} detected", list.detected_count()),
        })
    };

    view! {
        <span
            class=move || format!("nr-status-pill {}", tone())
            aria-label=move || format!("CLI tool detection: {}", label())
        >
            <span></span>
            {label}
        </span>
    }
}

#[component]
fn ToolGrid(tools: Vec<ToolEntry>) -> impl IntoView {
    let label = format!("{} CLI tools reported by the router", tools.len());

    view! {
        <div class="nr-tool-grid nr-stagger" aria-label=label>
            {tools.into_iter().map(|tool| view! { <ToolCard tool /> }).collect_view()}
        </div>
    }
}

#[component]
fn ToolCard(tool: ToolEntry) -> impl IntoView {
    let detection = tool.detection();
    let routing = tool.routing();
    let absent = detection != Detection::Installed;

    view! {
        <div class="nr-tool-card" class:is-absent=move || absent>
            <div class="nr-tool-card-top">
                <h3>{tool.label.clone()}</h3>
                <span class=format!("nr-status-pill {}", detection.class_name())>
                    <span></span>
                    {detection.label()}
                </span>
            </div>
            <p>{tool.summary().to_owned()}</p>
            <div class="nr-tool-card-pills">
                <span class=format!("nr-status-pill {}", routing.class_name())>
                    <span></span>
                    {routing.label()}
                </span>
            </div>
            <div class="nr-tool-card-actions">
                <a
                    class="nr-button secondary small"
                    href=tool.detail_href()
                    aria-label=tool.open_label()
                >
                    "Open"
                </a>
            </div>
        </div>
    }
}

#[component]
fn ToolsSkeleton() -> impl IntoView {
    view! {
        <div class="nr-tool-grid" aria-busy="true" aria-label="Loading CLI tool statuses">
            {(0..6)
                .map(|_| {
                    view! {
                        <div class="nr-tool-skeleton-card">
                            <span class="nr-skeleton nr-skeleton-text-short">"loading"</span>
                            <span class="nr-skeleton nr-skeleton-text">"loading"</span>
                            <span class="nr-skeleton nr-skeleton-text">"loading"</span>
                        </div>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// A failed fetch, with the reason and a retry.
#[component]
fn ToolsFailure(error: ApiError, heading: &'static str, on_retry: Callback<()>) -> impl IntoView {
    view! {
        <div class="nr-tool-notice is-error" role="alert">
            <strong>{heading}</strong>
            <span>{error.message()}</span>
            <button
                type="button"
                class="nr-button secondary small"
                on:click=move |_| on_retry.run(())
            >
                "Retry"
            </button>
        </div>
    }
}

/// The Cowork MCP registry, reported as the endpoint describes it.
#[component]
fn McpRegistryCard(registry: ReadSignal<Hydrate<McpRegistry>>) -> impl IntoView {
    view! {
        <article class="nr-card">
            <div class="nr-card-head">
                <div>
                    <h2><span class="nr-card-icon">"mcp"</span>"Cowork MCP Servers"</h2>
                    <p>"Read from GET /api/cli-tools/cowork-mcp-registry."</p>
                </div>
            </div>
            {move || match registry.get() {
                Hydrate::Loading => {
                    view! {
                        <div class="nr-tool-skeletons" aria-busy="true" aria-label="Loading MCP registry">
                            <span class="nr-skeleton nr-skeleton-row">"loading"</span>
                        </div>
                    }
                        .into_any()
                }
                Hydrate::Failed(error) => {
                    view! {
                        <div class="nr-tool-notice is-error" role="alert">
                            <strong>"The MCP registry could not be read"</strong>
                            <span>{error.message()}</span>
                        </div>
                    }
                        .into_any()
                }
                Hydrate::Ready(found) => {
                    let summary = found.summary();
                    if found.servers.is_empty() {
                        view! {
                            <div class="nr-tool-notice">
                                <strong>"No MCP servers listed"</strong>
                                <span>{summary}</span>
                            </div>
                        }
                            .into_any()
                    } else {
                        view! {
                            <p class="nr-tool-meta">{summary}</p>
                            <ul class="nr-tool-skeletons nr-stagger" aria-label="MCP servers">
                                {found
                                    .servers
                                    .into_iter()
                                    .map(|name| view! { <li class="nr-tool-path">{name}</li> })
                                    .collect_view()}
                            </ul>
                        }
                            .into_any()
                    }
                }
            }}
        </article>
    }
}

#[component]
pub(super) fn CliToolDetailPanel(tool_id: String) -> impl IntoView {
    let tool_id = StoredValue::new(tool_id);
    let label = tool_display_name(&tool_id.get_value());
    let (status, set_status) = signal(Hydrate::<ToolStatus>::Loading);
    let draft = RwSignal::new(ToolConfigDraft {
        base_url: default_base_url(),
        api_key: String::new(),
        model: String::new(),
    });
    let save = RwSignal::new(Save::Idle);
    let outcome: RwSignal<Option<ApplyOutcome>> = RwSignal::new(None);
    let reload = move || {
        set_status.set(Hydrate::Loading);
        fetch_tool_status(tool_id.get_value(), set_status);
    };

    fetch_tool_status(tool_id.get_value(), set_status);

    view! {
        <style>{TOOLS_LIVE_STYLES}</style>
        <div class="nr-panel-stack">
            <article class="nr-card nr-card-hero">
                <div>
                    <p class="nr-eyebrow">"CLI Tool"</p>
                    <h2>{label.clone()}</h2>
                    <p>
                        {move || {
                            format!(
                                "Read from the local router over GET {}.",
                                settings_path(&tool_id.get_value()),
                            )
                        }}
                    </p>
                </div>
                <div class="nr-tool-detail-head">
                    <a class="nr-button secondary small" href="/dashboard/cli-tools">
                        "Back to CLI Tools"
                    </a>
                </div>
            </article>
            {move || match status.get() {
                Hydrate::Loading => {
                    view! {
                        <div class="nr-tool-skeletons" aria-busy="true" aria-label="Loading tool status">
                            <span class="nr-skeleton nr-skeleton-row">"loading"</span>
                            <span class="nr-skeleton nr-skeleton-row">"loading"</span>
                        </div>
                    }
                        .into_any()
                }
                Hydrate::Failed(error) => {
                    view! {
                        <ToolsFailure
                            error
                            heading="This tool's status could not be read"
                            on_retry=Callback::new(move |()| reload())
                        />
                    }
                        .into_any()
                }
                Hydrate::Ready(found) => {
                    view! {
                        <div class="nr-tool-detail-grid">
                            <ToolStatusCard status=found.clone() />
                            <ToolConfigForm tool_id draft save outcome set_status />
                        </div>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

/// What the router reported about this tool.
#[component]
fn ToolStatusCard(status: ToolStatus) -> impl IntoView {
    let detection = status.detection();
    let routing = status.routing();
    let config = status.config_text().map(ToOwned::to_owned);
    let path = status.config_path.clone();
    let summary = status.summary().to_owned();

    view! {
        <article class="nr-card">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"chk"</span>"Detected State"</h2>
                    <p>{detection.detail()}</p>
                </div>
                <span class=format!("nr-status-pill {}", detection.class_name())>
                    <span></span>
                    {detection.label()}
                </span>
            </div>
            <p class="nr-tool-meta">
                <span class=format!("nr-status-pill {}", routing.class_name())>
                    <span></span>
                    {routing.label()}
                </span>
                <span>{summary}</span>
            </p>
            {path.map(|found| {
                view! {
                    <p class="nr-tool-meta">
                        <span>"Config file"</span>
                        <code class="nr-tool-path">{found}</code>
                    </p>
                }
            })}
            {config.map_or_else(
                || {
                    view! {
                        <div class="nr-tool-notice">
                            <strong>"No configuration read"</strong>
                            <span>
                                "The router did not return this tool's configuration, so none is shown."
                            </span>
                        </div>
                    }
                        .into_any()
                },
                |text| {
                    view! {
                        <pre class="nr-tool-config" aria-label="Configuration as the router read it">
                            <code>{text}</code>
                        </pre>
                    }
                        .into_any()
                },
            )}
        </article>
    }
}

/// The config form: a real `POST`, with the result reported on the form.
#[component]
fn ToolConfigForm(
    tool_id: StoredValue<String>,
    draft: RwSignal<ToolConfigDraft>,
    save: RwSignal<Save>,
    outcome: RwSignal<Option<ApplyOutcome>>,
    set_status: WriteSignal<Hydrate<ToolStatus>>,
) -> impl IntoView {
    let saving = move || save.with(Save::is_saving);
    let blocked = move || draft.with(ToolConfigDraft::validation_error);
    let submit = move || {
        if saving() {
            return;
        }
        let Ok(body) = draft.with(ToolConfigDraft::apply_body) else {
            return;
        };
        save.set(Save::Saving);
        outcome.set(None);
        apply(tool_id.get_value(), body, save, outcome, set_status);
    };

    view! {
        <article class="nr-card">
            <div class="nr-card-head">
                <div>
                    <h2><span class="nr-card-icon">"cfg"</span>"Point This Tool Here"</h2>
                    <p>
                        {move || {
                            format!(
                                "Sends baseUrl, apiKey, and model to POST {}. The result below is what the router reported.",
                                settings_path(&tool_id.get_value()),
                            )
                        }}
                    </p>
                </div>
            </div>
            <div class="nr-tool-form">
                <ToolField
                    id="nr-tool-base-url"
                    label="Base URL"
                    detail="The endpoint the tool should call."
                    value=Signal::derive(move || draft.with(|found| found.base_url.clone()))
                    disabled=Signal::derive(saving)
                    on_input=Callback::new(move |text: String| {
                        draft.update(|found| found.base_url = text);
                    })
                />
                <ToolField
                    id="nr-tool-api-key"
                    label="API key"
                    detail="Sent to the router, which writes it into the tool's own config file."
                    secret=true
                    value=Signal::derive(move || draft.with(|found| found.api_key.clone()))
                    disabled=Signal::derive(saving)
                    on_input=Callback::new(move |text: String| {
                        draft.update(|found| found.api_key = text);
                    })
                />
                <ToolField
                    id="nr-tool-model"
                    label="Model"
                    detail="The model id the tool should request by default."
                    value=Signal::derive(move || draft.with(|found| found.model.clone()))
                    disabled=Signal::derive(saving)
                    on_input=Callback::new(move |text: String| {
                        draft.update(|found| found.model = text);
                    })
                />
                <div class="nr-tool-form-actions">
                    <button
                        type="button"
                        class="nr-button primary small"
                        disabled=move || saving() || blocked().is_some()
                        aria-describedby="nr-tool-apply-status"
                        on:click=move |_| submit()
                    >
                        "Apply configuration"
                    </button>
                    <Show when=saving>
                        <span class="nr-spinner" aria-hidden="true"></span>
                    </Show>
                    {move || {
                        blocked()
                            .map(|error| view! { <span class="nr-tool-status">{error.message()}</span> })
                    }}
                </div>
                <ApplyStatus save outcome />
            </div>
        </article>
    }
}

/// One labelled input.
#[component]
fn ToolField(
    id: &'static str,
    label: &'static str,
    detail: &'static str,
    #[prop(optional)] secret: bool,
    value: Signal<String>,
    disabled: Signal<bool>,
    on_input: Callback<String>,
) -> impl IntoView {
    let describe = format!("{id}-desc");
    let input_type = if secret { "password" } else { "text" };

    view! {
        <div class="nr-tool-field">
            <label for=id>{label}</label>
            <small id=describe.clone()>{detail}</small>
            <input
                id=id
                class="nr-tool-input"
                type=input_type
                spellcheck="false"
                autocomplete="off"
                aria-describedby=describe
                prop:value=move || value.get()
                disabled=move || disabled.get()
                on:input=move |event| on_input.run(event_target_value(&event))
            />
        </div>
    }
}

/// The write's result, announced politely.
#[component]
fn ApplyStatus(save: RwSignal<Save>, outcome: RwSignal<Option<ApplyOutcome>>) -> impl IntoView {
    let failed =
        move || outcome.with(|found| found.as_ref().is_some_and(|result| !result.wrote_config()));
    let text = move || {
        outcome.with(|found| match found {
            Some(result) => result.message(),
            None => save.with(|state| state.status().unwrap_or_default().to_owned()),
        })
    };

    view! {
        <p
            id="nr-tool-apply-status"
            class=move || {
                if failed() { "nr-tool-status is-failed" } else { "nr-tool-status" }
            }
            class:nr-tick=move || {
                outcome.with(|found| found.as_ref().is_some_and(ApplyOutcome::wrote_config))
            }
            role="status"
            aria-live="polite"
            aria-atomic="true"
        >
            {text}
        </p>
    }
}

// ── requests ────────────────────────────────────────────────────────────────
//
// Each spawns on the wasm target and reports `ApiError::Environment` natively,
// so a native render shows a failure rather than a fabricated success.

#[cfg(target_arch = "wasm32")]
fn fetch_tools(set_tools: WriteSignal<Hydrate<ToolList>>) {
    wasm_bindgen_futures::spawn_local(async move {
        set_tools.set(match crate::dashboard::cli_tools_live::load_tools().await {
            Ok(list) => Hydrate::Ready(list),
            Err(error) => Hydrate::Failed(error),
        });
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_tools(set_tools: WriteSignal<Hydrate<ToolList>>) {
    set_tools.set(Hydrate::Failed(ApiError::Environment));
}

#[cfg(target_arch = "wasm32")]
fn fetch_registry(set_registry: WriteSignal<Hydrate<McpRegistry>>) {
    wasm_bindgen_futures::spawn_local(async move {
        set_registry.set(
            match crate::dashboard::cli_tools_live::load_mcp_registry().await {
                Ok(found) => Hydrate::Ready(found),
                Err(error) => Hydrate::Failed(error),
            },
        );
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_registry(set_registry: WriteSignal<Hydrate<McpRegistry>>) {
    set_registry.set(Hydrate::Failed(ApiError::Environment));
}

#[cfg(target_arch = "wasm32")]
fn fetch_tool_status(tool_id: String, set_status: WriteSignal<Hydrate<ToolStatus>>) {
    wasm_bindgen_futures::spawn_local(async move {
        set_status.set(
            match crate::dashboard::cli_tools_live::load_tool_status(&tool_id).await {
                Ok(found) => Hydrate::Ready(found),
                Err(error) => Hydrate::Failed(error),
            },
        );
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_tool_status(_tool_id: String, set_status: WriteSignal<Hydrate<ToolStatus>>) {
    set_status.set(Hydrate::Failed(ApiError::Environment));
}

/// Send the config, then re-read the tool so the panel shows the stored state.
#[cfg(target_arch = "wasm32")]
fn apply(
    tool_id: String,
    body: String,
    save: RwSignal<Save>,
    outcome: RwSignal<Option<ApplyOutcome>>,
    set_status: WriteSignal<Hydrate<ToolStatus>>,
) {
    wasm_bindgen_futures::spawn_local(async move {
        let result = crate::dashboard::cli_tools_live::apply_tool_config(&tool_id, body).await;
        let wrote = result.wrote_config();
        save.set(if wrote { Save::Saved } else { Save::Idle });
        outcome.set(Some(result));
        // Only re-read when something was written: a refused write left the
        // stored config untouched, and re-fetching would only flicker.
        if wrote {
            set_status.set(Hydrate::Loading);
            fetch_tool_status(tool_id, set_status);
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn apply(
    _tool_id: String,
    _body: String,
    save: RwSignal<Save>,
    outcome: RwSignal<Option<ApplyOutcome>>,
    _set_status: WriteSignal<Hydrate<ToolStatus>>,
) {
    save.set(Save::Idle);
    outcome.set(Some(ApplyOutcome::Rejected(ApiError::Environment)));
}
