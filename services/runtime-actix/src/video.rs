//! Async video jobs: `POST /v1/videos/{generations,edits,extensions}` and
//! `GET /v1/videos/{id}`.
//!
//! Ports `inspire/src/sse/handlers/videoGeneration.js` and
//! `inspire/open-sse/handlers/videoCore.js`.
//!
//! Three properties make this endpoint family different from every other one the
//! runtime serves, and each one shapes the code below:
//!
//! * **The body is forwarded byte-for-byte.** Edits and extensions accept multipart
//!   uploads. Parsing a multipart body and re-encoding it mints a fresh boundary
//!   that no longer matches the client's `Content-Type`, so the bytes are passed
//!   through untouched. Only a JSON body is parsed, and only to strip the
//!   `provider/` prefix from `model`.
//! * **A creation POST is a billable job.** It is never retried on a 5xx or a
//!   network error, because either may have created the job upstream. The only
//!   rotation is to a different account on 401/403/429, which upstream rejects
//!   *before* creating anything.
//! * **A job is account-bound.** Only the account that created a job can poll it,
//!   so creation returns the connection id and a poll pins to it.

use std::time::Instant;

use actix_web::http::StatusCode;
use nullrouter_execute::{Credentials, RawRequest, build_error_body};
use nullrouter_providers::{ServiceKind, model};
use serde_json::Value;

use crate::pipeline::{ChatContext, Runtime};
use crate::responses;
use crate::state_client::Selection;

/// Video generation is xAI-only today. A request with no provider prefix — or a
/// multipart body, which is deliberately not parsed — lands here.
const DEFAULT_VIDEO_PROVIDER: &str = "xai";

/// Header naming the account a job belongs to.
///
/// Emitted on every answer and read back on a poll. The name matches 9Router so an
/// existing client keeps working; `x-connection-id` is also accepted inbound,
/// because that is what upstream's own clients send.
pub(crate) const CONNECTION_HEADER: &str = "x-9router-connection-id";

/// Which creation failures may be retried against another account.
///
/// Only statuses upstream rejects before creating a job. A 5xx is returned to the
/// caller as-is: re-sending it could create — and bill for — a second job.
const CREATE_ROTATION_STATUSES: [u16; 3] = [401, 403, 429];

/// How many accounts a creation POST may try.
const MAX_CREATE_ATTEMPTS: usize = 10;

/// Which video endpoint was called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoAction {
    Generations,
    Edits,
    Extensions,
}

impl VideoAction {
    /// Parse the trailing path segment.
    ///
    /// Anything else is a poll id, not an action, so this returns `None` rather
    /// than guessing.
    pub(crate) fn parse(segment: &str) -> Option<Self> {
        match segment {
            "generations" => Some(Self::Generations),
            "edits" => Some(Self::Edits),
            "extensions" => Some(Self::Extensions),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Generations => "generations",
            Self::Edits => "edits",
            Self::Extensions => "extensions",
        }
    }
}

/// Build the upstream URL for a creation or a poll.
///
/// The job id is percent-encoded: it arrives from the client's path and is spliced
/// into a URL.
pub(crate) fn upstream_url(
    base: &str,
    action: Option<VideoAction>,
    job_id: Option<&str>,
) -> String {
    let base = base.trim_end_matches('/');
    if let Some(id) = job_id {
        return format!("{base}/{}", encode_segment(id));
    }
    format!(
        "{base}/{}",
        action.map_or("generations", VideoAction::as_str)
    )
}

/// Percent-encode everything outside RFC 3986 `unreserved`.
fn encode_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0F));
        }
    }
    encoded
}

/// One uppercase hex digit for the low nibble of `value`.
const fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'A' + value - 10) as char,
    }
}

/// What a request body says about routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VideoRoute {
    /// Provider and the model id to forward (empty when the client named none).
    Resolved { provider: String, model: String },
    /// The request cannot be routed. Carries the client-facing message.
    Rejected(String),
}

/// Resolve the provider and model for a creation request.
///
/// `body` is `None` for a body this endpoint does not parse (multipart), which
/// routes to the default provider — the same fallback upstream uses.
pub(crate) fn resolve_route(body: Option<&Value>) -> VideoRoute {
    let Some(requested) = body
        .and_then(|body| body.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return VideoRoute::Resolved {
            provider: DEFAULT_VIDEO_PROVIDER.to_owned(),
            model: String::new(),
        };
    };

    let parsed = model::parse_model(requested);
    let Some(provider) = parsed.provider else {
        // A bare model id has no provider prefix to trust. Prefix-less inference
        // targets chat providers, so video falls back to its default provider and
        // forwards the id unchanged.
        return VideoRoute::Resolved {
            provider: DEFAULT_VIDEO_PROVIDER.to_owned(),
            model: requested.to_owned(),
        };
    };

    if !nullrouter_providers::supports_service(&provider, ServiceKind::Video) {
        return VideoRoute::Rejected(format!(
            "Provider '{provider}' does not support video generation"
        ));
    }
    VideoRoute::Resolved {
        provider,
        model: parsed.model,
    }
}

/// The bytes to forward for a creation request.
///
/// Returns `None` when the original bytes must be sent unchanged — which is the
/// common case. A JSON body is re-encoded only to strip the `provider/` prefix
/// from `model`, because forwarding `xai/grok-imagine-video` would name a model the
/// provider does not have.
pub(crate) fn rewritten_body(parsed: Option<&Value>, model: &str) -> Option<Vec<u8>> {
    let parsed = parsed?;
    if model.is_empty() {
        return None;
    }
    let current = parsed.get("model").and_then(Value::as_str)?;
    if current == model {
        return None;
    }
    let mut body = parsed.clone();
    body.as_object_mut()?
        .insert("model".to_owned(), Value::String(model.to_owned()));
    serde_json::to_vec(&body).ok()
}

/// Whether a creation failure may be retried against another account.
pub(crate) fn rotates_on(status: u16) -> bool {
    CREATE_ROTATION_STATUSES.contains(&status)
}

/// Everything one video call needs beyond the runtime itself.
#[derive(Debug)]
pub(crate) struct VideoRequest<'a> {
    /// Client-visible path, for usage attribution.
    pub endpoint: &'a str,
    /// `Some` for a creation POST, `None` for a poll.
    pub action: Option<VideoAction>,
    /// `Some` for a poll.
    pub job_id: Option<&'a str>,
    /// Raw request bytes, forwarded unchanged.
    pub body: &'a [u8],
    /// The client's `Content-Type`, if it sent one.
    pub content_type: Option<&'a str>,
    /// The client's `Idempotency-Key`, forwarded upstream.
    pub idempotency_key: Option<&'a str>,
    /// The account to pin to, from `x-connection-id`.
    pub preferred_connection: Option<&'a str>,
}

impl Runtime {
    /// Serve one video request: create a job, or poll one.
    pub(crate) async fn execute_video(
        &self,
        request: &VideoRequest<'_>,
    ) -> actix_web::HttpResponse {
        // A JSON body is parsed for its `model`; anything else is opaque here.
        let parsed = request
            .content_type
            .is_some_and(|value| value.contains("application/json"))
            .then(|| serde_json::from_slice::<Value>(request.body))
            .transpose();
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(_error) => {
                return responses::json(
                    StatusCode::BAD_REQUEST,
                    &build_error_body(400, "Invalid JSON body"),
                );
            }
        };

        let (provider, model) = match resolve_route(parsed.as_ref()) {
            VideoRoute::Resolved { provider, model } => (provider, model),
            VideoRoute::Rejected(message) => {
                return responses::json(StatusCode::BAD_REQUEST, &build_error_body(400, &message));
            }
        };

        let Some(base) = nullrouter_providers::service_endpoint(&provider, ServiceKind::Video)
            .and_then(|endpoint| endpoint.base_url.as_deref())
        else {
            return responses::json(
                StatusCode::NOT_IMPLEMENTED,
                &build_error_body(
                    501,
                    &format!("Provider '{provider}' does not support video generation"),
                ),
            );
        };

        let target = model::ModelTarget {
            provider,
            model: model.clone(),
        };
        // The context exists for usage recording; the body is not translated, so a
        // large multipart upload is never cloned into it.
        let context = ChatContext {
            endpoint: request.endpoint,
            body: Value::Null,
            stream: false,
            source_format: nullrouter_providers::Format::OpenAi,
            requested_model: if model.is_empty() {
                target.provider.clone()
            } else {
                format!("{}/{model}", target.provider)
            },
        };

        let forward = rewritten_body(parsed.as_ref(), &model);
        let payload: &[u8] = forward.as_deref().unwrap_or(request.body);
        let url = upstream_url(base, request.action, request.job_id);

        self.dispatch_video(request, &context, &target, &url, payload)
            .await
    }

    /// Select an account, call upstream, and rotate only where it is safe to.
    async fn dispatch_video(
        &self,
        request: &VideoRequest<'_>,
        context: &ChatContext<'_>,
        target: &model::ModelTarget,
        url: &str,
        payload: &[u8],
    ) -> actix_web::HttpResponse {
        // A poll must reach the account that owns the job, so it never rotates.
        let attempts = if request.job_id.is_some() {
            1
        } else {
            MAX_CREATE_ATTEMPTS
        };
        let mut excluded: Vec<String> = Vec::new();
        let mut last: Option<(u16, String)> = None;

        for _ in 0..attempts {
            let selection = self
                .video_credentials(target, request.preferred_connection, &excluded)
                .await;
            let credentials = match selection {
                Selection::Selected(credentials) => *credentials,
                Selection::NoCredentials { message } => {
                    return self
                        .video_fail(context, target, StatusCode::BAD_REQUEST, &message)
                        .await;
                }
                Selection::AllRateLimited {
                    retry_at_ms,
                    last_error,
                    last_error_code,
                } => {
                    return self
                        .video_rate_limited(
                            context,
                            target,
                            retry_at_ms,
                            last.as_ref()
                                .map(|(_, message)| message.clone())
                                .or(last_error),
                            last.as_ref().map(|(status, _)| *status).or(last_error_code),
                        )
                        .await;
                }
                Selection::Exhausted => break,
                Selection::Unavailable { message } => {
                    return self
                        .video_fail(context, target, StatusCode::SERVICE_UNAVAILABLE, &message)
                        .await;
                }
            };

            match self
                .call_video(&VideoCall {
                    request,
                    context,
                    target,
                    url,
                    payload,
                    credentials: &credentials,
                })
                .await
            {
                VideoAttempt::Answered(response) => return response,
                VideoAttempt::Rotate { status, message } => {
                    excluded.push(credentials.connection_id.clone());
                    last = Some((status, message));
                }
            }
        }

        let (status, message) =
            last.unwrap_or_else(|| (503, "All provider accounts are unavailable".to_owned()));
        let status = StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
        responses::json(status, &build_error_body(status.as_u16(), &message))
    }
}

/// One upstream video call's inputs.
#[derive(Debug, Clone, Copy)]
struct VideoCall<'a> {
    request: &'a VideoRequest<'a>,
    context: &'a ChatContext<'a>,
    target: &'a model::ModelTarget,
    /// Absolute upstream URL.
    url: &'a str,
    /// Bytes to forward, already prefix-stripped where that was needed.
    payload: &'a [u8],
    credentials: &'a Credentials,
}

/// One video call's outcome, for the usage log.
#[derive(Debug)]
pub(crate) struct VideoRecord<'a> {
    pub context: &'a ChatContext<'a>,
    pub target: &'a model::ModelTarget,
    pub credentials: Option<&'a Credentials>,
    pub status: u16,
    pub started: Instant,
    pub error: Option<String>,
}

/// The result of one upstream video call.
enum VideoAttempt {
    /// A response for the client, whether or not upstream succeeded.
    Answered(actix_web::HttpResponse),
    /// Safe to try another account.
    Rotate { status: u16, message: String },
}

impl Runtime {
    /// Ask state for an account, honouring the client's pin.
    async fn video_credentials(
        &self,
        target: &model::ModelTarget,
        preferred: Option<&str>,
        excluded: &[String],
    ) -> Selection {
        let model = (!target.model.is_empty()).then_some(target.model.as_str());
        self.state_client()
            .select_credentials_pinned(&target.provider, model, excluded, preferred)
            .await
    }

    /// One upstream call, plus the bookkeeping its result implies.
    async fn call_video(&self, call: &VideoCall<'_>) -> VideoAttempt {
        let VideoCall {
            request,
            context,
            target,
            url,
            payload,
            credentials,
        } = *call;
        let started = Instant::now();
        let is_create = request.job_id.is_none();
        let mut extra: Vec<(&str, &str)> = Vec::new();
        if let Some(key) = request.idempotency_key.filter(|_| is_create) {
            extra.push(("Idempotency-Key", key));
        }

        let outcome = self
            .executor_ref()
            .execute_raw(RawRequest {
                provider: &target.provider,
                url,
                post: is_create,
                body: payload,
                // A poll sends no body, so it declares no content type.
                content_type: request.content_type.filter(|_| is_create),
                extra_headers: &extra,
                credentials,
            })
            .await;

        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                let status = error.client_status();
                let message = error.to_string();
                self.record_video(&VideoRecord {
                    context,
                    target,
                    credentials: Some(credentials),
                    status,
                    started,
                    error: Some(message.clone()),
                })
                .await;
                // A transport failure on a creation POST may still have created the
                // job upstream, so this is reported rather than retried anywhere.
                let status_code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
                return VideoAttempt::Answered(responses::json(
                    status_code,
                    &build_error_body(status, &message),
                ));
            }
        };

        let status = outcome.status().as_u16();
        let content_type = outcome
            .response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/json")
            .to_owned();
        let body = outcome.response.text().await.unwrap_or_default();

        if !(200..300).contains(&status) {
            let message = scrub(&body, credentials);
            self.record_video(&VideoRecord {
                context,
                target,
                credentials: Some(credentials),
                status,
                started,
                error: Some(message.clone()),
            })
            .await;
            self.cool_down_video(credentials, target, status, &message)
                .await;

            if is_create && rotates_on(status) {
                return VideoAttempt::Rotate { status, message };
            }
            let status_code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
            // The provider's own error body is relayed, so a client sees why the job
            // was refused rather than a synthesised message.
            return VideoAttempt::Answered(responses::relay(
                status_code,
                &content_type,
                message.into_bytes(),
                &[(CONNECTION_HEADER, &credentials.connection_id)],
            ));
        }

        self.record_video(&VideoRecord {
            context,
            target,
            credentials: Some(credentials),
            status,
            started,
            error: None,
        })
        .await;
        self.clear_video_error(credentials, target).await;

        let status_code = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
        // The upstream JSON passes through untouched: `request_id`, `status`, and
        // `video.url` are the provider's contract, not this router's.
        VideoAttempt::Answered(responses::relay(
            status_code,
            &content_type,
            body.into_bytes(),
            &[(CONNECTION_HEADER, &credentials.connection_id)],
        ))
    }
}

/// Replace anything secret-shaped in text bound for a client or a log.
///
/// Upstream error bodies sometimes quote the credential that failed, and this text
/// is relayed verbatim, so the credential is removed first.
pub(crate) fn scrub(text: &str, credentials: &Credentials) -> String {
    const MAX_RELAYED: usize = 2000;
    let mut out: String = text.chars().take(MAX_RELAYED).collect();
    for secret in [
        credentials.access_token.as_deref(),
        credentials.refresh_token.as_deref(),
        credentials.api_key.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|secret| secret.len() >= 8)
    {
        out = out.replace(secret, "[redacted]");
    }
    scrub_bearer(&out)
}

/// Replace `Bearer <token>` runs, which appear even when the token is not one this
/// request's credentials hold.
fn scrub_bearer(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = find_bearer(rest) {
        let (before, tail) = rest.split_at(position);
        out.push_str(before);
        let token_start = tail
            .char_indices()
            .find(|(index, character)| *index >= 6 && !character.is_whitespace())
            .map_or(tail.len(), |(index, _)| index);
        let token_len = tail
            .get(token_start..)
            .unwrap_or_default()
            .chars()
            .take_while(|character| {
                character.is_ascii_alphanumeric()
                    || matches!(character, '.' | '_' | '~' | '+' | '/' | '=' | '-')
            })
            .count();
        if token_len >= 8 {
            out.push_str("Bearer [redacted]");
            rest = tail.get(token_start + token_len..).unwrap_or_default();
        } else {
            out.push_str(tail.get(..6).unwrap_or_default());
            rest = tail.get(6..).unwrap_or_default();
        }
    }
    out.push_str(rest);
    out
}

/// Byte offset of the next case-insensitive `bearer`.
fn find_bearer(text: &str) -> Option<usize> {
    text.char_indices()
        .find(|(index, _)| {
            text.get(*index..index + 6)
                .is_some_and(|window| window.eq_ignore_ascii_case("bearer"))
        })
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::{
        VideoAction, VideoRoute, resolve_route, rewritten_body, rotates_on, scrub_bearer,
        upstream_url,
    };
    use serde_json::json;

    #[test]
    fn only_the_three_creation_actions_are_actions() {
        assert_eq!(
            VideoAction::parse("generations"),
            Some(VideoAction::Generations)
        );
        assert_eq!(VideoAction::parse("edits"), Some(VideoAction::Edits));
        assert_eq!(
            VideoAction::parse("extensions"),
            Some(VideoAction::Extensions)
        );
        // A job id must not be mistaken for an action, or a poll would create a job.
        assert_eq!(VideoAction::parse("vid_123"), None);
        assert_eq!(VideoAction::parse("generation"), None);
        assert_eq!(VideoAction::parse(""), None);
    }

    #[test]
    fn upstream_urls_are_built_from_the_registry_base() {
        let base = "https://api.x.ai/v1/videos";
        assert_eq!(
            upstream_url(base, Some(VideoAction::Generations), None),
            "https://api.x.ai/v1/videos/generations"
        );
        assert_eq!(
            upstream_url(base, Some(VideoAction::Edits), None),
            "https://api.x.ai/v1/videos/edits"
        );
        // A trailing slash in the config must not double up.
        assert_eq!(
            upstream_url(
                "https://api.x.ai/v1/videos/",
                Some(VideoAction::Extensions),
                None
            ),
            "https://api.x.ai/v1/videos/extensions"
        );
        // A poll addresses the job, and the id is encoded: it came from a URL path.
        assert_eq!(
            upstream_url(base, None, Some("vid_123")),
            "https://api.x.ai/v1/videos/vid_123"
        );
        assert_eq!(
            upstream_url(base, None, Some("../../models")),
            "https://api.x.ai/v1/videos/..%2F..%2Fmodels"
        );
    }

    #[test]
    fn routing_falls_back_to_the_only_video_provider() {
        // No body at all (multipart): the default provider, no model named.
        assert_eq!(
            resolve_route(None),
            VideoRoute::Resolved {
                provider: "xai".to_owned(),
                model: String::new(),
            }
        );
        // A bare model id keeps the id and uses the default provider, because
        // prefix-less inference targets chat providers.
        assert_eq!(
            resolve_route(Some(&json!({ "model": "grok-imagine-video" }))),
            VideoRoute::Resolved {
                provider: "xai".to_owned(),
                model: "grok-imagine-video".to_owned(),
            }
        );
        // An explicit prefix routes there and the prefix is stripped.
        assert_eq!(
            resolve_route(Some(&json!({ "model": "xai/grok-imagine-video" }))),
            VideoRoute::Resolved {
                provider: "xai".to_owned(),
                model: "grok-imagine-video".to_owned(),
            }
        );
    }

    #[test]
    fn a_provider_without_video_support_is_refused_not_rerouted() {
        // Silently rerouting to xAI would bill an account the client did not name.
        let refused = resolve_route(Some(&json!({ "model": "openai/sora" })));
        match refused {
            VideoRoute::Rejected(message) => {
                assert!(message.contains("openai"), "got {message}");
                assert!(message.contains("video"), "got {message}");
            }
            other @ VideoRoute::Resolved { .. } => {
                panic!("expected a rejection, got {other:?}")
            }
        }
    }

    #[test]
    fn the_body_is_only_rewritten_to_strip_a_provider_prefix() {
        // Prefixed: rewritten with the bare model id.
        let body = json!({ "model": "xai/grok-imagine-video", "prompt": "a cat" });
        let rewritten = rewritten_body(Some(&body), "grok-imagine-video").expect("rewritten");
        let parsed: serde_json::Value = serde_json::from_slice(&rewritten).expect("json");
        assert_eq!(
            parsed.get("model").and_then(|v| v.as_str()),
            Some("grok-imagine-video")
        );
        // Every other field survives.
        assert_eq!(parsed.get("prompt").and_then(|v| v.as_str()), Some("a cat"));

        // Already bare: nothing to rewrite, so the original bytes are forwarded.
        let bare = json!({ "model": "grok-imagine-video" });
        assert!(rewritten_body(Some(&bare), "grok-imagine-video").is_none());
        // No parsed body (multipart): never rewritten, so the boundary survives.
        assert!(rewritten_body(None, "grok-imagine-video").is_none());
        // No model resolved: nothing to substitute.
        assert!(rewritten_body(Some(&bare), "").is_none());
    }

    #[test]
    fn only_pre_creation_failures_rotate_accounts() {
        // Upstream rejects these before creating a job, so another account is safe.
        for status in [401, 403, 429] {
            assert!(rotates_on(status), "{status} must rotate");
        }
        // A 5xx may have created a billable job; re-sending could create a second.
        for status in [500, 502, 503, 504, 400, 404, 200] {
            assert!(!rotates_on(status), "{status} must not rotate");
        }
    }

    #[test]
    fn bearer_tokens_are_removed_from_relayed_text() {
        assert_eq!(
            scrub_bearer("auth failed for Bearer sk-live-abcdefgh1234 on this account"),
            "auth failed for Bearer [redacted] on this account"
        );
        // Case-insensitive, matching the upstream regex.
        assert_eq!(scrub_bearer("bearer AAAABBBBCCCCDDDD"), "Bearer [redacted]");
        // A short run is not a token, and the prose must survive.
        assert_eq!(scrub_bearer("bearer of bad news"), "bearer of bad news");
        // Text with no bearer at all is untouched.
        assert_eq!(scrub_bearer("model not found"), "model not found");
    }
}
