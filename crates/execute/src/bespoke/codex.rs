//! Request shaping for `codex`, the ChatGPT Codex backend.
//!
//! Unlike the other bespoke protocols this one *is* the Responses API, so the registry already routes
//! it correctly and the generic executor can already talk to it. What it cannot do is satisfy the
//! backend's requirements about the body, and those are strict enough that an unshaped request is
//! rejected rather than merely degraded.
//!
//! Ports the body rules from `open-sse/executors/codex.js`. The ones that carry real weight:
//!
//! * **`store: false` is mandatory**, and it makes every server-generated item id unresolvable. So
//!   `rs_`/`fc_`/`resp_`/`msg_`-prefixed ids and `item_reference` entries have to be stripped from the
//!   input, or the backend answers 404 for an id it issued itself.
//! * **A `system` role must become `developer`.** Not cosmetic: Codex keeps the developer message in
//!   the cacheable prefix, so a system role costs the whole prompt cache on every turn.
//! * **The field set is an allowlist, not a blocklist.** An unknown field triggers
//!   `routing_unsupported` upstream, and clients send plenty — `max_output_tokens`, `metadata`,
//!   `stream_options`, `safety_identifier`, `previous_response_id`. A blocklist would need updating
//!   every time a new client appeared.
//! * **The reasoning effort can arrive three ways** — an explicit `reasoning.effort`, a
//!   `reasoning_effort` param, or a suffix on the model name — and the suffix has to be removed from
//!   the model before it is sent.

use serde_json::{Map, Value, json};

/// The instructions Codex's own CLI sends when a client provides none.
///
/// Embedded rather than inlined: it is 11.7KB of prose, and a file keeps it diffable against upstream.
const DEFAULT_INSTRUCTIONS: &str = include_str!("../../data/codex-instructions.md");

/// Fields the Codex Responses endpoint accepts. Everything else is dropped.
const ALLOWED: [&str; 13] = [
    "model",
    "input",
    "instructions",
    "tools",
    "tool_choice",
    "stream",
    "store",
    "reasoning",
    "service_tier",
    "include",
    "prompt_cache_key",
    "client_metadata",
    "text",
];

/// Prefixes marking an id the *server* generated.
///
/// With `store: false` the backend has nothing to resolve them against, so sending one back is a 404
/// on an id it issued.
const SERVER_ID_PREFIXES: [&str; 4] = ["rs_", "fc_", "resp_", "msg_"];

/// Effort levels recognised as a model-name suffix, longest first.
///
/// Order matters: `minimal` must be tested before `low` would ever match a name ending `-minimal`, and
/// checking short names first would mis-split a longer one.
const EFFORT_LEVELS: [&str; 6] = ["minimal", "medium", "xhigh", "high", "none", "low"];

/// Tool types Codex runs server-side. Passed through untouched.
const HOSTED_TOOLS: [&str; 10] = [
    "image_generation",
    "web_search",
    "web_search_preview",
    "file_search",
    "computer",
    "computer_use_preview",
    "code_interpreter",
    "mcp",
    "local_shell",
    "tool_search",
];

/// Longest a tool name may be.
const MAX_TOOL_NAME: usize = 128;

/// Shape a request body for the Codex backend.
///
/// Returns the body to send. The caller passes the session id it resolved, which becomes both the
/// `session_id` header and the `prompt_cache_key` — Codex's prompt cache is keyed by it, so a value
/// that changes per request throws the cache away on every turn.
pub(crate) fn shape_body(body: &Value, session_id: &str) -> Value {
    let mut out = body.as_object().cloned().unwrap_or_default();

    // Input must be a non-empty array. Codex rejects an empty one outright.
    let input = normalise_input(out.get("input"));
    out.insert("input".to_owned(), input);

    convert_system_to_developer(&mut out);
    strip_server_item_ids(&mut out);
    normalise_tools(&mut out);

    // Codex only streams. A non-streaming request is refused rather than answered as JSON.
    out.insert("stream".to_owned(), json!(true));
    // Mandatory, and the reason the id stripping above exists.
    out.insert("store".to_owned(), json!(false));

    if out
        .get("instructions")
        .and_then(Value::as_str)
        .is_none_or(|text| text.trim().is_empty())
    {
        out.insert("instructions".to_owned(), json!(DEFAULT_INSTRUCTIONS));
    }
    if !session_id.is_empty()
        && out
            .get("prompt_cache_key")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        out.insert("prompt_cache_key".to_owned(), json!(session_id));
    }

    apply_reasoning(&mut out);
    normalise_service_tier(&mut out);

    // The allowlist last, so nothing added above survives by accident and nothing a client sent
    // slips through. An unknown field is `routing_unsupported` upstream.
    out.retain(|key, _value| ALLOWED.contains(&key.as_str()));
    Value::Object(out)
}

/// A non-empty input array.
///
/// A string input is wrapped; an absent or empty one becomes a single placeholder message, as upstream
/// does. The placeholder is deliberate: Codex rejects empty input, and a request with no input is
/// usually a client sending its conversation in `instructions` alone.
fn normalise_input(input: Option<&Value>) -> Value {
    match input {
        Some(Value::Array(items)) if !items.is_empty() => Value::Array(items.clone()),
        Some(Value::String(text)) if !text.is_empty() => json!([{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": text }],
        }]),
        _empty => json!([{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "..." }],
        }]),
    }
}

/// Rewrite `role: "system"` to `role: "developer"` on message items.
///
/// Codex keeps the developer message inside the cacheable prefix. A system role is accepted but falls
/// outside it, so every turn pays for the whole prompt again.
fn convert_system_to_developer(out: &mut Map<String, Value>) {
    let Some(items) = out.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    for item in items {
        let Some(object) = item.as_object_mut() else {
            continue;
        };
        let is_message = object
            .get("type")
            .and_then(Value::as_str)
            .is_none_or(|kind| kind == "message");
        if is_message && object.get("role").and_then(Value::as_str) == Some("system") {
            object.insert("role".to_owned(), json!("developer"));
        }
    }
}

/// Remove references to items only the server could resolve.
fn strip_server_item_ids(out: &mut Map<String, Value>) {
    let Some(items) = out.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    items.retain(|item| match item {
        // A bare string that is a server id is a reference to something unresolvable.
        Value::String(text) => !is_server_id(text),
        Value::Object(object) => {
            object.get("type").and_then(Value::as_str) != Some("item_reference")
        }
        _other => true,
    });
    for item in items {
        if let Some(object) = item.as_object_mut()
            && object
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(is_server_id)
        {
            // The item itself is kept — its content is the conversation. Only the unresolvable id goes.
            object.remove("id");
        }
    }
}

fn is_server_id(value: &str) -> bool {
    SERVER_ID_PREFIXES
        .iter()
        .any(|prefix| value.starts_with(prefix))
}

/// Flatten chat-completions tools into the Responses shape and drop what Codex cannot run.
fn normalise_tools(out: &mut Map<String, Value>) {
    let Some(tools) = out.get("tools").and_then(Value::as_array).cloned() else {
        return;
    };
    let mut valid_names: Vec<String> = Vec::new();
    let mut normalised: Vec<Value> = Vec::new();

    for tool in &tools {
        let Some(object) = tool.as_object() else {
            continue;
        };
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if kind == "namespace" {
            // A namespace carries its own tools; their names still have to be known so `tool_choice`
            // can be validated against them.
            if let Some(inner) = object.get("tools").and_then(Value::as_array) {
                for sub in inner {
                    if let Some(name) = sub.get("name").and_then(Value::as_str) {
                        let trimmed: String = name.trim().chars().take(MAX_TOOL_NAME).collect();
                        if !trimmed.is_empty() {
                            valid_names.push(trimmed);
                        }
                    }
                }
            }
            normalised.push(tool.clone());
            continue;
        }

        if kind != "function" {
            // A Responses-native freeform tool passes through intact.
            if kind == "custom" {
                normalised.push(tool.clone());
            } else if !kind.is_empty()
                && object.get("function").is_none()
                && object.get("name").is_none()
                && HOSTED_TOOLS.contains(&kind)
            {
                // Codex runs this one server-side.
                normalised.push(tool.clone());
            }
            // Anything else is a tool Codex cannot run; dropped rather than sent and refused.
            continue;
        }

        let function = object.get("function").and_then(Value::as_object);
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| {
                function
                    .and_then(|inner| inner.get("name"))
                    .and_then(Value::as_str)
            })
            .unwrap_or_default()
            .trim();
        if name.is_empty() {
            continue;
        }
        let name: String = name.chars().take(MAX_TOOL_NAME).collect();
        let description = object
            .get("description")
            .and_then(Value::as_str)
            .or_else(|| {
                function
                    .and_then(|inner| inner.get("description"))
                    .and_then(Value::as_str)
            })
            .unwrap_or_default();
        let parameters = object
            .get("parameters")
            .filter(|value| value.is_object())
            .or_else(|| {
                function
                    .and_then(|inner| inner.get("parameters"))
                    .filter(|value| value.is_object())
            })
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));

        // Rebuilt rather than edited: the Responses shape is flat, and leaving the nested `function`
        // alongside the flattened fields is what makes the backend reject the tool.
        let mut flat = Map::new();
        flat.insert("type".to_owned(), json!("function"));
        flat.insert("name".to_owned(), json!(name));
        if !description.is_empty() {
            flat.insert("description".to_owned(), json!(description));
        }
        flat.insert("parameters".to_owned(), parameters);
        valid_names.push(name);
        normalised.push(Value::Object(flat));
    }

    out.insert("tools".to_owned(), Value::Array(normalised));

    // A `tool_choice` naming a function that did not survive would ask for a tool the backend has not
    // been given.
    let names_a_missing_function = out
        .get("tool_choice")
        .and_then(Value::as_object)
        .filter(|choice| choice.get("type").and_then(Value::as_str) == Some("function"))
        .is_some_and(|choice| {
            choice
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_none_or(|name| {
                    name.is_empty() || !valid_names.iter().any(|known| known == name)
                })
        });
    if names_a_missing_function {
        out.remove("tool_choice");
    }
}

/// Resolve the reasoning block and strip an effort suffix from the model name.
fn apply_reasoning(out: &mut Map<String, Value>) {
    let model = out
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let (base_model, suffix_effort) = split_effort_suffix(&model);
    if base_model != model {
        out.insert("model".to_owned(), json!(base_model));
    }

    let param_effort = out
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .map(str::to_owned);

    match out.get("reasoning").and_then(Value::as_object).cloned() {
        Some(mut reasoning) => {
            let effort = reasoning
                .get("effort")
                .and_then(Value::as_str)
                .unwrap_or("low");
            reasoning.insert("effort".to_owned(), json!(normalise_effort(effort)));
            if reasoning.get("summary").is_none() {
                reasoning.insert("summary".to_owned(), json!("auto"));
            }
            out.insert("reasoning".to_owned(), Value::Object(reasoning));
        }
        None => {
            // Explicit param, else the model suffix, else upstream's default.
            let effort = param_effort
                .clone()
                .or(suffix_effort)
                .unwrap_or_else(|| "low".to_owned());
            out.insert(
                "reasoning".to_owned(),
                json!({ "effort": normalise_effort(&effort), "summary": "auto" }),
            );
        }
    }
    out.remove("reasoning_effort");

    // Reasoning models need the encrypted content included or the backend refuses the request.
    let effort_is_active = out
        .get("reasoning")
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(Value::as_str)
        .is_some_and(|effort| effort != "none");
    if effort_is_active {
        out.insert("include".to_owned(), json!(["reasoning.encrypted_content"]));
    }
}

/// Split a trailing `-<effort>` off a model name.
fn split_effort_suffix(model: &str) -> (String, Option<String>) {
    for level in EFFORT_LEVELS {
        let suffix = format!("-{level}");
        if let Some(base) = model.strip_suffix(&suffix) {
            return (base.to_owned(), Some((*level).to_owned()));
        }
    }
    (model.to_owned(), None)
}

/// Map an effort name onto one Codex accepts.
///
/// `ultra` and `max` are this router's own higher levels; Codex's ceiling is `xhigh`, so they map onto
/// it rather than being sent through and rejected.
fn normalise_effort(value: &str) -> String {
    match value {
        "ultra" | "max" => "xhigh".to_owned(),
        other => other.to_owned(),
    }
}

/// Codex accepts only `priority`; `fast` means that, and anything else is dropped.
fn normalise_service_tier(out: &mut Map<String, Value>) {
    let tier = out
        .get("service_tier")
        .and_then(Value::as_str)
        .map(str::to_owned);
    match tier.as_deref() {
        Some("fast" | "priority") => {
            out.insert("service_tier".to_owned(), json!("priority"));
        }
        Some(_other) => {
            out.remove("service_tier");
        }
        None => {}
    }
}

/// Headers Codex needs beyond the registry's own.
///
/// The account binding is the one that matters with more than one Codex connection configured: without
/// it a request can bind to the wrong OpenAI account and fail as `token_invalid`, which looks like an
/// expired token rather than a mis-routed request.
pub(crate) fn headers(session_id: &str, account_id: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![(
        "session_id".to_owned(),
        if session_id.is_empty() {
            "default".to_owned()
        } else {
            session_id.to_owned()
        },
    )];
    if let Some(account) = account_id.filter(|account| !account.trim().is_empty()) {
        headers.push(("ChatGPT-Account-ID".to_owned(), account.to_owned()));
    }
    headers
}

/// The account id to bind a request to, in upstream's order of preference.
pub(crate) fn account_id(credentials: &crate::credentials::Credentials) -> Option<String> {
    for key in ["workspaceId", "chatgptAccountId", "accountId"] {
        if let Some(value) = credentials
            .setting(key)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{DEFAULT_INSTRUCTIONS, headers, shape_body};

    fn shaped(body: &Value) -> Value {
        shape_body(body, "sess-1")
    }

    #[test]
    fn store_is_forced_off_and_streaming_on() {
        // Both are Codex requirements rather than preferences: a request with `store: true` or without
        // streaming is refused.
        let out = shaped(&json!({ "model": "gpt-5.3-codex", "store": true, "stream": false }));
        assert_eq!(out.get("store"), Some(&json!(false)));
        assert_eq!(out.get("stream"), Some(&json!(true)));
    }

    #[test]
    fn a_system_role_becomes_developer_to_stay_in_the_cacheable_prefix() {
        // Codex accepts `system`, so this is not about validity — it is about the prompt cache. A
        // system message falls outside the cacheable prefix, so every turn pays for the whole prompt.
        let out = shaped(&json!({
            "model": "gpt-5.3-codex",
            "input": [
                { "type": "message", "role": "system", "content": [{ "type": "input_text", "text": "rules" }] },
                { "role": "user", "content": [{ "type": "input_text", "text": "hi" }] },
            ],
        }));
        assert_eq!(out.pointer("/input/0/role"), Some(&json!("developer")));
        assert_eq!(out.pointer("/input/1/role"), Some(&json!("user")));
    }

    #[test]
    fn server_generated_ids_are_stripped_because_store_is_false() {
        // The backend cannot resolve an id it issued once `store: false`, so sending one back is a 404
        // on its own id. The item's content is kept; only the id goes.
        let out = shaped(&json!({
            "model": "gpt-5.3-codex",
            "input": [
                { "id": "rs_abc", "role": "assistant", "content": [{ "type": "output_text", "text": "prior" }] },
                { "type": "item_reference", "id": "msg_xyz" },
                "fc_bare_reference",
                { "id": "local-1", "role": "user", "content": [{ "type": "input_text", "text": "now" }] },
            ],
        }));
        let input = out.get("input").and_then(Value::as_array).expect("input");

        // The reference item and the bare server id are gone.
        assert_eq!(input.len(), 2, "{input:?}");
        // The first item survives without its id, because its content is the conversation.
        assert!(input.first().and_then(|item| item.get("id")).is_none());
        assert_eq!(
            input
                .first()
                .and_then(|item| item.pointer("/content/0/text")),
            Some(&json!("prior"))
        );
        // A client-generated id is not a server id and stays.
        assert_eq!(
            input.get(1).and_then(|item| item.get("id")),
            Some(&json!("local-1"))
        );
    }

    #[test]
    fn unknown_fields_are_dropped_by_allowlist() {
        // A blocklist would need updating for every new client. These are the fields real clients send
        // that Codex answers `routing_unsupported` for.
        let out = shaped(&json!({
            "model": "gpt-5.3-codex",
            "temperature": 0.7,
            "top_p": 1,
            "max_output_tokens": 4096,
            "metadata": { "a": 1 },
            "stream_options": { "include_usage": true },
            "safety_identifier": "x",
            "previous_response_id": "resp_1",
            "user": "someone",
            "invented_by_a_future_client": true,
        }));
        let object = out.as_object().expect("an object");
        for dropped in [
            "temperature",
            "top_p",
            "max_output_tokens",
            "metadata",
            "stream_options",
            "safety_identifier",
            "previous_response_id",
            "user",
            "invented_by_a_future_client",
        ] {
            assert!(!object.contains_key(dropped), "{dropped} must not be sent");
        }
        // And what Codex does accept survives.
        assert!(object.contains_key("model"));
        assert!(object.contains_key("input"));
        assert!(object.contains_key("instructions"));
    }

    #[test]
    fn default_instructions_are_injected_only_when_absent() {
        let injected = shaped(&json!({ "model": "gpt-5.3-codex" }));
        assert_eq!(
            injected.get("instructions").and_then(Value::as_str),
            Some(DEFAULT_INSTRUCTIONS)
        );
        // A client's own instructions are never replaced.
        let supplied = shaped(&json!({ "model": "gpt-5.3-codex", "instructions": "be brief" }));
        assert_eq!(
            supplied.get("instructions").and_then(Value::as_str),
            Some("be brief")
        );
        // Whitespace-only counts as absent.
        let blank = shaped(&json!({ "model": "gpt-5.3-codex", "instructions": "   " }));
        assert_eq!(
            blank.get("instructions").and_then(Value::as_str),
            Some(DEFAULT_INSTRUCTIONS)
        );
    }

    #[test]
    fn an_effort_suffix_is_read_then_removed_from_the_model() {
        // The suffix is this router's convention, not a model Codex knows. Leaving it on sends a model
        // name the backend has never heard of.
        let out = shaped(&json!({ "model": "gpt-5.3-codex-high" }));
        assert_eq!(out.get("model"), Some(&json!("gpt-5.3-codex")));
        assert_eq!(out.pointer("/reasoning/effort"), Some(&json!("high")));
        assert_eq!(out.pointer("/reasoning/summary"), Some(&json!("auto")));
    }

    #[test]
    fn an_explicit_effort_outranks_a_model_suffix() {
        let out = shaped(&json!({ "model": "gpt-5.3-codex-low", "reasoning_effort": "high" }));
        assert_eq!(out.pointer("/reasoning/effort"), Some(&json!("high")));
        // The param itself is not a Codex field.
        assert!(out.get("reasoning_effort").is_none());
    }

    #[test]
    fn this_routers_higher_levels_map_onto_codexs_ceiling() {
        // `ultra` and `max` are ours. Codex stops at `xhigh`, so sending them through is a rejection.
        for level in ["ultra", "max"] {
            let out = shaped(&json!({ "model": "gpt-5.3-codex", "reasoning_effort": level }));
            assert_eq!(
                out.pointer("/reasoning/effort"),
                Some(&json!("xhigh")),
                "{level}"
            );
        }
    }

    #[test]
    fn a_reasoning_request_includes_encrypted_content() {
        // Required by the backend for reasoning models; without it the request is refused.
        let out = shaped(&json!({ "model": "gpt-5.3-codex", "reasoning_effort": "medium" }));
        assert_eq!(
            out.get("include"),
            Some(&json!(["reasoning.encrypted_content"]))
        );
        // Not sent when reasoning is off, where it would be meaningless.
        let off = shaped(&json!({ "model": "gpt-5.3-codex", "reasoning_effort": "none" }));
        assert!(off.get("include").is_none());
    }

    #[test]
    fn chat_completions_tools_are_flattened_into_the_responses_shape() {
        // The Responses shape is flat. Leaving the nested `function` beside the flattened fields is
        // what makes the backend reject the tool.
        let out = shaped(&json!({
            "model": "gpt-5.3-codex",
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Current weather",
                    "parameters": { "type": "object", "properties": { "city": { "type": "string" } } },
                },
            }],
        }));
        let tool = out.pointer("/tools/0").expect("a tool");
        assert_eq!(tool.get("type"), Some(&json!("function")));
        assert_eq!(tool.get("name"), Some(&json!("get_weather")));
        assert_eq!(tool.get("description"), Some(&json!("Current weather")));
        assert!(
            tool.get("function").is_none(),
            "the nested form must not survive alongside the flat one: {tool}"
        );
        assert_eq!(
            tool.pointer("/parameters/properties/city/type"),
            Some(&json!("string"))
        );
    }

    #[test]
    fn a_hosted_tool_passes_through_and_an_unrunnable_one_is_dropped() {
        let out = shaped(&json!({
            "model": "gpt-5.3-codex",
            "tools": [
                { "type": "web_search" },
                { "type": "some_tool_codex_cannot_run" },
            ],
        }));
        let tools = out.get("tools").and_then(Value::as_array).expect("tools");
        assert_eq!(tools.len(), 1, "{tools:?}");
        assert_eq!(
            tools.first().and_then(|tool| tool.get("type")),
            Some(&json!("web_search"))
        );
    }

    #[test]
    fn a_tool_choice_naming_a_dropped_function_is_removed() {
        // Asking for a tool the backend was never given is a request that cannot be satisfied.
        let out = shaped(&json!({
            "model": "gpt-5.3-codex",
            "tools": [{ "type": "function", "name": "kept", "parameters": {} }],
            "tool_choice": { "type": "function", "name": "never_declared" },
        }));
        assert!(out.get("tool_choice").is_none(), "{out}");

        let valid = shaped(&json!({
            "model": "gpt-5.3-codex",
            "tools": [{ "type": "function", "name": "kept", "parameters": {} }],
            "tool_choice": { "type": "function", "name": "kept" },
        }));
        assert_eq!(valid.pointer("/tool_choice/name"), Some(&json!("kept")));
    }

    #[test]
    fn empty_input_becomes_a_placeholder_rather_than_a_refusal() {
        // Codex rejects empty input outright.
        for body in [
            json!({ "model": "m" }),
            json!({ "model": "m", "input": [] }),
        ] {
            let out = shaped(&body);
            let input = out.get("input").and_then(Value::as_array).expect("input");
            assert_eq!(input.len(), 1, "{input:?}");
            assert_eq!(
                input
                    .first()
                    .and_then(|item| item.pointer("/content/0/text")),
                Some(&json!("..."))
            );
        }
    }

    #[test]
    fn a_string_input_is_wrapped_in_a_message() {
        let out = shaped(&json!({ "model": "m", "input": "just text" }));
        assert_eq!(
            out.pointer("/input/0/content/0/text"),
            Some(&json!("just text"))
        );
        assert_eq!(out.pointer("/input/0/role"), Some(&json!("user")));
    }

    #[test]
    fn the_prompt_cache_key_is_the_session_so_the_cache_survives_a_turn() {
        // Codex keys its prompt cache by this. A value that changes per request discards the cache
        // every turn, which is a cost rather than a failure — and so easy to miss.
        let out = shape_body(&json!({ "model": "m" }), "conversation-7");
        assert_eq!(out.get("prompt_cache_key"), Some(&json!("conversation-7")));
        // A client's own key wins.
        let supplied = shape_body(
            &json!({ "model": "m", "prompt_cache_key": "mine" }),
            "other",
        );
        assert_eq!(supplied.get("prompt_cache_key"), Some(&json!("mine")));
    }

    #[test]
    fn only_priority_survives_as_a_service_tier() {
        assert_eq!(
            shaped(&json!({ "model": "m", "service_tier": "fast" })).get("service_tier"),
            Some(&json!("priority"))
        );
        assert_eq!(
            shaped(&json!({ "model": "m", "service_tier": "priority" })).get("service_tier"),
            Some(&json!("priority"))
        );
        assert!(
            shaped(&json!({ "model": "m", "service_tier": "flex" }))
                .get("service_tier")
                .is_none()
        );
    }

    #[test]
    fn the_account_binding_header_is_sent_when_known() {
        // With more than one Codex connection this is what keeps a request on the right account. Its
        // absence surfaces as `token_invalid`, which reads as an expired token rather than a
        // mis-routed request.
        let sent = headers("sess-9", Some("acct-1"));
        assert!(sent.contains(&("session_id".to_owned(), "sess-9".to_owned())));
        assert!(sent.contains(&("ChatGPT-Account-ID".to_owned(), "acct-1".to_owned())));

        // No account known: the header is omitted rather than sent empty.
        let without = headers("sess-9", None);
        assert!(
            without
                .iter()
                .all(|(key, _value)| key != "ChatGPT-Account-ID")
        );
        // And a missing session still sends something rather than an empty header.
        assert!(headers("", None).contains(&("session_id".to_owned(), "default".to_owned())));
    }
}
