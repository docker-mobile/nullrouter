//! The chat execution pipeline.
//!
//! `future_not_send` is allowed module-wide: these futures carry actix's
//! `HttpResponse`, which is `!Send`. Actix drives them on a per-worker
//! single-threaded executor, so `Send` is not required.
#![allow(
    clippy::future_not_send,
    reason = "actix runs handlers on a !Send per-worker executor; HttpResponse is !Send by design"
)]
//!
//! Ports the orchestration in `inspire/src/sse/handlers/chat.js` plus
//! `open-sse/handlers/chatCore.js`: resolve the model, pick an account,
//! translate, dispatch, and fall back to the next account on a retryable
//! failure — recording usage either way.

use std::time::Instant;

use actix_web::HttpResponse;
use actix_web::http::StatusCode;
use nullrouter_execute::errors::{format_provider_error, parse_upstream_error};
use nullrouter_execute::{
    Credentials, ExecuteRequest, Executor, ProbeCache, RefreshCache, build_error_body,
    check_fallback_error, collapse_stream_to_json, is_executor_supported, pipe_stream,
    unsupported_executor_message,
};
use nullrouter_providers::{Format, ServiceKind, model, target_format};
use nullrouter_pxpipe::TokenSaver;
use nullrouter_translate::state::Clock;
use nullrouter_translate::{RequestRoute, StreamState, translate_request};
use serde_json::Value;

use crate::combo::{self, ComboStrategy, RotationState};
use crate::fusion;
use crate::responses;
use crate::state_client::{Selection, StateClient, UsageReport};

/// Frames buffered between the provider read and the client write.
///
/// Bounded so a slow client throttles the upstream read instead of letting
/// frames pile up in memory. Small enough to keep latency low, large enough to
/// absorb a burst without stalling on every frame.
const STREAM_CHANNEL_FRAMES: usize = 64;

/// A [`FrameSink`] that forwards frames to the client over a bounded channel.
///
/// `send` awaits capacity rather than dropping when full: a dropped frame would
/// truncate JSON mid-object and corrupt the client's parse.
/// Concatenate a batch of SSE frames into one response chunk.
///
/// Separate from the stream so the concatenation can be tested without depending on scheduling:
/// whether several frames are ever queued at once is a property of how fast the socket drains, and
/// an in-process test harness that polls the body after every send never queues more than one.
fn coalesce_frames(batch: &mut [String]) -> actix_web::web::Bytes {
    match batch {
        // Nothing to join. `recv_many` returning 0 is handled by the caller, so this is only
        // reachable if the batch was somehow emptied; an empty chunk is harmless either way.
        [] => actix_web::web::Bytes::new(),
        // One frame, the common case when the producer is not running ahead: hand over the
        // String's own allocation instead of copying it.
        [only] => actix_web::web::Bytes::from(std::mem::take(only)),
        frames => {
            let mut joined = String::with_capacity(frames.iter().map(String::len).sum::<usize>());
            for frame in frames {
                joined.push_str(frame);
            }
            actix_web::web::Bytes::from(joined)
        }
    }
}

struct ChannelSink {
    sender: tokio::sync::mpsc::Sender<String>,
}

impl nullrouter_execute::stream::FrameSink for ChannelSink {
    async fn send(&mut self, frame: String) -> Result<(), ()> {
        // Err only when the receiver is dropped, i.e. the client disconnected.
        self.sender.send(frame).await.map_err(|_| ())
    }
}

/// Read an OpenAI-shaped `usage` object out of a non-streaming reply.
///
/// Accepts the Claude spelling (`input_tokens`/`output_tokens`) too, since a
/// Claude-format provider answering non-streaming returns that shape and its
/// tokens would otherwise be dropped.
fn usage_from_body(body: &Value) -> Option<nullrouter_translate::Usage> {
    let usage = body.get("usage").filter(|usage| usage.is_object())?;
    let read = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let detail = |parent: &str, key: &str| {
        usage
            .get(parent)
            .and_then(|details| details.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };

    let cached =
        detail("prompt_tokens_details", "cached_tokens").max(read("cache_read_input_tokens"));
    let cache_creation = detail("prompt_tokens_details", "cache_creation_tokens")
        .max(read("cache_creation_input_tokens"));
    // OpenAI folds cache into prompt_tokens; Claude reports input separately.
    let prompt = match read("prompt_tokens") {
        0 => read("input_tokens") + cached + cache_creation,
        openai => openai,
    };
    let completion = match read("completion_tokens") {
        0 => read("output_tokens"),
        openai => openai,
    };
    let total = match read("total_tokens") {
        0 => prompt + completion,
        stated => stated,
    };

    // An all-zero usage object carries no information; treat it as absent so a
    // real zero is not confused with a missing field.
    if prompt == 0 && completion == 0 && total == 0 {
        return None;
    }

    Some(nullrouter_translate::Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: total,
        cached_tokens: cached,
        cache_creation_tokens: cache_creation,
        reasoning_tokens: detail("completion_tokens_details", "reasoning_tokens"),
    })
}

/// Human-readable service name, for error messages.
const fn service_label(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Embedding => "embeddings",
        ServiceKind::TextToSpeech => "text-to-speech",
        ServiceKind::SpeechToText => "speech-to-text",
        ServiceKind::ImageGeneration => "image generation",
        ServiceKind::Video => "video generation",
        ServiceKind::Search => "search",
        ServiceKind::Fetch => "fetch",
    }
}

/// How many accounts to try before giving up.
///
/// Upstream loops until credentials run out; this bounds the walk so a
/// misconfigured provider cannot spin indefinitely.
const MAX_ACCOUNT_ATTEMPTS: usize = 10;

/// How long a connection is locked out after its refresh token is rejected.
///
/// Long, because only the user can fix it: retrying a revoked token every request
/// spends requests against a provider that has already refused.
const REAUTH_COOLDOWN_MS: u64 = 60 * 60 * 1000;

/// Upstream's `comboStickyRoundRobinLimit` default, used when state does not
/// report one (an older state service, say).
const DEFAULT_COMBO_STICKY_LIMIT: u32 = 1;

/// One inbound chat-style request.
#[derive(Debug)]
pub(crate) struct ChatContext<'a> {
    /// Client-visible path, for format detection and usage attribution.
    pub endpoint: &'a str,
    /// Parsed request body.
    pub body: Value,
    /// Whether the client asked for a stream.
    pub stream: bool,
    /// Client wire format.
    pub source_format: Format,
    /// Model string exactly as the client sent it.
    pub requested_model: String,
    /// PXPIPE settings for this request, read once from state.
    ///
    /// `None` means "not looked up", which is how every handler builds a context and
    /// what [`Runtime::execute_chat`] fills in. Read once per request rather than per
    /// attempt: a combo can attempt several models, and the answer cannot change
    /// between them.
    pub pxpipe: Option<PxpipeSettings>,
}

/// The PXPIPE settings that govern one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PxpipeSettings {
    pub enabled: bool,
    pub min_chars: u64,
    pub timeout_ms: u64,
}

/// Shared execution dependencies.
///
/// `rotation` is shared rather than per-request: round-robin only means anything
/// relative to the previous request, so the cursor has to outlive one call.
#[derive(Debug, Clone)]
pub struct Runtime {
    executor: Executor,
    state: StateClient,
    rotation: RotationState,
    /// Shared so two concurrent requests on one expiring connection do not both
    /// spend its refresh token. A provider that invalidates a reused refresh token
    /// would otherwise lock the account out.
    refreshes: RefreshCache,
    /// The PXPIPE token saver, holding the Node worker.
    ///
    /// On the runtime rather than in `nullrouter-api` because the transform runs on
    /// the request path: a worker anywhere else would report itself running while
    /// every request here bypassed.
    pxpipe: TokenSaver,
    /// Remote `/models` results for user-added compatible providers.
    ///
    /// Shared and TTL'd because editors call `/v1/models` on startup and sometimes per
    /// completion; an uncached probe would put a provider round trip on a route that is
    /// expected to be cheap.
    probes: ProbeCache,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    /// Runtime wired to the state service from the environment.
    pub fn new() -> Self {
        Self {
            executor: Executor::new(),
            state: StateClient::from_env(),
            rotation: RotationState::new(),
            refreshes: RefreshCache::new(),
            pxpipe: TokenSaver::discover(),
            probes: ProbeCache::new(),
        }
    }

    /// Runtime pointed at an explicit state address, for tests.
    pub fn with_state_addr(addr: &str) -> Self {
        Self {
            executor: Executor::new(),
            state: StateClient::new(addr),
            rotation: RotationState::new(),
            refreshes: RefreshCache::new(),
            pxpipe: TokenSaver::discover(),
            probes: ProbeCache::new(),
        }
    }

    /// Runtime whose token saver uses an explicit data directory, for tests.
    ///
    /// Without this a test would install into, and write events to, whatever
    /// `DATA_DIR` or `$HOME` the suite happened to run under.
    pub fn with_state_addr_and_pxpipe_dir(addr: &str, data_dir: &std::path::Path) -> Self {
        Self {
            executor: Executor::new(),
            state: StateClient::new(addr),
            rotation: RotationState::new(),
            refreshes: RefreshCache::new(),
            pxpipe: TokenSaver::new(nullrouter_pxpipe::Paths::new(data_dir)),
            probes: ProbeCache::new(),
        }
    }

    /// The token saver, for the control routes and the request path.
    pub(crate) const fn token_saver(&self) -> &TokenSaver {
        &self.pxpipe
    }

    /// The PXPIPE settings, from state.
    ///
    /// A state outage reads as disabled, which is the safe default: dispatching the
    /// client's own body is always correct, and imaging it on a guess is not.
    async fn pxpipe_settings(&self) -> PxpipeSettings {
        let settings = self.state.routing_context().await.settings;
        PxpipeSettings {
            enabled: settings.pxpipe_enabled,
            min_chars: settings.pxpipe_min_chars,
            timeout_ms: settings.pxpipe_timeout_ms,
        }
    }

    /// Run the body through PXPIPE, or `None` to dispatch it unchanged.
    ///
    /// Fails open at every step, and the one that matters most is the first: when the
    /// saver is off — which is the default — this returns before doing any work at
    /// all, so a router with PXPIPE disabled pays a settings read it was already
    /// making and nothing else.
    ///
    /// Every attempt is recorded, including the skips, because "I turned it on and
    /// nothing happened" is the common question and the recorded reason is the answer:
    /// the package's own threshold counts compressible content rather than body size,
    /// and it images only a few model families unless configured otherwise, so a large
    /// request on the wrong model is refused for reasons no amount of guessing
    /// recovers.
    async fn compress_body(
        &self,
        context: &ChatContext<'_>,
        target_format: Format,
        upstream_model: &str,
        body: &Value,
    ) -> Option<Value> {
        let settings = context.pxpipe.as_ref()?;
        if !settings.enabled {
            return None;
        }
        let serialised = serde_json::to_string(body).ok()?;
        let gate = nullrouter_pxpipe::Gate {
            enabled: true,
            claude_format: target_format == Format::Claude,
            format: format!("{target_format:?}").to_lowercase(),
            min_chars: settings.min_chars,
            timeout_ms: settings.timeout_ms,
        };
        let result = self
            .pxpipe
            .compress(&serialised, upstream_model, &gate)
            .await;
        if let Some(line) = result.summary.log_line() {
            tracing::info!(provider = %upstream_model, "pxpipe: {line}");
        }
        // A replacement that will not parse is discarded rather than dispatched: a
        // token saver must not be able to turn a valid request into a broken one.
        let replaced = result.body?;
        match serde_json::from_str::<Value>(&replaced) {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::warn!(%error, "pxpipe returned a body that is not JSON; dispatching the original");
                None
            }
        }
    }

    /// The state client, for endpoint families implemented outside this module.
    pub(crate) const fn state_client(&self) -> &StateClient {
        &self.state
    }

    /// The HTTP executor, for endpoint families implemented outside this module.
    pub(crate) const fn executor_ref(&self) -> &Executor {
        &self.executor
    }

    /// Record one video call's outcome.
    ///
    /// Video jobs report no token usage — the provider bills per second of output —
    /// so the usage row carries the status and latency only.
    pub(crate) async fn record_video(&self, outcome: &crate::video::VideoRecord<'_>) {
        let succeeded = (200..300).contains(&outcome.status);
        self.record(
            outcome.context,
            outcome.target,
            outcome.credentials,
            if succeeded { "success" } else { "error" },
            Some(outcome.status),
            nullrouter_translate::Usage::default(),
            outcome.started,
            outcome.error.clone(),
        );
    }

    /// Lock an account after a video failure, using the shared fallback policy.
    pub(crate) async fn cool_down_video(
        &self,
        credentials: &Credentials,
        target: &model::ModelTarget,
        status: u16,
        message: &str,
    ) {
        let decision = check_fallback_error(status, message, 0);
        if !decision.should_fallback {
            return;
        }
        self.state
            .mark_unavailable(&crate::state_client::Cooldown {
                connection_id: &credentials.connection_id,
                model: (!target.model.is_empty()).then_some(target.model.as_str()),
                status,
                reason: message,
                duration_ms: decision.cooldown_ms,
                backoff_level: decision.new_backoff_level,
            })
            .await;
    }

    /// Clear a prior cooldown after a video call succeeds.
    pub(crate) async fn clear_video_error(
        &self,
        credentials: &Credentials,
        target: &model::ModelTarget,
    ) {
        self.state
            .clear_error(
                &credentials.connection_id,
                (!target.model.is_empty()).then_some(target.model.as_str()),
            )
            .await;
    }

    /// [`Self::fail`], reachable from the video module.
    pub(crate) async fn video_fail(
        &self,
        context: &ChatContext<'_>,
        target: &model::ModelTarget,
        status: StatusCode,
        message: &str,
    ) -> HttpResponse {
        self.fail(context, target, status, message).await
    }

    /// [`Self::rate_limited`], reachable from the video module.
    pub(crate) async fn video_rate_limited(
        &self,
        context: &ChatContext<'_>,
        target: &model::ModelTarget,
        retry_at_ms: u64,
        last_error: Option<String>,
        last_error_code: Option<u16>,
    ) -> HttpResponse {
        self.rate_limited(context, target, retry_at_ms, last_error, last_error_code)
            .await
    }

    /// Enforce `requireApiKey` when state has it enabled.
    ///
    /// The gateway's own managed-key flag is static configuration; upstream
    /// reads this from persisted settings, so it is enforced here too — the last
    /// hop before a provider call — or a dashboard toggle would be silently
    /// ignored.
    ///
    /// Returns an error response when the request must be rejected.
    pub(crate) async fn enforce_api_key(&self, api_key: Option<&str>) -> Option<HttpResponse> {
        let presented = api_key.map(str::trim).filter(|key| !key.is_empty());
        let Some(gate) = self.state.api_key_gate(presented).await else {
            // State is the authority for admission. A gate-route failure cannot be treated as a
            // public setting: credential selection is a separate route and may still be available.
            return Some(responses::json(
                StatusCode::SERVICE_UNAVAILABLE,
                &build_error_body(503, "API-key gate unavailable"),
            ));
        };
        if !gate.require_api_key {
            return None;
        }
        if presented.is_none() {
            return Some(responses::json(
                StatusCode::UNAUTHORIZED,
                &build_error_body(401, "Missing API key"),
            ));
        }
        if gate.valid && gate.active {
            return None;
        }
        Some(responses::json(
            StatusCode::UNAUTHORIZED,
            &build_error_body(401, "Invalid API key"),
        ))
    }

    /// Resolve, execute, and respond.
    ///
    /// A combo yields several targets. `fusion` asks all of them at once and has a
    /// judge write the answer; the other strategies try them in turn, and a failure
    /// only advances to the next model when it is one worth retrying elsewhere. A
    /// refusal that would recur on every model — a malformed request, say — is
    /// returned immediately rather than replayed against the whole combo.
    pub(crate) async fn execute_chat(&self, mut context: ChatContext<'_>) -> HttpResponse {
        if context.pxpipe.is_none() {
            context.pxpipe = Some(self.pxpipe_settings().await);
        }
        let resolved = self.resolve_targets(&context.requested_model).await;
        if resolved.strategy == ComboStrategy::Fusion && resolved.targets.len() > 1 {
            return self
                .execute_fusion(&context, &resolved.targets, resolved.tuning)
                .await;
        }
        let Some((last, leading)) = resolved.targets.split_last() else {
            return responses::json(
                StatusCode::BAD_REQUEST,
                &build_error_body(400, "Invalid model format"),
            );
        };

        for target in leading {
            match self.execute_for_target(&context, target).await {
                TargetOutcome::Responded(response) => return response,
                TargetOutcome::Failed { .. } => {}
            }
        }
        // The last model owns the client-visible outcome: there is nothing left to
        // fall back to, so its own error is reported rather than a synthesised
        // one. A single-model request takes this path too, which is why its
        // failure keeps its real status and message.
        match self.execute_for_target(&context, last).await {
            TargetOutcome::Responded(response) | TargetOutcome::Failed { response } => response,
        }
    }

    /// Ask every panel model at once, then have a judge write the answer.
    ///
    /// Collection is quorum-graced: once `min_panel` answers arrive the rest get a
    /// bounded window, so the slowest model does not set the request's latency.
    /// A hard timeout caps the whole fan-out regardless.
    ///
    /// Degrades the way upstream does. No answers is a 503 — there is nothing to
    /// judge. Exactly one answer is returned directly: asking a judge to
    /// "synthesise" a single response would spend a second call to paraphrase it.
    async fn execute_fusion(
        &self,
        context: &ChatContext<'_>,
        panel: &[model::ModelTarget],
        tuning: fusion::FusionTuning,
    ) -> HttpResponse {
        // Only reached with a panel of two or more, so a first model always exists.
        let Some(first) = panel.first() else {
            return responses::json(
                StatusCode::BAD_REQUEST,
                &build_error_body(400, "Fusion combo has no models"),
            );
        };
        let quorum = tuning.quorum(panel.len());
        let panel_body = fusion::panel_body(&context.body);

        let answers = self
            .collect_panel(context, panel, &panel_body, quorum, tuning)
            .await;

        match answers.len() {
            0 => {
                self.fail(
                    context,
                    first,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "All fusion panel models failed",
                )
                .await
            }
            // One answer is the answer. Re-running it through the normal path keeps
            // the client's streaming and tool settings, which the panel call dropped.
            1 => {
                let only = answers
                    .first()
                    .and_then(|answer| {
                        panel
                            .iter()
                            .find(|target| Self::target_label(target) == answer.model)
                    })
                    .unwrap_or(first);
                match self.execute_for_target(context, only).await {
                    TargetOutcome::Responded(response) | TargetOutcome::Failed { response } => {
                        response
                    }
                }
            }
            _ => {
                // The judge keeps the client's original stream flag and tools; only
                // the panel calls were forced to prose. The judge is the first panel
                // model, as upstream defaults when no judge is configured.
                let judge_context = ChatContext {
                    endpoint: context.endpoint,
                    body: fusion::judge_body(&context.body, &answers),
                    stream: context.stream,
                    source_format: context.source_format,
                    requested_model: context.requested_model.clone(),
                    pxpipe: context.pxpipe,
                };
                match self.execute_for_target(&judge_context, first).await {
                    TargetOutcome::Responded(response) | TargetOutcome::Failed { response } => {
                        response
                    }
                }
            }
        }
    }

    /// Run every panel model concurrently and collect whatever answered in time.
    async fn collect_panel(
        &self,
        context: &ChatContext<'_>,
        panel: &[model::ModelTarget],
        panel_body: &Value,
        quorum: usize,
        tuning: fusion::FusionTuning,
    ) -> Vec<fusion::PanelAnswer> {
        use futures_util::stream::{FuturesUnordered, StreamExt};

        let mut calls: FuturesUnordered<_> = panel
            .iter()
            .map(|target| async move {
                let text = self.panel_text(context, target, panel_body).await;
                (Self::target_label(target), text)
            })
            .collect();

        let mut answers: Vec<fusion::PanelAnswer> = Vec::with_capacity(panel.len());
        let hard_deadline = tokio::time::sleep(std::time::Duration::from_millis(
            tuning.panel_hard_timeout_ms,
        ));
        tokio::pin!(hard_deadline);
        // Only armed once quorum is reached; before that there is nothing to be
        // late for.
        let mut grace: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;

        loop {
            tokio::select! {
                biased;
                () = &mut hard_deadline => break,
                () = async {
                    match grace.as_mut() {
                        Some(timer) => timer.await,
                        // No grace armed yet: never completes, so the other
                        // branches drive the loop.
                        None => std::future::pending().await,
                    }
                } => break,
                next = calls.next() => {
                    let Some((model, text)) = next else {
                        // Every model has reported.
                        break;
                    };
                    if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
                        answers.push(fusion::PanelAnswer { model, text });
                        if answers.len() >= quorum && grace.is_none() {
                            grace = Some(Box::pin(tokio::time::sleep(
                                std::time::Duration::from_millis(tuning.straggler_grace_ms),
                            )));
                        }
                    }
                }
            }
        }
        answers
    }

    /// Execute one panel call and return its assistant text.
    ///
    /// `None` when the model did not answer usefully — no credentials, an upstream
    /// refusal, an unreadable body. A panel model that fails is dropped rather than
    /// failing the request: that is what the other panel models are for.
    async fn panel_text(
        &self,
        context: &ChatContext<'_>,
        target: &model::ModelTarget,
        panel_body: &Value,
    ) -> Option<String> {
        if !is_executor_supported(&target.provider) {
            return None;
        }
        let selection = self
            .state
            .select_credentials(&target.provider, Some(&target.model), &[])
            .await;
        let Selection::Selected(credentials) = selection else {
            return None;
        };
        let credentials = *credentials;

        let target_format = target_format(&target.provider);
        let upstream_model = model::upstream_model_id(&target.provider, &target.model);
        let ceiling = nullrouter_providers::max_output(&target.provider, &target.model);
        // Panels are always non-streaming: the judge needs the whole answer, and a
        // provider that only streams has its stream collapsed below.
        let provider_forces_stream =
            nullrouter_execute::credentials::forces_stream(&target.provider);
        let translated = translate_request(
            RequestRoute {
                source: context.source_format,
                target: target_format,
                provider: &target.provider,
                model: &upstream_model,
            },
            panel_body,
            provider_forces_stream,
            ceiling,
        );

        let started = Instant::now();
        let outcome = self
            .executor
            .execute(ExecuteRequest {
                provider: &target.provider,
                body: &translated.body,
                stream: provider_forces_stream,
                credentials: &credentials,
            })
            .await
            .ok()?;
        if !outcome.is_success() {
            return None;
        }

        let mut state = StreamState::new(Clock::System);
        state.tool_name_map = translated.tool_name_map;
        let body = if outcome.is_event_stream() {
            collapse_stream_to_json(outcome.response, target_format, &upstream_model, &mut state)
                .await
        } else {
            outcome.response.json::<Value>().await.ok()?
        };

        // A panel call is a real provider request and is recorded as one, or the
        // usage page would under-report a fusion combo by the size of its panel.
        let usage = state
            .usage
            .or_else(|| usage_from_body(&body))
            .unwrap_or_default();
        self.record(
            context,
            target,
            Some(&credentials),
            "success",
            Some(200),
            usage,
            started,
            None,
        );

        let text = fusion::extract_panel_text(&body);
        (!text.trim().is_empty()).then_some(text)
    }

    /// A stable label for a target, used to match an answer back to its model.
    fn target_label(target: &model::ModelTarget) -> String {
        format!("{}/{}", target.provider, target.model)
    }

    /// Run one resolved target through account selection and execution.
    async fn execute_for_target(
        &self,
        context: &ChatContext<'_>,
        target: &model::ModelTarget,
    ) -> TargetOutcome {
        // A provider whose protocol needs a bespoke executor is refused with an
        // explicit message rather than a wrong answer. Inside a combo it is worth
        // stepping past — another model may well be executable — but the 501 is
        // kept as the response in case this is the last one.
        if !is_executor_supported(&target.provider) {
            let message = unsupported_executor_message(&target.provider);
            return TargetOutcome::Failed {
                response: self
                    .fail(context, target, StatusCode::NOT_IMPLEMENTED, &message)
                    .await,
            };
        }

        let mut excluded: Vec<String> = Vec::new();
        let mut last_error: Option<(u16, String)> = None;

        for _ in 0..MAX_ACCOUNT_ATTEMPTS {
            let selection = self
                .state
                .select_credentials(&target.provider, Some(&target.model), &excluded)
                .await;

            let credentials = match selection {
                Selection::Selected(credentials) => *credentials,
                // No credentials for this provider at all. Inside a combo that is
                // exactly what the next model is for.
                Selection::NoCredentials { message } => {
                    return TargetOutcome::Failed {
                        response: self
                            .fail(context, target, StatusCode::NOT_FOUND, &message)
                            .await,
                    };
                }
                Selection::AllRateLimited {
                    retry_at_ms,
                    last_error: reported,
                    last_error_code,
                } => {
                    return TargetOutcome::Failed {
                        response: self
                            .rate_limited(
                                context,
                                target,
                                retry_at_ms,
                                last_error
                                    .as_ref()
                                    .map(|(_, message)| message.clone())
                                    .or(reported),
                                last_error
                                    .as_ref()
                                    .map(|(status, _)| *status)
                                    .or(last_error_code),
                            )
                            .await,
                    };
                }
                Selection::Exhausted => break,
                Selection::Unavailable { message } => {
                    return TargetOutcome::Failed {
                        response: self
                            .fail(context, target, StatusCode::SERVICE_UNAVAILABLE, &message)
                            .await,
                    };
                }
            };

            match self.attempt(context, target, &credentials).await {
                Attempt::Responded(response) => return TargetOutcome::Responded(response),
                Attempt::Retryable {
                    status,
                    message,
                    cooldown_ms,
                    backoff_level,
                } => {
                    self.state
                        .mark_unavailable(&crate::state_client::Cooldown {
                            connection_id: &credentials.connection_id,
                            model: Some(&target.model),
                            status,
                            reason: &message,
                            duration_ms: cooldown_ms,
                            backoff_level,
                        })
                        .await;
                    excluded.push(credentials.connection_id.clone());
                    last_error = Some((status, message));
                }
            }
        }

        // Every account for this model failed; report the last real upstream
        // error. Inside a combo the next model is tried first, and this response
        // is only shown if there is none.
        let (status, message) =
            last_error.unwrap_or_else(|| (503, "All provider accounts are unavailable".to_owned()));
        let status = StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
        TargetOutcome::Failed {
            response: self.fail(context, target, status, &message).await,
        }
    }

    /// Build the OpenAI-compatible model list for the requested service kinds.
    ///
    /// Backed by the registry, the caller's configured connections, and — for a compatible
    /// provider with no configured model list — the provider's own `/models`, so `/v1/models`
    /// reflects what is actually reachable.
    ///
    /// `probe: false` is for a request carrying another router's probe header: answering from
    /// configuration alone is what stops two routers pointed at each other from probing back
    /// and forth on every call.
    pub(crate) async fn models_list_with(&self, kinds: &[&str], probe: bool) -> Value {
        let context = self.state.routing_context().await;
        let probed = if probe {
            self.probed_models(&context).await
        } else {
            std::collections::BTreeMap::new()
        };
        let input = nullrouter_providers::ModelsListInput {
            connections: context
                .connections
                .iter()
                .map(|connection| nullrouter_providers::ConnectionView {
                    provider: connection.provider.clone(),
                    prefix: connection.prefix.clone(),
                    // A configured list is the owner's own choice and wins. Probing only
                    // fills the gap where there is nothing to show: a compatible node has
                    // no registry models, so without this the route reports the connection
                    // and none of its models.
                    enabled_models: if connection.enabled_models.is_empty() {
                        probed
                            .get(&connection.provider)
                            .cloned()
                            .unwrap_or_default()
                    } else {
                        connection.enabled_models.clone()
                    },
                })
                .collect(),
            combos: context
                .combos
                .iter()
                .map(|combo| nullrouter_providers::ComboView {
                    name: combo.name.clone(),
                    kind: combo.kind.clone(),
                })
                .collect(),
            ..nullrouter_providers::ModelsListInput::default()
        };
        let rows = nullrouter_providers::build_models_list(&input, kinds);
        serde_json::json!({ "object": "list", "data": rows })
    }

    /// Model ids per provider, asked of the providers themselves.
    ///
    /// Only for connections the routing context shows with no configured model list, and
    /// only for the compatible families — a registry provider's models are in the
    /// registry, so probing one would be asking a question already answered.
    ///
    /// Probes run concurrently: they are independent network calls, and doing them in
    /// turn would make `/v1/models` as slow as the sum of every provider's latency.
    ///
    /// **A failed probe contributes nothing rather than an empty list.** The caller
    /// keeps whatever was configured, so a provider that is briefly slow does not empty
    /// a working model picker.
    async fn probed_models(
        &self,
        context: &crate::state_client::RoutingContext,
    ) -> std::collections::BTreeMap<String, Vec<String>> {
        // Only the first connection per provider decides, because only the first
        // contributes rows to `build_models_list`. Asking "does *any* connection for this
        // provider lack a list" would probe on behalf of a connection whose models the
        // route then ignores — a real provider call for nothing.
        let mut first_seen = std::collections::BTreeSet::new();
        let needs_probe: std::collections::BTreeSet<&str> = context
            .connections
            .iter()
            .filter(|connection| first_seen.insert(connection.provider.as_str()))
            .filter(|connection| connection.enabled_models.is_empty())
            .map(|connection| connection.provider.as_str())
            .collect();
        if needs_probe.is_empty() {
            return std::collections::BTreeMap::new();
        }

        // One target per provider, the first, matching `build_models_list`'s own
        // first-connection-per-provider rule. A compatible node can hold several
        // connections as a key pool; probing each would make N provider calls to answer a
        // question about one host, and collecting the results by provider would then keep
        // whichever finished last.
        let mut seen = std::collections::BTreeSet::new();
        let targets: Vec<_> = self
            .state
            .probe_targets()
            .await
            .into_iter()
            .filter(|target| needs_probe.contains(target.provider.as_str()))
            .filter(|target| seen.insert(target.provider.clone()))
            .collect();
        if targets.is_empty() {
            return std::collections::BTreeMap::new();
        }

        let futures = targets.into_iter().map(|target| async move {
            let models = match self.probes.get(&target.connection_id) {
                Some(cached) => cached,
                None => {
                    let fresh = self
                        .executor
                        .probe_models(
                            &target.provider,
                            &target.credentials,
                            nullrouter_execute::probe::DEFAULT_TIMEOUT,
                        )
                        .await;
                    self.probes.put(&target.connection_id, &fresh);
                    fresh
                }
            };
            match models {
                Ok(models) => Some((
                    target.provider,
                    models.into_iter().map(|model| model.id).collect::<Vec<_>>(),
                )),
                Err(error) => {
                    // Logged at info, not warn: a user-added provider that rejects a key
                    // or times out is the owner's configuration to fix, and this route is
                    // polled often enough that warn would drown the log.
                    tracing::info!(
                        provider = %target.provider,
                        "model probe failed: {}", error.describe()
                    );
                    None
                }
            }
        });

        futures_util::future::join_all(futures)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// Execute a non-chat provider call (embeddings, audio, images, search,
    /// fetch).
    ///
    /// These have no translation matrix: the client's body is forwarded as-is
    /// to the provider's service-specific endpoint.
    #[allow(
        clippy::too_many_lines,
        reason = "one linear non-chat dispatch: resolve, select, execute, record"
    )]
    pub(crate) async fn execute_passthrough(&self, context: ChatContext<'_>) -> HttpResponse {
        // On these endpoints a bare token names a provider, not a model alias:
        // `/v1/search` takes `provider: "tavily"`. Running it through model
        // inference would misroute it to the default chat provider.
        let Some(target) = self.resolve_service_target(&context.requested_model).await else {
            return responses::json(
                StatusCode::BAD_REQUEST,
                &build_error_body(400, "Invalid model format"),
            );
        };

        let Some(kind) = ServiceKind::from_path(context.endpoint) else {
            return responses::json(
                StatusCode::NOT_FOUND,
                &build_error_body(404, "Unknown service endpoint"),
            );
        };

        let Some(endpoint) = nullrouter_providers::service_endpoint(&target.provider, kind)
            .and_then(|endpoint| endpoint.base_url.as_deref())
        else {
            let message = format!(
                "Provider '{}' does not expose a {} endpoint",
                target.provider,
                service_label(kind)
            );
            return self
                .fail(&context, &target, StatusCode::NOT_IMPLEMENTED, &message)
                .await;
        };

        let started = Instant::now();
        let selection = self
            .state
            .select_credentials(&target.provider, Some(&target.model), &[])
            .await;
        let credentials = match selection {
            Selection::Selected(credentials) => *credentials,
            Selection::NoCredentials { message } => {
                return self
                    .fail(&context, &target, StatusCode::NOT_FOUND, &message)
                    .await;
            }
            Selection::AllRateLimited {
                retry_at_ms,
                last_error,
                last_error_code,
            } => {
                return self
                    .rate_limited(&context, &target, retry_at_ms, last_error, last_error_code)
                    .await;
            }
            Selection::Exhausted => {
                return self
                    .fail(
                        &context,
                        &target,
                        StatusCode::SERVICE_UNAVAILABLE,
                        "All provider accounts are unavailable",
                    )
                    .await;
            }
            Selection::Unavailable { message } => {
                return self
                    .fail(&context, &target, StatusCode::SERVICE_UNAVAILABLE, &message)
                    .await;
            }
        };

        // The upstream model id replaces any alias the client used.
        let mut body = context.body.clone();
        if let Some(object) = body.as_object_mut()
            && object.contains_key("model")
        {
            let upstream = model::upstream_model_id(&target.provider, &target.model);
            object.insert("model".to_owned(), Value::String(upstream));
        }

        match self
            .executor
            .execute_at(
                endpoint,
                ExecuteRequest {
                    provider: &target.provider,
                    body: &body,
                    stream: false,
                    credentials: &credentials,
                },
            )
            .await
        {
            Ok(outcome) => {
                // reqwest and actix-web resolve different `http` versions, so
                // statuses cross the boundary as u16.
                let upstream_status = outcome.status().as_u16();
                let succeeded = outcome.is_success();
                let payload = outcome
                    .response
                    .json::<Value>()
                    .await
                    .unwrap_or(Value::Null);

                self.record(
                    &context,
                    &target,
                    Some(&credentials),
                    if succeeded { "success" } else { "error" },
                    Some(upstream_status),
                    nullrouter_translate::Usage::default(),
                    started,
                    (!succeeded).then(|| format!("upstream returned {upstream_status}")),
                );

                if payload.is_null() {
                    return responses::json(
                        StatusCode::BAD_GATEWAY,
                        &build_error_body(502, "upstream returned an unreadable body"),
                    );
                }
                let status =
                    StatusCode::from_u16(upstream_status).unwrap_or(StatusCode::BAD_GATEWAY);
                responses::json(status, &payload)
            }
            Err(error) => {
                let status =
                    StatusCode::from_u16(error.client_status()).unwrap_or(StatusCode::BAD_GATEWAY);
                let message = error.to_string();
                self.fail(&context, &target, status, &message).await
            }
        }
    }

    /// Resolve the routing target for a non-chat service endpoint.
    ///
    /// A `provider/model` string parses normally; a bare token that the
    /// registry recognizes is treated as a provider id (upstream's
    /// `required_provider_or_model` semantics), and anything else falls back to
    /// the chat resolution path.
    /// A non-chat service takes the first resolved target only. These endpoints
    /// dispatch one call to one provider; there is no fallback chain to walk, so a
    /// combo naming several models contributes only its first.
    async fn resolve_service_target(&self, requested: &str) -> Option<model::ModelTarget> {
        if requested.contains('/') {
            return self
                .resolve_targets(requested)
                .await
                .targets
                .into_iter()
                .next();
        }
        let canonical = nullrouter_providers::resolve_provider_id(requested);
        if nullrouter_providers::entry(canonical).is_some() {
            return Some(model::ModelTarget {
                provider: canonical.to_owned(),
                // No specific model: the service config supplies the default.
                model: String::new(),
            });
        }
        self.resolve_targets(requested)
            .await
            .targets
            .into_iter()
            .next()
    }

    /// Resolve a client model string to the ordered list of targets to try.
    ///
    /// A plain `provider/model` yields one target. A combo name yields one per
    /// configured model, in the order its strategy chose — so every model in the
    /// combo is a fallback for the ones before it. The strategy comes back too,
    /// because `fusion` asks all of them at once rather than in turn.
    async fn resolve_targets(&self, requested: &str) -> ResolvedTargets {
        let parsed = model::parse_model(requested);
        if !parsed.is_alias {
            let mut targets: Vec<model::ModelTarget> = parsed
                .provider
                .map(|provider| model::ModelTarget {
                    provider,
                    model: parsed.model,
                })
                .into_iter()
                .collect();
            if let Some(target) = targets.first_mut()
                && let Some(alias) = parsed.provider_alias.as_deref()
                && let Some(provider) = self.resolve_node_prefix(alias).await
            {
                target.provider = provider;
            }
            return ResolvedTargets {
                targets,
                // A single model is not a combo, so no strategy applies.
                strategy: ComboStrategy::Fallback,
                tuning: fusion::FusionTuning::default(),
            };
        }

        let context = self.state.routing_context().await;
        if let Some(combo) = context
            .combos
            .iter()
            .find(|combo| combo.name == parsed.model)
            && !combo.models.is_empty()
        {
            // A per-combo override governs that combo alone; the global setting is the
            // default for every combo without one. Upstream's dashboard deletes an
            // entry when a combo returns to `fallback`, so an absent entry and one
            // naming the default are the same thing — which falls out of reading the
            // override first and the global second.
            let override_entry = context.settings.combo_strategies.get(&combo.name);
            let strategy = ComboStrategy::from_settings(
                override_entry
                    .and_then(|entry| entry.fallback_strategy.as_deref())
                    .or(context.settings.combo_strategy.as_deref()),
            );
            let sticky = context
                .settings
                .combo_sticky_round_robin_limit
                .unwrap_or(DEFAULT_COMBO_STICKY_LIMIT);
            let tuning = override_entry.map_or_else(fusion::FusionTuning::default, |entry| {
                fusion::FusionTuning::from_override(
                    entry.min_panel,
                    entry.straggler_grace_ms,
                    entry.panel_hard_timeout_ms,
                )
            });
            let ordered =
                combo::ordered_models(&combo.models, &combo.name, strategy, sticky, &self.rotation);
            let targets: Vec<model::ModelTarget> = ordered
                .iter()
                .map(|entry| {
                    let resolved = model::parse_model(entry);
                    resolved.provider.map_or_else(
                        || model::infer_target(&resolved.model),
                        |provider| model::ModelTarget {
                            provider,
                            model: resolved.model.clone(),
                        },
                    )
                })
                .collect();
            if !targets.is_empty() {
                return ResolvedTargets {
                    targets,
                    strategy,
                    tuning,
                };
            }
        }

        // Fall back to prefix inference, as upstream does for bare aliases.
        ResolvedTargets {
            targets: vec![model::infer_target(&parsed.model)],
            strategy: ComboStrategy::Fallback,
            tuning: fusion::FusionTuning::default(),
        }
    }

    /// Resolve a client model string for the translator inspector.
    ///
    /// Deliberately the *same* `resolve_targets` the live path uses, rather than a lookup of
    /// its own: an inspector that resolved models differently from the request path would
    /// show a translation nobody is actually performing, which is worse than showing nothing.
    pub(crate) async fn inspector_target(&self, requested: &str) -> Option<model::ModelTarget> {
        self.resolve_targets(requested)
            .await
            .targets
            .into_iter()
            .next()
    }

    /// The outbound URL and headers a dispatch would use, with credentials redacted.
    ///
    /// `None` when no active connection exists, which the inspector reports as such — that is
    /// itself the answer to "why is my request not going anywhere".
    pub(crate) async fn inspector_wire(
        &self,
        provider: &str,
        model: &str,
        dispatch: Format,
    ) -> Option<crate::inspector::InspectorWire> {
        let selection = self
            .state
            .select_credentials(provider, Some(model), &[])
            .await;
        let Selection::Selected(credentials) = selection else {
            return None;
        };
        let mut credentials = *credentials;
        // The inspector is asked about a specific dispatch format, which is what decides the
        // transport for a multi-transport provider — and therefore the URL and the auth
        // header. Setting it here makes the panes agree with what the live path would send.
        credentials.runtime_format = Some(dispatch);

        let url = nullrouter_execute::credentials::build_url(provider, &credentials, 0)?;
        let headers = nullrouter_execute::credentials::build_headers(provider, &credentials, false)
            .into_iter()
            .map(|(name, value)| {
                let shown = if crate::inspector::is_secret_header(&name) {
                    crate::inspector::redact(&value)
                } else {
                    value
                };
                (name, shown)
            })
            .collect();
        Some(crate::inspector::InspectorWire { url, headers })
    }

    /// Resolve a user-defined provider-node prefix to the connection's provider id
    /// (upstream `getModelInfo`).
    ///
    /// A compatible provider is addressed by the prefix its owner chose, not by the
    /// opaque node id: `myllm/some-model`, where the connection's provider is
    /// `openai-compatible-chat-<uuid>`. Without this a migrated install can only be
    /// reached by that uuid, so every client config that worked against 9Router breaks.
    ///
    /// Returns `None` for a registry provider, and that check comes first for a reason:
    /// `routing_context` is an HTTP hop to the state service, and `provider/model` is the
    /// common path. Consulting connections unconditionally would put a round trip on every
    /// request. It also keeps a user's prefix from shadowing a built-in id or alias, which
    /// is upstream's `RESERVED_PROVIDER_PREFIXES` guard.
    async fn resolve_node_prefix(&self, alias: &str) -> Option<String> {
        if nullrouter_providers::registry::entry(alias).is_some() {
            return None;
        }
        // `parse_model` already mapped registry aliases, so a name that still resolves to
        // a known provider is reserved too.
        if nullrouter_providers::registry::entry(
            nullrouter_providers::registry::resolve_provider_id(alias),
        )
        .is_some()
        {
            return None;
        }
        let context = self.state.routing_context().await;
        context
            .connections
            .iter()
            .find(|connection| connection.prefix.as_deref() == Some(alias))
            .map(|connection| connection.provider.clone())
    }

    /// One account attempt.
    async fn attempt(
        &self,
        context: &ChatContext<'_>,
        target: &model::ModelTarget,
        credentials: &Credentials,
    ) -> Attempt {
        // An access token near its expiry is exchanged before the call rather than
        // after a 401: the refresh token was stored and never used, so an OAuth
        // connection worked until its token expired and then failed until the user
        // re-authorised by hand.
        let refreshed = self.refresh_if_due(&target.provider, credentials).await;
        let credentials = refreshed.as_ref().unwrap_or(credentials);
        let started = Instant::now();
        let upstream_model = model::upstream_model_id(&target.provider, &target.model);

        // A provider fronting several endpoints on one host is addressed in the
        // client's own format where it can be, which removes the translation hop
        // entirely: deepseek answers Claude requests at `/anthropic/v1/messages`, so a
        // Claude client should not have its body rewritten to OpenAI and back.
        //
        // Gated per model, because `opencode-go` fronts several vendors and its
        // kimi/glm models serve `/chat/completions` only — routing a Claude request
        // there to `/messages` would 404 a provider that works.
        let direct = nullrouter_providers::runtime_transport(
            &target.provider,
            &target.model,
            context.source_format,
        );
        let target_format = if direct.is_some() {
            context.source_format
        } else {
            target_format(&target.provider)
        };
        // Carried on the credentials, which is where the URL and header builders
        // already look. `attempt` owns a clone for the duration of the call.
        let mut credentials = credentials.clone();
        credentials.runtime_format = direct.map(|_| context.source_format);
        let credentials = &credentials;
        // A provider that only streams is called with stream=true regardless of
        // what the client asked for; the response is collapsed if needed.
        let provider_forces_stream =
            nullrouter_execute::credentials::forces_stream(&target.provider);
        let upstream_stream = context.stream || provider_forces_stream;

        // The model's real output ceiling, not the conservative 64000 default:
        // clamping a 128k-output model to the default would silently truncate
        // long completions.
        let ceiling = nullrouter_providers::max_output(&target.provider, &target.model);

        let translated = translate_request(
            RequestRoute {
                source: context.source_format,
                target: target_format,
                provider: &target.provider,
                model: &upstream_model,
            },
            &context.body,
            upstream_stream,
            ceiling,
        );

        // PXPIPE, the last saver before dispatch: bulky Claude-format context is
        // rendered to dense images, which bill by pixel rather than by token.
        //
        // Applied to the *translated* body, after every other reshaping, because the
        // package rewrites Anthropic content blocks and only the translated body is
        // in that shape. It fails open in every case — see `compress_body`.
        let dispatch_body = self
            .compress_body(context, target_format, &upstream_model, &translated.body)
            .await;
        let dispatch_body = dispatch_body.as_ref().unwrap_or(&translated.body);

        let outcome = self
            .executor
            .execute(ExecuteRequest {
                provider: &target.provider,
                body: dispatch_body,
                stream: upstream_stream,
                credentials,
            })
            .await;

        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                // Transport failures are retryable against another account.
                let status = error.client_status();
                let message = error.to_string();
                let decision = check_fallback_error(status, &message, 0);
                return Attempt::Retryable {
                    status,
                    message,
                    cooldown_ms: decision.cooldown_ms,
                    backoff_level: decision.new_backoff_level,
                };
            }
        };

        if !outcome.is_success() {
            let status = outcome.status().as_u16();
            let body = outcome.response.text().await.unwrap_or_default();
            let parsed = parse_upstream_error(status, &body);
            let decision = check_fallback_error(status, &parsed.message, 0);

            self.record(
                context,
                target,
                Some(credentials),
                "error",
                Some(status),
                nullrouter_translate::Usage::default(),
                started,
                Some(parsed.message.clone()),
            );

            if decision.should_fallback {
                return Attempt::Retryable {
                    status,
                    message: parsed.message,
                    cooldown_ms: decision.cooldown_ms,
                    backoff_level: decision.new_backoff_level,
                };
            }

            let status_code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
            let message = format_provider_error(status, &parsed.message);
            return Attempt::Responded(Self::error_response(context, status_code, &message));
        }

        // Success: clear any prior cooldown on this account.
        self.state
            .clear_error(&credentials.connection_id, Some(&target.model))
            .await;

        let mut state = StreamState::new(Clock::System);
        state.tool_name_map = translated.tool_name_map;

        if context.stream {
            // Not awaited: the response returns immediately and frames flow as
            // the provider produces them.
            self.stream_response(
                context,
                target,
                credentials,
                outcome,
                target_format,
                state,
                started,
            )
        } else {
            self.json_response(
                context,
                target,
                credentials,
                outcome,
                target_format,
                state,
                started,
            )
            .await
        }
    }

    /// Stream the upstream response to the client, translating as it arrives.
    ///
    /// Frames are forwarded the moment they are translated, so time-to-first-token
    /// tracks the provider's own latency instead of the full completion time, and
    /// memory stays bounded by the channel rather than the response length.
    ///
    /// The pipe runs in a detached task on the current worker thread — the
    /// translation state and upstream body are `!Send`, and actix's runtime is
    /// per-worker single-threaded, so no `Send` bound is needed. Usage is
    /// recorded inside that task once the stream drains.
    #[allow(
        clippy::too_many_arguments,
        reason = "threads the request, routing target, credentials, and stream state into one response; a wrapper struct would be used once"
    )]
    fn stream_response(
        &self,
        context: &ChatContext<'_>,
        target: &model::ModelTarget,
        credentials: &Credentials,
        outcome: nullrouter_execute::ExecuteOutcome,
        target_format: Format,
        mut state: StreamState,
        started: Instant,
    ) -> Attempt {
        // Bounded so a slow client applies backpressure to the provider read
        // rather than letting frames accumulate without limit.
        let (sender, receiver) = tokio::sync::mpsc::channel::<String>(STREAM_CHANNEL_FRAMES);

        let source_format = context.source_format;
        let endpoint = context.endpoint.to_owned();
        let state_client = self.state.clone();
        let usage_target = target.clone();
        let connection_id = credentials.connection_id.clone();
        // The body as it was sent, needed to key a provider-side conversation once the stream ends.
        let thread_provider = target.provider.clone();
        let thread_body = outcome.sent_body.clone();

        actix_web::rt::spawn(async move {
            let summary = pipe_stream(
                outcome.response,
                target_format,
                source_format,
                &mut state,
                ChannelSink { sender },
            )
            .await;

            // A provider that keeps the conversation itself — perplexity — returns a thread id. Storing
            // it against this exchange is what lets the next request continue server-side instead of
            // resending the whole history.
            if let (Some(thread), Some(answer)) = (
                summary.upstream_thread.as_deref(),
                summary.upstream_answer.as_deref(),
            ) {
                nullrouter_execute::bespoke::remember_thread(
                    &thread_provider,
                    &thread_body,
                    thread,
                    answer,
                );
            }

            let usage = summary.usage.unwrap_or_default();
            let report = UsageReport {
                provider: usage_target.provider,
                model: usage_target.model,
                connection_id: Some(connection_id),
                endpoint: Some(endpoint),
                status: "success".to_owned(),
                status_code: Some(200),
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                cached_tokens: usage.cached_tokens,
                latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                error: None,
            };
            // Same bound as the non-streaming path. This task exists regardless — it is the one
            // piping the stream — so the risk here is not task growth but each finished stream
            // lingering for the five-second state timeout while state is down. Skipping the
            // best-effort row keeps those tasks retiring promptly.
            if let Some(permit) = state_client.try_reserve_usage_slot() {
                state_client.record_usage(&report).await;
                drop(permit);
            } else {
                tracing::warn!(
                    provider = %report.provider,
                    "usage recording saturated; dropping this streamed record"
                );
            }
        });

        // Frames already sitting in the channel are concatenated into one chunk rather than
        // written one at a time. A 2000-frame response used to mean 2000 chunked-encoding writes;
        // when the translator runs ahead of the socket — which it does, since translating a frame
        // is faster than sending one — most of those writes were carrying a single ~40-byte frame.
        //
        // This adds no latency: `recv_many` returns as soon as anything is available and only
        // takes what is already queued, so a slow producer still yields one frame per chunk. SSE
        // framing is unaffected because the boundaries are the `\n\n` inside the payload, not the
        // transport chunks — a client cannot distinguish this from the previous behaviour except
        // by timing.
        let body = futures_util::stream::unfold(
            (receiver, Vec::with_capacity(STREAM_CHANNEL_FRAMES)),
            |(mut receiver, mut batch)| async move {
                batch.clear();
                if receiver.recv_many(&mut batch, STREAM_CHANNEL_FRAMES).await == 0 {
                    // Producer finished and the channel is drained.
                    return None;
                }
                Some((Ok(coalesce_frames(&mut batch)), (receiver, batch)))
            },
        );

        Attempt::Responded(responses::sse_stream(StatusCode::OK, body))
    }

    /// Return a single JSON body, collapsing the stream when upstream forced one.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors stream_response's context threading"
    )]
    async fn json_response(
        &self,
        context: &ChatContext<'_>,
        target: &model::ModelTarget,
        credentials: &Credentials,
        outcome: nullrouter_execute::ExecuteOutcome,
        target_format: Format,
        mut state: StreamState,
        started: Instant,
    ) -> Attempt {
        let upstream_model = model::upstream_model_id(&target.provider, &target.model);

        let body = if outcome.is_event_stream() {
            // A collapsed stream is already in the client's format: every frame went
            // through the streaming translator on the way.
            collapse_stream_to_json(outcome.response, target_format, &upstream_model, &mut state)
                .await
        } else {
            // A genuinely non-streaming provider response, in the provider's own
            // shape. It has no frames, so the whole body is translated here —
            // without this an OpenAI client asking a Claude provider for
            // `stream: false` received `content[]` where it expects `choices[]`,
            // which reads as an empty completion rather than an error.
            let raw = match outcome.response.json::<Value>().await {
                Ok(body) => body,
                Err(error) => {
                    let message = format!("upstream returned an unreadable body: {error}");
                    return Attempt::Responded(Self::error_response(
                        context,
                        StatusCode::BAD_GATEWAY,
                        &message,
                    ));
                }
            };
            nullrouter_translate::translate_body(target_format, context.source_format, &raw, &state)
        };

        // A collapsed stream populates `state.usage` as it translates, but a
        // genuinely non-streaming reply is parsed straight to JSON and never
        // touches the translator — so its `usage` object has to be read from the
        // body here. Without this, every non-streaming request recorded zero
        // tokens while still counting as a request.
        let usage = state
            .usage
            .or_else(|| usage_from_body(&body))
            .unwrap_or_default();
        self.record(
            context,
            target,
            Some(credentials),
            "success",
            Some(200),
            usage,
            started,
            None,
        );

        Attempt::Responded(responses::json(StatusCode::OK, &body))
    }

    /// Refresh this connection's access token when it is due.
    ///
    /// Returns the updated credentials, or `None` when nothing was refreshed —
    /// which is the common case, and also what happens on a transient failure: the
    /// existing token may still work, and refusing the request because a token
    /// endpoint was briefly unreachable would be worse than trying it.
    async fn refresh_if_due(
        &self,
        provider: &str,
        credentials: &Credentials,
    ) -> Option<Credentials> {
        if !nullrouter_execute::refresh::should_refresh(
            provider,
            credentials,
            nullrouter_execute::refresh::now_millis(),
        ) {
            return None;
        }

        match self
            .executor
            .refresh_credentials(provider, credentials, &self.refreshes)
            .await
        {
            Ok(refreshed) => {
                let payload = nullrouter_execute::refresh::persist_body(
                    &credentials.connection_id,
                    &refreshed,
                );
                // Persisted before use so a restart does not lose the rotation: the
                // old refresh token may already be invalid upstream.
                if !self.state.store_refreshed(&payload).await {
                    tracing::warn!(
                        provider,
                        connection_id = %credentials.connection_id,
                        "refreshed token could not be persisted; using it for this request only"
                    );
                }
                let mut updated = credentials.clone();
                updated.access_token = Some(refreshed.access_token);
                updated.refresh_token = Some(refreshed.refresh_token);
                updated.expires_at = refreshed.expires_at;
                Some(updated)
            }
            Err(error) => {
                if error.is_permanent() {
                    // The user has to re-authorise. Recorded on the connection so
                    // the dashboard says so instead of showing a generic failure.
                    let message = format!("OAuth refresh failed: {error:?}");
                    self.state
                        .mark_unavailable(&crate::state_client::Cooldown {
                            connection_id: &credentials.connection_id,
                            model: None,
                            status: 401,
                            reason: &message,
                            duration_ms: REAUTH_COOLDOWN_MS,
                            backoff_level: None,
                        })
                        .await;
                } else {
                    tracing::warn!(
                        provider,
                        ?error,
                        "token refresh failed; trying the existing token"
                    );
                }
                None
            }
        }
    }

    /// Report a terminal failure, recording it as usage first.
    async fn fail(
        &self,
        context: &ChatContext<'_>,
        target: &model::ModelTarget,
        status: StatusCode,
        message: &str,
    ) -> HttpResponse {
        self.record(
            context,
            target,
            None,
            "error",
            Some(status.as_u16()),
            nullrouter_translate::Usage::default(),
            Instant::now(),
            Some(message.to_owned()),
        );
        Self::error_response(context, status, message)
    }

    /// Report that every account is cooling down, with a retry hint.
    async fn rate_limited(
        &self,
        context: &ChatContext<'_>,
        target: &model::ModelTarget,
        retry_at_ms: u64,
        last_error: Option<String>,
        last_error_code: Option<u16>,
    ) -> HttpResponse {
        let now_ms = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis())
                .unwrap_or_default(),
        )
        .unwrap_or(0);
        let remaining_ms = i64::try_from(retry_at_ms.saturating_sub(now_ms)).unwrap_or(0);
        let hint = nullrouter_execute::errors::format_retry_after(remaining_ms);
        let base = last_error.unwrap_or_else(|| "Unavailable".to_owned());
        let message = format!("[{}/{}] {base} ({hint})", target.provider, target.model);
        let status = last_error_code
            .and_then(|code| StatusCode::from_u16(code).ok())
            .unwrap_or(StatusCode::SERVICE_UNAVAILABLE);

        self.record(
            context,
            target,
            None,
            "error",
            Some(status.as_u16()),
            nullrouter_translate::Usage::default(),
            Instant::now(),
            Some(message.clone()),
        );

        let retry_after_seconds = remaining_ms.div_euclid(1000).max(1);
        let mut response = Self::error_response(context, status, &message);
        if let Ok(value) = retry_after_seconds.to_string().parse() {
            response
                .headers_mut()
                .insert(actix_web::http::header::RETRY_AFTER, value);
        }
        response
    }

    /// Render an error in the client's expected shape and framing.
    fn error_response(
        context: &ChatContext<'_>,
        status: StatusCode,
        message: &str,
    ) -> HttpResponse {
        if context.stream {
            // A streaming client gets the error as frames, not a bare body.
            let body = nullrouter_execute::stream::error_stream_body(
                status.as_u16(),
                message,
                context.source_format,
            );
            return responses::sse_body(status, body);
        }
        responses::json(status, &build_error_body(status.as_u16(), message))
    }

    /// Send a usage record to state. Best-effort.
    #[allow(
        clippy::too_many_arguments,
        reason = "one usage row's worth of fields, assembled at several call sites"
    )]
    /// Not `async`: the usage POST is spawned, so there is nothing here to await. Keeping the
    /// signature async would make every caller's `.await` a no-op that reads as if it waited.
    fn record(
        &self,
        context: &ChatContext<'_>,
        target: &model::ModelTarget,
        credentials: Option<&Credentials>,
        status: &str,
        status_code: Option<u16>,
        usage: nullrouter_translate::Usage,
        started: Instant,
        error: Option<String>,
    ) {
        let report = UsageReport {
            provider: target.provider.clone(),
            model: target.model.clone(),
            connection_id: credentials.map(|credentials| credentials.connection_id.clone()),
            endpoint: Some(context.endpoint.to_owned()),
            status: status.to_owned(),
            status_code,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            cached_tokens: usage.cached_tokens,
            latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            error,
        };
        // Spawned rather than awaited. The caller is holding a finished response, and the client
        // was waiting on this round trip — ~1.7ms of the router's overhead spent after the answer
        // already existed. Usage is best-effort by construction (`record_usage` logs and moves on),
        // so nothing downstream reads its result.
        //
        // On the runtime's own runtime, so it is cancelled if the service stops; a record lost to
        // shutdown is the same outcome the previous `await` had if state was unreachable.
        // Bounded: each spawned POST holds a permit until it completes or hits the five-second state
        // timeout, so a state outage under load cannot accumulate one task and one retained report
        // per request without limit. Dropping a best-effort telemetry row beats exhausting the
        // runtime that is still serving traffic.
        let Some(permit) = self.state.try_reserve_usage_slot() else {
            tracing::warn!(
                provider = %target.provider,
                "usage recording saturated; dropping this record"
            );
            return;
        };
        let state = self.state.clone();
        actix_web::rt::spawn(async move {
            state.record_usage(&report).await;
            drop(permit);
        });
    }
}

/// Result of one account attempt.
/// The targets a model string resolved to, and how to use them.
struct ResolvedTargets {
    /// In the order the strategy chose. Empty only for an unparseable model.
    targets: Vec<model::ModelTarget>,
    /// `Fusion` means ask all of them; anything else means try them in turn.
    strategy: ComboStrategy,
    /// Fusion tuning for this combo, after any per-combo override.
    tuning: fusion::FusionTuning,
}

/// What running one combo model produced.
enum TargetOutcome {
    /// A terminal response: the provider answered, and no other model is asked.
    Responded(HttpResponse),
    /// This model did not answer. The response is carried anyway, so the last
    /// model in a combo can report its real error instead of a synthesised one.
    Failed { response: HttpResponse },
}

enum Attempt {
    /// A client response is ready.
    Responded(HttpResponse),
    /// This account failed in a way that warrants trying the next one.
    Retryable {
        status: u16,
        message: String,
        cooldown_ms: u64,
        backoff_level: Option<u32>,
    },
}

#[cfg(test)]
mod coalesce_tests {
    use super::coalesce_frames;

    #[test]
    fn one_frame_passes_through_byte_for_byte() {
        let mut batch = vec!["data: {\"a\":1}\n\n".to_owned()];
        assert_eq!(coalesce_frames(&mut batch).as_ref(), b"data: {\"a\":1}\n\n");
    }

    #[test]
    fn several_frames_are_concatenated_with_nothing_inserted_between_them() {
        // The property that keeps SSE framing intact: the frames already end in `\n\n`, so joining
        // them means appending, never inserting a separator. A separator here would produce a
        // blank event that a strict client counts as a message.
        let mut batch = vec![
            "data: one\n\n".to_owned(),
            "data: two\n\n".to_owned(),
            "data: [DONE]\n\n".to_owned(),
        ];
        let chunk = coalesce_frames(&mut batch);
        assert_eq!(
            chunk.as_ref(),
            b"data: one\n\ndata: two\n\ndata: [DONE]\n\n"
        );
        // And the count of terminators is preserved exactly.
        assert_eq!(
            String::from_utf8_lossy(chunk.as_ref())
                .matches("\n\n")
                .count(),
            3
        );
    }

    #[test]
    fn the_concatenation_is_the_same_bytes_as_writing_each_frame_separately() {
        // Stated as an equivalence, because that is the whole claim being made: coalescing changes
        // how many writes happen, not what the client receives.
        let frames: Vec<String> = (0..200)
            .map(|index| format!("data: {{\"i\":{index}}}\n\n"))
            .collect();
        let separately: Vec<u8> = frames.concat().into_bytes();

        let mut batch = frames;
        let coalesced = coalesce_frames(&mut batch);
        assert_eq!(coalesced.as_ref(), separately.as_slice());
    }

    #[test]
    fn an_empty_batch_yields_an_empty_chunk_rather_than_panicking() {
        let mut batch: Vec<String> = Vec::new();
        assert!(coalesce_frames(&mut batch).is_empty());
    }

    #[test]
    fn a_frame_containing_a_blank_line_is_not_mangled() {
        // Content with an embedded blank line is legal inside a JSON string payload, and must not
        // be treated as a frame boundary by anything here — this function does not parse.
        let mut batch = vec!["data: {\"text\":\"a\\n\\nb\"}\n\n".to_owned()];
        assert_eq!(
            coalesce_frames(&mut batch).as_ref(),
            b"data: {\"text\":\"a\\n\\nb\"}\n\n"
        );
    }
}
