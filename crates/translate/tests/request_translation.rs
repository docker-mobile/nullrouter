//! Request-translation parity tests against the frozen 9Router behavior.

use nullrouter_providers::Format;
use nullrouter_translate::schema::DEFAULT_MAX_TOKENS;
use nullrouter_translate::{request, translate_request};
use serde_json::{Value, json};

fn openai_to_claude(body: &Value) -> Value {
    request::openai_to_claude::translate("claude-sonnet-4.5", body, true, DEFAULT_MAX_TOKENS).body
}

#[test]
fn openai_to_claude_hoists_system_and_injects_claude_code_prompt() {
    let body = json!({
        "messages": [
            { "role": "system", "content": "be terse" },
            { "role": "user", "content": "hi" },
        ],
    });
    let result = openai_to_claude(&body);

    // System becomes a block array: Claude Code prompt first, caller text second.
    assert_eq!(
        result.pointer("/system/0/text"),
        Some(&json!(
            "You are Claude Code, Anthropic's official CLI for Claude."
        ))
    );
    assert_eq!(result.pointer("/system/1/text"), Some(&json!("be terse")));
    assert_eq!(
        result.pointer("/system/1/cache_control/ttl"),
        Some(&json!("1h"))
    );
    // System messages are removed from the message list.
    assert_eq!(result.pointer("/messages/0/role"), Some(&json!("user")));
    assert_eq!(
        result
            .pointer("/messages")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(result.get("max_tokens"), Some(&json!(DEFAULT_MAX_TOKENS)));
    assert_eq!(result.get("stream"), Some(&json!(true)));
}

#[test]
fn openai_to_claude_emits_only_the_prompt_when_no_system_text() {
    let body = json!({ "messages": [{ "role": "user", "content": "hi" }] });
    let result = openai_to_claude(&body);
    assert_eq!(
        result
            .pointer("/system")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
}

#[test]
fn openai_to_claude_separates_tool_results_into_their_own_message() {
    let body = json!({
        "messages": [
            { "role": "user", "content": "read it" },
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "Read", "arguments": "{\"file_path\":\"/a\"}" },
                }],
            },
            { "role": "tool", "tool_call_id": "call_1", "content": "contents" },
            { "role": "user", "content": "thanks" },
        ],
    });
    let result = openai_to_claude(&body);
    let messages = result
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages");

    assert_eq!(messages.len(), 4);
    // tool_use is flushed into its own assistant message.
    assert_eq!(
        messages.get(1).and_then(|m| m.get("role")),
        Some(&json!("assistant"))
    );
    assert_eq!(
        messages.get(1).and_then(|m| m.pointer("/content/0/type")),
        Some(&json!("tool_use"))
    );
    // Arguments are parsed into a Claude `input` object.
    assert_eq!(
        messages
            .get(1)
            .and_then(|m| m.pointer("/content/0/input/file_path")),
        Some(&json!("/a"))
    );
    // tool_result lands in a separate user message, immediately after.
    assert_eq!(
        messages.get(2).and_then(|m| m.get("role")),
        Some(&json!("user"))
    );
    assert_eq!(
        messages.get(2).and_then(|m| m.pointer("/content/0/type")),
        Some(&json!("tool_result"))
    );
    assert_eq!(
        messages
            .get(2)
            .and_then(|m| m.pointer("/content/0/tool_use_id")),
        Some(&json!("call_1"))
    );
}

#[test]
fn openai_to_claude_marks_last_assistant_block_ephemeral() {
    let body = json!({
        "messages": [
            { "role": "user", "content": "hi" },
            { "role": "assistant", "content": [{ "type": "text", "text": "hello" }] },
        ],
    });
    let result = openai_to_claude(&body);
    assert_eq!(
        result.pointer("/messages/1/content/0/cache_control/type"),
        Some(&json!("ephemeral"))
    );
}

#[test]
fn openai_to_claude_never_caches_thinking_blocks() {
    let body = json!({
        "messages": [
            { "role": "user", "content": "hi" },
            {
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "answer" },
                    { "type": "thinking", "thinking": "reasoning", "cache_control": { "type": "ephemeral" } },
                ],
            },
        ],
    });
    let result = openai_to_claude(&body);
    // cache_control is stripped from the thinking block...
    assert!(
        result
            .pointer("/messages/1/content/1/cache_control")
            .is_none()
    );
    // ...and applied to the text block instead.
    assert_eq!(
        result.pointer("/messages/1/content/0/cache_control/type"),
        Some(&json!("ephemeral"))
    );
}

#[test]
fn openai_to_claude_converts_tools_and_caches_the_last_one() {
    let body = json!({
        "messages": [{ "role": "user", "content": "hi" }],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "Read",
                    "description": "read a file",
                    "parameters": { "type": "object", "properties": { "p": { "type": "string" } } },
                },
            },
            {
                "type": "function",
                "function": { "name": "Write", "parameters": { "type": "object" } },
            },
        ],
    });
    let translated =
        request::openai_to_claude::translate("claude-sonnet-4.5", &body, true, DEFAULT_MAX_TOKENS);
    let result = &translated.body;

    assert_eq!(result.pointer("/tools/0/name"), Some(&json!("Read")));
    assert_eq!(
        result.pointer("/tools/0/description"),
        Some(&json!("read a file"))
    );
    // OpenAI `parameters` becomes Claude `input_schema`.
    assert_eq!(
        result.pointer("/tools/0/input_schema/properties/p/type"),
        Some(&json!("string"))
    );
    // Only the final tool carries cache_control.
    assert!(result.pointer("/tools/0/cache_control").is_none());
    assert_eq!(
        result.pointer("/tools/1/cache_control/ttl"),
        Some(&json!("1h"))
    );
    // The map is populated for response-side restoration.
    assert_eq!(
        translated.tool_name_map.get("Read").map(String::as_str),
        Some("Read")
    );
}

#[test]
fn openai_to_claude_never_forwards_a_tool_choice_claude_rejects() {
    let with_function = json!({
        "messages": [{ "role": "user", "content": "hi" }],
        "tool_choice": { "type": "function", "function": { "name": "Read" } },
    });
    assert_eq!(
        openai_to_claude(&with_function).get("tool_choice"),
        Some(&json!({ "type": "tool", "name": "Read" }))
    );

    let required = json!({
        "messages": [{ "role": "user", "content": "hi" }],
        "tool_choice": "required",
    });
    assert_eq!(
        openai_to_claude(&required).get("tool_choice"),
        Some(&json!({ "type": "any" }))
    );

    // An unknown object type degrades to auto rather than leaking through.
    let bogus = json!({
        "messages": [{ "role": "user", "content": "hi" }],
        "tool_choice": { "type": "nonsense" },
    });
    assert_eq!(
        openai_to_claude(&bogus).get("tool_choice"),
        Some(&json!({ "type": "auto" }))
    );
}

#[test]
fn openai_to_claude_turns_json_mode_into_a_system_instruction() {
    let body = json!({
        "messages": [{ "role": "user", "content": "hi" }],
        "response_format": { "type": "json_object" },
    });
    let result = openai_to_claude(&body);
    let text = result
        .pointer("/system/1/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(text.contains("valid JSON"), "got: {text}");
}

#[test]
fn openai_to_claude_converts_image_and_pdf_parts() {
    let body = json!({
        "messages": [{
            "role": "user",
            "content": [
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,QUJD" } },
                { "type": "image_url", "image_url": { "url": "https://example.test/a.png" } },
                { "type": "file", "file": { "file_data": "data:application/pdf;base64,UERG" } },
                { "type": "file", "file": { "file_data": "data:text/plain;base64,QQ" } },
            ],
        }],
    });
    let result = openai_to_claude(&body);
    let blocks = result
        .pointer("/messages/0/content")
        .and_then(Value::as_array)
        .expect("content blocks");

    // base64 image, remote image, PDF document — the text/plain file is dropped.
    assert_eq!(blocks.len(), 3);
    assert_eq!(
        blocks.first().and_then(|b| b.pointer("/source/type")),
        Some(&json!("base64"))
    );
    assert_eq!(
        blocks.first().and_then(|b| b.pointer("/source/media_type")),
        Some(&json!("image/png"))
    );
    assert_eq!(
        blocks.get(1).and_then(|b| b.pointer("/source/type")),
        Some(&json!("url"))
    );
    assert_eq!(
        blocks.get(2).and_then(|b| b.get("type")),
        Some(&json!("document"))
    );
}

#[test]
fn claude_to_openai_flattens_system_blocks_and_strips_billing_header() {
    let body = json!({
        "system": [
            { "type": "text", "text": "x-anthropic-billing-header: cc\nreal instruction" },
            { "type": "text", "text": "second" },
        ],
        "messages": [{ "role": "user", "content": "hi" }],
        "max_tokens": 100,
    });
    let result = request::claude_to_openai::translate("gpt-5", &body, false);

    assert_eq!(result.pointer("/messages/0/role"), Some(&json!("system")));
    assert_eq!(
        result.pointer("/messages/0/content"),
        Some(&json!("real instruction\nsecond"))
    );
    assert_eq!(result.get("max_tokens"), Some(&json!(100)));
    assert_eq!(result.get("stream"), Some(&json!(false)));
}

#[test]
fn claude_to_openai_inserts_placeholders_for_unanswered_tool_calls() {
    let body = json!({
        "messages": [
            {
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "call_1", "name": "Read", "input": { "p": 1 } },
                    { "type": "tool_use", "id": "call_2", "name": "Write", "input": {} },
                ],
            },
            {
                "role": "user",
                "content": [{ "type": "tool_result", "tool_use_id": "call_1", "content": "ok" }],
            },
        ],
    });
    let result = request::claude_to_openai::translate("gpt-5", &body, false);
    let messages = result
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages");

    // assistant + reply for call_1 + synthesized reply for call_2.
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages.get(2).and_then(|m| m.get("tool_call_id")),
        Some(&json!("call_2"))
    );
    assert_eq!(
        messages.get(2).and_then(|m| m.get("content")),
        Some(&json!("[No response received]"))
    );
}

#[test]
fn claude_to_openai_wraps_mid_conversation_system_as_instructions() {
    let body = json!({
        "messages": [
            { "role": "user", "content": "hi" },
            { "role": "system", "content": [{ "type": "text", "text": "stay terse" }] },
        ],
    });
    let result = request::claude_to_openai::translate("gpt-5", &body, false);
    assert_eq!(result.pointer("/messages/1/role"), Some(&json!("user")));
    assert_eq!(
        result.pointer("/messages/1/content"),
        Some(&json!("<instructions>\nstay terse\n</instructions>"))
    );
}

#[test]
fn claude_to_openai_maps_tool_choice_and_reasoning_effort() {
    let body = json!({
        "messages": [{ "role": "user", "content": "hi" }],
        "tool_choice": { "type": "any" },
        "reasoning": { "effort": "high" },
    });
    let result = request::claude_to_openai::translate("gpt-5", &body, false);
    assert_eq!(result.get("tool_choice"), Some(&json!("required")));
    assert_eq!(result.get("reasoning_effort"), Some(&json!("high")));
}

#[test]
fn gemini_to_openai_maps_config_contents_and_tools() {
    let body = json!({
        "generationConfig": { "maxOutputTokens": 500, "temperature": 0.5, "topP": 0.9 },
        "systemInstruction": { "parts": [{ "text": "be brief" }] },
        "contents": [
            { "role": "user", "parts": [{ "text": "hi" }] },
            { "role": "model", "parts": [{ "text": "hello" }] },
        ],
        "tools": [{
            "functionDeclarations": [
                { "name": "Read", "description": "read", "parameters": { "type": "object" } },
            ],
        }],
    });
    let result = request::gemini_to_openai::translate("gpt-5", &body, true);

    // The body declares tools, so adjustMaxTokens raises 500 to the
    // tool-calling floor (DEFAULT_MIN_TOKENS) exactly as upstream does.
    assert_eq!(result.get("max_tokens"), Some(&json!(32000)));
    assert_eq!(result.get("temperature"), Some(&json!(0.5)));
    assert_eq!(result.get("top_p"), Some(&json!(0.9)));
    assert_eq!(
        result.pointer("/messages/0/content"),
        Some(&json!("be brief"))
    );
    assert_eq!(result.pointer("/messages/1/role"), Some(&json!("user")));
    // Gemini `model` maps to OpenAI `assistant`.
    assert_eq!(
        result.pointer("/messages/2/role"),
        Some(&json!("assistant"))
    );
    assert_eq!(
        result.pointer("/tools/0/function/name"),
        Some(&json!("Read"))
    );
}

#[test]
fn gemini_to_openai_derives_stable_tool_call_ids() {
    let body = json!({
        "contents": [
            {
                "role": "model",
                "parts": [{ "functionCall": { "name": "Read", "args": { "p": 1 } } }],
            },
            {
                "role": "user",
                "parts": [{
                    "functionResponse": { "name": "Read", "response": { "result": "ok" } },
                }],
            },
        ],
    });
    let result = request::gemini_to_openai::translate("gpt-5", &body, false);
    // Call and response must agree on the derived id so providers can pair them.
    assert_eq!(
        result.pointer("/messages/0/tool_calls/0/id"),
        Some(&json!("call_Read"))
    );
    assert_eq!(result.pointer("/messages/1/role"), Some(&json!("tool")));
    assert_eq!(
        result.pointer("/messages/1/tool_call_id"),
        Some(&json!("call_Read"))
    );
}

#[test]
fn openai_to_gemini_builds_contents_and_merges_same_role_turns() {
    let body = json!({
        "messages": [
            { "role": "system", "content": "be brief" },
            { "role": "user", "content": "one" },
            { "role": "user", "content": "two" },
        ],
        "temperature": 0.3,
        "max_tokens": 256,
    });
    let result = request::openai_to_gemini::translate("gemini-2.5-pro", &body, true);

    assert_eq!(
        result.pointer("/systemInstruction/parts/0/text"),
        Some(&json!("be brief"))
    );
    assert_eq!(
        result.pointer("/generationConfig/temperature"),
        Some(&json!(0.3))
    );
    assert_eq!(
        result.pointer("/generationConfig/maxOutputTokens"),
        Some(&json!(256))
    );
    // Consecutive user turns are merged into one content entry.
    assert_eq!(
        result
            .pointer("/contents")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        result
            .pointer("/contents/0/parts")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2)
    );
    assert!(result.get("safetySettings").is_some());
}

#[test]
fn openai_to_gemini_pairs_function_calls_with_responses() {
    let body = json!({
        "messages": [
            { "role": "user", "content": "read" },
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "Read", "arguments": "{\"p\":1}" },
                }],
            },
            { "role": "tool", "tool_call_id": "call_1", "content": "file contents" },
        ],
    });
    let result = request::openai_to_gemini::translate("gemini-2.5-pro", &body, false);
    let contents = result
        .get("contents")
        .and_then(Value::as_array)
        .expect("contents");

    assert_eq!(contents.len(), 3);
    assert_eq!(
        contents.get(1).and_then(|c| c.get("role")),
        Some(&json!("model"))
    );
    assert_eq!(
        contents
            .get(1)
            .and_then(|c| c.pointer("/parts/0/functionCall/name")),
        Some(&json!("Read"))
    );
    // Responses come back as a user turn holding functionResponse parts.
    assert_eq!(
        contents.get(2).and_then(|c| c.get("role")),
        Some(&json!("user"))
    );
    assert_eq!(
        contents
            .get(2)
            .and_then(|c| c.pointer("/parts/0/functionResponse/name")),
        Some(&json!("Read"))
    );
    // A non-JSON reply is wrapped as `{result: text}` and then nested under
    // `response.result`, producing upstream's double-wrapped shape.
    assert_eq!(
        contents
            .get(2)
            .and_then(|c| c.pointer("/parts/0/functionResponse/response/result")),
        Some(&json!({ "result": "file contents" }))
    );
}

#[test]
fn openai_to_gemini_sanitizes_function_names_and_cleans_schemas() {
    let body = json!({
        "messages": [{ "role": "user", "content": "hi" }],
        "tools": [{
            "type": "function",
            "function": {
                "name": "bad name!@#",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "mode": { "const": "fast" },
                        "size": { "type": ["string", "null"], "minLength": 2, "format": "uuid" },
                    },
                    "required": ["mode", "ghost"],
                    "$schema": "http://json-schema.org/draft-07/schema#",
                },
            },
        }],
    });
    let result = request::openai_to_gemini::translate("gemini-2.5-pro", &body, false);
    let function = result
        .pointer("/tools/0/functionDeclarations/0")
        .expect("declaration");

    assert_eq!(function.get("name"), Some(&json!("bad_name___")));
    // const -> enum, coerced to strings with an explicit type.
    assert_eq!(
        function.pointer("/parameters/properties/mode/enum"),
        Some(&json!(["fast"]))
    );
    assert_eq!(
        function.pointer("/parameters/properties/mode/type"),
        Some(&json!("string"))
    );
    // Type arrays collapse to the first non-null entry.
    assert_eq!(
        function.pointer("/parameters/properties/size/type"),
        Some(&json!("string"))
    );
    // Unsupported keywords are stripped at every level.
    assert!(
        function
            .pointer("/parameters/properties/size/minLength")
            .is_none()
    );
    assert!(
        function
            .pointer("/parameters/properties/size/format")
            .is_none()
    );
    assert!(function.pointer("/parameters/$schema").is_none());
    // `required` keeps only fields that exist in properties.
    assert_eq!(
        function.pointer("/parameters/required"),
        Some(&json!(["mode"]))
    );
}

#[test]
fn gemini_schema_placeholder_fills_empty_object_tools() {
    let body = json!({
        "messages": [{ "role": "user", "content": "hi" }],
        "tools": [{
            "type": "function",
            "function": { "name": "ping", "parameters": { "type": "object", "properties": {} } },
        }],
    });
    let result = request::openai_to_gemini::translate("gemini-2.5-pro", &body, false);
    // Gemini rejects an object schema with no properties.
    assert_eq!(
        result.pointer("/tools/0/functionDeclarations/0/parameters/required"),
        Some(&json!(["reason"]))
    );
    assert!(
        result
            .pointer("/tools/0/functionDeclarations/0/parameters/properties/reason")
            .is_some()
    );
}

#[test]
fn dispatch_passes_through_matching_formats_but_rewrites_the_model() {
    let body = json!({ "messages": [{ "role": "user", "content": "hi" }], "model": "alias" });
    let result = translate_request(
        Format::OpenAi,
        Format::OpenAi,
        "gpt-5",
        &body,
        true,
        DEFAULT_MAX_TOKENS,
    );
    assert_eq!(result.body.get("model"), Some(&json!("gpt-5")));
    assert_eq!(
        result.body.pointer("/messages/0/content"),
        Some(&json!("hi"))
    );
}

#[test]
fn dispatch_pivots_claude_to_gemini_through_openai() {
    let body = json!({
        "system": "be brief",
        "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }] }],
        "max_tokens": 100,
    });
    let result = translate_request(
        Format::Claude,
        Format::Gemini,
        "gemini-2.5-pro",
        &body,
        true,
        DEFAULT_MAX_TOKENS,
    );
    // Claude system -> OpenAI system message -> Gemini systemInstruction.
    assert_eq!(
        result.body.pointer("/systemInstruction/parts/0/text"),
        Some(&json!("be brief"))
    );
    assert_eq!(
        result.body.pointer("/contents/0/role"),
        Some(&json!("user"))
    );
    assert_eq!(
        result.body.pointer("/contents/0/parts/0/text"),
        Some(&json!("hi"))
    );
}

#[test]
fn dispatch_carries_tool_name_map_for_claude_targets() {
    let body = json!({
        "messages": [{ "role": "user", "content": "hi" }],
        "tools": [{
            "type": "function",
            "function": { "name": "Read", "parameters": { "type": "object" } },
        }],
    });
    let result = translate_request(
        Format::OpenAi,
        Format::Claude,
        "claude-sonnet-4.5",
        &body,
        true,
        DEFAULT_MAX_TOKENS,
    );
    assert_eq!(
        result.tool_name_map.get("Read").map(String::as_str),
        Some("Read")
    );
}
