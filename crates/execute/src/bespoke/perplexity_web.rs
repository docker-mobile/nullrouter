//! Request shaping for `perplexity-web`, the perplexity.ai web surface.
//!
//! Like `grok-web` this is the site's own endpoint rather than an API, so the request has to look like
//! the front end's. Three things make it stranger than grok's:
//!
//! * **The query is a JSON document in a string field.** `query_str` carries a serialised object with
//!   `instructions`, `history` and `query` keys. Perplexity has no notion of a system prompt or a
//!   message array, so the whole conversation is encoded into the one field it does have.
//! * **Follow-ups are server-side.** Perplexity keeps the conversation itself and returns a
//!   `backend_uuid`; sending that back as `last_backend_uuid` continues the thread, and then the query
//!   is *just the new message* rather than the whole document. So this module keeps a small cache
//!   mapping a conversation's history onto the uuid the site gave for it.
//! * **The credential can be either shape.** An access token goes in `Authorization`; a session cookie
//!   goes in `Cookie`. Upstream prefers the token, and so does this.
//!
//! The cache is the only stateful thing in `bespoke`. It is bounded in both size and age, because it is
//! keyed by user content and would otherwise be an unbounded map of conversation hashes held for the
//! life of the process.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Perplexity's own API version, sent both as a header and inside the params.
pub(crate) const API_VERSION: &str = "2.18";

/// The browser `User-Agent` the front end sends.
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

/// Perplexity's cookie name for a signed-in session.
const SESSION_COOKIE: &str = "__Secure-next-auth.session-token";

/// Longest query document sent. Upstream's cap.
///
/// The *tail* is kept when it overflows, not the head: the current question is at the end, and a
/// truncated document that lost the question would be answered as though it were never asked.
const MAX_QUERY: usize = 96_000;

/// How long a remembered conversation stays usable.
const SESSION_MAX_AGE: Duration = Duration::from_secs(3600);

/// How many conversations are remembered before the oldest is dropped.
const SESSION_MAX_ENTRIES: usize = 200;

/// One OpenAI model name mapped onto perplexity's mode plus model preference.
struct PplxModel {
    mode: &'static str,
    preference: &'static str,
}

/// The non-thinking table.
const MODELS: &[(&str, PplxModel)] = &[
    (
        "pplx-auto",
        PplxModel {
            mode: "concise",
            preference: "pplx_pro",
        },
    ),
    (
        "pplx-sonar",
        PplxModel {
            mode: "copilot",
            preference: "experimental",
        },
    ),
    (
        "pplx-gpt",
        PplxModel {
            mode: "copilot",
            preference: "gpt54",
        },
    ),
    (
        "pplx-gemini",
        PplxModel {
            mode: "copilot",
            preference: "gemini31pro_high",
        },
    ),
    (
        "pplx-sonnet",
        PplxModel {
            mode: "copilot",
            preference: "claude46sonnet",
        },
    ),
    (
        "pplx-opus",
        PplxModel {
            mode: "copilot",
            preference: "claude46opus",
        },
    ),
    (
        "pplx-nemotron",
        PplxModel {
            mode: "copilot",
            preference: "nv_nemotron_3_super",
        },
    ),
];

/// Models with a distinct reasoning preference, used when the request asks for thinking.
const THINKING: &[(&str, &str)] = &[
    ("pplx-gpt", "gpt54_thinking"),
    ("pplx-sonnet", "claude46sonnetthinking"),
    ("pplx-opus", "claude46opusthinking"),
];

/// The mode and preference for a model, honouring a thinking request.
///
/// An unmapped name is passed through as a raw preference on `copilot` rather than refused: perplexity
/// accepts preferences this table does not list, and refusing would block a model that works.
pub(crate) fn resolve_model(model: &str, thinking: bool) -> (String, String) {
    if thinking
        && let Some((_name, preference)) = THINKING.iter().find(|(name, _pref)| *name == model)
    {
        return ("copilot".to_owned(), (*preference).to_owned());
    }
    MODELS
        .iter()
        .find(|(name, _mapping)| *name == model)
        .map_or_else(
            || ("copilot".to_owned(), model.to_owned()),
            |(_name, mapping)| (mapping.mode.to_owned(), mapping.preference.to_owned()),
        )
}

/// Whether a request is asking for reasoning.
///
/// Upstream reads `thinking: true` or any `reasoning_effort` other than `none`. Both spellings reach
/// here because a client may send either.
pub(crate) fn wants_thinking(body: &Value) -> bool {
    if body.get("thinking").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    body.get("reasoning_effort")
        .and_then(Value::as_str)
        .is_some_and(|effort| effort != "none")
}

/// An OpenAI message array split into the three things perplexity's query document needs.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Conversation {
    /// Concatenated system content, which becomes an instruction.
    pub(crate) system: String,
    /// Prior user/assistant turns, excluding the final user message.
    pub(crate) history: Vec<(String, String)>,
    /// The final user message, which is the actual question.
    pub(crate) current: String,
}

/// Split a message array the way perplexity's document expects.
///
/// The final user turn is *removed* from the history and kept separately: it is the question, and the
/// document distinguishes it from context. A conversation ending on an assistant turn leaves `current`
/// empty, which upstream also allows.
pub(crate) fn parse_messages(messages: &[Value]) -> Conversation {
    let mut conversation = Conversation::default();
    for message in messages {
        let role = match message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user")
        {
            "developer" => "system",
            other => other,
        };
        let text = message_text(message.get("content"));
        if text.trim().is_empty() {
            continue;
        }
        match role {
            "system" => {
                conversation.system.push_str(&text);
                conversation.system.push('\n');
            }
            "user" | "assistant" => conversation.history.push((role.to_owned(), text)),
            // A tool result has no place in a surface with no tool protocol.
            _other => {}
        }
    }
    if conversation
        .history
        .last()
        .is_some_and(|(role, _text)| role == "user")
        && let Some((_role, text)) = conversation.history.pop()
    {
        conversation.current = text;
    }
    conversation
}

/// The text of one message's content, dropping non-text parts.
fn message_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        Some(_) | None => String::new(),
    }
}

/// A one-line-per-tool hint appended to the instructions.
///
/// Perplexity cannot call a tool, so the tools a client declared are described rather than offered.
/// Saying they exist and cannot be invoked is more useful than silently dropping them: a model that
/// knows it has no tools answers directly instead of describing a call it cannot make.
pub(crate) fn tools_hint(tools: Option<&Value>) -> String {
    let Some(tools) = tools
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
    else {
        return String::new();
    };
    let lines: Vec<String> = tools
        .iter()
        .map(|tool| {
            let function = tool.get("function").unwrap_or(tool);
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unnamed");
            let description = function
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect::<String>();
            format!("- {name}: {description}")
        })
        .collect();
    format!(
        "Available tools (reference only, cannot invoke):\n{}",
        lines.join("\n")
    )
}

/// Build the `query_str` field.
///
/// Two shapes, and which one applies is the whole point of the session cache: continuing a remembered
/// thread sends only the new message, because perplexity still holds the rest. A fresh thread sends the
/// serialised document.
pub(crate) fn build_query(
    conversation: &Conversation,
    follow_up: Option<&str>,
    tools: Option<&Value>,
) -> String {
    if follow_up.is_some() {
        return conversation.current.clone();
    }
    let mut instructions: Vec<String> = Vec::new();
    if !conversation.system.trim().is_empty() {
        instructions.push(conversation.system.trim().to_owned());
    }
    let hint = tools_hint(tools);
    if !hint.is_empty() {
        instructions.push(hint);
    }
    instructions.push(
        "You have built-in web search. Answer questions directly using search results.".to_owned(),
    );

    let mut document = serde_json::Map::new();
    document.insert("instructions".to_owned(), json!(instructions));
    if !conversation.history.is_empty() {
        document.insert(
            "history".to_owned(),
            Value::Array(
                conversation
                    .history
                    .iter()
                    .map(|(role, content)| json!({ "role": role, "content": content }))
                    .collect(),
            ),
        );
    }
    if !conversation.current.is_empty() || conversation.history.is_empty() {
        document.insert("query".to_owned(), json!(conversation.current));
    }

    let encoded = Value::Object(document).to_string();
    if encoded.len() <= MAX_QUERY {
        return encoded;
    }
    // The tail, not the head: the question is at the end. Sliced on a character boundary so a
    // multi-byte character cannot be split, which would make the field invalid UTF-8.
    let start = encoded
        .char_indices()
        .nth(encoded.chars().count().saturating_sub(MAX_QUERY))
        .map_or(0, |(index, _character)| index);
    encoded.get(start..).unwrap_or(&encoded).to_owned()
}

/// The request body perplexity's front end posts.
pub(crate) fn payload(query: &str, mode: &str, preference: &str, follow_up: Option<&str>) -> Value {
    json!({
        "query_str": query,
        "params": {
            "query_str": query,
            "search_focus": "internet",
            "mode": mode,
            "model_preference": preference,
            "sources": ["web"],
            "attachments": [],
            "frontend_uuid": super::session_id(),
            "frontend_context_uuid": super::session_id(),
            "version": API_VERSION,
            "language": "en-US",
            // Fixed rather than read from the host: the timezone is a fingerprint the front end sends,
            // and a router's own clock zone says nothing about the person asking.
            "timezone": "UTC",
            "search_recency_filter": Value::Null,
            // Keeps routed traffic out of the account's saved threads.
            "is_incognito": true,
            "use_schematized_api": true,
            "last_backend_uuid": follow_up.map_or(Value::Null, |uuid| json!(uuid)),
        },
    })
}

/// The headers perplexity's front end sends.
pub(crate) fn headers() -> Vec<(String, String)> {
    vec![
        ("Content-Type".to_owned(), "application/json".to_owned()),
        ("Accept".to_owned(), "text/event-stream".to_owned()),
        ("Origin".to_owned(), "https://www.perplexity.ai".to_owned()),
        (
            "Referer".to_owned(),
            "https://www.perplexity.ai/".to_owned(),
        ),
        ("User-Agent".to_owned(), USER_AGENT.to_owned()),
        ("X-App-ApiClient".to_owned(), "default".to_owned()),
        ("X-App-ApiVersion".to_owned(), API_VERSION.to_owned()),
    ]
}

/// The cookie header for a pasted session token.
pub(crate) fn cookie_header(token: &str) -> String {
    let token = token.trim();
    format!(
        "{SESSION_COOKIE}={}",
        token
            .strip_prefix(&format!("{SESSION_COOKIE}="))
            .unwrap_or(token)
    )
}

/// FNV-1a over the conversation, as upstream keys its cache.
///
/// Reproduced exactly rather than replaced with a stronger hash: the key only has to agree with itself
/// across two requests in one process, and matching upstream keeps the behaviour comparable. It is not
/// a security boundary — a collision continues the wrong thread, which is why the stored uuid is also
/// checked against its age.
pub(crate) fn session_key(history: &[(String, String)]) -> String {
    let joined = history
        .iter()
        .map(|(role, content)| format!("{role}:{content}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut hash: u32 = 0x811c_9dc5;
    // Upstream hashes UTF-16 code units, since it reads `charCodeAt`. Matching that means encoding to
    // UTF-16 here rather than hashing bytes, or the same conversation would key differently.
    for unit in joined.encode_utf16() {
        hash ^= u32::from(unit);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

/// One remembered conversation.
struct Session {
    backend_uuid: String,
    stored: Instant,
}

fn cache() -> &'static Mutex<HashMap<String, Session>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The uuid perplexity gave for this conversation, if it is still fresh.
pub(crate) fn session_lookup(history: &[(String, String)]) -> Option<String> {
    if history.is_empty() {
        return None;
    }
    let key = session_key(history);
    let mut cache = cache().lock().ok()?;
    let entry = cache.get(&key)?;
    if entry.stored.elapsed() > SESSION_MAX_AGE {
        cache.remove(&key);
        return None;
    }
    Some(entry.backend_uuid.clone())
}

/// Remember the uuid for the conversation as it stands after this exchange.
///
/// Keyed by the *whole* exchange including the answer, because that is what the next request's history
/// will hash to.
pub(crate) fn session_store(
    history: &[(String, String)],
    current: &str,
    answer: &str,
    backend_uuid: &str,
) {
    if backend_uuid.is_empty() {
        return;
    }
    let mut full = history.to_vec();
    full.push(("user".to_owned(), current.to_owned()));
    full.push(("assistant".to_owned(), answer.to_owned()));
    let key = session_key(&full);

    let Ok(mut cache) = cache().lock() else {
        return;
    };
    cache.insert(
        key,
        Session {
            backend_uuid: backend_uuid.to_owned(),
            stored: Instant::now(),
        },
    );
    // Bounded: this map is keyed by user content and would otherwise grow for the life of the process.
    if cache.len() > SESSION_MAX_ENTRIES
        && let Some(oldest) = cache
            .iter()
            .min_by_key(|(_key, session)| session.stored)
            .map(|(key, _session)| key.clone())
    {
        cache.remove(&oldest);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        Conversation, build_query, cookie_header, parse_messages, payload, resolve_model,
        session_key, session_lookup, session_store, tools_hint, wants_thinking,
    };

    #[test]
    fn a_thinking_request_selects_the_reasoning_preference() {
        // The mode stays `copilot`; the preference is what changes. Reading only the mode would send a
        // thinking request as an ordinary one.
        assert_eq!(
            resolve_model("pplx-sonnet", true),
            ("copilot".to_owned(), "claude46sonnetthinking".to_owned())
        );
        assert_eq!(
            resolve_model("pplx-sonnet", false),
            ("copilot".to_owned(), "claude46sonnet".to_owned())
        );
    }

    #[test]
    fn a_model_with_no_thinking_variant_keeps_its_ordinary_preference() {
        // `pplx-auto` has no reasoning form. Asking for thinking must not invent one.
        assert_eq!(
            resolve_model("pplx-auto", true),
            ("concise".to_owned(), "pplx_pro".to_owned())
        );
    }

    #[test]
    fn an_unmapped_model_is_passed_through_as_a_raw_preference() {
        // Perplexity accepts preferences this table does not list.
        assert_eq!(
            resolve_model("some_new_backend_id", false),
            ("copilot".to_owned(), "some_new_backend_id".to_owned())
        );
    }

    #[test]
    fn either_spelling_of_a_thinking_request_is_recognised() {
        assert!(wants_thinking(&json!({ "thinking": true })));
        assert!(wants_thinking(&json!({ "reasoning_effort": "high" })));
        assert!(!wants_thinking(&json!({ "reasoning_effort": "none" })));
        assert!(!wants_thinking(&json!({})));
    }

    #[test]
    fn the_final_user_turn_is_separated_from_the_history() {
        // The document distinguishes the question from its context, so the last user turn cannot stay
        // in `history` — it would be answered as though it were background.
        let parsed = parse_messages(&[
            json!({ "role": "system", "content": "Be terse." }),
            json!({ "role": "user", "content": "First" }),
            json!({ "role": "assistant", "content": "Sure" }),
            json!({ "role": "user", "content": "Second" }),
        ]);
        assert_eq!(parsed.system, "Be terse.\n");
        assert_eq!(
            parsed.history,
            vec![
                ("user".to_owned(), "First".to_owned()),
                ("assistant".to_owned(), "Sure".to_owned()),
            ]
        );
        assert_eq!(parsed.current, "Second");
    }

    #[test]
    fn a_conversation_ending_on_an_assistant_turn_has_no_current_question() {
        let parsed = parse_messages(&[
            json!({ "role": "user", "content": "Hi" }),
            json!({ "role": "assistant", "content": "Hello" }),
        ]);
        assert!(parsed.current.is_empty());
        assert_eq!(parsed.history.len(), 2);
    }

    #[test]
    fn the_query_is_a_json_document_carrying_instructions_and_history() {
        // Perplexity has one string field and no notion of a system prompt, so the conversation is
        // encoded into it. A bare question would silently drop the system instruction.
        let conversation = Conversation {
            system: "Be terse.\n".to_owned(),
            history: vec![("user".to_owned(), "First".to_owned())],
            current: "Second".to_owned(),
        };
        let query = build_query(&conversation, None, None);
        let parsed: serde_json::Value = serde_json::from_str(&query).expect("a JSON document");

        assert_eq!(parsed.pointer("/instructions/0"), Some(&json!("Be terse.")));
        // The built-in-search instruction is always last.
        assert!(
            parsed
                .pointer("/instructions/1")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| text.contains("built-in web search")),
            "{parsed}"
        );
        assert_eq!(parsed.pointer("/history/0/content"), Some(&json!("First")));
        assert_eq!(parsed.get("query"), Some(&json!("Second")));
    }

    #[test]
    fn continuing_a_remembered_thread_sends_only_the_new_message() {
        // The whole point of the session cache: perplexity still holds the thread, so resending it
        // would duplicate the context it already has.
        let conversation = Conversation {
            system: "Be terse.".to_owned(),
            history: vec![("user".to_owned(), "First".to_owned())],
            current: "Second".to_owned(),
        };
        let query = build_query(&conversation, Some("uuid-1234"), None);
        assert_eq!(query, "Second");
    }

    #[test]
    fn declared_tools_are_described_rather_than_dropped() {
        // Perplexity cannot call a tool. A model told they exist answers directly instead of
        // describing a call it cannot make.
        let tools = json!([
            { "function": { "name": "get_weather", "description": "Current weather\nsecond line" } },
        ]);
        let hint = tools_hint(Some(&tools));
        assert!(hint.contains("cannot invoke"), "{hint}");
        assert!(hint.contains("- get_weather: Current weather"), "{hint}");
        // Only the first line of a description.
        assert!(!hint.contains("second line"), "{hint}");
        assert!(tools_hint(None).is_empty());
        assert!(tools_hint(Some(&json!([]))).is_empty());
    }

    #[test]
    fn the_payload_keeps_the_thread_incognito_and_carries_the_follow_up_uuid() {
        let fresh = payload("q", "copilot", "gpt54", None);
        assert_eq!(fresh.pointer("/params/is_incognito"), Some(&json!(true)));
        assert_eq!(
            fresh.pointer("/params/last_backend_uuid"),
            Some(&json!(null))
        );
        // The query appears in both places the front end puts it.
        assert_eq!(fresh.get("query_str"), Some(&json!("q")));
        assert_eq!(fresh.pointer("/params/query_str"), Some(&json!("q")));

        let continued = payload("q", "copilot", "gpt54", Some("uuid-1"));
        assert_eq!(
            continued.pointer("/params/last_backend_uuid"),
            Some(&json!("uuid-1"))
        );
    }

    #[test]
    fn each_request_gets_its_own_frontend_uuids() {
        let first = payload("q", "copilot", "gpt54", None);
        let second = payload("q", "copilot", "gpt54", None);
        assert_ne!(
            first.pointer("/params/frontend_uuid"),
            second.pointer("/params/frontend_uuid")
        );
    }

    #[test]
    fn a_pasted_cookie_name_is_not_doubled() {
        assert_eq!(cookie_header("abc"), "__Secure-next-auth.session-token=abc");
        assert_eq!(
            cookie_header("__Secure-next-auth.session-token=abc"),
            "__Secure-next-auth.session-token=abc"
        );
    }

    #[test]
    fn the_session_key_hashes_utf16_units_as_upstream_does() {
        // Upstream reads `charCodeAt`, which is UTF-16. Hashing bytes instead would key the same
        // conversation differently and silently lose every follow-up containing a non-ASCII character.
        let history = vec![("user".to_owned(), "héllo".to_owned())];
        let key = session_key(&history);
        assert_eq!(key.len(), 8, "eight hex characters: {key}");
        // Stable across calls, and distinct from a different conversation.
        assert_eq!(key, session_key(&history));
        assert_ne!(key, session_key(&[("user".to_owned(), "hello".to_owned())]));
    }

    #[test]
    fn a_stored_thread_is_found_by_the_history_the_next_request_will_send() {
        // The key covers the whole exchange including the answer, because that is what the client's
        // next `messages` array will contain.
        let history = vec![("user".to_owned(), "Q1".to_owned())];
        session_store(&history, "Q2", "A2", "backend-uuid-9");

        let next_request_history = vec![
            ("user".to_owned(), "Q1".to_owned()),
            ("user".to_owned(), "Q2".to_owned()),
            ("assistant".to_owned(), "A2".to_owned()),
        ];
        assert_eq!(
            session_lookup(&next_request_history),
            Some("backend-uuid-9".to_owned())
        );
    }

    #[test]
    fn an_empty_history_is_never_a_follow_up() {
        assert_eq!(session_lookup(&[]), None);
    }

    #[test]
    fn an_empty_uuid_is_not_stored() {
        let history = vec![("user".to_owned(), "unique-never-stored".to_owned())];
        session_store(&history, "x", "y", "");
        let full = vec![
            ("user".to_owned(), "unique-never-stored".to_owned()),
            ("user".to_owned(), "x".to_owned()),
            ("assistant".to_owned(), "y".to_owned()),
        ];
        assert_eq!(session_lookup(&full), None);
    }
}
