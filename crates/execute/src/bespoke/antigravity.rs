//! Request shaping for `antigravity`, Google's Antigravity IDE backend.
//!
//! Ports `open-sse/executors/antigravity.js`. This is Cloud Code Assist reached as the Antigravity IDE
//! reaches it, so the Gemini payload is wrapped in an envelope and the whole thing has to look like that
//! IDE's traffic.
//!
//! Four rules here exist because Antigravity *refuses* a request rather than degrading it, and one
//! exists because it bans you:
//!
//! * **Gemini 3+ rejects a `functionCall` part with no `thoughtSignature`.** Clients do not persist that
//!   signature in their history, so a second turn carrying a prior tool call arrives without one. It is
//!   backfilled with the IDE's own default signature.
//! * **Tool groups must be merged into one.** Gemini accepts a single `functionDeclarations` group; two
//!   groups is a rejection.
//! * **Thinking fields must be stripped from both levels.** This router's thinking translation writes
//!   them at the body root, and Google rejects the field wherever it appears.
//! * **A competitor's system prompt gets the request flagged.** Upstream found that Zed's Claude prompt
//!   draws an immediate 429 quota-exhausted, so that sentence is removed. Not evasion of a rate limit —
//!   it is avoiding a block triggered by naming another vendor's agent.

use std::fmt::Write as _;

use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

/// The IDE's own default thinking signature, backfilled onto tool calls that arrive without one.
const DEFAULT_THINKING_SIGNATURE: &str = include_str!("../../data/antigravity-signature.txt");

/// Antigravity's output ceiling. A larger request is refused.
const MAX_OUTPUT_TOKENS: u64 = 64_000;

/// Fields Google's `generateContent` rejects outright.
///
/// Written at the body root by this router's own thinking translation, so they are stripped from both
/// the envelope and the inner request.
const BLACKLIST: [&str; 7] = [
    "output_config",
    "thinking",
    "reasoning_effort",
    "reasoning",
    "enable_thinking",
    "thinking_budget",
    "thinkingConfig",
];

/// The sentence that gets a request flagged and 429'd.
const FLAGGED_PROMPT: &str = "You are a Claude agent, built on Anthropic's Claude Agent SDK.";

/// Longest function name Gemini accepts.
const MAX_FUNCTION_NAME: usize = 64;

/// Whether a model name asks for image generation.
///
/// Image requests take a different envelope entirely and cannot stream, so this decides both.
pub(crate) fn is_image_model(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower.contains("image") || lower.contains("imagen")
}

/// The URL suffix for a request: the streaming method, or the blocking one.
pub(crate) fn url_suffix(model: &str, stream: bool) -> &'static str {
    if stream && !is_image_model(model) {
        "/v1internal:streamGenerateContent?alt=sse"
    } else {
        // Image generation must use the blocking method; the streaming one refuses it.
        "/v1internal:generateContent"
    }
}

/// Wrap a Gemini body in the Antigravity envelope.
pub(crate) fn envelope(body: &Value, session_id: &str, project_id: &str) -> Value {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if is_image_model(model) {
        return image_envelope(body, session_id, project_id, model);
    }

    // The inner request is whatever the client sent under `request`, or the body itself when a client
    // posted a bare Gemini payload.
    let source = body.get("request").unwrap_or(body);
    let mut request = source.as_object().cloned().unwrap_or_default();

    // Fields Google rejects, at the inner level.
    for key in BLACKLIST {
        request.remove(key);
    }

    if let Some(contents) = request.get("contents").and_then(Value::as_array) {
        request.insert("contents".to_owned(), Value::Array(fix_contents(contents)));
    }

    let declarations = merge_tool_declarations(source.get("tools"));
    request.remove("tools");
    request.remove("toolConfig");
    if !declarations.is_empty() {
        request.insert(
            "tools".to_owned(),
            json!([{ "functionDeclarations": declarations }]),
        );
        // `VALIDATED` is what the IDE sends: the model may only call a declared function.
        request.insert(
            "toolConfig".to_owned(),
            json!({ "functionCallingConfig": { "mode": "VALIDATED" } }),
        );
    }

    rewrite_flagged_prompt(&mut request);
    cap_output_tokens(&mut request);
    // Explicitly not forwarded: the IDE sends none, and a client's settings here are rejected.
    request.remove("safetySettings");

    let session = source
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or(session_id);
    request.insert("sessionId".to_owned(), json!(session));

    let content_count = request
        .get("contents")
        .and_then(Value::as_array)
        .map_or(1, Vec::len);

    // The envelope keeps the client's own top-level fields, minus the blacklist.
    let mut out = body.as_object().cloned().unwrap_or_default();
    for key in BLACKLIST {
        out.remove(key);
    }
    out.insert("project".to_owned(), json!(project_id));
    out.insert("model".to_owned(), json!(model));
    out.insert("userAgent".to_owned(), json!("antigravity"));
    out.insert("requestType".to_owned(), json!("agent"));
    out.insert(
        "requestId".to_owned(),
        json!(request_id(
            body.get("requestId").and_then(Value::as_str),
            session,
            model,
            "agent",
            content_count
        )),
    );
    out.insert("request".to_owned(), Value::Object(request));
    Value::Object(out)
}

/// The envelope for an image request, which shares almost nothing with the standard one.
///
/// No tools, no system instruction, no safety settings — and text-only contents, because the endpoint
/// takes a prompt rather than a conversation.
fn image_envelope(body: &Value, session_id: &str, project_id: &str, model: &str) -> Value {
    let source = body.get("request").unwrap_or(body);
    let contents: Vec<Value> = source
        .get("contents")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|content| {
            let parts: Vec<Value> = content
                .get("parts")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .filter(|part| part.get("text").is_some())
                .map(|part| json!({ "text": part.get("text").cloned().unwrap_or(Value::Null) }))
                .collect();
            (!parts.is_empty()).then(|| {
                json!({
                    "role": content.get("role").and_then(Value::as_str).unwrap_or("user"),
                    "parts": parts,
                })
            })
        })
        .collect();

    let request = json!({
        "contents": contents,
        "generationConfig": {
            "temperature": 1.0,
            "topP": 0.95,
            "topK": 40,
            "maxOutputTokens": 8192,
            "imageConfig": { "aspectRatio": aspect_ratio(model) },
        },
        "sessionId": session_id,
    });
    let content_count = request
        .get("contents")
        .and_then(Value::as_array)
        .map_or(1, Vec::len);
    // The dimension suffix is this router's convention, not a model Antigravity knows.
    let clean_model = strip_dimension_suffix(model);

    json!({
        "project": project_id,
        "model": clean_model,
        "userAgent": "antigravity",
        "requestType": "image_gen",
        "requestId": request_id(
            body.get("requestId").and_then(Value::as_str),
            session_id,
            &clean_model,
            "image_gen",
            content_count,
        ),
        "request": request,
    })
}

/// Per-turn fixes to the conversation.
fn fix_contents(contents: &[Value]) -> Vec<Value> {
    contents
        .iter()
        .map(|content| {
            let Some(object) = content.as_object() else {
                return content.clone();
            };
            let mut fixed = object.clone();
            let parts = object
                .get("parts")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);

            // A tool result must be attributed to the user for Claude models reached through
            // Antigravity; the assistant role is rejected for one.
            if parts
                .iter()
                .any(|part| part.get("functionResponse").is_some())
            {
                fixed.insert("role".to_owned(), json!("user"));
            }

            let kept: Vec<Value> = parts
                .iter()
                .filter(|part| {
                    // A thought-only part is the model's scratch work and is not sent back.
                    if part.get("thought").is_some() && part.get("functionCall").is_none() {
                        return false;
                    }
                    // A bare signature with nothing attached carries nothing.
                    if part.get("thoughtSignature").is_some()
                        && part.get("functionCall").is_none()
                        && part.get("text").is_none()
                    {
                        return false;
                    }
                    true
                })
                .map(|part| {
                    // Gemini 3+ rejects a `functionCall` with no signature, and clients do not persist
                    // one. Backfilling the IDE's default is what makes a second turn work at all.
                    let needs_signature = part.get("functionCall").is_some()
                        && part.get("thoughtSignature").is_none();
                    if !needs_signature {
                        return (*part).clone();
                    }
                    let mut filled = part.as_object().cloned().unwrap_or_default();
                    filled.insert(
                        "thoughtSignature".to_owned(),
                        json!(DEFAULT_THINKING_SIGNATURE.trim()),
                    );
                    Value::Object(filled)
                })
                .collect();
            fixed.insert("parts".to_owned(), Value::Array(kept));
            Value::Object(fixed)
        })
        .collect()
}

/// Flatten every tool group into one declaration list, sanitising names.
fn merge_tool_declarations(tools: Option<&Value>) -> Vec<Value> {
    let Some(groups) = tools.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen: Vec<String> = Vec::new();
    let mut declarations = Vec::new();

    for group in groups {
        let functions = group
            .get("functionDeclarations")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        for function in functions {
            let name = sanitize_function_name(function.get("name").and_then(Value::as_str));
            if seen.contains(&name) {
                // A duplicate name after sanitising would be two tools the model cannot tell apart.
                continue;
            }
            let mut declaration = function.as_object().cloned().unwrap_or_default();
            declaration.insert("name".to_owned(), json!(name.clone()));
            // A function with no parameters still needs a schema; Gemini rejects an absent one. The
            // placeholder asks for a reason, which is upstream's own choice and keeps the call useful.
            let parameters = declaration
                .get("parameters")
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or_else(|| {
                    json!({
                        "type": "object",
                        "properties": { "reason": { "type": "string", "description": "Brief explanation" } },
                        "required": ["reason"],
                    })
                });
            declaration.insert("parameters".to_owned(), parameters);
            seen.push(name);
            declarations.push(Value::Object(declaration));
        }
    }
    declarations
}

/// Coerce a name into what Gemini accepts: `[a-zA-Z_][a-zA-Z0-9_.:-]{0,63}`.
fn sanitize_function_name(name: Option<&str>) -> String {
    let Some(name) = name.filter(|name| !name.is_empty()) else {
        return "_unknown".to_owned();
    };
    let mut sanitized: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | ':' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if !sanitized
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
    {
        sanitized.insert(0, '_');
    }
    sanitized.chars().take(MAX_FUNCTION_NAME).collect()
}

/// Remove the sentence that gets a request flagged.
fn rewrite_flagged_prompt(request: &mut Map<String, Value>) {
    let Some(parts) = request
        .get_mut("systemInstruction")
        .and_then(|instruction| instruction.get_mut("parts"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for part in parts {
        let Some(rewritten) = part
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| text.contains(FLAGGED_PROMPT))
            .map(|text| text.replace(FLAGGED_PROMPT, ""))
        else {
            continue;
        };
        if let Some(object) = part.as_object_mut() {
            object.insert("text".to_owned(), json!(rewritten));
        }
    }
}

/// Clamp `maxOutputTokens` to what Antigravity accepts.
fn cap_output_tokens(request: &mut Map<String, Value>) {
    let Some(config) = request
        .get_mut("generationConfig")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if config
        .get("maxOutputTokens")
        .and_then(Value::as_u64)
        .is_some_and(|max| max > MAX_OUTPUT_TOKENS)
    {
        config.insert("maxOutputTokens".to_owned(), json!(MAX_OUTPUT_TOKENS));
    }
}

/// The aspect ratio named by a model-name suffix.
///
/// `-16x9` is a ratio; `-1024x768` is a resolution and is reduced to one. Both spellings exist because
/// both are natural to ask for.
fn aspect_ratio(model: &str) -> String {
    let Some((width, height)) = dimension_suffix(model) else {
        return "1:1".to_owned();
    };
    if width <= 16 && height <= 16 {
        return format!("{width}:{height}");
    }
    let divisor = gcd(width, height);
    format!("{}:{}", width / divisor, height / divisor)
}

/// The trailing `-<width>x<height>` of a model name, if it has one.
fn dimension_suffix(model: &str) -> Option<(u64, u64)> {
    let (_head, tail) = model.rsplit_once('-')?;
    let (width, height) = tail.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

fn strip_dimension_suffix(model: &str) -> String {
    match (dimension_suffix(model), model.rsplit_once('-')) {
        (Some(_dimensions), Some((head, _tail))) => head.to_owned(),
        _no_suffix => model.to_owned(),
    }
}

const fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The IDE-shaped request id.
///
/// `agent/<conversation>/<millis>/<trajectory>/<step>`. A client that already sent one in this shape has
/// it preserved — it is the IDE's own id and identifies the conversation better than anything derived
/// here. Otherwise both uuids are derived from the session so they stay stable across a conversation.
fn request_id(
    supplied: Option<&str>,
    session_id: &str,
    model: &str,
    request_type: &str,
    content_count: usize,
) -> String {
    if let Some(existing) = supplied.filter(|id| is_ide_request_id(id)) {
        return existing.to_owned();
    }
    let conversation = uuid_from_seed(&format!("antigravity:conversation:{session_id}"));
    let trajectory = uuid_from_seed(&format!(
        "antigravity:trajectory:{session_id}:{model}:{request_type}"
    ));
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    // Each turn is a request and a response, so the step count runs two per content entry.
    let step = content_count.saturating_mul(2).saturating_sub(1).max(1);
    format!("agent/{conversation}/{millis}/{trajectory}/{step}")
}

/// Whether a supplied id already has the IDE's shape: `agent/<x>/<digits>/<y>/<digits>`.
fn is_ide_request_id(value: &str) -> bool {
    let parts: Vec<&str> = value.split('/').collect();
    parts.len() == 5
        && parts.first() == Some(&"agent")
        && parts.get(1).is_some_and(|part| !part.is_empty())
        && parts
            .get(2)
            .is_some_and(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        && parts.get(3).is_some_and(|part| !part.is_empty())
        && parts
            .get(4)
            .is_some_and(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// A deterministic v5-shaped uuid from a seed.
///
/// Deterministic on purpose: the conversation and trajectory ids must be the same on every turn of one
/// conversation, or Antigravity sees each request as a new agent run.
fn uuid_from_seed(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes = [0_u8; 16];
    for (target, source) in bytes.iter_mut().zip(digest.iter()) {
        *target = *source;
    }
    if let Some(byte) = bytes.get_mut(6) {
        *byte = (*byte & 0x0F) | 0x50;
    }
    if let Some(byte) = bytes.get_mut(8) {
        *byte = (*byte & 0x3F) | 0x80;
    }
    let hex = bytes
        .iter()
        .fold(String::with_capacity(32), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    [
        hex.get(..8),
        hex.get(8..12),
        hex.get(12..16),
        hex.get(16..20),
        hex.get(20..32),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("-")
}

/// A project id, derived from the session so it is stable for one connection.
///
/// Upstream generates a random one per process. Deriving it keeps it stable across a restart, which is
/// the more useful behaviour: the id appears in Google's own logs against the account.
pub(crate) fn project_id(session_id: &str) -> String {
    const ADJECTIVES: [&str; 5] = ["useful", "bright", "swift", "calm", "bold"];
    const NOUNS: [&str; 5] = ["fuze", "wave", "spark", "flow", "core"];
    let digest = Sha256::digest(format!("antigravity:project:{session_id}").as_bytes());
    let pick = |index: usize, list: &'static [&'static str; 5]| -> &'static str {
        let byte = digest.get(index).copied().unwrap_or(0);
        list.get(usize::from(byte) % list.len())
            .copied()
            .unwrap_or("useful")
    };
    let suffix: String = digest
        .iter()
        .skip(2)
        .take(3)
        .fold(String::with_capacity(6), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
        .chars()
        .take(5)
        .collect();
    format!("{}-{}-{suffix}", pick(0, &ADJECTIVES), pick(1, &NOUNS))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        aspect_ratio, envelope, is_image_model, project_id, sanitize_function_name, url_suffix,
    };

    fn gemini_body() -> Value {
        json!({
            "model": "gemini-3-pro",
            "request": {
                "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            },
        })
    }

    #[test]
    fn the_gemini_payload_is_wrapped_in_the_ide_envelope() {
        let out = envelope(&gemini_body(), "sess-1", "proj-1");
        assert_eq!(out.get("project"), Some(&json!("proj-1")));
        assert_eq!(out.get("userAgent"), Some(&json!("antigravity")));
        assert_eq!(out.get("requestType"), Some(&json!("agent")));
        assert_eq!(out.pointer("/request/sessionId"), Some(&json!("sess-1")));
        assert_eq!(
            out.pointer("/request/contents/0/parts/0/text"),
            Some(&json!("hi"))
        );
    }

    #[test]
    fn a_tool_call_without_a_signature_is_backfilled() {
        // Gemini 3+ rejects a `functionCall` part with no `thoughtSignature`, and clients do not persist
        // one in their history. Without this backfill, every second turn carrying a prior tool call
        // fails.
        let out = envelope(
            &json!({
                "model": "gemini-3-pro",
                "request": {
                    "contents": [{
                        "role": "model",
                        "parts": [{ "functionCall": { "name": "read_file", "args": {} } }],
                    }],
                },
            }),
            "sess-1",
            "proj-1",
        );
        let signature = out
            .pointer("/request/contents/0/parts/0/thoughtSignature")
            .and_then(Value::as_str)
            .expect("a backfilled signature");
        assert!(!signature.is_empty());
    }

    #[test]
    fn a_tool_result_is_attributed_to_the_user() {
        // A Claude model reached through Antigravity rejects a tool result on the assistant role.
        let out = envelope(
            &json!({
                "model": "claude-sonnet-4",
                "request": {
                    "contents": [{
                        "role": "model",
                        "parts": [{ "functionResponse": { "name": "read_file", "response": {} } }],
                    }],
                },
            }),
            "sess-1",
            "proj-1",
        );
        assert_eq!(
            out.pointer("/request/contents/0/role"),
            Some(&json!("user"))
        );
    }

    #[test]
    fn thought_only_parts_are_not_sent_back() {
        let out = envelope(
            &json!({
                "model": "gemini-3-pro",
                "request": {
                    "contents": [{
                        "role": "model",
                        "parts": [
                            { "thought": true, "text": "scratch work" },
                            { "text": "the answer" },
                        ],
                    }],
                },
            }),
            "sess-1",
            "proj-1",
        );
        let parts = out
            .pointer("/request/contents/0/parts")
            .and_then(Value::as_array)
            .expect("parts");
        assert_eq!(parts.len(), 1, "{parts:?}");
        assert_eq!(
            parts.first().and_then(|part| part.get("text")),
            Some(&json!("the answer"))
        );
    }

    #[test]
    fn tool_groups_are_merged_into_one_declaration_list() {
        // Gemini accepts a single `functionDeclarations` group; two is a rejection.
        let out = envelope(
            &json!({
                "model": "gemini-3-pro",
                "request": {
                    "contents": [],
                    "tools": [
                        { "functionDeclarations": [{ "name": "a", "parameters": { "type": "object" } }] },
                        { "functionDeclarations": [{ "name": "b" }] },
                    ],
                },
            }),
            "sess-1",
            "proj-1",
        );
        let groups = out
            .pointer("/request/tools")
            .and_then(Value::as_array)
            .expect("one group");
        assert_eq!(groups.len(), 1);
        let declarations = groups
            .first()
            .and_then(|group| group.get("functionDeclarations"))
            .and_then(Value::as_array)
            .expect("declarations");
        assert_eq!(declarations.len(), 2);
        // A function with no schema gets a placeholder, because Gemini rejects an absent one.
        assert_eq!(
            declarations
                .get(1)
                .and_then(|d| d.pointer("/parameters/type")),
            Some(&json!("object"))
        );
        // Tools present means the IDE's calling mode.
        assert_eq!(
            out.pointer("/request/toolConfig/functionCallingConfig/mode"),
            Some(&json!("VALIDATED"))
        );
    }

    #[test]
    fn a_function_name_is_coerced_into_what_gemini_accepts() {
        assert_eq!(sanitize_function_name(Some("read file!")), "read_file_");
        // Must start with a letter or underscore.
        assert_eq!(sanitize_function_name(Some("1tool")), "_1tool");
        assert_eq!(sanitize_function_name(None), "_unknown");
        // Truncated to 64.
        assert_eq!(sanitize_function_name(Some(&"x".repeat(80))).len(), 64);
    }

    #[test]
    fn thinking_fields_are_stripped_from_both_levels() {
        // This router's own thinking translation writes them at the body root, and Google rejects the
        // field wherever it appears.
        let out = envelope(
            &json!({
                "model": "gemini-3-pro",
                "thinking": { "type": "enabled" },
                "reasoning_effort": "high",
                "request": {
                    "contents": [],
                    "thinkingConfig": { "thinkingBudget": 1024 },
                    "thinking_budget": 2048,
                },
            }),
            "sess-1",
            "proj-1",
        );
        let top = out.as_object().expect("an object");
        assert!(!top.contains_key("thinking"));
        assert!(!top.contains_key("reasoning_effort"));
        let request = out
            .get("request")
            .and_then(Value::as_object)
            .expect("request");
        assert!(!request.contains_key("thinkingConfig"));
        assert!(!request.contains_key("thinking_budget"));
    }

    #[test]
    fn a_competitors_prompt_is_removed_to_avoid_an_immediate_block() {
        // Upstream found this draws a 429 quota-exhausted on the spot. Not evasion of a rate limit —
        // avoiding a block triggered by naming another vendor's agent.
        let out = envelope(
            &json!({
                "model": "gemini-3-pro",
                "request": {
                    "contents": [],
                    "systemInstruction": {
                        "parts": [{
                            "text": "You are a Claude agent, built on Anthropic's Claude Agent SDK. Be terse.",
                        }],
                    },
                },
            }),
            "sess-1",
            "proj-1",
        );
        let text = out
            .pointer("/request/systemInstruction/parts/0/text")
            .and_then(Value::as_str)
            .expect("the instruction");
        assert!(!text.contains("Claude Agent SDK"), "{text}");
        assert!(text.contains("Be terse."), "{text}");
    }

    #[test]
    fn output_tokens_are_capped_and_safety_settings_dropped() {
        let out = envelope(
            &json!({
                "model": "gemini-3-pro",
                "request": {
                    "contents": [],
                    "generationConfig": { "maxOutputTokens": 200_000 },
                    "safetySettings": [{ "category": "HARM_CATEGORY_HATE_SPEECH" }],
                },
            }),
            "sess-1",
            "proj-1",
        );
        assert_eq!(
            out.pointer("/request/generationConfig/maxOutputTokens"),
            Some(&json!(64_000))
        );
        // The IDE sends none, and a client's own settings are rejected.
        assert!(out.pointer("/request/safetySettings").is_none());
    }

    #[test]
    fn the_request_id_is_stable_for_one_conversation() {
        // Both uuids are derived from the session, so Antigravity sees one agent run across turns
        // rather than a new one per request.
        let first = envelope(&gemini_body(), "sess-stable", "proj-1");
        let second = envelope(&gemini_body(), "sess-stable", "proj-1");
        let parts = |value: &Value| {
            value
                .get("requestId")
                .and_then(Value::as_str)
                .map(|id| id.split('/').map(str::to_owned).collect::<Vec<_>>())
                .expect("a request id")
        };
        let (a, b) = (parts(&first), parts(&second));
        assert_eq!(a.first(), Some(&"agent".to_owned()));
        // Conversation and trajectory match; only the timestamp differs.
        assert_eq!(a.get(1), b.get(1));
        assert_eq!(a.get(3), b.get(3));
        // A different session is a different conversation.
        let other = parts(&envelope(&gemini_body(), "sess-other", "proj-1"));
        assert_ne!(a.get(1), other.get(1));
    }

    #[test]
    fn an_ide_supplied_request_id_is_preserved() {
        // It is the IDE's own id and identifies the conversation better than anything derived here.
        let supplied = "agent/11111111-2222-5333-8444-555555555555/1700000000000/66666666-7777-5888-8999-aaaaaaaaaaaa/3";
        let mut body = gemini_body();
        if let Some(object) = body.as_object_mut() {
            object.insert("requestId".to_owned(), json!(supplied));
        }
        let out = envelope(&body, "sess-1", "proj-1");
        assert_eq!(out.get("requestId"), Some(&json!(supplied)));

        // A malformed one is replaced rather than forwarded.
        if let Some(object) = body.as_object_mut() {
            object.insert("requestId".to_owned(), json!("not-an-ide-id"));
        }
        let replaced = envelope(&body, "sess-1", "proj-1");
        assert_ne!(replaced.get("requestId"), Some(&json!("not-an-ide-id")));
    }

    #[test]
    fn an_image_request_takes_a_different_envelope_and_cannot_stream() {
        assert!(is_image_model("gemini-3.1-flash-image"));
        assert!(!is_image_model("gemini-3-pro"));
        // Streaming is refused for image generation, so the blocking method is used either way.
        assert_eq!(
            url_suffix("gemini-3.1-flash-image", true),
            "/v1internal:generateContent"
        );
        assert_eq!(
            url_suffix("gemini-3-pro", true),
            "/v1internal:streamGenerateContent?alt=sse"
        );

        let out = envelope(
            &json!({
                "model": "gemini-3.1-flash-image-16x9",
                "request": {
                    "contents": [{ "role": "user", "parts": [
                        { "text": "a cat" },
                        { "functionCall": { "name": "ignored", "args": {} } },
                    ] }],
                    "tools": [{ "functionDeclarations": [{ "name": "a" }] }],
                },
            }),
            "sess-1",
            "proj-1",
        );
        assert_eq!(out.get("requestType"), Some(&json!("image_gen")));
        // The dimension suffix is this router's convention, not a model name Antigravity knows.
        assert_eq!(out.get("model"), Some(&json!("gemini-3.1-flash-image")));
        assert_eq!(
            out.pointer("/request/generationConfig/imageConfig/aspectRatio"),
            Some(&json!("16:9"))
        );
        // Text only, and no tools at all.
        let parts = out
            .pointer("/request/contents/0/parts")
            .and_then(Value::as_array)
            .expect("parts");
        assert_eq!(parts.len(), 1, "{parts:?}");
        assert!(out.pointer("/request/tools").is_none());
    }

    #[test]
    fn a_resolution_suffix_is_reduced_to_an_aspect_ratio() {
        assert_eq!(aspect_ratio("model-1024x768"), "4:3");
        assert_eq!(aspect_ratio("model-16x9"), "16:9");
        assert_eq!(aspect_ratio("model-with-no-suffix"), "1:1");
    }

    #[test]
    fn a_project_id_is_stable_per_session_and_shaped_like_the_ides() {
        let first = project_id("sess-p");
        assert_eq!(first, project_id("sess-p"));
        assert_ne!(first, project_id("sess-q"));
        let segments: Vec<&str> = first.split('-').collect();
        assert_eq!(segments.len(), 3, "{first}");
        assert!(!segments.first().unwrap_or(&"").is_empty());
    }
}
