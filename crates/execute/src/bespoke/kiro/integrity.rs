//! Kiro's integrity gate: detect a truncated or malformed final, re-prompt once.
//!
//! Ports the detectors in `open-sse/executors/kiro.js`. Upstream inspects the finished answer and,
//! when it looks incomplete, silently re-runs the user's request with an extra instruction. That
//! spends a second call against the user's quota on a heuristic about prose, so a false positive
//! is a real cost. The detectors therefore copy the reference's own regex family and its
//! completed / result / user-wait exemptions rather than inventing a broader one.
//!
//! Re-prompt is bounded: at most one repair attempt. If the repaired answer still fails the same
//! check, the original is returned — a second failure is not evidence the first was wrong, and
//! looping would spend quota indefinitely.

use serde_json::{Value, json};

/// Why a finished Kiro answer is considered incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The whole answer was `...` or `…`.
    Ellipsis,
    /// A `tool_call` wrapper was missing `name` or `arguments`.
    InvalidTool,
    /// A short final that only announced a future action.
    ShortFinal,
}

/// The instruction the reference appends for a given failure.
pub(crate) fn instruction(kind: Kind) -> &'static str {
    match kind {
        Kind::Ellipsis => {
            "Retry the previous response because it ended with only an ellipsis. Return the complete final answer, not only ... or …."
        }
        Kind::InvalidTool => {
            "Retry the previous response because its Kiro tool_call wrapper was malformed. If you use the wrapper tool named tool_call, its input must contain a non-empty name and an arguments field."
        }
        Kind::ShortFinal => {
            "Retry the previous response because its final only announced a future action. Complete the check now and return the result or a concrete blocker."
        }
    }
}

/// Inspect a finished answer. `None` means it looks complete.
///
/// Tool conversations are exempt from the ellipsis and short-final checks: a tool call *is* the
/// turn's payload, and judging its prose would false-positive every one.
pub(crate) fn inspect(content: &str, tools: &[ToolCall]) -> Option<Kind> {
    if let Some(tool) = tools.iter().find(|tool| tool.is_malformed_wrapper()) {
        let _ = tool;
        return Some(Kind::InvalidTool);
    }
    if !tools.is_empty() {
        return None;
    }
    if is_ellipsis_only(content) {
        return Some(Kind::Ellipsis);
    }
    if is_short_future_action(content) {
        return Some(Kind::ShortFinal);
    }
    None
}

/// A tool call as the integrity gate sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolCall {
    /// The function's name.
    pub name: String,
    /// The function's arguments, as JSON text.
    pub arguments: String,
}

impl ToolCall {
    /// Kiro's MCP wrapper: a tool literally named `tool_call` whose input must carry a nested name
    /// and an arguments field.
    fn is_malformed_wrapper(&self) -> bool {
        if self.name != "tool_call" {
            return false;
        }
        let Ok(value) = serde_json::from_str::<Value>(&self.arguments) else {
            return true;
        };
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if name.is_empty() {
            return true;
        }
        if value.get("arguments").is_none() {
            return true;
        }
        false
    }
}

/// Append the repair instruction to a Kiro request body, matching the reference's
/// `appendRepairInstruction`.
pub(crate) fn append_instruction(body: &Value, kind: Kind) -> Value {
    let mut repaired = body.clone();
    let extra = instruction(kind);
    match repaired.get_mut("systemPrompt") {
        Some(Value::String(existing)) if !existing.is_empty() => {
            existing.push_str("\n\n");
            existing.push_str(extra);
        }
        _otherwise => {
            repaired
                .as_object_mut()
                .map(|object| object.insert("systemPrompt".to_owned(), json!(extra)));
        }
    }
    repaired
}

/// After a repair attempt, keep the original if the new answer still fails.
pub(crate) fn pick_after_repair(
    original: &str,
    original_tools: &[ToolCall],
    repaired: &str,
    repaired_tools: &[ToolCall],
) -> Outcome {
    match inspect(repaired, repaired_tools) {
        None => Outcome::UseRepaired,
        Some(_still_bad) => {
            let _ = original;
            let _ = original_tools;
            Outcome::KeepOriginal
        }
    }
}

/// Which answer to surface after the one allowed retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The retry produced a complete answer.
    UseRepaired,
    /// The retry did not improve it; keep what the user already got.
    KeepOriginal,
}

fn is_ellipsis_only(value: &str) -> bool {
    matches!(value.trim(), "..." | "…")
}

fn is_short_future_action(value: &str) -> bool {
    let text = value.trim().replace('\u{2019}', "'");
    if re_matches(&OBSERVED_TRAILING_FUTURE_ACTION, &text) {
        return true;
    }
    if re_matches(&ENGLISH_FUTURE_ACTION, &text) && re_matches(&ENGLISH_RESULT_CLAUSE, &text) {
        return false;
    }
    if re_matches(&CHINESE_FUTURE_ACTION, &text) && re_matches(&CHINESE_RESULT_CLAUSE, &text) {
        return false;
    }
    !text.is_empty()
        && text.chars().count() <= SHORT_FINAL_MAX_CHARS
        && re_matches(&SHORT_FUTURE_ACTION, &text)
        && !re_matches(&USER_WAIT, &text)
        && !re_matches(&COMPLETED_FINAL, &text)
        && !re_matches(&RESULT_EVIDENCE, &text)
}

const SHORT_FINAL_MAX_CHARS: usize = 800;

use std::sync::LazyLock;

use regex::Regex;

fn compiled(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

fn re_matches(pattern: &LazyLock<Option<Regex>>, text: &str) -> bool {
    pattern.as_ref().is_some_and(|regex| regex.is_match(text))
}

static SHORT_FUTURE_ACTION: LazyLock<Option<Regex>> = LazyLock::new(|| {
    compiled(
        r"(?iu)^(?:(?:(?:現在|接著|接下來|下一步)[，,:：\s]*(?:我(?:只)?(?:會|要|將|再)?\s*)?|我只再)(?:補|查|確認|驗證|追(?:查|蹤)?|繼續|檢查|測試)|我(?:會|要|將)(?:再|重新)?(?:補(?:齊|查)?|抓取|查(?:詢)?|確認|驗證|追(?:查|蹤)?|繼續|檢查|測試)|(?:(?:next|now|then)\b[\s,:-]*)?(?:i(?:'ll| will| am going to| need to)|let me)\s+(?:verify|check|confirm|validate|investigate|trace|continue|follow up|test)\b)",
    )
});
static OBSERVED_TRAILING_FUTURE_ACTION: LazyLock<Option<Regex>> = LazyLock::new(|| {
    compiled(
        r"(?iu)^目前證據顯示[\s\S]{1,700}[。.!?；;]\s*最後補查\s+504\s+access\s+log[，,]\s*確認\s+host[／/]路徑與是否為集中流量[。.!]?$",
    )
});
static ENGLISH_FUTURE_ACTION: LazyLock<Option<Regex>> = LazyLock::new(|| {
    compiled(
        r"(?iu)^(?:(?:next|now|then)\b[\s,:-]*)?(?:i(?:'ll| will| am going to| need to)|let me)\s+(?:verify|check|confirm|validate|investigate|trace|continue|follow up|test)\b",
    )
});
static ENGLISH_RESULT_CLAUSE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    compiled(
        r"(?iu)(?:[:;\n]|[.!?]\s+\S|\b(?:status|checksum|response|deployment)\s+(?:is|are|was|were|matches?|equals?|returned)\b)",
    )
});
static CHINESE_FUTURE_ACTION: LazyLock<Option<Regex>> = LazyLock::new(|| {
    compiled(
        r"(?u)^(?:(?:現在|接著|接下來|下一步)[，,:：\s]*(?:我(?:只)?(?:會|要|將|再)?\s*)?|我只再|我(?:會|要|將)(?:再|重新)?)(?:補|抓取|查|確認|驗證|追|繼續|檢查|測試)",
    )
});
static CHINESE_RESULT_CLAUSE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    compiled(r"(?u)(?:[。！？]\s*\S|(?:版本|狀態|回應|結果|部署|校驗碼)(?:是|為|等於|顯示))")
});
static USER_WAIT: LazyLock<Option<Regex>> = LazyLock::new(|| {
    compiled(
        r"(?iu)(?:請(?:你|先)|你(?:先|需要|可以|提供|確認|批准|允許)|等待(?:你|使用者)|等你|核准|同意|授權|\b(?:after|when|once)\s+you\b|\byour\s+(?:approval|confirmation|permission|input)\b|\bwait(?:ing)?\s+for\s+you\b|\bplease\s+(?:approve|confirm|provide|send)\b)",
    )
});
static COMPLETED_FINAL: LazyLock<Option<Regex>> = LazyLock::new(|| {
    compiled(
        r"(?iu)(?:已(?:經)?完成|完成(?:了|驗證|確認)|修復完成|確認無誤|驗證(?:完成|通過)|測試(?:均)?通過|結論|總結|\b(?:done|completed|fixed|verified|confirmed|passed|in conclusion|summary)\b|\b(?:is|are) complete\b)",
    )
});
static RESULT_EVIDENCE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    compiled(
        r"(?iu)(?:顯示|發現|因此|成功|失敗|正常|無錯誤|沒有錯誤|\b(?:found|shows?|showed|because|therefore|succeeded|failed|healthy|green|no errors?)\b)",
    )
});

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "a regex that does not compile is a test-setup failure"
)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        CHINESE_FUTURE_ACTION, CHINESE_RESULT_CLAUSE, COMPLETED_FINAL, ENGLISH_FUTURE_ACTION,
        ENGLISH_RESULT_CLAUSE, Kind, OBSERVED_TRAILING_FUTURE_ACTION, Outcome, RESULT_EVIDENCE,
        SHORT_FUTURE_ACTION, ToolCall, USER_WAIT, append_instruction, inspect, instruction,
        pick_after_repair,
    };

    #[test]
    fn every_reference_regex_compiles() {
        for pattern in [
            &*SHORT_FUTURE_ACTION,
            &*OBSERVED_TRAILING_FUTURE_ACTION,
            &*ENGLISH_FUTURE_ACTION,
            &*ENGLISH_RESULT_CLAUSE,
            &*CHINESE_FUTURE_ACTION,
            &*CHINESE_RESULT_CLAUSE,
            &*USER_WAIT,
            &*COMPLETED_FINAL,
            &*RESULT_EVIDENCE,
        ] {
            assert!(pattern.is_some(), "a reference regex failed to compile");
        }
    }

    #[test]
    fn an_ellipsis_only_answer_is_detected() {
        assert_eq!(inspect("...", &[]), Some(Kind::Ellipsis));
        assert_eq!(inspect("…", &[]), Some(Kind::Ellipsis));
        assert_eq!(inspect("  ...  ", &[]), Some(Kind::Ellipsis));
    }

    #[test]
    fn a_malformed_tool_call_wrapper_is_detected() {
        let missing_name = ToolCall {
            name: "tool_call".to_owned(),
            arguments: r#"{"arguments":{"city":"Hanoi"}}"#.to_owned(),
        };
        assert_eq!(inspect("calling", &[missing_name]), Some(Kind::InvalidTool));
        let missing_args = ToolCall {
            name: "tool_call".to_owned(),
            arguments: r#"{"name":"get_weather"}"#.to_owned(),
        };
        assert_eq!(inspect("", &[missing_args]), Some(Kind::InvalidTool));
        let not_json = ToolCall {
            name: "tool_call".to_owned(),
            arguments: "not-json".to_owned(),
        };
        assert_eq!(inspect("", &[not_json]), Some(Kind::InvalidTool));
    }

    #[test]
    fn a_well_formed_wrapper_and_an_ordinary_tool_are_not() {
        let good = ToolCall {
            name: "tool_call".to_owned(),
            arguments: r#"{"name":"get_weather","arguments":{"city":"Hanoi"}}"#.to_owned(),
        };
        assert_eq!(inspect("", &[good]), None);
        let ordinary = ToolCall {
            name: "get_weather".to_owned(),
            arguments: r#"{"city":"Hanoi"}"#.to_owned(),
        };
        assert_eq!(inspect("", &[ordinary]), None);
    }

    #[test]
    fn a_short_final_that_only_announces_a_future_action_is_detected() {
        assert_eq!(
            inspect("I will check the access log next.", &[]),
            Some(Kind::ShortFinal)
        );
        assert_eq!(
            inspect("Let me verify the deployment.", &[]),
            Some(Kind::ShortFinal)
        );
    }

    #[test]
    fn a_legitimate_completed_answer_is_not() {
        assert_eq!(
            inspect("The checksum is abc123. Verification completed.", &[]),
            None
        );
        assert_eq!(
            inspect("I will check this after you confirm the host.", &[]),
            None
        );
        assert_eq!(
            inspect("Status is green because the deployment succeeded.", &[]),
            None
        );
        assert_eq!(inspect("Here is the full answer you asked for.", &[]), None);
    }

    #[test]
    fn the_observed_chinese_trailing_sentence_is_detected() {
        let text = "目前證據顯示流量來自單一來源。最後補查 504 access log，確認 host/路徑與是否為集中流量。";
        assert_eq!(inspect(text, &[]), Some(Kind::ShortFinal));
    }

    #[test]
    fn repair_instructions_are_the_reference_text() {
        assert!(instruction(Kind::Ellipsis).contains("ellipsis"));
        assert!(instruction(Kind::InvalidTool).contains("tool_call"));
        assert!(instruction(Kind::ShortFinal).contains("future action"));
    }

    #[test]
    fn append_instruction_writes_the_reference_text_onto_the_body() {
        let repaired = append_instruction(&json!({"conversationState": {}}), Kind::Ellipsis);
        assert_eq!(
            repaired.get("systemPrompt").and_then(Value::as_str),
            Some(instruction(Kind::Ellipsis))
        );
        let twice = append_instruction(&repaired, Kind::ShortFinal);
        let prompt = twice
            .get("systemPrompt")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(prompt.contains(instruction(Kind::Ellipsis)));
        assert!(prompt.contains(instruction(Kind::ShortFinal)));
    }

    #[test]
    fn a_repair_that_does_not_improve_keeps_the_original() {
        assert_eq!(
            pick_after_repair("...", &[], "...", &[]),
            Outcome::KeepOriginal
        );
        assert_eq!(
            pick_after_repair("...", &[], "The actual completed answer.", &[]),
            Outcome::UseRepaired
        );
    }
}
