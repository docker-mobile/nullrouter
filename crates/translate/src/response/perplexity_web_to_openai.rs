//! perplexity.ai SSE responses -> OpenAI `chat.completion.chunk` stream.
//!
//! Ports the response half of `open-sse/executors/perplexity-web.js`.
//!
//! Perplexity does not stream deltas. Each event carries the answer *as it stands*, so the delta a
//! client needs is the part beyond what was already sent — tracked here as a high-water mark. Sending
//! each event's text as-is would repeat the whole answer once per event.
//!
//! Events are structured as `blocks`, routed by `intended_usage`:
//!
//! * `markdown_block` — the answer. Its `chunks` are joined, and `progress: "DONE"` means the joined
//!   text is the final full answer rather than an increment.
//! * `pro_search_steps` — the searches and page reads it performed. Surfaced as reasoning, which is
//!   what they are: visible work leading to the answer.
//! * `plan` — the goals it set itself. Also reasoning.
//!
//! Perplexity also embeds citation markers (`[1]`, `[2]`) and occasionally raw XML or `<grok:…>` tags
//! in its markdown. Those are stripped, because a client rendering the text shows them verbatim and
//! they refer to a citation list this surface never returns.

use serde_json::{Value, json};

use crate::concerns::{ChunkMeta, build_chunk};
use crate::state::StreamState;

fn chunk_meta(state: &StreamState) -> ChunkMeta {
    ChunkMeta {
        id: state
            .message_id
            .clone()
            .unwrap_or_else(|| format!("chatcmpl-pplx-{}", state.clock.now_millis())),
        created: state
            .pplx_created
            .unwrap_or_else(|| state.clock.now_seconds()),
        model: state.model.clone().unwrap_or_default(),
    }
}

/// Strip perplexity's own markup from answer text.
///
/// `collapse` is applied only to a finished answer: doing it to a mid-stream delta would rewrite
/// whitespace that the next delta continues from, and the high-water mark would then be measuring
/// different text than was sent.
pub fn clean(text: &str, collapse: bool) -> String {
    let mut cleaned = strip_xml_declarations(text);
    cleaned = strip_grok_tags(&cleaned);
    cleaned = strip_response_tags(&cleaned);
    cleaned = strip_citations(&cleaned);
    if collapse {
        cleaned = collapse_whitespace(&cleaned);
        return cleaned.trim().to_owned();
    }
    cleaned
}

/// Remove `<?xml … ?>` declarations.
fn strip_xml_declarations(text: &str) -> String {
    strip_between(text, "<?xml", "?>")
}

/// Remove `<grok:…>…</grok:…>` pairs and `<grok:… />` singletons.
fn strip_grok_tags(text: &str) -> String {
    // Paired form first: removing the opening tag alone would leave its content and closing tag.
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<grok:") {
        out.push_str(rest.get(..start).unwrap_or_default());
        let after = rest.get(start..).unwrap_or_default();
        // A paired tag closes with `</grok:…>`; a self-closing one ends at the first `/>`.
        if let Some(close) = after.find("</grok:") {
            let end = after
                .get(close..)
                .and_then(|tail| tail.find('>'))
                .map_or(after.len(), |offset| close + offset + 1);
            rest = after.get(end..).unwrap_or_default();
        } else if let Some(end) = after.find("/>") {
            rest = after.get(end + 2..).unwrap_or_default();
        } else if let Some(end) = after.find('>') {
            rest = after.get(end + 1..).unwrap_or_default();
        } else {
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// Remove `<response>` and `</response>` wrappers, whatever their case.
fn strip_response_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some(open) = rest.find('<') else {
            out.push_str(rest);
            return out;
        };
        let after = rest.get(open..).unwrap_or_default();
        let Some(close) = after.find('>') else {
            out.push_str(rest);
            return out;
        };
        let tag = after.get(..=close).unwrap_or_default();
        let inner = tag
            .trim_start_matches('<')
            .trim_end_matches('>')
            .trim_start_matches('/')
            .trim();
        let is_response = inner
            .split_whitespace()
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("response"));
        out.push_str(rest.get(..open).unwrap_or_default());
        if !is_response {
            out.push_str(tag);
        }
        rest = after.get(close + 1..).unwrap_or_default();
    }
}

/// Remove `[12]`-style citation markers.
///
/// Only all-digit brackets: `[note]` and a markdown link's `[text](url)` must survive, since those are
/// the user's or the model's own prose rather than perplexity's citation apparatus.
fn strip_citations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = rest.get(open..).unwrap_or_default();
        let Some(close) = after.find(']') else {
            break;
        };
        let inner = after.get(1..close).unwrap_or_default();
        out.push_str(rest.get(..open).unwrap_or_default());
        if !inner.is_empty() && inner.chars().all(|character| character.is_ascii_digit()) {
            // A citation marker: dropped.
        } else {
            out.push_str(after.get(..=close).unwrap_or_default());
        }
        rest = after.get(close + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// Remove everything between a literal opener and its closer, repeatedly.
fn strip_between(text: &str, opener: &str, closer: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(opener) {
        out.push_str(rest.get(..start).unwrap_or_default());
        let after = rest.get(start..).unwrap_or_default();
        match after.find(closer) {
            Some(end) => rest = after.get(end + closer.len()..).unwrap_or_default(),
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Collapse runs of spaces to one and runs of blank lines to one blank line.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut spaces = 0_usize;
    let mut newlines = 0_usize;
    for character in text.chars() {
        match character {
            ' ' => {
                spaces += 1;
                newlines = 0;
                if spaces == 1 {
                    out.push(' ');
                }
            }
            '\n' => {
                newlines += 1;
                spaces = 0;
                if newlines <= 2 {
                    out.push('\n');
                }
            }
            other => {
                spaces = 0;
                newlines = 0;
                out.push(other);
            }
        }
    }
    out
}

/// Translate one perplexity SSE event into zero or more OpenAI chunks.
pub fn translate(raw: &Value, state: &mut StreamState) -> Vec<Value> {
    if !raw.is_object() {
        return Vec::new();
    }
    if state.message_id.is_none() {
        state.message_id = Some(format!("chatcmpl-pplx-{}", state.clock.now_millis()));
        state.pplx_created = Some(state.clock.now_seconds());
    }

    // An error replaces the answer. Surfaced as content: a stream that just stops looks like a
    // truncated reply.
    if raw.get("error_code").is_some() || raw.get("error_message").is_some() {
        let message = raw
            .get("error_message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                raw.get("error_code")
                    .map(|code| format!("Perplexity error: {code}"))
            })
            .unwrap_or_else(|| "Perplexity error".to_owned());
        state.finish_reason = Some("stop".to_owned());
        return vec![build_chunk(
            &chunk_meta(state),
            json!({ "content": format!("[Error: {message}]") }),
            Some("stop"),
        )];
    }

    // The thread id, kept so the executor can remember it for a follow-up.
    if let Some(uuid) = raw
        .get("backend_uuid")
        .and_then(Value::as_str)
        .filter(|uuid| !uuid.is_empty())
    {
        state.pplx_backend_uuid = Some(uuid.to_owned());
    }

    let mut chunks = Vec::new();
    let blocks = raw
        .get("blocks")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);

    for block in blocks {
        let usage = block
            .get("intended_usage")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match usage {
            "pro_search_steps" => chunks.extend(search_steps(block, state)),
            "plan" => chunks.extend(plan_goals(block, state)),
            // Any usage naming markdown is answer text; perplexity has used more than one spelling.
            _ if usage.contains("markdown") => chunks.extend(answer(block, state)),
            _other => {}
        }
    }

    // An event with no blocks may still carry plain text.
    if blocks.is_empty()
        && let Some(text) = raw.get("text").and_then(Value::as_str)
        && let Some(chunk) = advance(text.trim(), state)
    {
        chunks.push(chunk);
    }
    chunks
}

/// The searches and reads perplexity performed, as reasoning.
fn search_steps(block: &Value, state: &mut StreamState) -> Vec<Value> {
    let steps = block
        .pointer("/plan_block/steps")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut out = Vec::new();
    for step in steps {
        match step.get("step_type").and_then(Value::as_str) {
            Some("SEARCH_WEB") => {
                let queries = step
                    .pointer("/search_web_content/queries")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice);
                for query in queries {
                    if let Some(text) = query.get("query").and_then(Value::as_str)
                        && let Some(chunk) = reasoning(&format!("Searching: {text}"), state)
                    {
                        out.push(chunk);
                    }
                }
            }
            Some("READ_RESULTS") => {
                let urls = step
                    .pointer("/read_results_content/urls")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice);
                // Three, as upstream: a search can read dozens and the rest is noise in a reasoning
                // trace a person reads.
                for url in urls.iter().take(3) {
                    if let Some(text) = url.as_str()
                        && let Some(chunk) = reasoning(&format!("Reading: {text}"), state)
                    {
                        out.push(chunk);
                    }
                }
            }
            Some(_) | None => {}
        }
    }
    out
}

/// The goals perplexity set itself, as reasoning.
fn plan_goals(block: &Value, state: &mut StreamState) -> Vec<Value> {
    block
        .pointer("/plan_block/goals")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|goal| goal.get("description").and_then(Value::as_str))
        .filter_map(|description| reasoning(description, state))
        .collect()
}

/// Answer text from a markdown block.
fn answer(block: &Value, state: &mut StreamState) -> Vec<Value> {
    let Some(markdown) = block.get("markdown_block") else {
        return Vec::new();
    };
    let chunks = markdown
        .get("chunks")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if chunks.is_empty() {
        return Vec::new();
    }
    let joined: String = chunks.iter().filter_map(Value::as_str).collect();
    advance(&joined, state).map_or_else(Vec::new, |chunk| vec![chunk])
}

/// Emit only the part of `full` beyond what has already been sent.
///
/// Perplexity re-sends the whole answer on every event, so this high-water mark is what turns that into
/// a delta stream. Comparing lengths rather than diffing is upstream's approach and is safe because the
/// answer only ever grows.
fn advance(full: &str, state: &mut StreamState) -> Option<Value> {
    if full.len() <= state.pplx_seen {
        return None;
    }
    let delta = full.get(state.pplx_seen..)?.to_owned();
    state.pplx_seen = full.len();
    state.pplx_answer = full.to_owned();
    // Markup is stripped without collapsing whitespace: a mid-stream delta continues from the previous
    // one, and collapsing would change text the high-water mark has already counted.
    let visible = clean(&delta, false);
    if visible.is_empty() {
        return None;
    }
    Some(build_chunk(
        &chunk_meta(state),
        json!({ "content": visible }),
        None,
    ))
}

/// One reasoning line, emitted once.
///
/// Deduplicated because perplexity repeats a step across events as its plan is refined, and a client
/// would otherwise show the same search several times.
fn reasoning(text: &str, state: &mut StreamState) -> Option<Value> {
    if text.is_empty() || state.pplx_seen_reasoning.iter().any(|seen| seen == text) {
        return None;
    }
    state.pplx_seen_reasoning.push(text.to_owned());
    Some(build_chunk(
        &chunk_meta(state),
        json!({ "reasoning_content": format!("{text}\n") }),
        None,
    ))
}

/// The terminal chunk. Perplexity's stream ends without a finish reason of its own.
pub fn finish(state: &mut StreamState) -> Value {
    state.finish_reason = Some("stop".to_owned());
    build_chunk(&chunk_meta(state), json!({}), Some("stop"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{clean, finish, translate};
    use crate::state::{Clock, StreamState};

    fn state() -> StreamState {
        let mut state = StreamState::new(Clock::Fixed(1_700_000_123_456));
        state.model = Some("pplx-sonnet".to_owned());
        state
    }

    fn markdown(text: &str, done: bool) -> serde_json::Value {
        json!({
            "blocks": [{
                "intended_usage": "ask_text_markdown",
                "markdown_block": {
                    "progress": if done { "DONE" } else { "IN_PROGRESS" },
                    "chunks": [text],
                },
            }],
        })
    }

    #[test]
    fn each_event_yields_only_the_text_beyond_what_was_sent() {
        // Perplexity re-sends the whole answer every event. Relaying each one as-is would repeat the
        // entire reply once per event.
        let mut state = state();
        let first = translate(&markdown("Hello", false), &mut state);
        let second = translate(&markdown("Hello world", false), &mut state);

        assert_eq!(
            first
                .first()
                .and_then(|chunk| chunk.pointer("/choices/0/delta/content")),
            Some(&json!("Hello"))
        );
        assert_eq!(
            second
                .first()
                .and_then(|chunk| chunk.pointer("/choices/0/delta/content")),
            Some(&json!(" world"))
        );
    }

    #[test]
    fn a_repeated_event_adds_nothing() {
        let mut state = state();
        translate(&markdown("Hello", false), &mut state);
        assert!(
            translate(&markdown("Hello", false), &mut state).is_empty(),
            "an unchanged answer is not a delta"
        );
    }

    #[test]
    fn citation_markers_are_stripped_but_markdown_links_survive() {
        // The markers refer to a citation list this surface never returns, so a client would render a
        // dangling `[1]`. A markdown link is the model's own prose and must survive.
        let mut state = state();
        let out = translate(
            &markdown(
                "Water boils at 100C [1] see [the docs](https://example.test)",
                false,
            ),
            &mut state,
        );
        let content = out
            .first()
            .and_then(|chunk| chunk.pointer("/choices/0/delta/content"))
            .and_then(serde_json::Value::as_str)
            .expect("content");
        assert!(!content.contains("[1]"), "{content}");
        assert!(
            content.contains("[the docs](https://example.test)"),
            "{content}"
        );
    }

    #[test]
    fn searches_and_reads_are_surfaced_as_reasoning() {
        // They are the visible work leading to the answer, which is what reasoning is for.
        let mut state = state();
        let out = translate(
            &json!({
                "blocks": [{
                    "intended_usage": "pro_search_steps",
                    "plan_block": {
                        "steps": [
                            {
                                "step_type": "SEARCH_WEB",
                                "search_web_content": { "queries": [{ "query": "rust async" }] },
                            },
                            {
                                "step_type": "READ_RESULTS",
                                "read_results_content": {
                                    "urls": ["https://a.test", "https://b.test", "https://c.test", "https://d.test"],
                                },
                            },
                        ],
                    },
                }],
            }),
            &mut state,
        );

        let reasoning: Vec<String> = out
            .iter()
            .filter_map(|chunk| chunk.pointer("/choices/0/delta/reasoning_content"))
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect();
        assert!(
            reasoning
                .iter()
                .any(|line| line.contains("Searching: rust async"))
        );
        // Three reads at most: a search can read dozens, and the rest is noise in a trace a person reads.
        assert_eq!(
            reasoning
                .iter()
                .filter(|line| line.contains("Reading:"))
                .count(),
            3,
            "{reasoning:?}"
        );
        // No answer content in a search block.
        assert!(
            out.iter()
                .all(|chunk| chunk.pointer("/choices/0/delta/content").is_none())
        );
    }

    #[test]
    fn a_repeated_search_step_is_not_shown_twice() {
        // Perplexity repeats steps across events as its plan is refined.
        let mut state = state();
        let event = json!({
            "blocks": [{
                "intended_usage": "pro_search_steps",
                "plan_block": {
                    "steps": [{
                        "step_type": "SEARCH_WEB",
                        "search_web_content": { "queries": [{ "query": "same query" }] },
                    }],
                },
            }],
        });
        assert_eq!(translate(&event, &mut state).len(), 1);
        assert!(translate(&event, &mut state).is_empty());
    }

    #[test]
    fn plan_goals_are_reasoning_too() {
        let mut state = state();
        let out = translate(
            &json!({
                "blocks": [{
                    "intended_usage": "plan",
                    "plan_block": { "goals": [{ "description": "Find the release date" }] },
                }],
            }),
            &mut state,
        );
        assert_eq!(
            out.first()
                .and_then(|chunk| chunk.pointer("/choices/0/delta/reasoning_content")),
            Some(&json!("Find the release date\n"))
        );
    }

    #[test]
    fn the_thread_id_is_recorded_for_a_follow_up() {
        // Without it every request starts a new thread and perplexity re-reads the whole context.
        let mut state = state();
        translate(
            &json!({ "backend_uuid": "uuid-77", "blocks": [] }),
            &mut state,
        );
        assert_eq!(state.pplx_backend_uuid.as_deref(), Some("uuid-77"));
    }

    #[test]
    fn a_blockless_event_with_text_still_carries_the_answer() {
        let mut state = state();
        let out = translate(&json!({ "text": "  plain answer  " }), &mut state);
        assert_eq!(
            out.first()
                .and_then(|chunk| chunk.pointer("/choices/0/delta/content")),
            Some(&json!("plain answer"))
        );
    }

    #[test]
    fn an_error_event_is_surfaced_and_stops_the_turn() {
        let mut state = state();
        let out = translate(
            &json!({ "error_code": 429, "error_message": "rate limited" }),
            &mut state,
        );
        let chunk = out.first().expect("an error chunk");
        assert_eq!(
            chunk.pointer("/choices/0/delta/content"),
            Some(&json!("[Error: rate limited]"))
        );
        assert_eq!(
            chunk.pointer("/choices/0/finish_reason"),
            Some(&json!("stop"))
        );
    }

    #[test]
    fn the_terminal_chunk_supplies_the_finish_reason_perplexity_omits() {
        let mut state = state();
        translate(&markdown("done", false), &mut state);
        let terminal = finish(&mut state);
        assert_eq!(
            terminal.pointer("/choices/0/finish_reason"),
            Some(&json!("stop"))
        );
    }

    #[test]
    fn cleaning_removes_perplexitys_own_markup() {
        // Raw XML and `<grok:…>` tags have both appeared in its markdown. A client renders them
        // verbatim.
        assert_eq!(clean("<?xml version=\"1.0\"?>Answer", true), "Answer");
        assert_eq!(clean("a<grok:card>inner</grok:card>b", true), "ab");
        assert_eq!(clean("a<grok:render />b", true), "ab");
        assert_eq!(clean("<response>text</response>", true), "text");
        // Collapsing only applies to a finished answer.
        assert_eq!(clean("a  b\n\n\n\nc", true), "a b\n\nc");
        assert_eq!(clean("a  b", false), "a  b");
    }

    #[test]
    fn cleaning_leaves_ordinary_angle_brackets_alone() {
        // A comparison in prose or a code sample is not markup to strip.
        assert_eq!(clean("if a < b and c > d", true), "if a < b and c > d");
        assert_eq!(clean("<div>kept</div>", true), "<div>kept</div>");
    }
}
