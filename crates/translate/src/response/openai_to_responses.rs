//! OpenAI Chat Completions stream -> OpenAI Responses API events.
//!
//! Ports `open-sse/translator/response/openai-responses.js`.
//!
//! The Responses API is not a reshaped chunk stream: it is a sequence of named
//! lifecycle events (`response.created`, `response.output_item.added`,
//! `response.output_text.delta`, …) carrying a monotonic `sequence_number`, with
//! every opened item explicitly closed. A client parses the event names, so
//! forwarding `chat.completion.chunk` frames here would simply not work.

use serde_json::{Map, Value, json};

use crate::concerns::extract_reasoning_text;
use crate::schema::role;
use crate::state::StreamState;

/// Responses item type discriminators.
mod item {
    pub(super) const MESSAGE: &str = "message";
    pub(super) const FUNCTION_CALL: &str = "function_call";
    pub(super) const CUSTOM_TOOL_CALL: &str = "custom_tool_call";
    pub(super) const OUTPUT_TEXT: &str = "output_text";
    pub(super) const REASONING: &str = "reasoning";
    pub(super) const SUMMARY_TEXT: &str = "summary_text";
}

/// One `event:`/`data:` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseEvent {
    pub event: String,
    pub data: Value,
}

/// Emitter that stamps each event with the next sequence number.
struct Emitter<'a> {
    state: &'a mut StreamState,
    events: Vec<ResponseEvent>,
}

impl Emitter<'_> {
    fn emit(&mut self, event_type: &str, mut data: Value) {
        self.state.responses_seq += 1;
        if let Some(object) = data.as_object_mut() {
            object.insert("type".to_owned(), json!(event_type));
            // Clients rely on this being monotonic across the whole response.
            object.insert(
                "sequence_number".to_owned(),
                json!(self.state.responses_seq),
            );
        }
        self.events.push(ResponseEvent {
            event: event_type.to_owned(),
            data,
        });
    }
}

/// Translate one OpenAI chunk into Responses events.
///
/// Passing `None` flushes: every open item is closed and `response.completed` is
/// emitted. That flush is mandatory — a client waits for it.
pub fn translate(chunk: Option<&Value>, state: &mut StreamState) -> Vec<ResponseEvent> {
    let Some(chunk) = chunk else {
        return flush(state);
    };
    let Some(choice) = chunk
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return Vec::new();
    };

    let index = choice.get("index").and_then(Value::as_u64).unwrap_or(0);
    let delta = choice.get("delta");

    // The response id derives from the upstream chunk id on first sight.
    if !state.responses_started
        && let Some(id) = chunk.get("id").and_then(Value::as_str)
    {
        state.response_id = Some(format!("resp_{id}"));
    }
    let response_id = state
        .response_id
        .clone()
        .unwrap_or_else(|| format!("resp_{}", state.clock.now_millis()));
    state.response_id = Some(response_id.clone());
    let created = state.responses_created.unwrap_or_else(|| {
        let created = state.clock.now_seconds();
        state.responses_created = Some(created);
        created
    });

    let mut emitter = Emitter {
        state,
        events: Vec::new(),
    };

    if !emitter.state.responses_started {
        emitter.state.responses_started = true;
        emitter.emit(
            "response.created",
            json!({
                "response": {
                    "id": response_id,
                    "object": "response",
                    "created_at": created,
                    "status": "in_progress",
                    "background": false,
                    "error": null,
                    "output": [],
                },
            }),
        );
        emitter.emit(
            "response.in_progress",
            json!({
                "response": {
                    "id": response_id,
                    "object": "response",
                    "created_at": created,
                    "status": "in_progress",
                },
            }),
        );
    }

    // Reasoning, across the vendor shapes upstream tolerates.
    let reasoning = extract_reasoning_text(delta);
    if !reasoning.is_empty() {
        start_reasoning(&mut emitter, &response_id, index);
        emitter.emit(
            "response.reasoning_summary_text.delta",
            json!({
                "item_id": reasoning_item_id(&response_id),
                "output_index": index,
                "summary_index": 0,
                "delta": reasoning,
            }),
        );
        emitter.state.reasoning_buffer.push_str(&reasoning);
    }

    // Assistant text.
    if let Some(content) = delta
        .and_then(|delta| delta.get("content"))
        .and_then(Value::as_str)
        .filter(|content| !content.is_empty())
    {
        // Reasoning closes before visible output starts.
        close_reasoning(&mut emitter, &response_id, index);
        emit_text(&mut emitter, &response_id, index, content);
    }

    if let Some(calls) = delta
        .and_then(|delta| delta.get("tool_calls"))
        .and_then(Value::as_array)
    {
        emit_tool_calls(&mut emitter, &response_id, calls);
    }

    // A finish reason means the turn is over: close everything and complete.
    if choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .is_some_and(|reason| !reason.is_empty())
    {
        let mut events = emitter.events;
        events.extend(flush(emitter.state));
        return events;
    }

    emitter.events
}

fn reasoning_item_id(response_id: &str) -> String {
    format!("rs_{response_id}")
}

fn message_item_id(response_id: &str, index: u64) -> String {
    format!("msg_{response_id}_{index}")
}

fn start_reasoning(emitter: &mut Emitter<'_>, response_id: &str, index: u64) {
    if emitter.state.reasoning_item_added {
        return;
    }
    emitter.state.reasoning_item_added = true;
    let item_id = reasoning_item_id(response_id);
    emitter.emit(
        "response.output_item.added",
        json!({
            "output_index": index,
            "item": { "id": item_id, "type": item::REASONING, "summary": [] },
        }),
    );
    emitter.emit(
        "response.reasoning_summary_part.added",
        json!({
            "item_id": reasoning_item_id(response_id),
            "output_index": index,
            "summary_index": 0,
            "part": { "type": item::SUMMARY_TEXT, "text": "" },
        }),
    );
}

fn close_reasoning(emitter: &mut Emitter<'_>, response_id: &str, index: u64) {
    if !emitter.state.reasoning_item_added || emitter.state.reasoning_item_done {
        return;
    }
    emitter.state.reasoning_item_done = true;
    let item_id = reasoning_item_id(response_id);
    let text = emitter.state.reasoning_buffer.clone();

    emitter.emit(
        "response.reasoning_summary_text.done",
        json!({
            "item_id": item_id,
            "output_index": index,
            "summary_index": 0,
            "text": text,
        }),
    );
    emitter.emit(
        "response.reasoning_summary_part.done",
        json!({
            "item_id": reasoning_item_id(response_id),
            "output_index": index,
            "summary_index": 0,
            "part": { "type": item::SUMMARY_TEXT, "text": emitter.state.reasoning_buffer.clone() },
        }),
    );
    emitter.emit(
        "response.output_item.done",
        json!({
            "output_index": index,
            "item": {
                "id": reasoning_item_id(response_id),
                "type": item::REASONING,
                "summary": [{
                    "type": item::SUMMARY_TEXT,
                    "text": emitter.state.reasoning_buffer.clone(),
                }],
            },
        }),
    );
}

fn emit_text(emitter: &mut Emitter<'_>, response_id: &str, index: u64, content: &str) {
    let item_id = message_item_id(response_id, index);

    if emitter.state.message_items_added.insert(index) {
        emitter.emit(
            "response.output_item.added",
            json!({
                "output_index": index,
                "item": {
                    "id": item_id,
                    "type": item::MESSAGE,
                    "content": [],
                    "role": role::ASSISTANT,
                },
            }),
        );
        emitter.emit(
            "response.content_part.added",
            json!({
                "item_id": message_item_id(response_id, index),
                "output_index": index,
                "content_index": 0,
                "part": {
                    "type": item::OUTPUT_TEXT,
                    "annotations": [],
                    "logprobs": [],
                    "text": "",
                },
            }),
        );
    }

    emitter.emit(
        "response.output_text.delta",
        json!({
            "item_id": message_item_id(response_id, index),
            "output_index": index,
            "content_index": 0,
            "delta": content,
            "logprobs": [],
        }),
    );

    emitter
        .state
        .message_text
        .entry(index)
        .or_default()
        .push_str(content);
}

fn close_message(emitter: &mut Emitter<'_>, response_id: &str, index: u64) {
    if !emitter.state.message_items_added.contains(&index)
        || emitter.state.message_items_done.contains(&index)
    {
        return;
    }
    emitter.state.message_items_done.insert(index);
    let item_id = message_item_id(response_id, index);
    let text = emitter
        .state
        .message_text
        .get(&index)
        .cloned()
        .unwrap_or_default();

    emitter.emit(
        "response.output_text.done",
        json!({
            "item_id": item_id,
            "output_index": index,
            "content_index": 0,
            "text": text,
            "logprobs": [],
        }),
    );
    emitter.emit(
        "response.content_part.done",
        json!({
            "item_id": message_item_id(response_id, index),
            "output_index": index,
            "content_index": 0,
            "part": {
                "type": item::OUTPUT_TEXT,
                "annotations": [],
                "logprobs": [],
                "text": text,
            },
        }),
    );
    emitter.emit(
        "response.output_item.done",
        json!({
            "output_index": index,
            "item": {
                "id": message_item_id(response_id, index),
                "type": item::MESSAGE,
                "content": [{
                    "type": item::OUTPUT_TEXT,
                    "annotations": [],
                    "logprobs": [],
                    "text": text,
                }],
                "role": role::ASSISTANT,
            },
        }),
    );
}

fn emit_tool_calls(emitter: &mut Emitter<'_>, response_id: &str, calls: &[Value]) {
    for call in calls {
        let index = call.get("index").and_then(Value::as_u64).unwrap_or(0);
        let name = call
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let call_id = call
            .get("id")
            .and_then(Value::as_str)
            .map_or_else(|| format!("call_{index}"), str::to_owned);

        if emitter.state.function_items_added.insert(index) {
            emitter
                .state
                .function_call_ids
                .insert(index, call_id.clone());
            if !name.is_empty() {
                emitter.state.function_names.insert(index, name.to_owned());
            }
            // Text closes before a tool item opens, so items never interleave.
            close_message(emitter, response_id, 0);

            let item_id = format!("fc_{response_id}_{index}");
            emitter.emit(
                "response.output_item.added",
                json!({
                    "output_index": index,
                    "item": {
                        "id": item_id,
                        "type": item::FUNCTION_CALL,
                        "call_id": call_id,
                        "name": name,
                        "arguments": "",
                    },
                }),
            );
        } else if !name.is_empty() {
            // Some vendors send the name only on a later delta.
            emitter
                .state
                .function_names
                .entry(index)
                .or_insert_with(|| name.to_owned());
        }

        if let Some(arguments) = call
            .get("function")
            .and_then(|function| function.get("arguments"))
            .and_then(Value::as_str)
            .filter(|arguments| !arguments.is_empty())
        {
            emitter
                .state
                .function_arguments
                .entry(index)
                .or_default()
                .push_str(arguments);
            emitter.emit(
                "response.function_call_arguments.delta",
                json!({
                    "item_id": format!("fc_{response_id}_{index}"),
                    "output_index": index,
                    "delta": arguments,
                }),
            );
        }
    }
}

fn close_tool_call(emitter: &mut Emitter<'_>, response_id: &str, index: u64) {
    if emitter.state.function_items_done.contains(&index) {
        return;
    }
    emitter.state.function_items_done.insert(index);

    let arguments = emitter
        .state
        .function_arguments
        .get(&index)
        .cloned()
        .unwrap_or_default();
    let name = emitter
        .state
        .function_names
        .get(&index)
        .cloned()
        .unwrap_or_default();
    let call_id = emitter
        .state
        .function_call_ids
        .get(&index)
        .cloned()
        .unwrap_or_else(|| format!("call_{index}"));
    let item_id = format!("fc_{response_id}_{index}");
    let is_custom = emitter.state.custom_tool_names.contains(&name);

    emitter.emit(
        "response.function_call_arguments.done",
        json!({
            "item_id": item_id,
            "output_index": index,
            "arguments": arguments,
        }),
    );

    // A custom tool reports its raw `input` rather than JSON arguments.
    let mut item = Map::new();
    item.insert("id".to_owned(), json!(format!("fc_{response_id}_{index}")));
    item.insert(
        "type".to_owned(),
        json!(if is_custom {
            item::CUSTOM_TOOL_CALL
        } else {
            item::FUNCTION_CALL
        }),
    );
    if is_custom {
        item.insert("input".to_owned(), json!(custom_tool_input(&arguments)));
    } else {
        item.insert("arguments".to_owned(), json!(arguments));
    }
    item.insert("call_id".to_owned(), json!(call_id));
    item.insert("name".to_owned(), json!(name));

    emitter.emit(
        "response.output_item.done",
        json!({ "output_index": index, "item": Value::Object(item) }),
    );
}

/// Unwrap the `{"input": "..."}` envelope a custom tool's arguments carry.
fn custom_tool_input(arguments: &str) -> String {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|parsed| {
            parsed
                .get("input")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| arguments.to_owned())
}

/// Close every open item and complete the response.
pub fn flush(state: &mut StreamState) -> Vec<ResponseEvent> {
    if state.responses_completed {
        return Vec::new();
    }
    // Nothing was ever started, so there is nothing to complete.
    if !state.responses_started {
        return Vec::new();
    }

    let response_id = state
        .response_id
        .clone()
        .unwrap_or_else(|| format!("resp_{}", state.clock.now_millis()));
    let created = state
        .responses_created
        .unwrap_or_else(|| state.clock.now_seconds());

    let mut emitter = Emitter {
        state,
        events: Vec::new(),
    };

    let message_indices: Vec<u64> = emitter.state.message_items_added.iter().copied().collect();
    for index in message_indices {
        close_message(&mut emitter, &response_id, index);
    }
    close_reasoning(&mut emitter, &response_id, 0);
    let function_indices: Vec<u64> = emitter.state.function_items_added.iter().copied().collect();
    for index in function_indices {
        close_tool_call(&mut emitter, &response_id, index);
    }

    emitter.state.responses_completed = true;
    emitter.emit(
        "response.completed",
        json!({
            "response": {
                "id": response_id,
                "object": "response",
                "created_at": created,
                "status": "completed",
                "background": false,
                "error": null,
            },
        }),
    );

    emitter.events
}
