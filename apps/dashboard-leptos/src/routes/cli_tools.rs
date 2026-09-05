//! Which CLI coding tools are installed, which of them point at this router, and applying the
//! config that makes them.
//!
//! `GET /api/cli-tools/all-statuses` answers with an object keyed by tool id rather than a list, so
//! this decodes a map and orders it here. A struct per tool would compile and then silently omit any
//! tool the server later adds.
//!
//! # The parsed config is fetched and never rendered
//!
//! Each status carries a `settings` object: the tool's own config file, parsed. For Claude Code on
//! a configured machine that object contains `ANTHROPIC_AUTH_TOKEN` and whatever else the user keeps
//! in their environment block, in full. This panel reports only *whether* it was readable, because
//! printing it would put live credentials on a screen that gets shared and screenshotted, and
//! nothing on this page needs their values to be useful.
//!
//! # One form, thirteen validators
//!
//! The thirteen writable tools do not agree on required fields: codex, cline and kilo want
//! `baseUrl` + `apiKey` + `model`; copilot, opencode and droid want `baseUrl` + `models[]`; grok,
//! openclaw, hermes and deepseek-tui want `baseUrl` + `model`; jcode wants `baseUrl` + `apiKey`;
//! Claude Code validates on the presence of an `env` object and takes no named fields at all. The
//! server's request type is explicitly a union of every field any tool reads, so one form fills all
//! of them and each writer takes the subset it needs.

use std::collections::BTreeMap;

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api::{ApiError, Hydrate, Method, encode, load_with};
use crate::routes::controls::{Action, Caution, Outcome, OutcomeLine, Section, Tone};
use crate::routes::{PageHeader, Panel};

/// One tool's status.
///
/// The four path fields are aliases: the server sets `settingsPath`, `configPath`, `authPath` and
/// `globalStatePath` to the same value because upstream names it differently per tool, and sends all
/// four so whichever a client reads is present. All four are decoded and the first populated one is
/// shown, rather than picking one and leaving the cell blank if that spelling ever stops being sent.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolStatus {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    installed: bool,
    /// Whether the tool's config already points at this router.
    ///
    /// `hasRouter` on the wire. The name was changed from the upstream spelling in this port;
    /// confirmed against `tool_status_body` in `services/api-actix/src/cli_tools.rs`.
    #[serde(default)]
    has_router: bool,
    /// `installed && this port can write the tool's config`. False for a tool that has a writer but
    /// is not present, so honouring it stops the panel offering to configure software the machine
    /// does not have.
    #[serde(default)]
    writable: bool,
    #[serde(default)]
    settings_path: String,
    #[serde(default)]
    config_path: String,
    #[serde(default)]
    auth_path: String,
    #[serde(default)]
    global_state_path: String,
    /// The binary that answered on `PATH`.
    #[serde(default)]
    source: String,
    /// Why the config could not be parsed. Sent alongside `settings: null` rather than as an error,
    /// so a stray comma reads as "unreadable" instead of "not configured".
    #[serde(default)]
    config_error: String,
    /// Present only when the tool is absent.
    #[serde(default)]
    message: String,
    /// The tool's parsed config. Held opaque and never rendered — see the module note.
    #[serde(default)]
    settings: Option<serde_json::Value>,
}

impl ToolStatus {
    /// The config file, whichever key carried it.
    fn path(&self) -> &str {
        [
            &self.config_path,
            &self.settings_path,
            &self.auth_path,
            &self.global_state_path,
        ]
        .into_iter()
        .find(|candidate| !candidate.is_empty())
        .map_or("", String::as_str)
    }

    /// Whether the config file parsed. `None` covers both "no file" and "unreadable"; `config_error`
    /// separates them.
    const fn config_readable(&self) -> bool {
        self.settings.is_some()
    }
}

/// One row, with the id the routes take.
#[derive(Clone, Debug)]
struct Tool {
    id: String,
    status: ToolStatus,
}

/// The decoded map, ordered for display.
///
/// Named rather than written as `Vec<Tool>` at the use site: `view!` parses a closure parameter's
/// type as markup, so the angle brackets of a generic are read as a tag and the whole macro fails.
type ToolRows = Vec<Tool>;

/// Decode the map and order it for display.
///
/// Configured first, then installed, then the rest — the tools someone came here to look at are the
/// ones already in use. Ties break on display name so the order is stable between polls.
fn parse_tools(body: &str) -> Result<ToolRows, ApiError> {
    let map: BTreeMap<String, ToolStatus> = crate::api::decode(body)?;
    let mut tools: Vec<Tool> = map
        .into_iter()
        .map(|(id, status)| Tool { id, status })
        .collect();
    tools.sort_by(|left, right| {
        let rank = |tool: &Tool| match (tool.status.has_router, tool.status.installed) {
            (true, _) => 0_u8,
            (false, true) => 1,
            (false, false) => 2,
        };
        rank(left).cmp(&rank(right)).then_with(|| {
            sort_name(&left.status, &left.id).cmp(&sort_name(&right.status, &right.id))
        })
    });
    Ok(tools)
}

/// Lower-cased display name, falling back to the id when the server sent no name.
fn sort_name(status: &ToolStatus, id: &str) -> String {
    if status.display_name.is_empty() {
        id.to_lowercase()
    } else {
        status.display_name.to_lowercase()
    }
}

/// The environment keys Claude Code's writer reads, mirroring the ones its revoke removes.
const CLAUDE_BASE_URL: &str = "ANTHROPIC_BASE_URL";
const CLAUDE_AUTH_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";
const CLAUDE_MODEL_KEYS: [&str; 3] = [
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
];

/// Every field any of the thirteen writers reads.
///
/// Sent whole to whichever tool is selected. The server's own request type is a union for the same
/// reason, and each writer ignores the fields it has no use for, so this is the shape the API
/// accepts rather than an over-send.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplyBody {
    base_url: String,
    api_key: String,
    model: String,
    /// The `models[]` half, for the three tools that validate a list rather than a single name.
    models: Vec<String>,
    /// Claude Code's whole environment block. Its validator rejects a payload without one.
    env: BTreeMap<String, String>,
}

impl ApplyBody {
    fn new(base_url: String, api_key: String, model: String) -> Self {
        let mut env = BTreeMap::new();
        env.insert(CLAUDE_BASE_URL.to_owned(), base_url.clone());
        env.insert(CLAUDE_AUTH_TOKEN.to_owned(), api_key.clone());
        if !model.is_empty() {
            for key in CLAUDE_MODEL_KEYS {
                env.insert(key.to_owned(), model.clone());
            }
        }
        Self {
            models: if model.is_empty() {
                Vec::new()
            } else {
                vec![model.clone()]
            },
            base_url,
            api_key,
            model,
            env,
        }
    }
}

/// This router's own origin, which is what a tool has to be pointed at.
///
/// Read from the address bar rather than from the server: the API has no endpoint that reports the
/// URL a client reached it on, and the host a browser used is the host that browser's user can
/// reach. Editable afterwards, because a tool on another machine needs the LAN address instead.
fn router_origin() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.location().origin().ok())
            .unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        String::new()
    }
}

#[component]
pub fn CliTools() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (tools, set_tools) = signal(Hydrate::<ToolRows>::Loading);

    let reload = move || {
        set_tools.set(Hydrate::Loading);
        load_with("/api/cli-tools/all-statuses", set_tools, parse_tools);
    };
    reload();

    view! {
        <PageHeader
            title=locale.get("nav.cli_tools").to_owned()
            description=locale.get("cli_tools.description").to_owned()
        />
        <div class="space-y-6">
            <ApplyForm tools=tools reload=reload />
            <Panel
                state=tools
                on_retry=Callback::new(move |()| reload())
                children=move |rows: ToolRows| view! { <ToolTable rows=rows /> }
            />
        </div>
    }
}

/// Apply this router's configuration to one tool, or take it back out.
///
/// The tool list comes from the same fetch the table renders, so the picker cannot offer a tool the
/// server did not report, and only the ones it marked `writable` are selectable.
#[component]
fn ApplyForm(
    tools: ReadSignal<Hydrate<ToolRows>>,
    reload: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (selected, set_selected) = signal(String::new());
    let (base_url, set_base_url) = signal(router_origin());
    let (api_key, set_api_key) = signal(String::new());
    let (model, set_model) = signal(String::new());
    let (outcome, set_outcome) = signal(None::<Outcome>);

    // Only the tools the server said it can write. A picker listing the rest would offer a button
    // that answers 501, or that writes a config for software this machine does not have.
    let writable = Memo::new(move |_| {
        tools.get().ready().map_or_else(Vec::new, |rows| {
            rows.iter()
                .filter(|tool| tool.status.writable)
                .map(|tool| {
                    (
                        tool.id.clone(),
                        if tool.status.display_name.is_empty() {
                            tool.id.clone()
                        } else {
                            tool.status.display_name.clone()
                        },
                    )
                })
                .collect()
        })
    });

    // Nothing is preselected: the first writable tool becoming the default would make a mis-click
    // write a config file for whichever tool happened to sort first.
    let chosen = Memo::new(move |_| {
        let id = selected.get();
        if id.is_empty() {
            return None;
        }
        writable
            .get()
            .into_iter()
            .find(|(candidate, _label)| *candidate == id)
            .map(|(id, _label)| id)
    });

    let held = Signal::derive(move || chosen.get().is_none());
    let apply_path = Memo::new(move |_| {
        chosen
            .get()
            .map_or_else(String::new, |id| format!("/api/cli-tools/{id}"))
    });

    view! {
        <Section title=locale.get("cli_tools.apply").to_owned()>
            <p class="text-sm text-muted-foreground">
                {locale.get("cli_tools.apply_hint").to_owned()}
            </p>
            {move || {
                // Re-acquired rather than captured: `Locale` owns its message table and is not
                // `Copy`, so moving the outer one into this closure would take it away from the
                // rest of the form.
                let locale = crate::i18n::use_locale();
                let options = writable.get();
                if options.is_empty() && tools.get().ready().is_some() {
                    return view! {
                        <p class="text-sm text-muted-foreground">
                            {locale.get("cli_tools.none_writable").to_owned()}
                        </p>
                    }
                        .into_any();
                }
                view! {
                    <label class="block space-y-1 text-sm">
                        <span class="text-muted-foreground">
                            {locale.get("cli_tools.tool").to_owned()}
                        </span>
                        <select
                            class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                            prop:value=move || selected.get()
                            on:change=move |ev| set_selected.set(event_target_value(&ev))
                        >
                            <option value="">{locale.get("cli_tools.pick").to_owned()}</option>
                            {options
                                .into_iter()
                                .map(|(id, label)| {
                                    view! { <option value=id>{label}</option> }
                                })
                                .collect_view()}
                        </select>
                    </label>
                }
                    .into_any()
            }}
            <div class="grid gap-3 sm:grid-cols-3">
                <label class="block space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("cli_tools.base_url").to_owned()}
                    </span>
                    <input
                        type="text"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || base_url.get()
                        on:input=move |ev| set_base_url.set(event_target_value(&ev))
                    />
                </label>
                <label class="block space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("cli_tools.api_key").to_owned()}
                    </span>
                    // Masked: this is a live credential being typed on a screen someone may be
                    // sharing.
                    <input
                        type="password"
                        autocomplete="off"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || api_key.get()
                        on:input=move |ev| set_api_key.set(event_target_value(&ev))
                    />
                </label>
                <label class="block space-y-1 text-sm">
                    <span class="text-muted-foreground">
                        {locale.get("cli_tools.model").to_owned()}
                    </span>
                    <input
                        type="text"
                        class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                        prop:value=move || model.get()
                        on:input=move |ev| set_model.set(event_target_value(&ev))
                    />
                </label>
            </div>
            <p class="text-xs text-muted-foreground">
                {locale.get("cli_tools.required_hint").to_owned()}
            </p>
            <div class="flex flex-wrap gap-2">
                {move || {
                    let locale = crate::i18n::use_locale();
                    let path = apply_path.get();
                    let revoke_path = path.clone();
                    view! {
                        <Action
                            label=locale.get("cli_tools.apply_action").to_owned()
                            path=path
                            method=Method::Post
                            tone=Tone::Primary
                            disabled=held
                            done_label=locale.get("cli_tools.applied").to_owned()
                            body=Callback::new(move |()| {
                                encode(
                                        &ApplyBody::new(
                                            base_url.get().trim().to_owned(),
                                            api_key.get().trim().to_owned(),
                                            model.get().trim().to_owned(),
                                        ),
                                    )
                                    .ok()
                            })
                            on_done=Callback::new(move |outcome: Outcome| {
                                let applied = outcome.ok;
                                set_outcome.set(Some(outcome));
                                if applied {
                                    reload();
                                }
                            })
                        />
                        // DELETE carries no body upstream, and the server accepts an empty one.
                        <Action
                            label=locale.get("cli_tools.revoke").to_owned()
                            path=revoke_path
                            method=Method::Delete
                            tone=Tone::Destructive
                            disabled=held
                            done_label=locale.get("cli_tools.revoked").to_owned()
                            on_done=Callback::new(move |outcome: Outcome| {
                                let removed = outcome.ok;
                                set_outcome.set(Some(outcome));
                                if removed {
                                    reload();
                                }
                            })
                        />
                    }
                }}
            </div>
            <Caution text=locale.get("cli_tools.revoke_caution").to_owned() />
            <OutcomeLine outcome=outcome />
        </Section>
    }
}

#[component]
fn ToolTable(rows: ToolRows) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">
                {locale.get("cli_tools.empty").to_owned()}
            </p>
        }
        .into_any();
    }
    view! {
        <div class="rounded-lg border border-border overflow-x-auto">
            <table class="w-full text-sm">
                <thead class="bg-muted/50 text-muted-foreground">
                    <tr>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("cli_tools.col_tool").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("cli_tools.col_installed").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("cli_tools.col_points_here").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("cli_tools.col_config").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("cli_tools.col_notes").to_owned()}
                        </th>
                    </tr>
                </thead>
                <tbody>
                    {rows.into_iter().map(|tool| view! { <ToolRow tool=tool /> }).collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

#[component]
fn ToolRow(tool: Tool) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let status = tool.status;
    let name = if status.display_name.is_empty() {
        tool.id.clone()
    } else {
        status.display_name.clone()
    };

    // What this build knows that is worth saying, in the order a reader needs it: why the tool is
    // absent, why its config would not parse, and whether the config was readable at all.
    let mut notes: Vec<String> = Vec::new();
    if !status.config_error.is_empty() {
        notes.push(format!(
            "{} {}",
            locale.get("cli_tools.config_error"),
            status.config_error
        ));
    } else if !status.message.is_empty() {
        notes.push(status.message.clone());
    }
    if status.installed && status.config_error.is_empty() {
        notes.push(
            if status.config_readable() {
                locale.get("cli_tools.config_read")
            } else {
                locale.get("cli_tools.config_absent")
            }
            .to_owned(),
        );
    }
    if status.installed && !status.writable {
        notes.push(locale.get("cli_tools.read_only").to_owned());
    }

    view! {
        <tr class="border-t border-border align-top">
            <td class="px-3 py-2">
                <div class="font-medium">{name}</div>
                <code class="text-xs text-muted-foreground">{tool.id}</code>
                {(!status.source.is_empty())
                    .then(|| {
                        view! {
                            <div class="text-xs text-muted-foreground break-all">
                                {status.source.clone()}
                            </div>
                        }
                    })}
            </td>
            <td class="px-3 py-2">
                <StateDot on=status.installed />
            </td>
            <td class="px-3 py-2">
                <StateDot on=status.has_router />
            </td>
            <td class="px-3 py-2 font-mono text-xs text-muted-foreground break-all">
                {let path = status.path().to_owned();
                if path.is_empty() { "—".to_owned() } else { path }}
            </td>
            <td class="px-3 py-2 text-muted-foreground space-y-1">
                {if notes.is_empty() {
                    view! { <span>"—"</span> }.into_any()
                } else {
                    notes
                        .into_iter()
                        .map(|note| view! { <div class="break-words">{note}</div> })
                        .collect_view()
                        .into_any()
                }}
            </td>
        </tr>
    }
}

/// A yes/no the server reported.
#[component]
fn StateDot(on: bool) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let label = if on {
        locale.get("state.yes").to_owned()
    } else {
        locale.get("state.no").to_owned()
    };
    view! {
        <span class="flex items-center gap-2 whitespace-nowrap">
            <span class=if on {
                "size-1.5 rounded-full bg-success"
            } else {
                "size-1.5 rounded-full bg-muted-foreground/40"
            } />
            <span>{label}</span>
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::{ApplyBody, CLAUDE_AUTH_TOKEN, CLAUDE_BASE_URL, ToolStatus, parse_tools};

    /// Two entries from a live `GET /api/cli-tools/all-statuses`, trimmed to the fields this panel
    /// reads plus the credential-bearing `settings` block it must not render.
    const LIVE_STATUSES: &str = r#"{
        "claude": {
            "authPath": "/root/.claude/settings.json",
            "configPath": "/root/.claude/settings.json",
            "displayName": "Claude Code",
            "globalStatePath": "/root/.claude/settings.json",
            "hasRouter": true,
            "installed": true,
            "settings": {
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "sk-live-secret",
                    "ANTHROPIC_BASE_URL": "https://example.invalid"
                },
                "theme": "dark"
            },
            "settingsPath": "/root/.claude/settings.json",
            "source": "/root/.local/bin/claude",
            "writable": true
        },
        "codex": {
            "authPath": "/root/.codex/config.toml",
            "configPath": "/root/.codex/config.toml",
            "displayName": "Codex",
            "globalStatePath": "/root/.codex/config.toml",
            "hasRouter": false,
            "installed": false,
            "message": "Codex is not installed: no binary on PATH and no config file.",
            "settings": null,
            "settingsPath": "/root/.codex/config.toml",
            "writable": false
        },
        "cowork": {
            "configError": "Cannot locate the home directory: $HOME is unset or empty.",
            "displayName": "Cowork",
            "hasRouter": false,
            "installed": false,
            "message": "Cowork is not installed: no binary on PATH and no config file.",
            "settings": null,
            "writable": false
        }
    }"#;

    #[test]
    fn the_live_map_decodes_every_tool_it_holds() {
        // A struct per tool would drop whichever key it did not name. The map cannot.
        let tools = parse_tools(LIVE_STATUSES).unwrap_or_default();
        assert_eq!(tools.len(), 3);
        let ids: Vec<&str> = tools.iter().map(|tool| tool.id.as_str()).collect();
        assert!(ids.contains(&"claude"));
        assert!(ids.contains(&"codex"));
        assert!(ids.contains(&"cowork"));
    }

    #[test]
    fn a_configured_tool_sorts_above_the_rest() {
        let tools = parse_tools(LIVE_STATUSES).unwrap_or_default();
        assert_eq!(
            tools.first().map(|tool| tool.id.as_str()),
            Some("claude"),
            "the tool already pointing here belongs first"
        );
    }

    #[test]
    fn has_router_is_read_from_the_field_the_server_actually_sends() {
        // The field was renamed from upstream's spelling. Reading the wrong one would report every
        // configured tool as unconfigured, which is exactly the kind of plausible falsehood the
        // dashboard's API layer exists to prevent.
        let tools = parse_tools(LIVE_STATUSES).unwrap_or_default();
        let claude = tools.iter().find(|tool| tool.id == "claude");
        assert_eq!(claude.map(|tool| tool.status.has_router), Some(true));
        assert!(!LIVE_STATUSES.contains("hasNineRouter"));
    }

    #[test]
    fn the_four_path_aliases_collapse_to_one_value() {
        let tools = parse_tools(LIVE_STATUSES).unwrap_or_default();
        let codex = tools.iter().find(|tool| tool.id == "codex");
        assert_eq!(
            codex.map(|tool| tool.status.path()),
            Some("/root/.codex/config.toml")
        );
    }

    #[test]
    fn a_tool_with_no_config_path_yields_an_empty_one_rather_than_a_guess() {
        let tools = parse_tools(LIVE_STATUSES).unwrap_or_default();
        let cowork = tools.iter().find(|tool| tool.id == "cowork");
        assert_eq!(cowork.map(|tool| tool.status.path()), Some(""));
        assert!(
            cowork.is_some_and(|tool| !tool.status.config_error.is_empty()),
            "the reason the path is missing has to survive the decode"
        );
    }

    #[test]
    fn readability_is_reported_without_the_contents() {
        let tools = parse_tools(LIVE_STATUSES).unwrap_or_default();
        let claude = tools.iter().find(|tool| tool.id == "claude");
        assert_eq!(claude.map(|tool| tool.status.config_readable()), Some(true));
        let codex = tools.iter().find(|tool| tool.id == "codex");
        assert_eq!(codex.map(|tool| tool.status.config_readable()), Some(false));
    }

    #[test]
    fn a_broken_body_is_a_failure_rather_than_an_empty_list() {
        assert!(parse_tools("[]").is_err());
        assert!(parse_tools("truncated").is_err());
    }

    #[test]
    fn an_unknown_tool_still_decodes_with_defaults() {
        // A tool added upstream must appear as a row, not take the panel down.
        let tools = parse_tools(r#"{"brand-new":{"installed":true}}"#).unwrap_or_default();
        assert_eq!(tools.len(), 1);
        assert!(tools.first().is_some_and(|tool| tool.status.installed));
        assert!(
            tools
                .first()
                .is_some_and(|tool| tool.status.display_name.is_empty())
        );
    }

    #[test]
    fn the_payload_satisfies_every_validator_at_once() {
        let body = ApplyBody::new(
            "http://127.0.0.1:20128".to_owned(),
            "sk-test".to_owned(),
            "gpt-5".to_owned(),
        );
        let json = serde_json::to_value(&body).unwrap_or_default();
        // codex, cline, kilo, hermes, deepseek-tui, grok-build, openclaw.
        assert_eq!(
            json.get("baseUrl").and_then(|v| v.as_str()),
            Some("http://127.0.0.1:20128")
        );
        assert_eq!(json.get("apiKey").and_then(|v| v.as_str()), Some("sk-test"));
        assert_eq!(json.get("model").and_then(|v| v.as_str()), Some("gpt-5"));
        // copilot, opencode, droid.
        assert_eq!(
            json.get("models").and_then(|v| v.as_array()).map(Vec::len),
            Some(1)
        );
        // claude, whose validator rejects a payload with no `env` object.
        let env = json.get("env").and_then(|v| v.as_object());
        assert!(env.is_some_and(|env| env.contains_key(CLAUDE_BASE_URL)));
        assert!(env.is_some_and(|env| env.contains_key(CLAUDE_AUTH_TOKEN)));
        assert!(env.is_some_and(|env| env.contains_key("ANTHROPIC_DEFAULT_OPUS_MODEL")));
    }

    #[test]
    fn an_empty_model_sends_no_model_names_rather_than_an_empty_one() {
        // jcode validates `baseUrl` + `apiKey` only, so a blank model is a legitimate submission.
        // An empty string in `models[]` would read as a model named "" to the tools that take a
        // list, and their own validators check for a non-empty name.
        let body = ApplyBody::new("http://host".to_owned(), "sk".to_owned(), String::new());
        assert!(body.models.is_empty());
        assert!(!body.env.contains_key("ANTHROPIC_DEFAULT_OPUS_MODEL"));
        assert!(body.env.contains_key(CLAUDE_BASE_URL));
    }

    #[test]
    fn a_missing_settings_key_is_not_read_as_a_readable_config() {
        let status: ToolStatus = serde_json::from_str("{}").unwrap_or_default();
        assert!(!status.config_readable());
        assert!(!status.installed);
        assert!(!status.has_router);
        assert!(!status.writable);
    }
}
