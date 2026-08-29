//! The gate and the arithmetic around one transform.
//!
//! Ports `inspire/open-sse/rtk/pxpipe.js`.
//!
//! Separate from [`crate::bridge`] because none of this needs a process: whether a
//! body is eligible, and what the transform saved, are decisions about numbers. They
//! are the parts worth testing exactly, and they are also the parts most easily got
//! wrong — see [`Summary::tokens_after_est`].

use crate::bridge::{TransformInfo, TransformOutcome};
use crate::events::Event;

/// Upstream's estimate for characters per token, used only for the *before* figure
/// and for text the transform left alone.
const EST_CHARS_PER_TOKEN: u64 = 4;

/// Anthropic bills an image at roughly `pixels / 750` tokens.
const PIXELS_PER_IMAGE_TOKEN: u64 = 750;

/// Fallback per-image estimate when the package reports neither tokens nor pixels
/// (upstream's own constant: a full-size tile).
const EST_TOKENS_PER_IMAGE: u64 = 4761;

/// Everything the gate needs to decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    /// The `pxpipeEnabled` setting.
    pub enabled: bool,
    /// Whether the body about to be dispatched is Claude-format.
    ///
    /// The package rewrites Anthropic `messages[].content[]` blocks specifically, so
    /// a body in any other wire format is refused rather than mangled.
    pub claude_format: bool,
    /// The wire format's name, for the refusal detail.
    pub format: String,
    /// `pxpipeMinChars`, or 0 for the default.
    pub min_chars: u64,
    /// `pxpipeTimeoutMs`, or 0 for the default.
    pub timeout_ms: u64,
}

/// The threshold to apply, treating 0 and absent alike as "use the default".
pub const fn threshold(min_chars: u64) -> u64 {
    if min_chars > 0 {
        min_chars
    } else {
        crate::DEFAULT_MIN_CHARS
    }
}

/// The budget to apply, likewise.
pub const fn budget(timeout_ms: u64) -> u64 {
    if timeout_ms > 0 {
        timeout_ms
    } else {
        crate::DEFAULT_TIMEOUT_MS
    }
}

/// Why a body is not eligible, or that it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eligibility {
    /// Attempt the transform, with this threshold.
    Eligible { min_chars: u64 },
    /// Do not attempt it. Recorded with this reason.
    Skip {
        reason: &'static str,
        detail: Option<String>,
    },
}

/// Decide whether a body is worth sending to the transform.
///
/// Checked before spending a pipe round trip, and in upstream's order so the reason
/// recorded for a given request matches.
pub fn eligibility(gate: &Gate, body_chars: u64) -> Eligibility {
    if !gate.enabled {
        return Eligibility::Skip {
            reason: "disabled",
            detail: None,
        };
    }
    if body_chars == 0 {
        return Eligibility::Skip {
            reason: "missing_body",
            detail: None,
        };
    }
    if !gate.claude_format {
        return Eligibility::Skip {
            reason: "unsupported_format",
            detail: Some(gate.format.clone()),
        };
    }
    let min_chars = threshold(gate.min_chars);
    if body_chars < min_chars {
        return Eligibility::Skip {
            reason: "below_threshold",
            // The numbers, because "below threshold" alone leaves a user guessing
            // whether to lower the setting or whether their requests are just small.
            detail: Some(format!("{body_chars} chars, threshold {min_chars}")),
        };
    }
    Eligibility::Eligible { min_chars }
}

/// What one transform attempt did, in the terms the dashboard reports.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub applied: bool,
    pub reason: String,
    pub detail: Option<String>,
    pub original_chars: u64,
    pub compressed_body_chars: u64,
    /// Characters the transform replaced with images.
    pub imaged_chars: u64,
    pub image_count: u64,
    pub image_bytes: u64,
    pub tokens_before_est: u64,
    /// Remaining text tokens plus image tokens.
    ///
    /// Deliberately **not** the new body's length over [`EST_CHARS_PER_TOKEN`]: the
    /// transformed body is far *larger* in bytes, because the images arrive as
    /// base64, while being cheaper in tokens, because an image bills by pixel. Using
    /// the body length here would report every successful compression as a loss.
    pub tokens_after_est: u64,
    pub tokens_saved_est: u64,
    pub saved_pct: f64,
    pub duration_ms: u64,
    /// The package pinned `cache_control` itself.
    pub cache_owns_control: bool,
}

impl Summary {
    /// A skipped attempt.
    pub fn skipped(reason: &str, detail: Option<String>, original_chars: u64) -> Self {
        Self {
            applied: false,
            reason: reason.to_owned(),
            detail,
            original_chars,
            ..Self::default()
        }
    }

    /// Build the summary for an outcome.
    ///
    /// `duration_ms` is measured by the caller, which is the only place that knows
    /// when the attempt started.
    pub fn from_outcome(outcome: &TransformOutcome, original_chars: u64, duration_ms: u64) -> Self {
        match outcome {
            TransformOutcome::Bypassed { reason, detail } => Self {
                duration_ms,
                ..Self::skipped(reason, detail.clone(), original_chars)
            },
            TransformOutcome::Applied {
                body,
                info,
                cache_owns_control,
            } => {
                let mut summary = Self {
                    applied: true,
                    reason: "applied".to_owned(),
                    detail: None,
                    original_chars,
                    compressed_body_chars: u64::try_from(body.chars().count()).unwrap_or(u64::MAX),
                    imaged_chars: info.compressed_chars,
                    image_count: info.image_count,
                    image_bytes: info.image_bytes,
                    tokens_before_est: if info.baseline_tokens > 0 {
                        info.baseline_tokens
                    } else {
                        est_tokens(original_chars)
                    },
                    tokens_after_est: est_tokens(remaining_chars(original_chars, info))
                        + image_tokens(info),
                    tokens_saved_est: 0,
                    saved_pct: 0.0,
                    duration_ms,
                    cache_owns_control: *cache_owns_control,
                };
                summary.tokens_saved_est = summary
                    .tokens_before_est
                    .saturating_sub(summary.tokens_after_est);
                summary.saved_pct =
                    crate::events::percentage(summary.tokens_saved_est, summary.tokens_before_est);
                summary
            }
        }
    }

    /// The event-log record for this attempt.
    pub fn event(&self, now_ms: u64) -> Event {
        Event {
            ts: now_ms,
            applied: self.applied,
            reason: self.reason.clone(),
            detail: self.detail.clone(),
            original_chars: self.original_chars,
            tokens_before_est: self.tokens_before_est,
            tokens_after_est: self.tokens_after_est,
            tokens_saved_est: self.tokens_saved_est,
            image_count: self.image_count,
            duration_ms: self.duration_ms,
        }
    }

    /// The one-line log tag, as upstream's `formatPxpipeLog` writes it.
    ///
    /// `None` when nothing was applied: a log line per skipped request would bury
    /// the ones that mattered.
    pub fn log_line(&self) -> Option<String> {
        if !self.applied {
            return None;
        }
        Some(format!(
            "imaged {}ch → {} image(s) | est {}→{} tokens (-{}%) | {}ms",
            self.imaged_chars,
            self.image_count,
            self.tokens_before_est,
            self.tokens_after_est,
            self.saved_pct,
            self.duration_ms
        ))
    }
}

const fn est_tokens(chars: u64) -> u64 {
    // Rounded, as upstream rounds, so the two agree on small bodies.
    (chars + EST_CHARS_PER_TOKEN / 2) / EST_CHARS_PER_TOKEN
}

/// Text characters still on the wire after the transform.
///
/// The package's own `outgoingTextChars` when it reports one, and only otherwise the
/// subtraction upstream does. This is a deliberate divergence from upstream, because
/// upstream's arithmetic is wrong on real data: `compressedChars` counts tool-result
/// prose while `originalChars` is the serialised body, so the two are not on the same
/// scale and the subtraction can saturate to zero. Measured on one real applied
/// request: `compressedChars` 46 407 against a 60 016-character body, where
/// subtracting gives 13 609 characters of "remaining text" that is not there, against
/// the 203 the package actually measured.
const fn remaining_chars(original_chars: u64, info: &TransformInfo) -> u64 {
    if info.outgoing_text_chars > 0 {
        return info.outgoing_text_chars;
    }
    original_chars.saturating_sub(info.compressed_chars)
}

/// The image half of the after-estimate, preferring what the package measured.
const fn image_tokens(info: &TransformInfo) -> u64 {
    if info.image_tokens > 0 {
        return info.image_tokens;
    }
    if info.image_pixels > 0 {
        return (info.image_pixels + PIXELS_PER_IMAGE_TOKEN / 2) / PIXELS_PER_IMAGE_TOKEN;
    }
    info.image_count * EST_TOKENS_PER_IMAGE
}

#[cfg(test)]
mod tests {
    use super::{Eligibility, Gate, Summary, budget, eligibility, threshold};
    use crate::bridge::{TransformInfo, TransformOutcome};

    fn gate() -> Gate {
        Gate {
            enabled: true,
            claude_format: true,
            format: "claude".to_owned(),
            min_chars: 0,
            timeout_ms: 0,
        }
    }

    #[test]
    fn zero_means_the_default_for_both_settings() {
        assert_eq!(threshold(0), crate::DEFAULT_MIN_CHARS);
        assert_eq!(threshold(100), 100);
        assert_eq!(budget(0), crate::DEFAULT_TIMEOUT_MS);
        assert_eq!(budget(500), 500);
    }

    #[test]
    fn a_disabled_saver_skips_before_anything_else_is_checked() {
        let gate = Gate {
            enabled: false,
            // Also not Claude-format and also too small: "disabled" still wins, so a
            // user who turned it off does not see a format complaint.
            claude_format: false,
            ..gate()
        };
        assert_eq!(
            eligibility(&gate, 10),
            Eligibility::Skip {
                reason: "disabled",
                detail: None
            }
        );
    }

    #[test]
    fn a_non_claude_body_is_refused_and_named() {
        let gate = Gate {
            claude_format: false,
            format: "openai".to_owned(),
            ..gate()
        };
        assert_eq!(
            eligibility(&gate, 100_000),
            Eligibility::Skip {
                reason: "unsupported_format",
                detail: Some("openai".to_owned()),
            }
        );
    }

    #[test]
    fn an_empty_body_is_skipped_rather_than_sent() {
        assert_eq!(
            eligibility(&gate(), 0),
            Eligibility::Skip {
                reason: "missing_body",
                detail: None
            }
        );
    }

    #[test]
    fn the_threshold_gates_on_the_bodys_own_size() {
        // Just under the default.
        match eligibility(&gate(), crate::DEFAULT_MIN_CHARS - 1) {
            Eligibility::Skip { reason, detail } => {
                assert_eq!(reason, "below_threshold");
                let detail = detail.unwrap_or_default();
                assert!(detail.contains("24999"), "{detail}");
                assert!(detail.contains("25000"), "{detail}");
            }
            Eligibility::Eligible { .. } => panic!("must not be eligible"),
        }
        // Exactly at it.
        assert_eq!(
            eligibility(&gate(), crate::DEFAULT_MIN_CHARS),
            Eligibility::Eligible {
                min_chars: crate::DEFAULT_MIN_CHARS
            }
        );
        // A configured threshold is honoured, and passed on so the package uses the
        // same number the gate did.
        let gate = Gate {
            min_chars: 500,
            ..gate()
        };
        assert_eq!(
            eligibility(&gate, 600),
            Eligibility::Eligible { min_chars: 500 }
        );
    }

    #[test]
    fn the_after_estimate_counts_pixels_not_the_bodys_new_length() {
        // 100k characters in; the package imaged 80k of them into 2 images and the
        // body came back three times bigger as base64.
        let outcome = TransformOutcome::Applied {
            body: "x".repeat(300_000),
            info: TransformInfo {
                orig_chars: 100_000,
                compressed_chars: 80_000,
                outgoing_text_chars: 20_000,
                image_count: 2,
                image_bytes: 250_000,
                image_pixels: 1_500_000,
                image_tokens: 0,
                baseline_tokens: 25_000,
            },
            cache_owns_control: false,
        };
        let summary = Summary::from_outcome(&outcome, 100_000, 420);
        assert!(summary.applied);
        assert_eq!(summary.tokens_before_est, 25_000, "the measured baseline");
        // Remaining text: 20 000 chars → 5 000 tokens. Images: 1 500 000 px / 750 →
        // 2 000 tokens. A chars/4 reading of the new body would have said 75 000 —
        // a reported *loss* on a request that in fact saved 72%.
        assert_eq!(summary.tokens_after_est, 7_000);
        assert_eq!(summary.tokens_saved_est, 18_000);
        assert!(
            (summary.saved_pct - 72.0).abs() < f64::EPSILON,
            "{}",
            summary.saved_pct
        );
        assert_eq!(summary.compressed_body_chars, 300_000);
        assert_eq!(summary.duration_ms, 420);
    }

    #[test]
    fn the_packages_own_remaining_text_measure_is_preferred() {
        // Real numbers from pxpipe-proxy 0.13.2 on an applied request: a 60 016-char
        // body, 46 407 chars replaced by 3 images of 1 956 864 px, and 203 chars of
        // text actually left on the wire.
        let outcome = TransformOutcome::Applied {
            body: "x".repeat(38_722),
            info: TransformInfo {
                orig_chars: 5_607,
                compressed_chars: 46_407,
                outgoing_text_chars: 203,
                image_count: 3,
                image_bytes: 28_359,
                image_pixels: 1_956_864,
                image_tokens: 0,
                baseline_tokens: 0,
            },
            cache_owns_control: false,
        };
        let summary = Summary::from_outcome(&outcome, 60_016, 200);
        // 203 chars → 51 tokens, plus 1 956 864 px / 750 → 2 609.
        assert_eq!(summary.tokens_after_est, 51 + 2_609);
        // Upstream's subtraction would have claimed 13 609 chars of remaining text
        // (→ 3 402 tokens) that the package says is not there — understating the
        // saving by a third on a request where it measured the answer.
        assert_ne!(summary.tokens_after_est, 3_402 + 2_609);
    }

    #[test]
    fn a_package_reporting_no_remaining_text_measure_falls_back_to_subtraction() {
        // Older versions do not report `outgoingTextChars`; the estimate degrades
        // rather than reading zero remaining text and overstating the saving.
        let outcome = TransformOutcome::Applied {
            body: "{}".to_owned(),
            info: TransformInfo {
                compressed_chars: 30_000,
                outgoing_text_chars: 0,
                image_count: 1,
                image_pixels: 750_000,
                ..TransformInfo::default()
            },
            cache_owns_control: false,
        };
        let summary = Summary::from_outcome(&outcome, 40_000, 10);
        // 10 000 chars remaining → 2 500 tokens, plus 1 000 image tokens.
        assert_eq!(summary.tokens_after_est, 2_500 + 1_000);
    }

    #[test]
    fn imaged_chars_exceeding_the_body_cannot_underflow() {
        // `compressedChars` is not bounded by the body length: it counts tool-result
        // prose, which the body's own character count does not separate out.
        let outcome = TransformOutcome::Applied {
            body: "{}".to_owned(),
            info: TransformInfo {
                compressed_chars: 90_000,
                outgoing_text_chars: 0,
                image_count: 1,
                image_tokens: 500,
                ..TransformInfo::default()
            },
            cache_owns_control: false,
        };
        let summary = Summary::from_outcome(&outcome, 40_000, 10);
        assert_eq!(summary.tokens_after_est, 500, "no remaining text, no panic");
    }

    #[test]
    fn a_measured_image_token_count_beats_both_estimates() {
        let outcome = TransformOutcome::Applied {
            body: "{}".to_owned(),
            info: TransformInfo {
                orig_chars: 40_000,
                compressed_chars: 40_000,
                outgoing_text_chars: 0,
                image_count: 4,
                image_pixels: 9_000_000,
                // The package measured it; that wins over pixels and over the
                // per-image fallback.
                image_tokens: 1_234,
                baseline_tokens: 10_000,
                image_bytes: 0,
            },
            cache_owns_control: false,
        };
        let summary = Summary::from_outcome(&outcome, 40_000, 10);
        assert_eq!(
            summary.tokens_after_est, 1_234,
            "no remaining text, so images only"
        );
    }

    #[test]
    fn without_measurements_the_estimate_falls_back_per_image() {
        let outcome = TransformOutcome::Applied {
            body: "{}".to_owned(),
            info: TransformInfo {
                compressed_chars: 0,
                image_count: 1,
                ..TransformInfo::default()
            },
            cache_owns_control: false,
        };
        let summary = Summary::from_outcome(&outcome, 40_000, 10);
        // Nothing imaged, so all 40 000 chars remain: 10 000 tokens, plus one
        // full-tile image.
        assert_eq!(summary.tokens_before_est, 10_000, "estimated, not measured");
        assert_eq!(summary.tokens_after_est, 10_000 + 4_761);
        // A transform that grew the estimate saves nothing rather than a negative.
        assert_eq!(summary.tokens_saved_est, 0);
        assert!(summary.saved_pct.abs() < f64::EPSILON);
    }

    #[test]
    fn a_bypass_keeps_its_reason_and_its_measured_duration() {
        let outcome = TransformOutcome::Bypassed {
            reason: "timeout",
            detail: Some("15000ms".to_owned()),
        };
        let summary = Summary::from_outcome(&outcome, 90_000, 15_001);
        assert!(!summary.applied);
        assert_eq!(summary.reason, "timeout");
        assert_eq!(summary.detail.as_deref(), Some("15000ms"));
        assert_eq!(summary.original_chars, 90_000);
        assert_eq!(summary.duration_ms, 15_001);
        assert_eq!(summary.tokens_saved_est, 0);
    }

    #[test]
    fn the_event_record_carries_the_summarys_numbers() {
        let outcome = TransformOutcome::Applied {
            body: "{}".to_owned(),
            info: TransformInfo {
                compressed_chars: 40_000,
                image_count: 2,
                image_tokens: 1_000,
                baseline_tokens: 10_000,
                ..TransformInfo::default()
            },
            cache_owns_control: false,
        };
        let summary = Summary::from_outcome(&outcome, 40_000, 250);
        let event = summary.event(1_700_000_000_000);
        assert_eq!(event.ts, 1_700_000_000_000);
        assert!(event.applied);
        assert_eq!(event.reason, "applied");
        assert_eq!(event.tokens_before_est, 10_000);
        assert_eq!(event.tokens_after_est, 1_000);
        assert_eq!(event.tokens_saved_est, 9_000);
        assert_eq!(event.image_count, 2);
        assert_eq!(event.duration_ms, 250);
    }

    #[test]
    fn only_an_applied_transform_writes_a_log_line() {
        let applied = Summary::from_outcome(
            &TransformOutcome::Applied {
                body: "{}".to_owned(),
                info: TransformInfo {
                    compressed_chars: 40_000,
                    image_count: 2,
                    image_tokens: 1_000,
                    baseline_tokens: 10_000,
                    ..TransformInfo::default()
                },
                cache_owns_control: false,
            },
            40_000,
            250,
        );
        let line = applied.log_line().unwrap_or_default();
        assert!(line.contains("imaged 40000ch → 2 image(s)"), "{line}");
        assert!(line.contains("est 10000→1000 tokens (-90%)"), "{line}");
        assert!(line.contains("250ms"), "{line}");

        let skipped = Summary::skipped("below_threshold", None, 10);
        assert!(skipped.log_line().is_none());
    }
}
