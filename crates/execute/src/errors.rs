//! Upstream error classification, client error envelopes, and account-fallback
//! cooldown policy.
//!
//! Ports `open-sse/utils/error.js`, `open-sse/config/errorConfig.js`, and the
//! cooldown half of `open-sse/services/accountFallback.js`.

use serde_json::{Value, json};

/// Exponential backoff base for quota errors (upstream `BACKOFF_CONFIG.base`).
const BACKOFF_BASE_MS: u64 = 2000;
/// Backoff ceiling (upstream `BACKOFF_CONFIG.max`), 5 minutes.
const BACKOFF_MAX_MS: u64 = 5 * 60 * 1000;
/// Highest backoff level tracked (upstream `BACKOFF_CONFIG.maxLevel`).
const BACKOFF_MAX_LEVEL: u32 = 15;
/// Cooldown for transient/unmatched errors (upstream `TRANSIENT_COOLDOWN_MS`).
const TRANSIENT_COOLDOWN_MS: u64 = 30 * 1000;
/// Hard cap on a provider-reported reset time (upstream
/// `MAX_RATE_LIMIT_COOLDOWN_MS`), 30 minutes.
pub const MAX_RATE_LIMIT_COOLDOWN_MS: u64 = 30 * 60 * 1000;

const COOLDOWN_LONG_MS: u64 = 2 * 60 * 1000;
const COOLDOWN_SHORT_MS: u64 = 5 * 1000;

/// OpenAI error `type`/`code` for a status (upstream `ERROR_TYPES`).
const fn error_type_for(status: u16) -> (&'static str, &'static str) {
    match status {
        400 => ("invalid_request_error", "bad_request"),
        401 => ("authentication_error", "invalid_api_key"),
        402 => ("billing_error", "payment_required"),
        403 => ("permission_error", "insufficient_quota"),
        404 => ("invalid_request_error", "model_not_found"),
        406 => ("invalid_request_error", "model_not_supported"),
        429 => ("rate_limit_error", "rate_limit_exceeded"),
        500 => ("server_error", "internal_server_error"),
        502 => ("server_error", "bad_gateway"),
        503 => ("server_error", "service_unavailable"),
        504 => ("server_error", "gateway_timeout"),
        // Unlisted statuses split on the 5xx boundary.
        status if status >= 500 => ("server_error", "internal_server_error"),
        _ => ("invalid_request_error", ""),
    }
}

/// Default client-facing message for a status
/// (upstream `DEFAULT_ERROR_MESSAGES`).
pub const fn default_error_message(status: u16) -> Option<&'static str> {
    Some(match status {
        400 => "Bad request",
        401 => "Invalid API key provided",
        402 => "Payment required",
        403 => "You exceeded your current quota",
        404 => "Model not found",
        406 => "Model not supported",
        429 => "Rate limit exceeded",
        500 => "Internal server error",
        502 => "Bad gateway - upstream provider error",
        503 => "Service temporarily unavailable",
        504 => "Gateway timeout",
        _ => return None,
    })
}

/// Build an OpenAI-compatible error body (upstream `buildErrorBody`).
pub fn build_error_body(status: u16, message: &str) -> Value {
    let (error_type, code) = error_type_for(status);
    let resolved = if message.is_empty() {
        default_error_message(status).unwrap_or("An error occurred")
    } else {
        message
    };
    json!({
        "error": {
            "message": resolved,
            "type": error_type,
            "code": code,
        },
    })
}

/// A classified upstream failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamError {
    pub status: u16,
    pub message: String,
    /// Provider-reported reset time, epoch millis (e.g. codex `resets_at`).
    pub resets_at_ms: Option<u64>,
}

/// Extract a human-readable message from an upstream error body
/// (upstream `parseUpstreamError`).
pub fn parse_upstream_error(status: u16, body_text: &str) -> UpstreamError {
    let message = serde_json::from_str::<Value>(body_text)
        .ok()
        .and_then(|json| {
            json.get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    json.get("message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .or_else(|| {
                    // `error` may itself be a string or a nested object.
                    json.get("error").map(|error| {
                        error
                            .as_str()
                            .map_or_else(|| error.to_string(), str::to_owned)
                    })
                })
        })
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| body_text.to_owned());

    let resolved = if message.is_empty() {
        default_error_message(status)
            .map_or_else(|| format!("Upstream error: {status}"), str::to_owned)
    } else {
        message
    };

    UpstreamError {
        status,
        message: resolved,
        resets_at_ms: None,
    }
}

/// Format an error with provider context (upstream `formatProviderError`).
pub fn format_provider_error(status: u16, message: &str) -> String {
    format!("[{status}]: {message}")
}

/// Cooldown decision for a failing account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FallbackDecision {
    /// Whether to try the next account.
    pub should_fallback: bool,
    pub cooldown_ms: u64,
    /// New backoff level, when this was a quota-style error.
    pub new_backoff_level: Option<u32>,
}

/// Exponential quota cooldown (upstream `getQuotaCooldown`).
///
/// Overflow saturates to the ceiling, matching JS where `Math.pow` grows past
/// the cap and `Math.min` clamps it. (`checked_shl` only guards the shift
/// amount, so a large level would silently wrap to zero.)
fn quota_cooldown(backoff_level: u32) -> u64 {
    let level = backoff_level.saturating_sub(1);
    2_u64
        .checked_pow(level)
        .and_then(|factor| factor.checked_mul(BACKOFF_BASE_MS))
        .unwrap_or(BACKOFF_MAX_MS)
        .min(BACKOFF_MAX_MS)
}

/// Text rules, in priority order (upstream `ERROR_RULES` text half).
/// `true` means the rule uses exponential backoff.
const TEXT_RULES: [(&str, bool, u64); 8] = [
    ("no credentials", false, COOLDOWN_LONG_MS),
    ("request not allowed", false, COOLDOWN_SHORT_MS),
    ("improperly formed request", false, COOLDOWN_LONG_MS),
    ("rate limit", true, 0),
    ("too many requests", true, 0),
    ("quota exceeded", true, 0),
    ("capacity", true, 0),
    ("overloaded", true, 0),
];

/// Status rules, applied only when no text rule matched.
const STATUS_RULES: [(u16, bool, u64); 5] = [
    (401, false, COOLDOWN_LONG_MS),
    (402, false, COOLDOWN_LONG_MS),
    (403, false, COOLDOWN_LONG_MS),
    (404, false, COOLDOWN_LONG_MS),
    (429, true, 0),
];

/// Classify a failure into a fallback + cooldown decision
/// (upstream `checkFallbackError`). Text rules win over status rules.
pub fn check_fallback_error(status: u16, error_text: &str, backoff_level: u32) -> FallbackDecision {
    let lowered = error_text.to_lowercase();

    for (needle, uses_backoff, cooldown_ms) in TEXT_RULES {
        if !lowered.is_empty() && lowered.contains(needle) {
            return decide(uses_backoff, cooldown_ms, backoff_level);
        }
    }
    for (rule_status, uses_backoff, cooldown_ms) in STATUS_RULES {
        if rule_status == status {
            return decide(uses_backoff, cooldown_ms, backoff_level);
        }
    }

    // Any unmatched error still falls back, on a short transient cooldown.
    FallbackDecision {
        should_fallback: true,
        cooldown_ms: TRANSIENT_COOLDOWN_MS,
        new_backoff_level: None,
    }
}

fn decide(uses_backoff: bool, cooldown_ms: u64, backoff_level: u32) -> FallbackDecision {
    if uses_backoff {
        let level = (backoff_level + 1).min(BACKOFF_MAX_LEVEL);
        return FallbackDecision {
            should_fallback: true,
            cooldown_ms: quota_cooldown(level),
            new_backoff_level: Some(level),
        };
    }
    FallbackDecision {
        should_fallback: true,
        cooldown_ms,
        new_backoff_level: None,
    }
}

/// Render a retry hint (upstream `formatRetryAfter`).
pub fn format_retry_after(remaining_ms: i64) -> String {
    if remaining_ms <= 0 {
        return "reset after 0s".to_owned();
    }
    let total_seconds =
        remaining_ms.div_euclid(1000) + i64::from(remaining_ms.rem_euclid(1000) > 0);
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    let mut parts: Vec<String> = Vec::new();
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if seconds > 0 || parts.is_empty() {
        parts.push(format!("{seconds}s"));
    }
    format!("reset after {}", parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_RATE_LIMIT_COOLDOWN_MS, build_error_body, check_fallback_error, default_error_message,
        format_provider_error, format_retry_after, parse_upstream_error, quota_cooldown,
    };
    use serde_json::json;

    #[test]
    fn error_bodies_carry_openai_type_and_code() {
        let body = build_error_body(429, "slow down");
        assert_eq!(body.pointer("/error/message"), Some(&json!("slow down")));
        assert_eq!(
            body.pointer("/error/type"),
            Some(&json!("rate_limit_error"))
        );
        assert_eq!(
            body.pointer("/error/code"),
            Some(&json!("rate_limit_exceeded"))
        );

        // Empty messages fall back to the per-status default.
        let defaulted = build_error_body(401, "");
        assert_eq!(
            defaulted.pointer("/error/message"),
            Some(&json!("Invalid API key provided"))
        );

        // Unlisted 5xx statuses classify as server errors.
        let unlisted = build_error_body(521, "cloudflare down");
        assert_eq!(
            unlisted.pointer("/error/type"),
            Some(&json!("server_error"))
        );
        // Unlisted 4xx statuses classify as invalid requests.
        let client = build_error_body(418, "teapot");
        assert_eq!(
            client.pointer("/error/type"),
            Some(&json!("invalid_request_error"))
        );
    }

    #[test]
    fn upstream_messages_are_extracted_from_common_shapes() {
        assert_eq!(
            parse_upstream_error(400, r#"{"error":{"message":"bad model"}}"#).message,
            "bad model"
        );
        assert_eq!(
            parse_upstream_error(400, r#"{"message":"flat message"}"#).message,
            "flat message"
        );
        assert_eq!(
            parse_upstream_error(400, r#"{"error":"string error"}"#).message,
            "string error"
        );
        // Non-JSON bodies pass through verbatim.
        assert_eq!(
            parse_upstream_error(502, "upstream exploded").message,
            "upstream exploded"
        );
        // An empty body falls back to the status default.
        assert_eq!(parse_upstream_error(429, "").message, "Rate limit exceeded");
        // A status with no default still produces something useful.
        assert_eq!(parse_upstream_error(418, "").message, "Upstream error: 418");
    }

    #[test]
    fn text_rules_take_priority_over_status_rules() {
        // A 401 whose text mentions a rate limit must use backoff, not the
        // fixed 401 cooldown.
        let decision = check_fallback_error(401, "Rate limit reached for model", 0);
        assert!(decision.should_fallback);
        assert_eq!(decision.new_backoff_level, Some(1));
        assert_eq!(decision.cooldown_ms, 2000);

        // Plain 401 uses the long fixed cooldown with no backoff level.
        let plain = check_fallback_error(401, "invalid key", 0);
        assert_eq!(plain.new_backoff_level, None);
        assert_eq!(plain.cooldown_ms, 120_000);
    }

    #[test]
    fn text_matching_is_case_insensitive() {
        let decision = check_fallback_error(500, "Service OVERLOADED", 0);
        assert_eq!(decision.new_backoff_level, Some(1));
    }

    #[test]
    fn quota_backoff_doubles_and_saturates() {
        assert_eq!(quota_cooldown(1), 2000);
        assert_eq!(quota_cooldown(2), 4000);
        assert_eq!(quota_cooldown(3), 8000);
        // Capped at the 5-minute ceiling, and never overflows at high levels.
        assert_eq!(quota_cooldown(15), 300_000);
        assert_eq!(quota_cooldown(64), 300_000);
        assert_eq!(quota_cooldown(u32::MAX), 300_000);
    }

    #[test]
    fn backoff_level_is_capped() {
        let decision = check_fallback_error(429, "", 99);
        assert_eq!(decision.new_backoff_level, Some(15));
    }

    #[test]
    fn unmatched_errors_still_fall_back_transiently() {
        let decision = check_fallback_error(418, "teapot", 0);
        assert!(decision.should_fallback);
        assert_eq!(decision.cooldown_ms, 30_000);
        assert_eq!(decision.new_backoff_level, None);
    }

    #[test]
    fn retry_hints_render_hours_minutes_seconds() {
        assert_eq!(format_retry_after(0), "reset after 0s");
        assert_eq!(format_retry_after(-5000), "reset after 0s");
        assert_eq!(format_retry_after(30_000), "reset after 30s");
        assert_eq!(format_retry_after(90_000), "reset after 1m 30s");
        assert_eq!(format_retry_after(3_600_000), "reset after 1h");
        assert_eq!(format_retry_after(3_661_000), "reset after 1h 1m 1s");
        // Partial seconds round up, matching Math.ceil upstream.
        assert_eq!(format_retry_after(1500), "reset after 2s");
    }

    #[test]
    fn provider_errors_are_prefixed_with_status() {
        assert_eq!(format_provider_error(502, "boom"), "[502]: boom");
    }

    #[test]
    fn rate_limit_cap_is_thirty_minutes() {
        assert_eq!(MAX_RATE_LIMIT_COOLDOWN_MS, 1_800_000);
        assert!(default_error_message(429).is_some());
        assert!(default_error_message(418).is_none());
    }
}
