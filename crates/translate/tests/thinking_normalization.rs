//! Provider-native thinking normalization.
//!
//! The regression these lock down is silent: `thinkingFormat` was carried in the
//! capability table and never applied, so a `reasoning_effort: "high"` bound for
//! an Anthropic budget model went upstream unchanged. Anthropic ignores the field,
//! the caller is billed for a non-reasoning answer, and nothing reports a problem.
//! The reverse — a `thinking: {budget_tokens}` sent to OpenAI — is a 400.
//!
//! So the assertions here are mostly "the field the *target* reads is present, and
//! the one it would choke on is gone".

use nullrouter_providers::Format;
use nullrouter_translate::thinking::{
    ThinkingIntent, apply, extract_thinking, parse_suffix, strip_thinking_suffix,
};
use serde_json::{Value, json};

/// Apply thinking to a body for one provider/model, returning the result.
fn normalized(target: Format, provider: &str, model: &str, body: &Value) -> Value {
    let mut result = body.clone();
    let intent = extract_thinking(body);
    apply(target, provider, model, &mut result, intent.as_ref());
    result
}

#[test]
fn openai_effort_becomes_an_anthropic_budget() {
    // Given: an OpenAI-shaped reasoning request bound for a Claude budget model.
    let body = json!({
        "model": "claude-sonnet-4-20250514",
        "reasoning_effort": "high",
        "messages": [{"role": "user", "content": "hi"}],
    });

    // When: it is normalized for Anthropic.
    let result = normalized(
        Format::Claude,
        "anthropic",
        "claude-sonnet-4-20250514",
        &body,
    );

    // Then: the effort is re-expressed as a token budget, and the OpenAI spelling
    // is gone. Leaving `reasoning_effort` beside `thinking` would send two
    // conflicting instructions; leaving it *instead* is the original bug.
    assert_eq!(
        result.get("thinking"),
        Some(&json!({"type": "enabled", "budget_tokens": 24576})),
        "high must map to the documented 24576-token budget"
    );
    assert!(
        result.get("reasoning_effort").is_none(),
        "the OpenAI spelling must not survive"
    );
}

#[test]
fn an_anthropic_budget_becomes_an_openai_effort() {
    // Given: a Claude-shaped thinking request bound for an OpenAI model.
    let body = json!({
        "model": "gpt-5",
        "thinking": {"type": "enabled", "budget_tokens": 24576},
        "messages": [{"role": "user", "content": "hi"}],
    });

    // When: it is normalized for OpenAI.
    let result = normalized(Format::OpenAi, "openai", "gpt-5", &body);

    // Then: the budget is re-expressed as a discrete level. Sending the Anthropic
    // object to OpenAI is a 400, not a downgrade.
    assert_eq!(result.get("reasoning_effort"), Some(&json!("high")));
    assert!(result.get("thinking").is_none());
}

#[test]
fn a_disable_survives_the_round_trip_in_each_dialect() {
    // Given: "stop reasoning", expressed three different ways.
    // When/Then: each target receives its own disable spelling, because none of
    // them reads the others'.
    let openai_off = normalized(
        Format::OpenAi,
        "openai",
        "gpt-5",
        &json!({"reasoning_effort": "none"}),
    );
    assert_eq!(openai_off.get("reasoning_effort"), Some(&json!("none")));

    let claude_off = normalized(
        Format::Claude,
        "anthropic",
        "claude-sonnet-4-20250514",
        &json!({"reasoning_effort": "none"}),
    );
    assert_eq!(
        claude_off.get("thinking"),
        Some(&json!({"type": "disabled"}))
    );

    let gemini_off = normalized(
        Format::Gemini,
        "gemini",
        "gemini-2.5-pro",
        &json!({"reasoning_effort": "none"}),
    );
    assert_eq!(
        gemini_off
            .get("generationConfig")
            .and_then(|config| config.get("thinkingConfig")),
        Some(&json!({"thinkingBudget": 0, "includeThoughts": false}))
    );
}

#[test]
fn a_non_reasoning_model_has_thinking_stripped_entirely() {
    // Given: a reasoning request aimed at a model that cannot reason. Several
    // providers 400 on an unrecognised top-level field rather than ignoring it.
    let body = json!({
        "model": "gpt-4o-mini",
        "reasoning_effort": "high",
        "thinking": {"type": "enabled", "budget_tokens": 8192},
        "enable_thinking": true,
    });

    // When: it is normalized.
    let result = normalized(Format::OpenAi, "openai", "gpt-4o-mini", &body);

    // Then: every thinking spelling is removed, not translated.
    for key in ["reasoning_effort", "thinking", "enable_thinking"] {
        assert!(result.get(key).is_none(), "{key} should have been stripped");
    }
}

#[test]
fn a_request_with_no_thinking_intent_is_left_alone() {
    // Given: an ordinary request that says nothing about reasoning.
    let body = json!({
        "model": "claude-sonnet-4-20250514",
        "messages": [{"role": "user", "content": "hi"}],
        "temperature": 0.7,
    });

    // When: it is normalized.
    let result = normalized(
        Format::Claude,
        "anthropic",
        "claude-sonnet-4-20250514",
        &body,
    );

    // Then: nothing is invented. Defaulting a reasoning model to "on" would spend
    // the caller's tokens on a choice they never made.
    assert_eq!(result, body);
    assert!(result.get("thinking").is_none());
}

#[test]
fn a_model_that_cannot_disable_thinking_gets_minimal_instead_of_off() {
    // Given: a Gemini 3 model, whose thinkingLevel enum has no "off".
    let body = json!({"reasoning_effort": "none"});

    // When: thinking is switched off for it.
    let result = normalized(Format::Gemini, "gemini", "gemini-3.7-flash", &body);
    let config = result
        .get("generationConfig")
        .and_then(|config| config.get("thinkingConfig"));

    // Then: it asks for the smallest level that exists rather than a disable the
    // model would reject.
    assert_eq!(
        config,
        Some(&json!({"thinkingLevel": "minimal", "includeThoughts": false})),
        "gemini-level has no off switch, so none clamps to minimal"
    );
}

#[test]
fn a_gemini_budget_is_clamped_to_the_models_stated_range() {
    // Given: a budget far above what gemini-2.5 accepts (its range caps at 24576).
    let body = json!({"thinking": {"type": "enabled", "budget_tokens": 999_999}});

    // When: it is normalized for that model.
    let result = normalized(Format::Gemini, "gemini", "gemini-2.5-pro", &body);
    let config = result
        .get("generationConfig")
        .and_then(|config| config.get("thinkingConfig"))
        .and_then(|config| config.get("thinkingBudget"));

    // Then: it is clamped rather than sent as-is and rejected.
    assert_eq!(config, Some(&json!(24576)));
}

#[test]
fn gemini_thinking_raises_the_output_ceiling() {
    // Given: a Gemini request with a small output ceiling and thinking on. Gemini
    // draws reasoning tokens from the output budget, so leaving the ceiling alone
    // spends it on thinking and truncates the visible answer.
    let body = json!({
        "generationConfig": {"maxOutputTokens": 256},
        "thinking": {"type": "enabled", "budget_tokens": 24576},
    });

    // When: it is normalized.
    let result = normalized(Format::Gemini, "gemini", "gemini-2.5-pro", &body);
    let generation = result
        .get("generationConfig")
        .and_then(Value::as_object)
        .expect("generationConfig should be present");

    // Then: the ceiling is raised to the floor for that budget.
    assert_eq!(
        generation.get("maxOutputTokens"),
        Some(&json!(32768)),
        "a 24576-token budget needs at least a 32768 ceiling"
    );
}

#[test]
fn the_gemini_cli_envelope_is_written_not_the_top_level() {
    // Given: a gemini-cli request, which wraps everything in a `request` envelope.
    // Writing to the top-level generationConfig sets a field the provider never
    // reads, which looks exactly like thinking silently not working.
    let body = json!({
        "request": {
            "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
            "generationConfig": {"maxOutputTokens": 256},
        },
        "reasoning_effort": "high",
    });

    // When: it is normalized for gemini-cli.
    let result = normalized(Format::GeminiCli, "gemini-cli", "gemini-2.5-pro", &body);

    // Then: the config inside the envelope carries it, and the top level does not
    // grow a stray one.
    let inner = result
        .get("request")
        .and_then(|request| request.get("generationConfig"))
        .and_then(|config| config.get("thinkingConfig"));
    assert!(inner.is_some(), "the envelope's config must be written");
    assert!(
        result.get("generationConfig").is_none(),
        "no top-level config should be created for an enveloped request"
    );
}

#[test]
fn zai_is_disabled_with_the_boolean_it_actually_reads() {
    // Given: a Z.ai model. It ignores an Anthropic-style `thinking: disabled`, so
    // sending only that would bill full-price reasoning while the caller believes
    // it is off.
    let body = json!({"thinking": {"type": "disabled"}});

    // When: it is normalized. glm-4.6 carries the `zai` thinking format.
    let result = normalized(Format::OpenAi, "alicode-intl", "glm-5", &body);

    // Then: the boolean it does read is set, and the field it ignores is removed.
    assert_eq!(result.get("enable_thinking"), Some(&json!(false)));
    assert!(
        result.get("thinking").is_none(),
        "the ignored Anthropic field must not be left behind"
    );
}

#[test]
fn a_model_suffix_overrides_the_body() {
    // Given: a request whose body asks for low effort but whose model id pins high.
    // The suffix is the more specific statement, so it wins.
    let body = json!({"reasoning_effort": "low"});
    let mut result = body.clone();
    let intent = extract_thinking(&body);
    apply(
        Format::OpenAi,
        "openai",
        "gpt-5(high)",
        &mut result,
        intent.as_ref(),
    );

    // Then: the suffix's level is what goes upstream.
    assert_eq!(result.get("reasoning_effort"), Some(&json!("high")));
}

#[test]
fn a_thinking_suffix_never_reaches_the_provider() {
    // Given: nullrouter's own `model(level)` routing syntax. A provider would
    // reject it as an unknown model id.
    // When/Then: it is split off the id, and the bare model survives.
    assert_eq!(parse_suffix("gpt-5(high)").0, "gpt-5");
    assert_eq!(strip_thinking_suffix("gpt-5(high)"), "gpt-5");
    assert_eq!(strip_thinking_suffix("gpt-5(8192)"), "gpt-5");
    assert_eq!(strip_thinking_suffix("gpt-5(none)"), "gpt-5");

    // And: a model id with no suffix is untouched, including one with parens that
    // do not form a suffix.
    assert_eq!(strip_thinking_suffix("gpt-5"), "gpt-5");
    assert_eq!(strip_thinking_suffix("gpt-5()"), "gpt-5()");
}

#[test]
fn every_suffix_form_parses_to_the_right_intent() {
    // Given: the suffix grammar — level, numeric budget, off, and auto.
    // When/Then: each maps to the intent it names.
    assert_eq!(
        parse_suffix("m(high)").1,
        Some(ThinkingIntent::Level(String::from("high")))
    );
    assert_eq!(
        parse_suffix("m(8192)").1,
        Some(ThinkingIntent::Budget(8192))
    );
    assert_eq!(parse_suffix("m(none)").1, Some(ThinkingIntent::Off));
    assert_eq!(parse_suffix("m(off)").1, Some(ThinkingIntent::Off));
    assert_eq!(parse_suffix("m(auto)").1, Some(ThinkingIntent::Auto));
    // Case is normalised, since the suffix is user-typed.
    assert_eq!(
        parse_suffix("m(HIGH)").1,
        Some(ThinkingIntent::Level(String::from("high")))
    );
    // An unrecognised suffix names no intent, but is still stripped.
    assert_eq!(parse_suffix("m(banana)"), ("m", None));
}

#[test]
fn each_dialect_is_read_back_as_the_same_intent() {
    // Given: "spend 24576 tokens thinking", written five different ways.
    // When/Then: all five read as the same intent, which is what makes the
    // cross-format translation above possible at all.
    for body in [
        json!({"thinking": {"type": "enabled", "budget_tokens": 24576}}),
        json!({"thinkingConfig": {"thinkingBudget": 24576}}),
        json!({"generationConfig": {"thinkingConfig": {"thinkingBudget": 24576}}}),
        json!({"request": {"generationConfig": {"thinkingConfig": {"thinkingBudget": 24576}}}}),
        json!({"enable_thinking": true, "thinking_budget": 24576}),
    ] {
        assert_eq!(
            extract_thinking(&body),
            Some(ThinkingIntent::Budget(24576)),
            "failed to read intent from {body}"
        );
    }
}

#[test]
fn the_sign_of_a_gemini_budget_is_load_bearing() {
    // Given: Gemini overloads thinkingBudget — 0 disables, negative means "you
    // decide". Reading either as a quantity would be wrong in opposite directions.
    assert_eq!(
        extract_thinking(&json!({"thinkingConfig": {"thinkingBudget": 0}})),
        Some(ThinkingIntent::Off)
    );
    assert_eq!(
        extract_thinking(&json!({"thinkingConfig": {"thinkingBudget": -1}})),
        Some(ThinkingIntent::Auto)
    );
    assert_eq!(
        extract_thinking(&json!({"thinkingConfig": {"thinkingBudget": 8192}})),
        Some(ThinkingIntent::Budget(8192))
    );
}

#[test]
fn an_explicit_effort_outranks_an_adaptive_marker() {
    // Given: a Claude request carrying both. `output_config.effort` is the more
    // specific statement, so upstream checks it first — and so must this.
    let body = json!({
        "thinking": {"type": "adaptive"},
        "output_config": {"effort": "low"},
    });

    // When/Then: the explicit effort is what is read.
    assert_eq!(
        extract_thinking(&body),
        Some(ThinkingIntent::Level(String::from("low")))
    );
}

#[test]
fn a_responses_api_reasoning_object_is_read() {
    // Given: the Responses API nests effort under `reasoning`, unlike chat's flat
    // `reasoning_effort`.
    assert_eq!(
        extract_thinking(&json!({"reasoning": {"effort": "medium"}})),
        Some(ThinkingIntent::Level(String::from("medium")))
    );
    // And: an adaptive request with no budget is "provider decides", not "off".
    assert_eq!(
        extract_thinking(&json!({"thinking": {"type": "adaptive"}})),
        Some(ThinkingIntent::Auto)
    );
}

#[test]
fn deepseek_collapses_the_level_range_to_its_own_two() {
    // Given: DeepSeek's effort enum is only high and max.
    // When/Then: lower levels round up to high, and xhigh/max reach max. Sending
    // "low" would be rejected outright.
    for (requested, expected) in [
        ("low", "high"),
        ("medium", "high"),
        ("high", "high"),
        ("xhigh", "max"),
        ("max", "max"),
    ] {
        let body = json!({"reasoning_effort": requested});
        let result = normalized(Format::OpenAi, "deepseek", "deepseek-v4-pro", &body);
        assert_eq!(
            result.get("reasoning_effort"),
            Some(&json!(expected)),
            "deepseek {requested} should send {expected}"
        );
        assert_eq!(result.get("thinking"), Some(&json!({"type": "enabled"})));
    }
}

#[test]
fn apply_reads_the_body_when_handed_no_captured_intent() {
    // Given: a caller that applies thinking without having captured the intent
    // first. Depending on caller discipline here would mean a forgotten capture
    // silently drops the user's reasoning request.
    let mut result = json!({"reasoning_effort": "high"});

    // When: apply is called with no intent at all.
    apply(
        Format::Claude,
        "anthropic",
        "claude-sonnet-4-20250514",
        &mut result,
        None,
    );

    // Then: the body's own intent is still honoured.
    assert_eq!(
        result.get("thinking"),
        Some(&json!({"type": "enabled", "budget_tokens": 24576}))
    );
}

#[test]
fn qwen_sends_its_boolean_plus_a_budget() {
    // Given: a Qwen model, which gates thinking on a boolean and takes the budget
    // beside it rather than inside an object.
    let body = json!({"reasoning_effort": "medium"});

    // When: it is normalized.
    let result = normalized(Format::OpenAi, "alicode-intl", "qwen3.5-plus", &body);

    // Then: both fields go out, and no Anthropic-shaped object does.
    assert_eq!(result.get("enable_thinking"), Some(&json!(true)));
    assert_eq!(result.get("thinking_budget"), Some(&json!(8192)));
    assert!(result.get("thinking").is_none());
}
