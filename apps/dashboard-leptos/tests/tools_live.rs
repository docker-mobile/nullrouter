//! Boundary tests for the live CLI Tools, Translator, and Console Log state.
//!
//! All three panels replaced fixtures, so every test here is about one property:
//! the UI may only show what a request returned. A body that is empty must
//! produce an empty state rather than the old fixture; a body whose shape changed
//! must produce a visible failure; and a frame the events service did not send
//! must never become a rendered line.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "an integration test asserting on parsed values should fail loudly at the assertion, not carry Option plumbing that obscures what is being tested"
)]

use nullrouter_dashboard_wasm::api::ApiError;
use nullrouter_dashboard_wasm::dashboard::cli_tools_live::{
    ApplyOutcome, Detection, DraftError, Routing, ToolConfigDraft, parse_all_statuses,
    parse_mcp_registry, parse_tool_status, settings_path, settle_apply,
};
use nullrouter_dashboard_wasm::dashboard::console_log_live::{
    ClearOutcome, FrameKind, LogBuffer, LogLevel, MAX_LINES, StreamState, parse_connected_frame,
    parse_console_frame, parse_history, settle_clear,
};
use nullrouter_dashboard_wasm::dashboard::translator_live::{
    RequestError, SaveOutcome, SendOutcome, StepSource, TranslateOutcome, TranslateStep,
    TranslationMeta, TranslatorFile, format_json, merge_meta, save_body, send_body, settle_save,
    settle_send, settle_translate, translate_body,
};

// ── CLI tools ───────────────────────────────────────────────────────────────

/// A batch response in the shape upstream's fan-out produces: one installed tool
/// with a config, one that is not installed, and one whose handler threw and so
/// mapped to `null`.
const REALISTIC_STATUSES: &str = r#"{
  "claude": {
    "installed": true,
    "has9Router": true,
    "config": "{\n  \"env\": {\n    \"ANTHROPIC_BASE_URL\": \"http://127.0.0.1:20128\"\n  }\n}",
    "configPath": "/home/dev/.claude/settings.json"
  },
  "codex": {
    "installed": false,
    "config": null,
    "message": "Codex CLI is not installed"
  },
  "opencode": null
}"#;

#[test]
fn a_realistic_batch_response_becomes_one_row_per_reported_tool() {
    let list = parse_all_statuses(REALISTIC_STATUSES).expect("an object of tools parses");

    // Ordered by display name: Claude Code, OpenAI Codex CLI, OpenCode.
    let names: Vec<&str> = list
        .tools()
        .iter()
        .map(|tool| tool.label.as_str())
        .collect();
    assert_eq!(names, ["Claude Code", "OpenAI Codex CLI", "OpenCode"]);
    assert_eq!(list.len(), 3);
    assert_eq!(list.detected_count(), 1);
    assert_eq!(list.routed_count(), 1);
    assert_eq!(list.unknown_count(), 1);
}

#[test]
fn a_tool_the_router_did_not_find_is_never_described_as_installed() {
    let list = parse_all_statuses(REALISTIC_STATUSES).expect("parses");

    let claude = list.tool("claude").expect("claude row");
    assert_eq!(claude.detection(), Detection::Installed);
    assert_eq!(claude.routing(), Routing::Configured);

    let codex = list.tool("codex").expect("codex row");
    assert_eq!(codex.detection(), Detection::Missing);
    // The router said nothing about `has9Router` for an absent tool, so the
    // config must read as unread rather than as "not routed".
    assert_eq!(codex.routing(), Routing::Unknown);
    assert_eq!(codex.summary(), "Codex CLI is not installed");

    // A handler that threw is reported as unknown, which is distinct from both.
    let opencode = list.tool("opencode").expect("opencode row");
    assert_eq!(opencode.detection(), Detection::Unknown);
    assert!(opencode.status.is_none());
    assert!(opencode.summary().contains("no status"));
}

#[test]
fn an_empty_batch_response_is_an_empty_list_not_a_fixture() {
    // The old panel always drew eight tiles. An empty object must draw none.
    let list = parse_all_statuses("{}").expect("an empty object is a valid answer");
    assert!(list.is_empty());
    assert_eq!(list.len(), 0);
    assert_eq!(list.tools(), &[]);
    assert!(list.summary().contains("no CLI tools"));
}

#[test]
fn a_malformed_or_non_object_batch_response_is_a_failure() {
    // Each of these must surface as `ApiError::Body` in the panel, not as
    // "you have no tools".
    for body in ["", "   ", "not json", "[]", "null", "\"claude\"", "{\"a\":"] {
        assert!(
            parse_all_statuses(body).is_none(),
            "{body:?} should not parse into a tool list"
        );
    }
}

#[test]
fn the_nullrouter_api_stub_response_reports_every_tool_as_not_detected() {
    // `services/api-actix/src/cli_tools.rs` answers with `installed: false` and
    // `has_9_router: false` for all twelve tools. The panel must show twelve
    // rows, none of them detected — not an empty page, and not a claim of
    // installation.
    let body = r#"{
      "claude": {"installed": false, "has9Router": false, "config": null, "settings": null,
                 "configPath": null,
                 "message": "CLI tool configuration is not supported by nullrouter-api"},
      "deepseek-tui": {"installed": false, "has9Router": false, "config": null, "settings": null,
                 "configPath": null,
                 "message": "CLI tool configuration is not supported by nullrouter-api"}
    }"#;
    let list = parse_all_statuses(body).expect("the stub shape parses");

    assert_eq!(list.len(), 2);
    assert_eq!(list.detected_count(), 0);
    assert_eq!(list.routed_count(), 0);
    assert_eq!(list.unknown_count(), 0);
    let tui = list.tool("deepseek-tui").expect("deepseek row");
    assert_eq!(tui.label, "DeepSeek TUI");
    assert_eq!(tui.detection(), Detection::Missing);
    assert_eq!(tui.routing(), Routing::NotConfigured);
}

#[test]
fn one_tool_status_reads_config_whether_it_is_text_or_json() {
    let text = parse_tool_status(r#"{"installed":true,"config":"model = \"gpt-5\""}"#)
        .expect("a text config parses");
    assert_eq!(text.config_text(), Some("model = \"gpt-5\""));

    // `opencode-settings` returns a parsed object; rendering it is honest,
    // dropping it would hide a config that exists.
    let json = parse_tool_status(r#"{"installed":true,"config":{"model":"gpt-5"}}"#)
        .expect("an object config parses");
    assert!(
        json.config_text()
            .is_some_and(|text| text.contains("gpt-5"))
    );

    // Whitespace-only config is no config.
    let blank =
        parse_tool_status(r#"{"installed":true,"config":"   "}"#).expect("a blank config parses");
    assert_eq!(blank.config_text(), None);

    assert!(parse_tool_status("[]").is_none());
    assert!(parse_tool_status("").is_none());
}

#[test]
fn a_tool_id_travels_through_the_url_encoded() {
    assert_eq!(
        settings_path("deepseek-tui"),
        "/api/cli-tools/deepseek-tui-settings"
    );
    // Anything outside RFC 3986 `unreserved` is percent-encoded rather than
    // trusted to be path-safe.
    assert_eq!(settings_path("a/b?c"), "/api/cli-tools/a%2Fb%3Fc-settings");
}

#[test]
fn the_config_form_refuses_an_incomplete_draft_before_spending_a_request() {
    let mut draft = ToolConfigDraft::default();
    assert_eq!(draft.validation_error(), Some(DraftError::BaseUrlMissing));

    draft.base_url = String::from("  http://127.0.0.1:20128/v1  ");
    assert_eq!(draft.validation_error(), Some(DraftError::ApiKeyMissing));

    draft.api_key = String::from("sk-test");
    assert_eq!(draft.validation_error(), Some(DraftError::ModelMissing));

    draft.model = String::from("gpt-5");
    let body = draft.apply_body().expect("a complete draft encodes");
    // Serialised through serde, and trimmed, so the sent value is the one shown.
    assert!(body.contains(r#""baseUrl":"http://127.0.0.1:20128/v1""#));
    assert!(body.contains(r#""model":"gpt-5""#));
}

#[test]
fn an_api_key_containing_quotes_cannot_break_out_of_the_request_body() {
    let draft = ToolConfigDraft {
        base_url: String::from("http://127.0.0.1:20128/v1"),
        api_key: String::from(r#"sk-"},{"model":"evil"#),
        model: String::from("gpt-5"),
    };
    let body = draft.apply_body().expect("encodes");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("still valid JSON");
    assert_eq!(
        parsed.get("apiKey").and_then(serde_json::Value::as_str),
        Some(r#"sk-"},{"model":"evil"#)
    );
}

#[test]
fn a_501_write_is_reported_as_nothing_written_not_as_a_failed_write() {
    // `nullrouter-api` answers 501 for every tool. Saying "write failed" would
    // imply a file was touched.
    let outcome = settle_apply(Err(ApiError::Status(501)));
    assert!(matches!(outcome, ApplyOutcome::Unsupported(_)));
    assert!(!outcome.wrote_config());
    assert!(outcome.message().contains("Nothing was written"));

    // The same when the body carries the flag on a 2xx.
    let flagged = settle_apply(Ok(
        r#"{"success":false,"unsupported":true,"message":"not supported"}"#,
    ));
    assert!(matches!(flagged, ApplyOutcome::Unsupported(_)));
    assert!(!flagged.wrote_config());
}

#[test]
fn only_a_confirmed_write_reports_a_config_path() {
    let applied = settle_apply(Ok(
        r#"{"success":true,"message":"Codex settings applied successfully!","configPath":"/home/dev/.codex/config.toml"}"#,
    ));
    assert!(applied.wrote_config());
    assert!(applied.message().contains("/home/dev/.codex/config.toml"));

    let refused = settle_apply(Ok(r#"{"error":"baseUrl, apiKey and model are required"}"#));
    assert!(matches!(refused, ApplyOutcome::Refused(_)));
    assert!(!refused.wrote_config());

    // A body this build cannot read is a transport failure, not a success.
    assert_eq!(
        settle_apply(Ok("not json")),
        ApplyOutcome::Rejected(ApiError::Body)
    );
    assert_eq!(
        settle_apply(Err(ApiError::Network)),
        ApplyOutcome::Rejected(ApiError::Network)
    );
}

#[test]
fn an_unsupported_mcp_registry_is_not_reported_as_an_empty_one() {
    let unsupported = parse_mcp_registry(
        r#"{"cached":true,"servers":[],"total":0,"unsupported":true,"message":"Cowork MCP discovery is not supported by nullrouter-api"}"#,
    )
    .expect("the stub shape parses");
    assert!(unsupported.unsupported);
    // "We do not do this" must not read as "the registry is empty".
    assert!(!unsupported.is_empty());
    assert!(unsupported.summary().contains("not supported"));

    let empty = parse_mcp_registry(r#"{"servers":[],"total":0}"#).expect("parses");
    assert!(empty.is_empty());
    assert!(empty.summary().contains("holds no MCP servers"));

    let named = parse_mcp_registry(r#"{"servers":[{"name":"fs"},"git",{"nope":1}],"total":3}"#)
        .expect("parses");
    // An entry with no name contributes nothing rather than a blank row.
    assert_eq!(named.servers, ["fs", "git"]);

    assert!(parse_mcp_registry("[]").is_none());
    assert!(parse_mcp_registry("").is_none());
}

// ── translator ──────────────────────────────────────────────────────────────

#[test]
fn every_stage_names_a_file_the_endpoint_accepts() {
    // Mirrors `ALLOWED_FILES` in `services/api-actix/src/translator.rs`; a
    // control that named anything else would be answered 400.
    const ALLOWED: [&str; 8] = [
        "1_req_client.json",
        "2_req_source.json",
        "3_req_openai.json",
        "4_req_target.json",
        "5_res_provider.txt",
        "6_res_openai.txt",
        "7_res_client.txt",
        "7_res_client.json",
    ];
    for file in TranslatorFile::ALL {
        assert!(
            ALLOWED.contains(&file.file_name()),
            "{} is not an accepted file name",
            file.file_name()
        );
        if let Some(alternate) = file.alternate_file() {
            assert!(ALLOWED.contains(&alternate), "{alternate} is not accepted");
        }
    }
    // Only steps 1-3 offer a translate control, because the endpoint rejects
    // anything else.
    let translatable: Vec<u8> = TranslatorFile::ALL
        .into_iter()
        .filter_map(|file| file.translate_step().map(TranslateStep::number))
        .collect();
    assert_eq!(translatable, [1, 2, 3]);
}

#[test]
fn a_translate_result_with_no_body_writes_nothing_into_the_next_step() {
    // This is the exact response `nullrouter-api` returns: success, with an
    // empty body. The fixture panel would have shown a translated payload.
    let outcome = settle_translate(Ok(
        r#"{"success":true,"result":{"step":2,"provider":null,"model":null,"sourceFormat":"unknown","targetFormat":"unknown","body":{}}}"#,
    ));
    let TranslateOutcome::Translated(result) = outcome else {
        panic!("a success body should parse as a translation");
    };
    assert_eq!(result.body, None);
    assert_eq!(result.headers, None);
    // "unknown" is a non-answer and must not be shown as a detected format.
    assert_eq!(result.meta.source_format, None);
    assert_eq!(result.meta.target_format, None);
    assert_eq!(result.meta.provider, None);

    let badges = result.meta.badges();
    for (label, value) in badges {
        assert_eq!(value, "—", "{label} should have no reading");
    }
}

#[test]
fn a_real_translate_result_is_rendered_rather_than_a_canned_example() {
    let outcome = settle_translate(Ok(
        r#"{"success":true,"result":{"url":"https://api.anthropic.com/v1/messages","headers":{"x-api-key":"redacted","anthropic-version":"2023-06-01"},"body":{"model":"claude-sonnet-4","max_tokens":1024}}}"#,
    ));
    let TranslateOutcome::Translated(result) = outcome else {
        panic!("expected a translation");
    };
    let body = result.body.expect("a non-empty body is rendered");
    assert!(body.contains("claude-sonnet-4"));
    assert!(body.contains("max_tokens"));
    assert!(
        result
            .headers
            .expect("headers")
            .contains("anthropic-version")
    );
    assert_eq!(
        result.meta.url.as_deref(),
        Some("https://api.anthropic.com/v1/messages")
    );
}

#[test]
fn a_refused_or_unreadable_translate_is_not_a_translation() {
    assert_eq!(
        settle_translate(Ok(r#"{"success":false,"error":"Invalid step (1-3)"}"#)),
        TranslateOutcome::Refused(String::from("Invalid step (1-3)"))
    );
    // Success with no `result` is a shape change, so it fails rather than
    // rendering an empty step as a translation.
    assert_eq!(
        settle_translate(Ok(r#"{"success":true}"#)),
        TranslateOutcome::Rejected(ApiError::Body)
    );
    assert_eq!(
        settle_translate(Ok("")),
        TranslateOutcome::Rejected(ApiError::Body)
    );
    assert_eq!(
        settle_translate(Err(ApiError::Status(500))),
        TranslateOutcome::Rejected(ApiError::Status(500))
    );
}

#[test]
fn detected_metadata_accumulates_across_steps_without_being_erased() {
    let detected = TranslationMeta {
        provider: Some(String::from("anthropic")),
        model: Some(String::from("claude-sonnet-4")),
        source_format: Some(String::from("openai")),
        target_format: Some(String::from("anthropic")),
        url: None,
    };
    // Step 3 reports a URL and nothing else; the provider and model must survive.
    let later = TranslationMeta {
        url: Some(String::from("https://api.anthropic.com/v1/messages")),
        ..TranslationMeta::default()
    };
    let merged = merge_meta(&detected, later);
    assert_eq!(merged.provider.as_deref(), Some("anthropic"));
    assert_eq!(merged.model.as_deref(), Some("claude-sonnet-4"));
    assert_eq!(
        merged.url.as_deref(),
        Some("https://api.anthropic.com/v1/messages")
    );
}

#[test]
fn a_step_that_needs_a_provider_says_so_instead_of_sending_without_one() {
    let empty = TranslationMeta::default();
    let body = r#"{"model":"gpt-5"}"#;

    // Step 3 and send both require provider and model, mirroring the endpoint.
    assert_eq!(
        translate_body(TranslateStep::ToTarget, body, &empty),
        Err(RequestError::ProviderMissing)
    );
    assert_eq!(send_body(body, &empty), Err(RequestError::ProviderMissing));

    let partial = TranslationMeta {
        provider: Some(String::from("openai")),
        ..TranslationMeta::default()
    };
    assert_eq!(send_body(body, &partial), Err(RequestError::ModelMissing));

    // Steps 1 and 2 do not, so they proceed without one.
    assert!(translate_body(TranslateStep::ToOpenAi, body, &empty).is_ok());
}

#[test]
fn a_buffer_is_parsed_before_it_is_sent_so_a_broken_edit_cannot_be_posted() {
    let meta = TranslationMeta {
        provider: Some(String::from("openai")),
        model: Some(String::from("gpt-5")),
        ..TranslationMeta::default()
    };
    assert_eq!(
        translate_body(TranslateStep::ToOpenAi, "{\"a\":", &meta),
        Err(RequestError::BodyInvalid)
    );
    assert_eq!(send_body("   ", &meta), Err(RequestError::BodyEmpty));

    let sent = send_body(r#"{"model":"gpt-5","stream":true}"#, &meta).expect("encodes");
    let parsed: serde_json::Value = serde_json::from_str(&sent).expect("valid JSON");
    assert_eq!(
        parsed.get("provider").and_then(serde_json::Value::as_str),
        Some("openai")
    );
    // The body travels as a JSON value, not as an interpolated string.
    assert!(parsed.get("body").is_some_and(serde_json::Value::is_object));
}

#[test]
fn a_save_is_only_reported_as_written_when_the_router_wrote_it() {
    assert_eq!(settle_save(Ok(r#"{"success":true}"#)), SaveOutcome::Written);

    // The api-actix build answers 200 with `unsupported: true`.
    let unsupported = settle_save(Ok(
        r#"{"success":false,"unsupported":true,"error":"Translator log persistence is not supported"}"#,
    ));
    assert!(matches!(unsupported, SaveOutcome::Unsupported(_)));
    assert!(!unsupported.wrote_file());
    assert!(unsupported.message().starts_with("Not saved."));

    let refused = settle_save(Ok(
        r#"{"success":false,"error":"File and content required"}"#,
    ));
    assert!(matches!(refused, SaveOutcome::Refused(_)));
    assert!(!refused.wrote_file());

    assert_eq!(
        settle_save(Err(ApiError::Status(401))),
        SaveOutcome::Rejected(ApiError::Status(401))
    );

    let body = save_body(TranslatorFile::TargetRequest, "{}").expect("encodes");
    assert!(body.contains(r#""file":"4_req_target.json""#));
}

#[test]
fn a_provider_stream_is_kept_verbatim_and_a_json_refusal_is_not() {
    // A successful send returns text/event-stream, which must not be parsed.
    let stream = "data: {\"type\":\"content_block_delta\"}\n\ndata: [DONE]\n\n";
    assert_eq!(
        settle_send(Ok(stream)),
        SendOutcome::Answered(stream.to_owned())
    );

    let refused = settle_send(Ok(
        r#"{"success":false,"error":"No active connection for provider: anthropic"}"#,
    ));
    assert!(matches!(refused, SendOutcome::Refused(_)));

    let unsupported = settle_send(Err(ApiError::Status(501)));
    assert!(matches!(unsupported, SendOutcome::Unsupported(_)));
    assert!(unsupported.message().contains("Nothing was sent"));

    // An empty 200 is not a provider answer.
    assert!(matches!(settle_send(Ok("   ")), SendOutcome::Refused(_)));
}

#[test]
fn formatting_never_silently_replaces_what_the_user_typed() {
    assert_eq!(
        format_json("{\"b\":2}").as_deref(),
        Some("{\n  \"b\": 2\n}")
    );
    assert_eq!(format_json("{oops"), None);
    assert_eq!(format_json(""), None);
}

#[test]
fn a_step_always_states_where_its_content_came_from() {
    // There is deliberately no "preview" source: every label names a real origin.
    for source in [
        StepSource::Empty,
        StepSource::Loaded,
        StepSource::Edited,
        StepSource::Translated,
        StepSource::Received,
    ] {
        let label = source.label();
        assert!(!label.is_empty());
        assert!(
            !label.to_ascii_lowercase().contains("preview"),
            "{label} claims to be a preview"
        );
    }
    assert_eq!(StepSource::default(), StepSource::Empty);
}

// ── console log ─────────────────────────────────────────────────────────────

#[test]
fn the_events_service_init_frame_decodes_to_an_empty_replacement() {
    // Exactly what `services/events-actix/src/routes.rs` emits.
    let frame = parse_console_frame(r#"{"type":"init","logs":[],"liveCapture":false}"#)
        .expect("the documented frame decodes");
    assert_eq!(frame.kind, FrameKind::Init);
    assert_eq!(frame.lines, Vec::<String>::new());
    assert_eq!(frame.live_capture, Some(false));
    // `liveCapture: false` means no new lines will arrive, which is not "live".
    assert_eq!(
        StreamState::from_capture(frame.live_capture),
        StreamState::NotCapturing
    );
    assert!(!StreamState::NotCapturing.is_live());
}

#[test]
fn a_frame_missing_fields_yields_no_lines_rather_than_blank_ones() {
    // No `logs` at all.
    let bare = parse_console_frame(r#"{"type":"init"}"#).expect("a bare init frame decodes");
    assert_eq!(bare.kind, FrameKind::Init);
    assert!(bare.lines.is_empty());
    // An omitted flag is not read as "not capturing".
    assert_eq!(bare.live_capture, None);
    assert_eq!(
        StreamState::from_capture(bare.live_capture),
        StreamState::Live
    );

    // `logs` present but carrying nulls, which is what the events service's unit
    // struct would serialise to.
    let nulls = parse_console_frame(r#"{"type":"init","logs":[null,null]}"#).expect("decodes");
    assert!(nulls.lines.is_empty());

    // A clear frame carries no lines by construction.
    let cleared = parse_console_frame(r#"{"type":"clear"}"#).expect("decodes");
    assert_eq!(cleared.kind, FrameKind::Clear);
    assert!(cleared.lines.is_empty());
}

#[test]
fn a_frame_with_no_recognised_type_is_ignored() {
    for data in [
        "",
        "not json",
        "[]",
        "null",
        "{}",
        r#"{"logs":["orphan"]}"#,
        r#"{"type":"heartbeat","logs":["x"]}"#,
        r#"{"type":123}"#,
    ] {
        assert!(
            parse_console_frame(data).is_none(),
            "{data:?} should be ignored, not appended"
        );
    }
}

#[test]
fn upstream_line_and_lines_frames_both_append() {
    let single = parse_console_frame(r#"{"type":"line","line":"[9Router] [INFO] ready"}"#)
        .expect("a line frame decodes");
    assert_eq!(single.kind, FrameKind::Append);
    assert_eq!(single.lines, ["[9Router] [INFO] ready"]);

    let batch = parse_console_frame(r#"{"type":"lines","lines":["one","two"]}"#)
        .expect("a lines frame decodes");
    assert_eq!(batch.kind, FrameKind::Append);
    assert_eq!(batch.lines, ["one", "two"]);

    // Object-wrapped entries are read too, so a future shape does not become
    // blank rows.
    let wrapped =
        parse_console_frame(r#"{"type":"lines","lines":[{"line":"wrapped"}]}"#).expect("decodes");
    assert_eq!(wrapped.lines, ["wrapped"]);
}

#[test]
fn the_connected_frame_is_read_without_claiming_capture() {
    assert_eq!(
        parse_connected_frame(
            r#"{"service":"nullrouter-events","stream":"translator.console_logs","connected":true}"#
        ),
        Some(true)
    );
    assert_eq!(parse_connected_frame(r#"{"connected":false}"#), Some(false));
    // Absent flag is not "connected".
    assert_eq!(parse_connected_frame("{}"), Some(false));
    assert_eq!(parse_connected_frame("[]"), None);
    assert_eq!(parse_connected_frame(""), None);
}

#[test]
fn a_long_running_stream_cannot_grow_the_buffer_without_limit() {
    let mut buffer = LogBuffer::default();
    // Ten times the cap, arriving in batches, as a busy router would.
    for batch in 0..100_u32 {
        buffer.extend((0..20).map(|line| format!("batch {batch} line {line}")));
    }

    assert_eq!(buffer.len(), MAX_LINES);
    assert_eq!(buffer.received(), 2000);
    assert_eq!(buffer.dropped(), 2000 - MAX_LINES as u64);
    // The newest lines are the ones kept.
    let last = buffer.lines().last().expect("a last line");
    assert_eq!(last.text, "batch 99 line 19");
    // And the panel says so rather than implying it holds everything.
    assert!(buffer.trim_label().contains("older dropped"));
    assert_eq!(buffer.retained_label(), format!("{MAX_LINES} retained"));
}

#[test]
fn a_replacing_frame_also_respects_the_cap() {
    let mut buffer = LogBuffer::default();
    buffer.replace((0..MAX_LINES + 25).map(|line| format!("line {line}")));
    assert_eq!(buffer.len(), MAX_LINES);
    assert_eq!(buffer.dropped(), 25);
    // A snapshot of existing output is not new output, so nothing pulses.
    assert!(buffer.lines().iter().all(|line| !line.fresh));
}

#[test]
fn only_the_newest_arrival_is_marked_fresh() {
    let mut buffer = LogBuffer::default();
    buffer.push(String::from("first"));
    buffer.push(String::from("second"));
    let fresh: Vec<&str> = buffer
        .lines()
        .iter()
        .filter(|line| line.fresh)
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(fresh, ["second"]);

    buffer.extend([String::from("third"), String::from("fourth")]);
    let fresh: Vec<&str> = buffer
        .lines()
        .iter()
        .filter(|line| line.fresh)
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(fresh, ["third", "fourth"]);
    // Sequence numbers are unique, so two identical lines stay two rows.
    let sequences: Vec<u64> = buffer.lines().iter().map(|line| line.sequence).collect();
    assert_eq!(sequences, [1, 2, 3, 4]);
}

#[test]
fn a_lines_level_comes_from_its_second_bracketed_tag() {
    // Upstream colours by `match[1]`, because the first tag is the subsystem.
    for (line, expected) in [
        ("[9Router] [WARN] quota low", LogLevel::Warn),
        ("[9Router] [ERROR] upstream 500", LogLevel::Error),
        ("[gateway] [DEBUG] cache hit", LogLevel::Debug),
        ("[gateway] [INFO] listening", LogLevel::Info),
        ("[gateway] [LOG] started", LogLevel::Log),
        // No second tag, or an unrecognised one, is plain LOG.
        ("[WARN] only one tag", LogLevel::Log),
        ("no tags at all", LogLevel::Log),
        ("[a] [2024-01-01] timestamped", LogLevel::Log),
    ] {
        assert_eq!(LogLevel::from_line(line), expected, "{line}");
    }
}

#[test]
fn a_new_line_pulses_and_an_old_one_does_not() {
    let mut buffer = LogBuffer::default();
    buffer.push(String::from("[nr] [ERROR] boom"));
    buffer.push(String::from("[nr] [INFO] fine"));
    let lines = buffer.lines();
    let old = lines.first().expect("first line");
    let new = lines.get(1).expect("second line");

    assert!(old.class_name().contains("nr-console-level-error"));
    assert!(!old.class_name().contains("nr-tick"));
    assert!(new.class_name().contains("nr-console-level-info"));
    assert!(new.class_name().contains("nr-tick"));
}

#[test]
fn an_empty_history_response_is_an_empty_console_not_a_failure() {
    assert_eq!(
        parse_history(r#"{"success":true,"logs":[]}"#),
        Some(Vec::new())
    );
    assert_eq!(
        parse_history(r#"{"success":true,"logs":["[nr] [INFO] up"]}"#),
        Some(vec![String::from("[nr] [INFO] up")])
    );
}

#[test]
fn a_history_response_without_a_logs_array_is_a_failure() {
    // The panel must show "could not be read", not "no console logs yet".
    for body in [
        "",
        "not json",
        "{}",
        "[]",
        r#"{"logs":null}"#,
        r#"{"logs":"x"}"#,
    ] {
        assert!(
            parse_history(body).is_none(),
            "{body:?} should not parse into a history"
        );
    }
}

#[test]
fn a_clear_is_only_reported_as_done_when_the_router_confirmed_it() {
    assert_eq!(
        settle_clear(Ok(r#"{"success":true}"#)),
        ClearOutcome::Cleared
    );
    assert!(ClearOutcome::Cleared.succeeded());

    let refused = settle_clear(Ok(r#"{"success":false}"#));
    assert_eq!(refused, ClearOutcome::Refused);
    assert!(!refused.succeeded());
    assert!(refused.message().contains("may remain"));

    assert_eq!(
        settle_clear(Ok("not json")),
        ClearOutcome::Rejected(ApiError::Body)
    );
    assert_eq!(
        settle_clear(Err(ApiError::Network)),
        ClearOutcome::Rejected(ApiError::Network)
    );
}

#[test]
fn every_stream_state_explains_itself_and_only_one_is_live() {
    let states = [
        StreamState::Connecting,
        StreamState::Live,
        StreamState::NotCapturing,
        StreamState::Interrupted,
        StreamState::Unavailable,
    ];
    for state in states {
        assert!(!state.label().is_empty());
        let detail = state.detail();
        assert!(detail.ends_with('.'), "{detail} should read as a sentence");
        assert!(!state.class_name().is_empty());
    }
    assert_eq!(states.iter().filter(|state| state.is_live()).count(), 1);
    // A dropped feed must say the entries on screen are not current.
    assert!(StreamState::Interrupted.detail().contains("not current"));
    assert_eq!(StreamState::Interrupted.label(), "Disconnected");
}
