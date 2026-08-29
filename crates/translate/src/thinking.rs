//! Provider-native thinking normalization.
//!
//! Ports `open-sse/translator/concerns/thinkingUnified.js` plus the level/budget
//! maps in `open-sse/translator/concerns/thinking.js`.
//!
//! Every vendor spells "think harder" differently: OpenAI takes a
//! `reasoning_effort` enum, Anthropic takes either an adaptive marker or a
//! `budget_tokens` integer, Gemini nests a `thinkingConfig` under
//! `generationConfig`, Qwen uses a boolean plus a budget, and Z.ai ignores the
//! Anthropic-style disable entirely. The shape here is therefore two steps:
//! read the client's *intent* out of whichever dialect it arrived in, then
//! re-emit that intent in the target provider's own spelling.
//!
//! Without this pass the field is simply carried to a provider that does not
//! read it. A `reasoning_effort: "high"` sent to an Anthropic budget model is
//! dropped on the floor — the user pays for a non-reasoning answer and is told
//! nothing — and a `thinking: {budget_tokens}` sent to OpenAI is a 400. Both
//! failures are silent from the caller's side, which is why this runs on every
//! request rather than only when the formats differ.
//!
//! [`extract_thinking`] must be called on the request **before** translation and
//! [`apply`] **after** it: the request translators only carry the fields their own
//! format owns, so the intent has to be captured while it is still readable.

use nullrouter_providers::{Capabilities, Format, ThinkingFormat, ThinkingRange};
use serde_json::{Map, Value, json};

/// The largest budget that survives a round trip through a JS `Number`.
///
/// Upstream reads these fields with `Number()`, so a value beyond the
/// double-precision integer range is not something it could have handled either.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// What the client asked for, independent of the dialect it asked in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkingIntent {
    /// Thinking off.
    Off,
    /// Let the provider decide how much to spend.
    Auto,
    /// A discrete effort level (`low`, `high`, `xhigh`, `ultra`, …).
    Level(String),
    /// An explicit token budget. Always positive; `0` reads as [`Self::Off`] and
    /// a negative value as [`Self::Auto`].
    Budget(u64),
}

/// Web-standard effort level to `budget_tokens`, per Anthropic's and Google's
/// published tables. `None` for a level neither vendor defines.
fn level_to_budget(level: &str) -> Option<u64> {
    Some(match level {
        "none" => 0,
        "minimal" => 512,
        "low" => 1024,
        "medium" => 8192,
        "high" => 24576,
        "xhigh" => 32768,
        "max" => 128_000,
        _ => return None,
    })
}

/// A numeric budget as the nearest discrete level. `None` at or below zero,
/// which is not a reasoning request at all.
const fn budget_to_level(budget: u64) -> Option<&'static str> {
    if budget == 0 {
        return None;
    }
    Some(if budget <= 768 {
        "minimal"
    } else if budget <= 4096 {
        "low"
    } else if budget <= 16384 {
        "medium"
    } else if budget <= 28672 {
        "high"
    } else {
        "xhigh"
    })
}

/// An OpenAI effort as a Gemini 3 `thinkingLevel`.
///
/// Gemini 3 cannot switch thinking off, so an "off" request becomes the smallest
/// level it does accept rather than an argument it would reject.
fn effort_to_thinking_level(effort: &str) -> &str {
    match effort {
        "none" | "off" => "minimal",
        "xhigh" | "max" => "high",
        other => other,
    }
}

/// A JSON number read as a signed budget.
///
/// The sign is load-bearing and must not be dropped: Gemini overloads this field
/// so that `0` disables thinking and a negative value means "provider decides".
fn budget_signal(value: &Value) -> Option<i64> {
    if let Some(exact) = value.as_i64() {
        return Some(exact);
    }
    // A fractional budget is not a shape any vendor documents, but upstream reads
    // the field with `Number()` and accepts one, so truncate rather than ignore.
    let approximate = value.as_f64()?;
    if !approximate.is_finite() || approximate > MAX_SAFE_INTEGER || approximate < -MAX_SAFE_INTEGER
    {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "bounded to the JS safe-integer range by the check above"
    )]
    Some(approximate.trunc() as i64)
}

/// A budget field read as a strictly positive token count.
fn positive_budget(value: Option<&Value>) -> Option<u64> {
    let signal = budget_signal(value?)?;
    u64::try_from(signal).ok().filter(|budget| *budget > 0)
}

/// An effort string, in any dialect, as an intent.
fn intent_from_effort(effort: &str) -> ThinkingIntent {
    let lowered = effort.to_ascii_lowercase();
    if lowered == "none" || lowered == "off" {
        ThinkingIntent::Off
    } else if lowered == "auto" {
        ThinkingIntent::Auto
    } else {
        ThinkingIntent::Level(lowered)
    }
}

/// Where a Gemini request carries its `thinkingConfig` for *reading*.
///
/// gemini-cli and antigravity wrap the whole request in a `{ request: … }`
/// envelope, so all three positions have to be probed.
fn gemini_thinking_config(object: &Map<String, Value>) -> Option<&Value> {
    object
        .get("thinkingConfig")
        .or_else(|| {
            object
                .get("generationConfig")
                .and_then(|config| config.get("thinkingConfig"))
        })
        .or_else(|| {
            object
                .get("request")
                .and_then(|request| request.get("generationConfig"))
                .and_then(|config| config.get("thinkingConfig"))
        })
        .filter(|config| config.is_object())
}

/// A `(...)` thinking suffix as an intent, or `None` when it names no level.
fn thinking_suffix_intent(raw: &str) -> Option<ThinkingIntent> {
    match raw {
        "none" | "off" => Some(ThinkingIntent::Off),
        "auto" => Some(ThinkingIntent::Auto),
        // `ultra` is above every level in the shared table, so it has no budget.
        "ultra" => Some(ThinkingIntent::Level(raw.to_owned())),
        _ if !raw.is_empty() && raw.bytes().all(|byte| byte.is_ascii_digit()) => {
            raw.parse().ok().map(ThinkingIntent::Budget)
        }
        _ if level_to_budget(raw).is_some() => Some(ThinkingIntent::Level(raw.to_owned())),
        _ => None,
    }
}

/// Split a trailing `model(value)` thinking suffix off a model id.
///
/// Returns the bare model plus whatever the suffix asked for. Mirrors upstream's
/// `^(.*)\([^()]+\)\s*$`: the *last* parenthesised group wins, it may not be
/// empty or contain nested parens, and trailing whitespace is tolerated.
///
/// A suffix that names nothing recognisable is still stripped, as upstream does —
/// it is a routing hint the provider must not receive either way.
pub fn parse_suffix(model: &str) -> (&str, Option<ThinkingIntent>) {
    // The grammar itself lives in `nullrouter_providers::split_thinking_suffix`,
    // which `upstream_model_id` already uses to decide what reaches the provider.
    // Reimplementing it here would let the two drift, and a suffix that one
    // stripped while the other kept would go upstream as an unknown model id.
    let (clean, suffix) = nullrouter_providers::split_thinking_suffix(model);
    let Some(inner) = suffix
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return (clean, None);
    };
    (
        clean,
        thinking_suffix_intent(&inner.trim().to_ascii_lowercase()),
    )
}

/// Drop a trailing `(...)` thinking suffix. A no-op when there is none.
///
/// Providers must never see the suffix: it is nullrouter's own routing syntax and
/// would be rejected as an unknown model id.
pub fn strip_thinking_suffix(model: &str) -> &str {
    parse_suffix(model).0
}

/// Read the client's thinking intent out of a request body.
///
/// Must run **before** translation: each request translator only carries the
/// fields its own format owns, so by the time the body has been converted the
/// original intent is gone.
///
/// Probe order is upstream's and is load-bearing. Anthropic's
/// `output_config.effort` is checked before `thinking`, because a request may
/// carry both and the explicit effort is the more specific statement.
pub fn extract_thinking(body: &Value) -> Option<ThinkingIntent> {
    extract_thinking_object(body.as_object()?)
}

/// [`extract_thinking`] against an already-unwrapped object.
fn extract_thinking_object(object: &Map<String, Value>) -> Option<ThinkingIntent> {
    // Claude `output_config.effort`.
    if let Some(effort) = object
        .get("output_config")
        .and_then(|config| config.get("effort"))
        .and_then(Value::as_str)
        .filter(|effort| !effort.is_empty())
    {
        return Some(intent_from_effort(effort));
    }

    // Claude `thinking`.
    if let Some(thinking) = object.get("thinking").and_then(Value::as_object) {
        match thinking.get("type").and_then(Value::as_str) {
            Some("disabled") => return Some(ThinkingIntent::Off),
            Some("adaptive" | "enabled") => {
                return Some(
                    positive_budget(thinking.get("budget_tokens"))
                        .map_or(ThinkingIntent::Auto, ThinkingIntent::Budget),
                );
            }
            _ => {}
        }
    }

    // OpenAI chat `reasoning_effort`, or Responses `reasoning.effort`.
    let effort = object
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .or_else(|| {
            object
                .get("reasoning")
                .filter(|reasoning| reasoning.is_object())
                .and_then(|reasoning| reasoning.get("effort"))
                .and_then(Value::as_str)
        });
    if let Some(effort) = effort.filter(|effort| !effort.is_empty()) {
        return Some(intent_from_effort(effort));
    }

    extract_gemini_or_qwen(object)
}

/// The Gemini and Qwen halves of [`extract_thinking`], split out to keep the
/// probe order readable.
fn extract_gemini_or_qwen(object: &Map<String, Value>) -> Option<ThinkingIntent> {
    if let Some(config) = gemini_thinking_config(object) {
        if let Some(level) = config.get("thinkingLevel").and_then(Value::as_str) {
            return Some(ThinkingIntent::Level(level.to_ascii_lowercase()));
        }
        if let Some(budget) = config.get("thinkingBudget").and_then(budget_signal) {
            // Gemini overloads the sign: 0 is off, negative is "you decide".
            if budget == 0 {
                return Some(ThinkingIntent::Off);
            }
            if budget < 0 {
                return Some(ThinkingIntent::Auto);
            }
            return Some(
                u64::try_from(budget).map_or(ThinkingIntent::Auto, ThinkingIntent::Budget),
            );
        }
    }

    // Qwen: a boolean switch with an optional budget beside it.
    match object.get("enable_thinking").and_then(Value::as_bool) {
        Some(false) => Some(ThinkingIntent::Off),
        Some(true) => Some(
            positive_budget(object.get("thinking_budget"))
                .map_or(ThinkingIntent::Auto, ThinkingIntent::Budget),
        ),
        None => None,
    }
}

/// Remove every thinking field this module knows how to write.
///
/// Run before re-applying, so a request that arrived in one dialect cannot leave
/// carrying both its original spelling and the target's.
fn strip_all(object: &mut Map<String, Value>) {
    for key in [
        "thinking",
        "reasoning_effort",
        "reasoning",
        "thinkingConfig",
        "enable_thinking",
        "thinking_budget",
        "output_config",
    ] {
        object.remove(key);
    }
    if let Some(config) = object
        .get_mut("generationConfig")
        .and_then(Value::as_object_mut)
    {
        config.remove("thinkingConfig");
    }
    if let Some(config) = object
        .get_mut("request")
        .and_then(Value::as_object_mut)
        .and_then(|request| request.get_mut("generationConfig"))
        .and_then(Value::as_object_mut)
    {
        config.remove("thinkingConfig");
    }
}

/// The thinking format a target wire format uses when the model names none.
///
/// Mirrors upstream's `FORMAT_TO_NATIVE`, whose lookup miss resolves to
/// `"openai"` — the shape the widest set of providers accepts.
const fn native_format(target: Format) -> ThinkingFormat {
    match target {
        Format::Claude => ThinkingFormat::ClaudeBudget,
        Format::Gemini | Format::GeminiCli | Format::Vertex | Format::Antigravity => {
            ThinkingFormat::GeminiBudget
        }
        Format::Kiro => ThinkingFormat::Kiro,
        Format::OpenAi
        | Format::OpenAiResponses
        | Format::Codex
        | Format::Cursor
        | Format::Ollama
        | Format::CommandCode
        | Format::GrokWeb
        | Format::PerplexityWeb => ThinkingFormat::OpenAi,
    }
}

/// An intent as a token budget, clamped to the model's stated range.
///
/// `Some(-1)` is the "provider decides" sentinel that budget-shaped formats use;
/// `None` means the intent names no budget at all.
fn to_budget(intent: &ThinkingIntent, range: Option<ThinkingRange>) -> Option<i64> {
    let mut budget = match intent {
        // The clamp is deliberately not applied to the sentinel: -1 is a mode,
        // not a quantity, and clamping it to `min` would request real tokens.
        ThinkingIntent::Auto => return Some(-1),
        ThinkingIntent::Off => return None,
        ThinkingIntent::Budget(budget) => i64::try_from(*budget).ok()?,
        ThinkingIntent::Level(level) => i64::try_from(level_to_budget(level)?).ok()?,
    };
    if let Some(range) = range {
        if let Some(min) = range.min.and_then(|min| i64::try_from(min).ok()) {
            budget = budget.max(min);
        }
        if let Some(max) = range.max.and_then(|max| i64::try_from(max).ok()) {
            budget = budget.min(max);
        }
    }
    Some(budget)
}

/// An intent as a discrete level. `"auto"` is returned literally, as upstream does.
fn to_level(intent: &ThinkingIntent) -> Option<String> {
    match intent {
        ThinkingIntent::Off => None,
        ThinkingIntent::Auto => Some(String::from("auto")),
        ThinkingIntent::Level(level) => Some(level.clone()),
        // A budget below the smallest band still means "reason a little", so it
        // resolves to medium rather than dropping the request's intent.
        ThinkingIntent::Budget(budget) => {
            Some(budget_to_level(*budget).unwrap_or("medium").to_owned())
        }
    }
}

/// Clamp a level OpenAI may not accept.
///
/// `max` and `ultra` exist only on some reasoning families. Sending one to a
/// model that does not list it is a 400, so it degrades to the highest level the
/// enum always carries.
fn normalize_openai_level(level: &str, supported: Option<&[&str]>) -> String {
    if level != "max" && level != "ultra" {
        return level.to_owned();
    }
    if supported.is_some_and(|levels| levels.contains(&level)) {
        return level.to_owned();
    }
    if level == "ultra" && supported.is_some_and(|levels| levels.contains(&"max")) {
        return String::from("max");
    }
    String::from("xhigh")
}

/// An intent as a Gemini 3 `thinkingLevel`.
fn to_gemini_thinking_level(intent: &ThinkingIntent) -> String {
    let raw = if matches!(intent, ThinkingIntent::Auto) {
        String::from("high")
    } else {
        to_level(intent).unwrap_or_else(|| String::from("high"))
    };
    effort_to_thinking_level(&raw).to_owned()
}

/// An intent as Kimi's `reasoning_effort`, whose enum omits `minimal`/`xhigh`.
fn to_kimi_reasoning_effort(intent: &ThinkingIntent) -> Option<String> {
    let level = to_level(intent)?;
    let mapped = match level.as_str() {
        "auto" => "high",
        "minimal" => "low",
        "xhigh" => "max",
        "low" | "medium" | "high" | "max" => return Some(level),
        _ => return None,
    };
    Some(mapped.to_owned())
}

/// The minimum output ceiling a Gemini `thinkingLevel` needs.
///
/// Thinking tokens are drawn from the output budget, so a ceiling left at the
/// client's default would be spent on reasoning and truncate the visible answer.
const fn gemini_level_output_floor(level: &str) -> u64 {
    match level.as_bytes() {
        b"minimal" => 4096,
        b"low" => 8192,
        b"medium" => 16384,
        _ => 65535,
    }
}

/// The minimum output ceiling a Gemini `thinkingBudget` needs.
const fn gemini_budget_output_floor(budget: i64) -> u64 {
    if budget < 0 {
        32768
    } else if budget <= 1024 {
        8192
    } else if budget <= 8192 {
        16384
    } else if budget <= 24576 {
        32768
    } else {
        65535
    }
}

/// The `generationConfig` a Gemini request's thinking is *written* into, created
/// when absent.
///
/// gemini-cli and antigravity wrap the request in a `{ request: … }` envelope. The
/// envelope's config wins when present: writing to the top level there sets a
/// field the provider never reads, which looks like thinking silently not working.
fn gemini_generation_config(object: &mut Map<String, Value>) -> Option<&mut Map<String, Value>> {
    let key = if object.get("request").is_some_and(Value::is_object) {
        let request = object.get_mut("request")?.as_object_mut()?;
        if !request
            .get("generationConfig")
            .is_some_and(Value::is_object)
        {
            request.insert("generationConfig".to_owned(), json!({}));
        }
        return request.get_mut("generationConfig")?.as_object_mut();
    } else {
        "generationConfig"
    };
    if !object.get(key).is_some_and(Value::is_object) {
        object.insert(key.to_owned(), json!({}));
    }
    object.get_mut(key)?.as_object_mut()
}

/// Write an intent into a request body in the target provider's own spelling.
fn apply_format(
    format: ThinkingFormat,
    object: &mut Map<String, Value>,
    intent: &ThinkingIntent,
    caps: Capabilities,
    supported: Option<&[&str]>,
) {
    let off = matches!(intent, ThinkingIntent::Off);
    // A model that cannot stop reasoning is asked for minimal effort instead of
    // "off". Sending a disable it ignores would bill full-price thinking while
    // the caller believes reasoning is switched off.
    let clamped = ThinkingIntent::Level(String::from("minimal"));
    let effective = if off && !caps.thinking_can_disable {
        &clamped
    } else {
        intent
    };
    // Whether an explicit "disabled" marker may be sent at all.
    let disable = off && caps.thinking_can_disable;

    match format {
        ThinkingFormat::OpenAi => {
            if disable {
                object.insert("reasoning_effort".to_owned(), Value::from("none"));
            } else if let Some(level) = to_level(effective) {
                let normalized = normalize_openai_level(&level, supported);
                object.insert("reasoning_effort".to_owned(), Value::from(normalized));
            }
        }
        ThinkingFormat::ClaudeAdaptive => apply_claude_adaptive(object, effective, disable),
        // Hunyuan reuses Anthropic's budget shape verbatim.
        ThinkingFormat::ClaudeBudget | ThinkingFormat::Hunyuan => {
            apply_budget_thinking(object, effective, caps.thinking_range, disable);
        }
        ThinkingFormat::GeminiLevel => apply_gemini_level(object, effective, caps, off),
        ThinkingFormat::GeminiBudget => apply_gemini_budget(object, effective, caps, disable),
        ThinkingFormat::Zai => apply_zai(object, disable),
        ThinkingFormat::Qwen => apply_qwen(object, effective, caps.thinking_range, disable),
        ThinkingFormat::DeepSeek => apply_deepseek(object, effective, disable),
        ThinkingFormat::Kimi => apply_kimi(object, effective, disable),
        ThinkingFormat::MiniMax => {
            // M3 is adaptive; M2.x cannot disable and is caught by the clamp above.
            let kind = if disable { "disabled" } else { "adaptive" };
            object.insert("thinking".to_owned(), json!({ "type": kind }));
        }
        ThinkingFormat::Step => apply_step(object, effective, disable),
        ThinkingFormat::TokenRouter => apply_tokenrouter(object, effective, off),
        // Kiro carries thinking in its own system-prompt envelope, applied inside
        // its executor rather than as a body field. An unrecognised format is left
        // alone rather than guessed at.
        ThinkingFormat::Kiro | ThinkingFormat::Unrecognized => {}
    }
}

/// Write a `thinkingConfig` into whichever `generationConfig` the request uses.
fn set_gemini_thinking(object: &mut Map<String, Value>, config: Value) {
    if let Some(generation) = gemini_generation_config(object) {
        generation.insert("thinkingConfig".to_owned(), config);
    }
}

/// Raise `maxOutputTokens` to `floor`, never above the model's own ceiling.
///
/// Gemini draws thinking tokens from the output budget, so leaving the client's
/// smaller ceiling in place would spend it on reasoning and truncate the answer.
fn ensure_gemini_output_floor(object: &mut Map<String, Value>, floor: u64, max_output: u64) {
    let target = floor.min(max_output);
    if let Some(generation) = gemini_generation_config(object) {
        let current = generation.get("maxOutputTokens").and_then(Value::as_u64);
        if current.is_none_or(|current| current < target) {
            generation.insert("maxOutputTokens".to_owned(), Value::from(target));
        }
    }
}

/// Anthropic's adaptive shape.
fn apply_claude_adaptive(object: &mut Map<String, Value>, intent: &ThinkingIntent, disable: bool) {
    if disable {
        object.insert("thinking".to_owned(), json!({ "type": "disabled" }));
        return;
    }
    // `output_config.effort` alone does not switch thinking on: Anthropic requires
    // the explicit adaptive marker on Opus 4.6+/Sonnet 4.6, and Anthropic-compatible
    // shims default it off even where the native model would not. Both go out.
    object.insert("thinking".to_owned(), json!({ "type": "adaptive" }));
    if let Some(level) = to_level(intent) {
        // The effort enum has no `xhigh`; `high` is its ceiling.
        let effort = if level == "xhigh" {
            String::from("high")
        } else {
            level
        };
        object.insert("output_config".to_owned(), json!({ "effort": effort }));
    }
}

/// Anthropic's `budget_tokens` shape, also used verbatim by Hunyuan.
fn apply_budget_thinking(
    object: &mut Map<String, Value>,
    intent: &ThinkingIntent,
    range: Option<ThinkingRange>,
    disable: bool,
) {
    if disable {
        object.insert("thinking".to_owned(), json!({ "type": "disabled" }));
        return;
    }
    let thinking = match to_budget(intent, range) {
        // -1 is "provider decides": send the marker with no quantity.
        Some(-1) => json!({ "type": "enabled" }),
        Some(budget) if budget > 0 => json!({ "type": "enabled", "budget_tokens": budget }),
        // A zero or unrecognised budget still means "reason": upstream's
        // `budget || 8192` lands on the medium default rather than disabling.
        _ => json!({ "type": "enabled", "budget_tokens": 8192 }),
    };
    object.insert("thinking".to_owned(), thinking);
}

/// Gemini 3's discrete `thinkingLevel`, which has no off switch.
fn apply_gemini_level(
    object: &mut Map<String, Value>,
    intent: &ThinkingIntent,
    caps: Capabilities,
    off: bool,
) {
    let level = if off {
        String::from("minimal")
    } else {
        to_gemini_thinking_level(intent)
    };
    let include_thoughts = level != "minimal";
    let floor = gemini_level_output_floor(&level);
    set_gemini_thinking(
        object,
        json!({ "thinkingLevel": level, "includeThoughts": include_thoughts }),
    );
    ensure_gemini_output_floor(object, floor, caps.max_output);
}

/// Gemini 2.x's numeric `thinkingBudget`.
fn apply_gemini_budget(
    object: &mut Map<String, Value>,
    intent: &ThinkingIntent,
    caps: Capabilities,
    disable: bool,
) {
    if disable {
        set_gemini_thinking(
            object,
            json!({ "thinkingBudget": 0, "includeThoughts": false }),
        );
        return;
    }
    // An intent naming no budget becomes -1, Gemini's "you decide".
    let budget = to_budget(intent, caps.thinking_range).unwrap_or(-1);
    set_gemini_thinking(
        object,
        json!({ "thinkingBudget": budget, "includeThoughts": true }),
    );
    ensure_gemini_output_floor(object, gemini_budget_output_floor(budget), caps.max_output);
}

/// Z.ai, which ignores an Anthropic-style `thinking.disabled`.
fn apply_zai(object: &mut Map<String, Value>, disable: bool) {
    if disable {
        // The boolean is the only switch Z.ai honours, and the Anthropic-shaped
        // field has to go or it re-enables thinking.
        object.insert("enable_thinking".to_owned(), Value::Bool(false));
        object.remove("thinking");
        return;
    }
    object.insert("thinking".to_owned(), json!({ "type": "enabled" }));
}

/// Qwen's boolean switch plus optional budget.
fn apply_qwen(
    object: &mut Map<String, Value>,
    intent: &ThinkingIntent,
    range: Option<ThinkingRange>,
    disable: bool,
) {
    if disable {
        object.insert("enable_thinking".to_owned(), Value::Bool(false));
        return;
    }
    object.insert("enable_thinking".to_owned(), Value::Bool(true));
    if let Some(budget) = to_budget(intent, range).filter(|budget| *budget > 0) {
        object.insert("thinking_budget".to_owned(), Value::from(budget));
    }
}

/// `DeepSeek`, whose effort enum is only `high` and `max`.
fn apply_deepseek(object: &mut Map<String, Value>, intent: &ThinkingIntent, disable: bool) {
    if disable {
        object.insert("thinking".to_owned(), json!({ "type": "disabled" }));
        return;
    }
    object.insert("thinking".to_owned(), json!({ "type": "enabled" }));
    let level = to_level(intent);
    let effort = if matches!(level.as_deref(), Some("xhigh" | "max")) {
        "max"
    } else {
        "high"
    };
    object.insert("reasoning_effort".to_owned(), Value::from(effort));
}

/// Kimi: an Anthropic-shaped disable plus an OpenAI-shaped effort.
fn apply_kimi(object: &mut Map<String, Value>, intent: &ThinkingIntent, disable: bool) {
    if disable {
        object.insert("thinking".to_owned(), json!({ "type": "disabled" }));
        return;
    }
    if let Some(effort) = to_kimi_reasoning_effort(intent) {
        object.insert("reasoning_effort".to_owned(), Value::from(effort));
    }
}

/// Step, whose effort enum tops out at `high`.
fn apply_step(object: &mut Map<String, Value>, intent: &ThinkingIntent, disable: bool) {
    // Step has no disable field at all: omitting the effort is how it is turned off.
    if disable {
        return;
    }
    if let Some(level) = to_level(intent) {
        let effort = if level == "xhigh" || level == "max" {
            String::from("high")
        } else {
            level
        };
        object.insert("reasoning_effort".to_owned(), Value::from(effort));
    }
}

/// `TokenRouter`, which rejects `none` and `auto` with a 400 but takes `max`.
fn apply_tokenrouter(object: &mut Map<String, Value>, intent: &ThinkingIntent, off: bool) {
    if off || matches!(intent, ThinkingIntent::Auto) {
        // Omitting the field leaves the upstream default in place.
        return;
    }
    if let Some(level) = to_level(intent) {
        object.insert("reasoning_effort".to_owned(), Value::from(level));
    }
}

/// Normalize thinking on an outbound request body.
///
/// `model` may still carry a `(...)` thinking suffix; a suffix wins over `intent`,
/// as it is the more specific statement. `intent` is what [`extract_thinking`]
/// read from the original body before translation.
///
/// Resolution order for the wire shape is the model's own stated format, then the
/// target format's native default.
pub fn apply(
    target: Format,
    provider: &str,
    model: &str,
    body: &mut Value,
    intent: Option<&ThinkingIntent>,
) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    let (clean_model, suffix) = parse_suffix(model);
    let caps = nullrouter_providers::capabilities_for_model(provider, clean_model);

    // A model that cannot reason must not carry thinking fields at all: an
    // unrecognised field is a 400 on several providers.
    if !caps.reasoning {
        strip_all(object);
        return;
    }

    // Suffix wins over a captured intent, and a body read is the last resort, so
    // this stays correct when called without a pre-captured intent rather than
    // depending on the caller having taken one.
    let carried = suffix
        .or_else(|| intent.cloned())
        .or_else(|| extract_thinking_object(object));
    let Some(resolved) = carried else {
        return;
    };

    let format = caps
        .thinking_format
        .unwrap_or_else(|| native_format(target));
    let levels = nullrouter_providers::thinking_levels(provider, clean_model);

    // Strip first: the body may still hold the source dialect's spelling, and
    // leaving it beside the target's would send two conflicting instructions.
    strip_all(object);
    apply_format(format, object, &resolved, caps, levels.as_deref());
}
