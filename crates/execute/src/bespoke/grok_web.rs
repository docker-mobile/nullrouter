//! Request shaping for `grok-web`, the grok.com web surface.
//!
//! This is not an API. It is the endpoint grok.com's own front end calls, so the request has to look
//! like that front end: a browser `User-Agent`, the site's `Origin` and `Referer`, and a payload of
//! flags the web app sends whether or not they matter. None of it is negotiable — the endpoint rejects
//! a request that does not look like its own client.
//!
//! Two consequences worth stating, because they shape everything below:
//!
//! * **The credential is a session cookie**, not a bearer token. It goes in `Cookie: sso=…`.
//! * **The conversation is one string.** grok.com takes a single `message`, not a message array, so a
//!   multi-turn OpenAI request is flattened with role prefixes. That is upstream's behaviour and it is
//!   lossy by nature; the alternative is dropping every turn but the last.

use serde_json::{Value, json};

/// How one OpenAI model name maps onto grok.com's model plus mode.
///
/// `thinking` decides whether reasoning deltas are expected, which the response side needs in order to
/// route them to `reasoning_content` rather than into the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GrokModel {
    /// The `modelName` field grok.com wants.
    pub(crate) name: &'static str,
    /// The `modelMode` enum value.
    pub(crate) mode: &'static str,
    /// Whether this mode streams reasoning before its answer.
    pub(crate) thinking: bool,
}

/// The model table, as upstream has it.
///
/// Note that several OpenAI-facing names collapse onto one `modelName` and differ only by mode:
/// `grok-3-mini` and `grok-3-thinking` are both `grok-3`. The mode is the load-bearing half.
const MODELS: &[(&str, GrokModel)] = &[
    (
        "grok-3",
        GrokModel {
            name: "grok-3",
            mode: "MODEL_MODE_GROK_3",
            thinking: false,
        },
    ),
    (
        "grok-3-mini",
        GrokModel {
            name: "grok-3",
            mode: "MODEL_MODE_GROK_3_MINI_THINKING",
            thinking: true,
        },
    ),
    (
        "grok-3-thinking",
        GrokModel {
            name: "grok-3",
            mode: "MODEL_MODE_GROK_3_THINKING",
            thinking: true,
        },
    ),
    (
        "grok-4",
        GrokModel {
            name: "grok-4",
            mode: "MODEL_MODE_GROK_4",
            thinking: false,
        },
    ),
    (
        "grok-4-mini",
        GrokModel {
            name: "grok-4-mini",
            mode: "MODEL_MODE_GROK_4_MINI_THINKING",
            thinking: true,
        },
    ),
    (
        "grok-4-thinking",
        GrokModel {
            name: "grok-4",
            mode: "MODEL_MODE_GROK_4_THINKING",
            thinking: true,
        },
    ),
    (
        "grok-4-heavy",
        GrokModel {
            name: "grok-4",
            mode: "MODEL_MODE_HEAVY",
            thinking: true,
        },
    ),
    (
        "grok-4.1-mini",
        GrokModel {
            name: "grok-4-1-thinking-1129",
            mode: "MODEL_MODE_GROK_4_1_MINI_THINKING",
            thinking: true,
        },
    ),
    (
        "grok-4.1-fast",
        GrokModel {
            name: "grok-4-1-thinking-1129",
            mode: "MODEL_MODE_FAST",
            thinking: false,
        },
    ),
    (
        "grok-4.1-expert",
        GrokModel {
            name: "grok-4-1-thinking-1129",
            mode: "MODEL_MODE_EXPERT",
            thinking: true,
        },
    ),
    (
        "grok-4.1-thinking",
        GrokModel {
            name: "grok-4-1-thinking-1129",
            mode: "MODEL_MODE_GROK_4_1_THINKING",
            thinking: true,
        },
    ),
    (
        "grok-4.2",
        GrokModel {
            name: "grok-420",
            mode: "MODEL_MODE_GROK_420",
            thinking: false,
        },
    ),
    (
        "grok-4.20",
        GrokModel {
            name: "grok-420",
            mode: "MODEL_MODE_GROK_420",
            thinking: false,
        },
    ),
    (
        "grok-4.20-beta",
        GrokModel {
            name: "grok-420",
            mode: "MODEL_MODE_GROK_420",
            thinking: false,
        },
    ),
];

/// What an unmapped model falls back to. Upstream's default, and it logs rather than refusing.
const DEFAULT_MODEL: &str = "grok-4.1-fast";

/// The mapping for a model name, falling back rather than refusing.
///
/// A name this table does not know is more likely a new grok model than a mistake, and answering it on
/// the default mode is what upstream does. Refusing would turn a working account into a dead one every
/// time xAI ships a name before this table learns it.
pub(crate) fn model_for(model: &str) -> GrokModel {
    MODELS
        .iter()
        .find(|(name, _mapping)| *name == model)
        .or_else(|| {
            MODELS
                .iter()
                .find(|(name, _mapping)| *name == DEFAULT_MODEL)
        })
        .map_or(
            GrokModel {
                name: "grok-4-1-thinking-1129",
                mode: "MODEL_MODE_FAST",
                thinking: false,
            },
            |(_name, mapping)| *mapping,
        )
}

/// Flatten an OpenAI message array into the single string grok.com accepts.
///
/// Every turn but the last user message is prefixed with its role, which is how the web app conveys
/// who said what through a field that has no structure for it. The last user message is left bare
/// because it is the actual prompt; prefixing it would put `user:` in front of the question.
///
/// `developer` is folded into `system`: grok.com knows nothing of the newer role, and dropping it
/// would silently discard an instruction.
pub(crate) fn flatten_messages(messages: &[Value]) -> String {
    let extracted: Vec<(String, String)> = messages
        .iter()
        .filter_map(|message| {
            let role = match message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user")
            {
                "developer" => "system",
                other => other,
            };
            let text = message_text(message.get("content"));
            (!text.trim().is_empty()).then(|| (role.to_owned(), text))
        })
        .collect();

    let last_user = extracted.iter().rposition(|(role, _text)| role == "user");

    extracted
        .iter()
        .enumerate()
        .map(|(index, (role, text))| {
            if Some(index) == last_user {
                text.clone()
            } else {
                format!("{role}: {text}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The text of one message's content, whether it is a string or a content-part array.
///
/// Non-text parts are dropped: grok.com's `message` is a string, so an image part has nowhere to go.
/// Upstream joins the text parts with a space rather than a newline, which is preserved.
fn message_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        // A number, object, or absent content has no text to send.
        Some(_) | None => String::new(),
    }
}

/// The payload grok.com's front end sends.
///
/// Most of these flags never vary, and they are sent because the endpoint's own client sends them —
/// a request missing them is a request that does not look like grok.com. `temporary: true` is the one
/// with a visible effect: it keeps the exchange out of the account's saved history.
pub(crate) fn payload(model: &GrokModel, message: &str) -> Value {
    json!({
        "temporary": true,
        "modelName": model.name,
        "modelMode": model.mode,
        "message": message,
        "fileAttachments": [],
        "imageAttachments": [],
        "disableSearch": false,
        "enableImageGeneration": false,
        "returnImageBytes": false,
        "returnRawGrokInXaiRequest": false,
        "enableImageStreaming": false,
        "imageGenerationCount": 0,
        "forceConcise": false,
        "toolOverrides": {},
        "enableSideBySide": true,
        "sendFinalMetadata": true,
        "isReasoning": false,
        "disableTextFollowUps": false,
        "disableMemory": true,
        "forceSideBySide": false,
        "isAsyncChat": false,
        "disableSelfHarmShortCircuit": false,
        // The web app reports its viewport. Fixed values rather than random ones: these are not a
        // fingerprint this port is trying to vary, and a plausible desktop size is what it sends.
        "deviceEnvInfo": {
            "darkModeEnabled": false,
            "devicePixelRatio": 2,
            "screenWidth": 2056,
            "screenHeight": 1329,
            "viewportWidth": 2056,
            "viewportHeight": 1083,
        },
    })
}

/// The browser `User-Agent` grok.com's front end sends.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36";

/// The headers grok.com's own client sends on this endpoint.
///
/// These are not decoration. The endpoint is the web app's, and it refuses a request that does not
/// carry the shape of one — `Origin`, `Referer` and a browser `User-Agent` in particular. The
/// `Sec-Ch-*` hints and the sentry `Baggage` string are sent because the front end sends them.
///
/// Two are per-request and generated here: `x-xai-request-id` and `traceparent`. A fixed value in
/// either would make every routed request look like one retried call in xAI's own tracing.
pub(crate) fn headers(token: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Accept".to_owned(), "*/*".to_owned()),
        ("Accept-Language".to_owned(), "en-US,en;q=0.9".to_owned()),
        ("Cache-Control".to_owned(), "no-cache".to_owned()),
        ("Content-Type".to_owned(), "application/json".to_owned()),
        ("Origin".to_owned(), "https://grok.com".to_owned()),
        ("Pragma".to_owned(), "no-cache".to_owned()),
        ("Referer".to_owned(), "https://grok.com/".to_owned()),
        (
            "Sec-Ch-Ua".to_owned(),
            "\"Google Chrome\";v=\"136\", \"Chromium\";v=\"136\", \"Not(A:Brand\";v=\"24\""
                .to_owned(),
        ),
        ("Sec-Ch-Ua-Mobile".to_owned(), "?0".to_owned()),
        ("Sec-Ch-Ua-Platform".to_owned(), "\"macOS\"".to_owned()),
        ("Sec-Fetch-Dest".to_owned(), "empty".to_owned()),
        ("Sec-Fetch-Mode".to_owned(), "cors".to_owned()),
        ("Sec-Fetch-Site".to_owned(), "same-origin".to_owned()),
        ("User-Agent".to_owned(), USER_AGENT.to_owned()),
        ("x-xai-request-id".to_owned(), super::session_id()),
        ("traceparent".to_owned(), traceparent()),
    ];
    // `Accept-Encoding` is deliberately not sent: reqwest sets it from the features it was built with,
    // and claiming `zstd` support this client does not have would produce a body it cannot decode.
    if let Some(token) = token.filter(|token| !token.trim().is_empty()) {
        headers.push(("Cookie".to_owned(), cookie_header(token)));
    }
    headers
}

/// A W3C `traceparent` with a fresh trace and span id.
fn traceparent() -> String {
    // Reuses the UUID generator's entropy rather than adding a second source: the hex is taken from
    // two independent ids, so a collision would need both to repeat.
    let trace = super::session_id().replace('-', "");
    let span = super::session_id().replace('-', "");
    // A UUID's hex is 32 characters, so both slices are present; the fallbacks exist so a shorter id
    // could never produce a malformed header rather than because it is expected.
    format!(
        "00-{}-{}-00",
        trace
            .get(..32)
            .unwrap_or("00000000000000000000000000000000"),
        span.get(..16).unwrap_or("0000000000000000"),
    )
}

/// The session cookie, with the prefix a user may have pasted along with it.
///
/// People copy `sso=abc…` out of devtools wholesale. Accepting both spellings costs one `strip_prefix`
/// and saves an import that would otherwise fail with an authentication error rather than a hint.
pub(crate) fn cookie_header(token: &str) -> String {
    let token = token.trim();
    format!("sso={}", token.strip_prefix("sso=").unwrap_or(token))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{cookie_header, flatten_messages, model_for, payload};

    #[test]
    fn a_thinking_mode_is_distinguished_from_its_base_model() {
        // Several names collapse onto one `modelName` and differ only by mode, so the mode is what
        // carries the request's meaning. Reading only the name would send every grok-3 variant as the
        // non-thinking one.
        let mini = model_for("grok-3-mini");
        let base = model_for("grok-3");
        assert_eq!(mini.name, base.name);
        assert_ne!(mini.mode, base.mode);
        assert!(mini.thinking);
        assert!(!base.thinking);
    }

    #[test]
    fn an_unknown_model_falls_back_rather_than_refusing() {
        // xAI ships names faster than a table learns them. A fallback answers; a refusal turns a
        // working account into a dead one until this file is edited.
        let unknown = model_for("grok-9-preview-does-not-exist");
        assert_eq!(unknown, model_for("grok-4.1-fast"));
    }

    #[test]
    fn only_the_last_user_turn_is_left_unprefixed() {
        // The prefixes are how a multi-turn conversation survives a single-string field. The final
        // user message is the actual prompt, so prefixing it would put "user:" before the question.
        let flattened = flatten_messages(&[
            json!({ "role": "system", "content": "Be brief." }),
            json!({ "role": "user", "content": "First?" }),
            json!({ "role": "assistant", "content": "Yes." }),
            json!({ "role": "user", "content": "And now?" }),
        ]);
        assert_eq!(
            flattened,
            "system: Be brief.\n\nuser: First?\n\nassistant: Yes.\n\nAnd now?"
        );
    }

    #[test]
    fn a_developer_role_is_folded_into_system() {
        // grok.com knows nothing of the newer role. Dropping it would silently discard an instruction.
        let flattened = flatten_messages(&[
            json!({ "role": "developer", "content": "Rules." }),
            json!({ "role": "user", "content": "Go" }),
        ]);
        assert!(flattened.starts_with("system: Rules."), "{flattened}");
    }

    #[test]
    fn content_parts_are_reduced_to_their_text() {
        // `message` is a string, so an image part has nowhere to go and is dropped rather than
        // stringified into the prompt.
        let flattened = flatten_messages(&[json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "describe" },
                { "type": "image_url", "image_url": { "url": "https://example.test/a.png" } },
                { "type": "text", "text": "this" },
            ],
        })]);
        assert_eq!(flattened, "describe this");
    }

    #[test]
    fn an_empty_turn_is_dropped_entirely() {
        // A blank message would become a bare "assistant:" line — a turn that says nothing, in a
        // format where every line costs prompt budget.
        let flattened = flatten_messages(&[
            json!({ "role": "assistant", "content": "   " }),
            json!({ "role": "user", "content": "only this" }),
        ]);
        assert_eq!(flattened, "only this");
    }

    #[test]
    fn the_payload_keeps_the_exchange_out_of_saved_history() {
        // `temporary` is the one flag here with a visible account-side effect: without it every routed
        // request would appear in the user's grok.com history.
        let body = payload(&model_for("grok-4"), "hello");
        assert_eq!(body.get("temporary"), Some(&json!(true)));
        assert_eq!(body.get("modelName"), Some(&json!("grok-4")));
        assert_eq!(body.get("message"), Some(&json!("hello")));
        assert_eq!(body.get("disableMemory"), Some(&json!(true)));
    }

    #[test]
    fn a_pasted_cookie_prefix_is_accepted_once_not_twice() {
        // People copy `sso=…` out of devtools wholesale. Both spellings have to reach the same header,
        // and the prefix must not end up doubled.
        assert_eq!(cookie_header("abc123"), "sso=abc123");
        assert_eq!(cookie_header("sso=abc123"), "sso=abc123");
        assert_eq!(cookie_header("  sso=abc123  "), "sso=abc123");
    }
}
