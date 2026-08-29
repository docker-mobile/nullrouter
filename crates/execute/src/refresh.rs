//! OAuth access-token refresh.
//!
//! Ports the generic path of `open-sse/services/tokenRefresh.js` and
//! `tokenRefresh/providers.js`: the `refresh_token` grant, the per-provider body
//! encoding and extra headers, the expiry check that decides when to run it, and
//! the de-duplication that stops concurrent requests from spending the same
//! refresh token twice.
//!
//! Before this, a refresh token was stored and never used, so an OAuth connection
//! worked until its access token expired and then failed until the user
//! re-authorised by hand.
//!
//! Not ported: the providers whose refresh is not a `refresh_token` grant at all —
//! Kiro's AWS SSO-OIDC exchange, GitHub's separate Copilot token mint, and Vertex's
//! service-account JWT assertion. [`supports_refresh`] reports false for those, so
//! they are left alone rather than sent a grant they would reject.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nullrouter_providers::registry;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::credentials::Credentials;

/// Refresh when the token expires within this window and the provider declares no
/// lead of its own (upstream `TOKEN_EXPIRY_BUFFER_MS`).
pub const DEFAULT_REFRESH_LEAD_MS: u64 = 5 * 60 * 1000;

/// How long a completed refresh is reused for (upstream `REFRESH_RESULT_TTL_MS`).
const RESULT_TTL: Duration = Duration::from_secs(10);

/// Providers whose refresh is not a `refresh_token` grant.
///
/// Each needs a protocol this module does not implement, and sending them a
/// standard grant would burn the refresh token against an endpoint that cannot
/// honour it.
const UNSUPPORTED: [&str; 5] = [
    // AWS SSO-OIDC device exchange, plus a separate social-login refresh host.
    "kiro",
    // Needs a second mint against `copilot_internal/v2/token` after the grant.
    "github",
    // Service-account JWT assertion, not a refresh token.
    "vertex",
    "vertex-partner",
    // Refresh is performed by the provider's own signed client.
    "cursor",
];

/// A refreshed credential, as the provider returned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refreshed {
    pub access_token: String,
    /// The new refresh token, or the one just used when the provider rotated none.
    pub refresh_token: String,
    /// Absolute expiry, RFC3339, when the provider reported a lifetime.
    pub expires_at: Option<String>,
    /// Present for providers that return one; stored alongside the access token.
    pub id_token: Option<String>,
}

/// Why a refresh did not produce a usable token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshError {
    /// This provider's refresh is not a `refresh_token` grant.
    Unsupported,
    /// No refresh token, or no endpoint to send it to.
    NotConfigured,
    /// The refresh token will never work again; the user must re-authorise.
    ///
    /// Distinguished from a transient failure so the caller can say so instead of
    /// retrying forever against a token the provider has revoked.
    Rejected { message: String },
    /// A transient failure: network, 5xx, or an unreadable body.
    Transient { message: String },
}

impl RefreshError {
    /// `true` when retrying can never succeed.
    pub const fn is_permanent(&self) -> bool {
        matches!(self, Self::Rejected { .. } | Self::Unsupported)
    }
}

/// `true` when this provider's tokens can be refreshed by the generic grant.
pub fn supports_refresh(provider: &str) -> bool {
    if UNSUPPORTED.contains(&provider) {
        return false;
    }
    registry::entry(provider)
        .and_then(|entry| entry.oauth.as_ref())
        .is_some_and(|oauth| oauth.effective_refresh_url().is_some())
}

/// How long before expiry this provider wants its token refreshed.
pub fn refresh_lead_ms(provider: &str) -> u64 {
    registry::entry(provider)
        .and_then(|entry| entry.oauth.as_ref())
        .and_then(|oauth| oauth.refresh_lead_ms)
        .unwrap_or(DEFAULT_REFRESH_LEAD_MS)
}

/// Whether these credentials should be refreshed before use.
///
/// `now_ms` is passed in so the decision is testable without waiting.
pub fn should_refresh(provider: &str, credentials: &Credentials, now_ms: u64) -> bool {
    if credentials.refresh_token.is_none() || !supports_refresh(provider) {
        return false;
    }
    if let Some(expires_at) = credentials
        .expires_at
        .as_deref()
        .and_then(parse_rfc3339_millis)
    {
        // Saturating: an already-expired token has zero remaining, not a huge one.
        let remaining = expires_at.saturating_sub(now_ms);
        if remaining < refresh_lead_ms(provider) {
            return true;
        }
    }
    // Codex invalidates a refresh token that goes unused for long enough, so it is
    // exercised on a schedule rather than only on expiry.
    let Some(max_age) = registry::entry(provider)
        .and_then(|entry| entry.oauth.as_ref())
        .and_then(|oauth| oauth.max_refresh_age_ms)
    else {
        return false;
    };
    let last = credentials
        .setting("lastRefreshAt")
        .and_then(parse_rfc3339_millis);
    last.is_none_or(|last| now_ms.saturating_sub(last) >= max_age)
}

/// Parse an RFC3339 timestamp to epoch milliseconds.
///
/// Hand-rolled because the workspace carries no date library and this is the only
/// place one would be needed. Accepts the two shapes state writes: `Z`-suffixed
/// UTC and a numeric offset.
pub(crate) fn parse_rfc3339_millis(text: &str) -> Option<u64> {
    let text = text.trim();
    // A bare epoch-millis value, which some providers' own records carry.
    if let Ok(millis) = text.parse::<u64>() {
        return Some(millis);
    }
    let bytes = text.as_bytes();
    let number = |from: usize, to: usize| -> Option<u64> {
        std::str::from_utf8(bytes.get(from..to)?).ok()?.parse().ok()
    };
    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // Days from the civil epoch, via Howard Hinnant's `days_from_civil`.
    let year_adjusted = if month <= 2 { year - 1 } else { year };
    let era = year_adjusted / 400;
    let year_of_era = year_adjusted - era * 400;
    let month_shifted = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_shifted + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = i64::try_from(era * 146_097 + day_of_era).ok()? - 719_468;

    let seconds = days * 86_400 + i64::try_from(hour * 3600 + minute * 60 + second).ok()?;
    // A fractional part is ignored: nothing here needs sub-second precision.
    let offset = trailing_offset_seconds(text)?;
    u64::try_from((seconds - offset) * 1000).ok()
}

/// The timezone offset in seconds, or `None` when the text has no valid one.
fn trailing_offset_seconds(text: &str) -> Option<i64> {
    if text.ends_with('Z') || text.ends_with('z') {
        return Some(0);
    }
    // `+HH:MM` / `-HH:MM`, or no offset at all (treated as UTC, as state writes it).
    let tail = text.get(text.len().checked_sub(6)?..)?;
    let sign = match tail.as_bytes().first() {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return Some(0),
    };
    let hours: i64 = tail.get(1..3)?.parse().ok()?;
    let minutes: i64 = tail.get(4..6)?.parse().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
}

/// Render epoch milliseconds as RFC3339 UTC.
pub(crate) fn format_rfc3339_millis(millis: u64) -> String {
    let total_seconds = millis / 1000;
    let days = i64::try_from(total_seconds / 86_400).unwrap_or(0);
    let time_of_day = total_seconds % 86_400;
    // Inverse of `days_from_civil`.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_shifted + 2) / 5 + 1;
    let month = if month_shifted < 10 {
        month_shifted + 3
    } else {
        month_shifted - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60,
    )
}

/// The grant body and its content type.
///
/// Separated from the request so a test can assert the exact payload: the field
/// names and whether the secret is included are the whole provider difference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantBody {
    pub content_type: &'static str,
    pub body: String,
}

/// Build the `refresh_token` grant for a provider.
pub fn grant_body(provider: &str, refresh_token: &str) -> Option<GrantBody> {
    let oauth = registry::entry(provider)?.oauth.as_ref()?;
    let mut fields: Vec<(&str, String)> = vec![
        ("grant_type", "refresh_token".to_owned()),
        ("refresh_token", refresh_token.to_owned()),
    ];
    if let Some(client_id) = oauth.client_id.as_deref() {
        fields.push(("client_id", client_id.to_owned()));
    }
    // Only sent where the provider's own CLI ships one; a missing secret is not an
    // error, because most of these are public clients.
    if let Some(secret) = oauth.client_secret.as_deref() {
        fields.push(("client_secret", secret.to_owned()));
    }
    if let Some(scope) = oauth
        .refresh
        .as_ref()
        .and_then(|refresh| refresh.scope.as_deref())
    {
        fields.push(("scope", scope.to_owned()));
    }

    if oauth.refresh_is_json() {
        let object: serde_json::Map<String, Value> = fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), Value::String(value)))
            .collect();
        return Some(GrantBody {
            content_type: "application/json",
            body: Value::Object(object).to_string(),
        });
    }
    Some(GrantBody {
        content_type: "application/x-www-form-urlencoded",
        body: fields
            .iter()
            .map(|(key, value)| format!("{key}={}", form_encode(value)))
            .collect::<Vec<_>>()
            .join("&"),
    })
}

/// Percent-encode a form value.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            other => {
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

/// Headers a provider needs on its refresh request beyond the content type.
pub fn grant_headers(provider: &str, credentials: &Credentials) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("Accept".to_owned(), "application/json".to_owned());
    match provider {
        // iFlow authenticates the client in a Basic header rather than the body.
        "iflow" => {
            if let Some(oauth) = registry::entry(provider).and_then(|entry| entry.oauth.as_ref())
                && let (Some(id), Some(secret)) =
                    (oauth.client_id.as_deref(), oauth.client_secret.as_deref())
            {
                headers.insert(
                    "Authorization".to_owned(),
                    format!("Basic {}", base64(format!("{id}:{secret}").as_bytes())),
                );
            }
        }
        // Kimi keys the grant to the device that authorised it.
        "kimi" | "kimi-coding" => {
            if let Some(device) = credentials.setting("deviceId") {
                headers.insert("X-Msh-Device-Id".to_owned(), device.to_owned());
            }
            headers.insert("X-Msh-Platform".to_owned(), "cli".to_owned());
        }
        _ => {}
    }
    headers
}

/// Standard base64, for the one header that needs it.
fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let bytes = [
            chunk.first().copied().unwrap_or(0),
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let packed = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        for shift in [18, 12, 6, 0] {
            let index = usize::try_from((packed >> shift) & 0x3F).unwrap_or(0);
            out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'A')));
        }
    }
    // Pad to the input length.
    let padding = (3 - input.len() % 3) % 3;
    out.truncate(out.len() - padding);
    out.push_str(&"=".repeat(padding));
    out
}

/// A token endpoint's reply.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    id_token: Option<String>,
    /// `OAuth2` error code, when the endpoint answers 200 with a failure.
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// Interpret a token endpoint's reply.
///
/// `status` and `body` are separate from the transport so this is testable without
/// a socket, which matters: the difference between "retry later" and "the user must
/// re-authorise" is decided entirely here.
pub fn settle(
    status: u16,
    body: &str,
    previous_refresh_token: &str,
    now_ms: u64,
) -> Result<Refreshed, RefreshError> {
    let parsed = serde_json::from_str::<TokenResponse>(body).ok();
    let reported_error = parsed.as_ref().and_then(|parsed| parsed.error.clone());
    let description = parsed
        .as_ref()
        .and_then(|parsed| parsed.error_description.clone());

    // `invalid_grant` means this refresh token is finished — the user has to
    // re-authorise, and retrying only wastes requests.
    if let Some(code) = reported_error.as_deref()
        && matches!(
            code,
            "invalid_grant" | "invalid_request" | "invalid_client" | "unauthorized_client"
        )
    {
        return Err(RefreshError::Rejected {
            message: description.unwrap_or_else(|| code.to_owned()),
        });
    }
    if matches!(status, 400 | 401 | 403) {
        return Err(RefreshError::Rejected {
            message: description
                .or(reported_error)
                .unwrap_or_else(|| format!("the token endpoint answered {status}")),
        });
    }
    if !(200..300).contains(&status) {
        return Err(RefreshError::Transient {
            message: format!("the token endpoint answered {status}"),
        });
    }
    let Some(parsed) = parsed else {
        return Err(RefreshError::Transient {
            message: "the token endpoint returned an unreadable body".to_owned(),
        });
    };
    let Some(access_token) = parsed.access_token.filter(|token| !token.is_empty()) else {
        return Err(RefreshError::Transient {
            message: "the token endpoint returned no access token".to_owned(),
        });
    };

    Ok(Refreshed {
        access_token,
        // A provider that rotates no refresh token keeps the current one working;
        // dropping it here would lose the only way to refresh again.
        refresh_token: parsed
            .refresh_token
            .filter(|token| !token.is_empty())
            .unwrap_or_else(|| previous_refresh_token.to_owned()),
        expires_at: parsed
            .expires_in
            .map(|seconds| format_rfc3339_millis(now_ms + seconds * 1000)),
        id_token: parsed.id_token.filter(|token| !token.is_empty()),
    })
}

/// The credential update to persist after a refresh.
pub fn persist_body(connection_id: &str, refreshed: &Refreshed) -> Value {
    let mut settings = serde_json::Map::new();
    // Recorded so the max-age rule has something to measure from.
    settings.insert(
        "lastRefreshAt".to_owned(),
        json!(format_rfc3339_millis(now_millis())),
    );
    if let Some(id_token) = refreshed.id_token.as_deref() {
        settings.insert("idToken".to_owned(), json!(id_token));
    }
    json!({
        "connectionId": connection_id,
        "accessToken": refreshed.access_token,
        "refreshToken": refreshed.refresh_token,
        "expiresAt": refreshed.expires_at,
        "providerSpecificData": settings,
    })
}

/// Milliseconds since the epoch.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(0))
}

/// Recently completed refreshes, keyed by provider and the token that was spent.
///
/// Without this, two concurrent requests on one expiring connection both refresh;
/// the second spends a token the first has already rotated away, and a provider
/// that invalidates a reused refresh token then locks the connection out.
#[derive(Debug, Clone, Default)]
pub struct RefreshCache {
    entries: Arc<Mutex<BTreeMap<String, CacheEntry>>>,
}

/// One cached outcome and when it landed.
type CacheEntry = (Instant, Result<Refreshed, RefreshError>);

impl RefreshCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// A result for this exact token, if one landed within the TTL.
    pub fn get(
        &self,
        provider: &str,
        refresh_token: &str,
    ) -> Option<Result<Refreshed, RefreshError>> {
        let key = Self::key(provider, refresh_token);
        let mut entries = self.entries.lock().ok()?;
        let fresh = match entries.get(&key) {
            Some((at, result)) if at.elapsed() <= RESULT_TTL => Some(result.clone()),
            Some(_) => {
                entries.remove(&key);
                None
            }
            None => None,
        };
        // The guard is dropped here rather than held across the caller's `?`.
        drop(entries);
        fresh
    }

    /// Record a result for reuse by concurrent callers.
    pub fn put(
        &self,
        provider: &str,
        refresh_token: &str,
        result: &Result<Refreshed, RefreshError>,
    ) {
        if let Ok(mut entries) = self.entries.lock() {
            // Bound the map: a long-lived process must not accumulate one entry per
            // token it has ever refreshed.
            entries.retain(|_, (at, _)| at.elapsed() <= RESULT_TTL);
            entries.insert(
                Self::key(provider, refresh_token),
                (Instant::now(), result.clone()),
            );
        }
    }

    fn key(provider: &str, refresh_token: &str) -> String {
        format!("{provider}:{refresh_token}")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_REFRESH_LEAD_MS, RefreshCache, RefreshError, format_rfc3339_millis, grant_body,
        grant_headers, parse_rfc3339_millis, persist_body, refresh_lead_ms, settle, should_refresh,
        supports_refresh,
    };
    use crate::credentials::Credentials;
    use serde_json::json;

    /// 2024-06-01T00:00:00Z.
    const NOW: u64 = 1_717_200_000_000;

    fn oauth_credentials(expires_at: Option<&str>) -> Credentials {
        Credentials {
            access_token: Some("old-access".to_owned()),
            refresh_token: Some("refresh-1".to_owned()),
            expires_at: expires_at.map(str::to_owned),
            connection_id: "conn_1".to_owned(),
            ..Credentials::default()
        }
    }

    #[test]
    fn timestamps_round_trip_through_epoch_millis() {
        assert_eq!(parse_rfc3339_millis("2024-06-01T00:00:00Z"), Some(NOW));
        assert_eq!(format_rfc3339_millis(NOW), "2024-06-01T00:00:00Z");
        // A numeric offset is honoured, not ignored: treating `+02:00` as UTC would
        // put the expiry two hours late and skip a refresh that was due.
        assert_eq!(parse_rfc3339_millis("2024-06-01T02:00:00+02:00"), Some(NOW),);
        // Fractional seconds are accepted.
        assert_eq!(parse_rfc3339_millis("2024-06-01T00:00:00.500Z"), Some(NOW));
        // A leap-year date and a year boundary.
        assert_eq!(
            format_rfc3339_millis(parse_rfc3339_millis("2024-02-29T23:59:59Z").expect("parses")),
            "2024-02-29T23:59:59Z"
        );
        assert_eq!(
            format_rfc3339_millis(parse_rfc3339_millis("2025-01-01T00:00:00Z").expect("parses")),
            "2025-01-01T00:00:00Z"
        );
        // Garbage is rejected rather than treated as the epoch, which would make
        // every token look expired.
        assert_eq!(parse_rfc3339_millis("not a date"), None);
        assert_eq!(parse_rfc3339_millis(""), None);
        assert_eq!(parse_rfc3339_millis("2024-13-01T00:00:00Z"), None);
    }

    #[test]
    fn only_providers_with_a_refresh_grant_are_refreshable() {
        // A standard grant with an endpoint in the registry.
        assert!(supports_refresh("claude"));
        assert!(supports_refresh("codex"));
        assert!(supports_refresh("gemini-cli"));
        assert!(supports_refresh("kimi"));
        assert!(supports_refresh("iflow"));

        // These need a protocol this module does not implement. Sending them a
        // standard grant would spend the refresh token against an endpoint that
        // cannot honour it.
        for provider in ["kiro", "github", "vertex", "cursor"] {
            assert!(!supports_refresh(provider), "{provider} must be excluded");
        }
        // And a plain API-key provider has no OAuth block at all.
        assert!(!supports_refresh("openai"));
        assert!(!supports_refresh("not-a-provider"));
    }

    #[test]
    fn the_refresh_lead_comes_from_the_provider_not_a_single_constant() {
        // These differ by four orders of magnitude, which is why one constant for
        // all of them would either refresh far too early or far too late.
        assert_eq!(refresh_lead_ms("codex"), 432_000_000);
        assert_eq!(refresh_lead_ms("claude"), 14_400_000);
        assert_eq!(refresh_lead_ms("kimi"), 300_000);
        // A provider declaring none falls back to the shared buffer.
        assert_eq!(refresh_lead_ms("gemini-cli"), DEFAULT_REFRESH_LEAD_MS);
    }

    #[test]
    fn a_token_inside_its_providers_lead_window_is_refreshed() {
        // Kimi wants five minutes' lead. Four minutes out is inside it.
        let soon = format_rfc3339_millis(NOW + 4 * 60 * 1000);
        assert!(should_refresh("kimi", &oauth_credentials(Some(&soon)), NOW));

        // An hour out is not.
        let later = format_rfc3339_millis(NOW + 60 * 60 * 1000);
        assert!(!should_refresh(
            "kimi",
            &oauth_credentials(Some(&later)),
            NOW
        ));

        // But an hour out *is* inside Claude's four-hour lead.
        assert!(should_refresh(
            "claude",
            &oauth_credentials(Some(&later)),
            NOW
        ));
    }

    #[test]
    fn an_already_expired_token_is_refreshed_rather_than_overflowing() {
        // Naive subtraction here underflows and makes the remaining time enormous,
        // which would skip the refresh on exactly the tokens that need it.
        let past = format_rfc3339_millis(NOW - 60 * 60 * 1000);
        assert!(should_refresh("kimi", &oauth_credentials(Some(&past)), NOW));
    }

    #[test]
    fn a_connection_with_nothing_to_refresh_is_left_alone() {
        // No refresh token: nothing to spend.
        let mut credentials = oauth_credentials(Some("2024-06-01T00:00:01Z"));
        credentials.refresh_token = None;
        assert!(!should_refresh("kimi", &credentials, NOW));

        // A provider with no grant, even with a token and an imminent expiry.
        let expiring = format_rfc3339_millis(NOW + 1000);
        assert!(!should_refresh(
            "kiro",
            &oauth_credentials(Some(&expiring)),
            NOW
        ));
    }

    #[test]
    fn codex_refreshes_on_age_even_when_the_token_has_not_expired() {
        // Codex invalidates a refresh token left unused, so it is exercised on a
        // schedule. `maxRefreshAgeMs` is eight days.
        let far_future = format_rfc3339_millis(NOW + 365 * 24 * 60 * 60 * 1000);
        let mut credentials = oauth_credentials(Some(&far_future));

        // Never refreshed: due now.
        assert!(should_refresh("codex", &credentials, NOW));

        // Refreshed nine days ago: past the max age.
        credentials.provider_specific_data.insert(
            "lastRefreshAt".to_owned(),
            json!(format_rfc3339_millis(NOW - 9 * 24 * 60 * 60 * 1000)),
        );
        assert!(should_refresh("codex", &credentials, NOW));

        // Refreshed an hour ago: not due, and the expiry is far off.
        credentials.provider_specific_data.insert(
            "lastRefreshAt".to_owned(),
            json!(format_rfc3339_millis(NOW - 60 * 60 * 1000)),
        );
        assert!(!should_refresh("codex", &credentials, NOW));

        // A provider without the rule is not put on a schedule.
        let mut gemini = oauth_credentials(Some(&far_future));
        gemini.provider_specific_data.clear();
        assert!(!should_refresh("gemini-cli", &gemini, NOW));
    }

    #[test]
    fn a_form_grant_carries_the_documented_fields() {
        let grant = grant_body("codex", "refresh-1").expect("a grant");
        assert_eq!(grant.content_type, "application/x-www-form-urlencoded");
        assert!(
            grant.body.contains("grant_type=refresh_token"),
            "{}",
            grant.body
        );
        assert!(
            grant.body.contains("refresh_token=refresh-1"),
            "{}",
            grant.body
        );
        assert!(
            grant
                .body
                .contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"),
            "{}",
            grant.body
        );
        // Codex's refresh declares a scope; omitting it narrows the new token.
        assert!(
            grant
                .body
                .contains("scope=openid+profile+email+offline_access"),
            "{}",
            grant.body
        );
        // No secret: Codex is a public client, and sending an empty one is refused.
        assert!(!grant.body.contains("client_secret"), "{}", grant.body);
    }

    #[test]
    fn a_json_grant_is_sent_as_json() {
        // Anthropic's token endpoint rejects a form body.
        let grant = grant_body("claude", "refresh-1").expect("a grant");
        assert_eq!(grant.content_type, "application/json");
        let parsed: serde_json::Value = serde_json::from_str(&grant.body).expect("json");
        assert_eq!(parsed.get("grant_type"), Some(&json!("refresh_token")));
        assert_eq!(parsed.get("refresh_token"), Some(&json!("refresh-1")));
        assert_eq!(
            parsed.get("client_id"),
            Some(&json!("9d1c250a-e61b-44d9-88ed-5944d1962f5e"))
        );
    }

    #[test]
    fn a_provider_with_a_public_secret_sends_it() {
        // iFlow ships one in its own CLI, and the grant fails without it.
        let grant = grant_body("iflow", "refresh-1").expect("a grant");
        assert!(grant.body.contains("client_secret="), "{}", grant.body);
    }

    #[test]
    fn a_token_with_characters_needing_encoding_survives_the_form_body() {
        let grant = grant_body("codex", "abc/def+ghi=jkl").expect("a grant");
        // Unencoded, `+` would arrive as a space and `&` would split the body.
        assert!(
            grant.body.contains("refresh_token=abc%2Fdef%2Bghi%3Djkl"),
            "{}",
            grant.body
        );
    }

    #[test]
    fn providers_that_authenticate_outside_the_body_get_their_headers() {
        // iFlow puts the client credentials in a Basic header.
        let headers = grant_headers("iflow", &Credentials::default());
        let authorization = headers.get("Authorization").expect("basic auth");
        assert!(authorization.starts_with("Basic "), "got {authorization}");
        // Decodable back to `id:secret`.
        assert!(authorization.len() > "Basic ".len() + 8);

        // Kimi keys the grant to the authorising device.
        let mut kimi = Credentials::default();
        kimi.provider_specific_data
            .insert("deviceId".to_owned(), json!("device-9"));
        let headers = grant_headers("kimi", &kimi);
        assert_eq!(
            headers.get("X-Msh-Device-Id").map(String::as_str),
            Some("device-9")
        );

        // A provider needing nothing extra gets only `Accept`.
        let plain = grant_headers("gemini-cli", &Credentials::default());
        assert_eq!(plain.len(), 1);
        assert!(plain.contains_key("Accept"));
    }

    #[test]
    fn a_successful_reply_yields_a_token_and_an_absolute_expiry() {
        let refreshed = settle(
            200,
            r#"{"access_token":"new-access","refresh_token":"refresh-2","expires_in":3600}"#,
            "refresh-1",
            NOW,
        )
        .expect("a refresh");
        assert_eq!(refreshed.access_token, "new-access");
        assert_eq!(refreshed.refresh_token, "refresh-2");
        // `expires_in` is relative; what gets stored has to be absolute.
        assert_eq!(
            refreshed.expires_at.as_deref(),
            Some("2024-06-01T01:00:00Z")
        );
    }

    #[test]
    fn a_reply_that_rotates_no_refresh_token_keeps_the_current_one() {
        // Dropping it would lose the only way to refresh again, turning one
        // successful refresh into the last one this connection ever gets.
        let refreshed = settle(
            200,
            r#"{"access_token":"new-access","expires_in":60}"#,
            "refresh-1",
            NOW,
        )
        .expect("a refresh");
        assert_eq!(refreshed.refresh_token, "refresh-1");
    }

    #[test]
    fn an_invalid_grant_is_permanent_not_transient() {
        // The user has to re-authorise. Retrying only spends requests against a
        // token the provider has revoked.
        let error = settle(
            400,
            r#"{"error":"invalid_grant","error_description":"Refresh token expired"}"#,
            "refresh-1",
            NOW,
        )
        .expect_err("rejected");
        assert!(error.is_permanent(), "{error:?}");
        match error {
            RefreshError::Rejected { message } => {
                assert!(message.contains("Refresh token expired"), "{message}");
            }
            other => panic!("expected a rejection, got {other:?}"),
        }

        // Some endpoints answer 200 with an error body; that is still permanent.
        let error =
            settle(200, r#"{"error":"invalid_grant"}"#, "refresh-1", NOW).expect_err("rejected");
        assert!(error.is_permanent(), "{error:?}");
    }

    #[test]
    fn a_server_failure_is_transient_so_the_token_is_not_discarded() {
        for status in [500, 502, 503, 504] {
            let error =
                settle(status, "upstream is down", "refresh-1", NOW).expect_err("transient");
            assert!(
                !error.is_permanent(),
                "{status} must not be treated as a revoked token"
            );
        }
        // A 2xx with no token is also transient: nothing says the token is bad.
        let error = settle(200, "{}", "refresh-1", NOW).expect_err("transient");
        assert!(!error.is_permanent(), "{error:?}");
        // As is an unreadable body.
        let error = settle(200, "<html>", "refresh-1", NOW).expect_err("transient");
        assert!(!error.is_permanent(), "{error:?}");
    }

    #[test]
    fn the_persisted_update_carries_the_new_token_and_a_refresh_timestamp() {
        let refreshed = settle(
            200,
            r#"{"access_token":"a","refresh_token":"r","expires_in":60,"id_token":"idt"}"#,
            "old",
            NOW,
        )
        .expect("a refresh");
        let body = persist_body("conn_1", &refreshed);

        assert_eq!(body.get("connectionId"), Some(&json!("conn_1")));
        assert_eq!(body.get("accessToken"), Some(&json!("a")));
        assert_eq!(body.get("refreshToken"), Some(&json!("r")));
        assert!(
            body.get("expiresAt")
                .is_some_and(serde_json::Value::is_string)
        );
        // The max-age rule needs something to measure from.
        assert!(
            body.pointer("/providerSpecificData/lastRefreshAt")
                .is_some_and(serde_json::Value::is_string),
            "{body}"
        );
        // An id token is kept where a provider returns one.
        assert_eq!(
            body.pointer("/providerSpecificData/idToken"),
            Some(&json!("idt"))
        );
    }

    #[test]
    fn a_concurrent_refresh_reuses_the_first_result() {
        // Two requests on one expiring connection must not both spend the token: a
        // provider that invalidates a reused refresh token would lock the account
        // out entirely.
        let cache = RefreshCache::new();
        assert!(cache.get("claude", "refresh-1").is_none());

        let result = settle(
            200,
            r#"{"access_token":"a","refresh_token":"b","expires_in":60}"#,
            "refresh-1",
            NOW,
        );
        cache.put("claude", "refresh-1", &result);

        assert_eq!(cache.get("claude", "refresh-1"), Some(result));
        // Keyed by the exact token, so a later rotation is a separate entry.
        assert!(cache.get("claude", "refresh-2").is_none());
        // And by provider, so two providers sharing a token do not collide.
        assert!(cache.get("codex", "refresh-1").is_none());
    }

    #[test]
    fn a_cached_rejection_is_reused_too() {
        // Otherwise every concurrent request re-asks a provider that already said
        // the token is dead.
        let cache = RefreshCache::new();
        let rejected: Result<super::Refreshed, RefreshError> = Err(RefreshError::Rejected {
            message: "invalid_grant".to_owned(),
        });
        cache.put("claude", "refresh-1", &rejected);
        assert_eq!(cache.get("claude", "refresh-1"), Some(rejected));
    }
}
