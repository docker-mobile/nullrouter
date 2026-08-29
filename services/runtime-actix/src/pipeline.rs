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
    Credentials, ExecuteRequest, Executor, build_error_body, check_fallback_error,
    collapse_stream_to_json, is_executor_supported, pipe_stream, unsupported_executor_message,
};
use nullrouter_providers::{Format, ServiceKind, model, target_format};
use nullrouter_translate::state::Clock;
use nullrouter_translate::{RequestRoute, StreamState, translate_request};
use serde_json::Value;

use crate::combo::{self, ComboStrategy, RotationState};
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
        ServiceKind::Search => "search",
        ServiceKind::Fetch => "fetch",
    }
}

/// How many accounts to try before giving up.
///
/// Upstream loops until credentials run out; this bounds the walk so a
/// misconfigured provider cannot spin indefinitely.
const MAX_ACCOUNT_ATTEMPTS: usize = 10;

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
        }
    }

    /// Runtime pointed at an explicit state address, for tests.
    pub fn with_state_addr(addr: &str) -> Self {
        Self {
            executor: Executor::new(),
            state: StateClient::new(addr),
            rotation: RotationState::new(),
        }
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
        let context = self.state.routing_context().await;
        if !context.settings.require_api_key {
            // A state outage defaults this to false, which does not open a
            // hole: credentials also come from state, so the request cannot
            // reach a provider anyway — it fails at selection with 503.
            return None;
        }
        let Some(api_key) = api_key.map(str::trim).filter(|key| !key.is_empty()) else {
            return Some(responses::json(
                StatusCode::UNAUTHORIZED,
                &build_error_body(401, "Missing API key"),
            ));
        };
        if self.state.validate_api_key(api_key).await {
            return None;
        }
        Some(responses::json(
            StatusCode::UNAUTHORIZED,
            &build_error_body(401, "Invalid API key"),
        ))
    }

    /// Resolve, execute, and respond.
    ///
    /// A combo yields several targets; each is tried in turn, and a failure only
    /// advances to the next model when it is one worth retrying elsewhere. A
    /// refusal that would recur on every model — a malformed request, say — is
    /// returned immediately rather than replayed against the whole combo.
    pub(crate) async fn execute_chat(&self, context: ChatContext<'_>) -> HttpResponse {
        let targets = self.resolve_targets(&context.requested_model).await;
        let Some((last, leading)) = targets.split_last() else {
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
    /// Backed by the registry and the caller's configured connections, so
    /// `/v1/models` reflects what is actually reachable.
    pub(crate) async fn models_list(&self, kinds: &[&str]) -> Value {
        let context = self.state.routing_context().await;
        let input = nullrouter_providers::ModelsListInput {
            connections: context
                .connections
                .iter()
                .map(|connection| nullrouter_providers::ConnectionView {
                    provider: connection.provider.clone(),
                    prefix: connection.prefix.clone(),
                    enabled_models: connection.enabled_models.clone(),
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
                )
                .await;

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
            return self.resolve_targets(requested).await.into_iter().next();
        }
        let canonical = nullrouter_providers::resolve_provider_id(requested);
        if nullrouter_providers::entry(canonical).is_some() {
            return Some(model::ModelTarget {
                provider: canonical.to_owned(),
                // No specific model: the service config supplies the default.
                model: String::new(),
            });
        }
        self.resolve_targets(requested).await.into_iter().next()
    }

    /// Resolve a client model string to the ordered list of targets to try.
    ///
    /// A plain `provider/model` yields one target. A combo name yields one per
    /// configured model, in the order its strategy chose — so every model in the
    /// combo is a fallback for the ones before it.
    async fn resolve_targets(&self, requested: &str) -> Vec<model::ModelTarget> {
        let parsed = model::parse_model(requested);
        if !parsed.is_alias {
            return parsed
                .provider
                .map(|provider| model::ModelTarget {
                    provider,
                    model: parsed.model,
                })
                .into_iter()
                .collect();
        }

        let context = self.state.routing_context().await;
        if let Some(combo) = context
            .combos
            .iter()
            .find(|combo| combo.name == parsed.model)
            && !combo.models.is_empty()
        {
            let strategy = ComboStrategy::from_settings(context.settings.combo_strategy.as_deref());
            let sticky = context
                .settings
                .combo_sticky_round_robin_limit
                .unwrap_or(DEFAULT_COMBO_STICKY_LIMIT);
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
                return targets;
            }
        }

        // Fall back to prefix inference, as upstream does for bare aliases.
        vec![model::infer_target(&parsed.model)]
    }

    /// One account attempt.
    async fn attempt(
        &self,
        context: &ChatContext<'_>,
        target: &model::ModelTarget,
        credentials: &Credentials,
    ) -> Attempt {
        let started = Instant::now();
        let target_format = target_format(&target.provider);
        let upstream_model = model::upstream_model_id(&target.provider, &target.model);
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

        let outcome = self
            .executor
            .execute(ExecuteRequest {
                provider: &target.provider,
                body: &translated.body,
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
            )
            .await;

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

        actix_web::rt::spawn(async move {
            let summary = pipe_stream(
                outcome.response,
                target_format,
                source_format,
                &mut state,
                ChannelSink { sender },
            )
            .await;

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
            state_client.record_usage(&report).await;
        });

        let body = futures_util::stream::unfold(receiver, |mut receiver| async move {
            receiver
                .recv()
                .await
                .map(|frame| (Ok(actix_web::web::Bytes::from(frame)), receiver))
        });

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
            collapse_stream_to_json(outcome.response, target_format, &upstream_model, &mut state)
                .await
        } else {
            // A genuinely non-streaming provider response.
            match outcome.response.json::<Value>().await {
                Ok(body) => body,
                Err(error) => {
                    let message = format!("upstream returned an unreadable body: {error}");
                    return Attempt::Responded(Self::error_response(
                        context,
                        StatusCode::BAD_GATEWAY,
                        &message,
                    ));
                }
            }
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
        )
        .await;

        Attempt::Responded(responses::json(StatusCode::OK, &body))
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
        )
        .await;
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
        )
        .await;

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
    async fn record(
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
        self.state.record_usage(&report).await;
    }
}

/// Result of one account attempt.
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
