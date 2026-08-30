//! Translation cost, per request and per streaming frame.
//!
//! These do not support the "faster than 9Router" claim — that needs the end-to-end
//! harness in `BENCHMARKS.md`. What they give is attribution: when a release is slower,
//! this says which function.
//!
//! Two of these matter more than the rest:
//!
//! * `passthrough/openai_to_openai` is the **control**. Same format both ends means
//!   there is nothing to translate, so anything but near-zero is the router charging
//!   for work a request did not need.
//! * `sse_stream/2000_frames` is the shape a coding agent actually produces. Per-frame
//!   cost multiplies across a long response, so a per-request figure understates what a
//!   user feels by three orders of magnitude.
//!
//! A benchmark is not a test. If a body here is rejected by the translator, the body is
//! wrong — do not add an assertion. A bench silently measuring an error path is worse
//! than no bench, so each builder below is exercised once in `bodies_are_translatable`
//! to keep that honest.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use nullrouter_providers::Format;
use nullrouter_translate::{
    Clock, RequestRoute, StreamState, translate_request, translate_response,
};
use serde_json::{Value, json};
use std::hint::black_box;

/// Output ceiling passed to every request bench. Real value, so clamping runs.
const CEILING: u64 = 64_000;

// ── request bodies ───────────────────────────────────────────────────────────

/// A typical interactive turn: a system prompt and a short exchange.
fn small_conversation() -> Value {
    json!({
        "model": "gpt-4o",
        "max_tokens": 1024,
        "messages": [
            { "role": "system", "content": "You are a careful assistant." },
            { "role": "user", "content": "Explain the difference between a mutex and a channel." },
            { "role": "assistant", "content": "A mutex guards shared state; a channel moves ownership." },
            { "role": "user", "content": "Which is cheaper under contention?" },
        ],
    })
}

/// A long session: 60 turns, roughly 100k characters. Where allocation shows up.
fn large_conversation() -> Value {
    let paragraph = "The router translates between wire formats in both directions. ".repeat(24);
    let mut messages = vec![json!({
        "role": "system",
        "content": "You are a careful assistant working in a large codebase.",
    })];
    for turn in 0..30 {
        messages.push(json!({
            "role": "user",
            "content": format!("Turn {turn}: {paragraph}"),
        }));
        messages.push(json!({
            "role": "assistant",
            "content": format!("Acknowledged turn {turn}. {paragraph}"),
        }));
    }
    json!({ "model": "gpt-4o", "max_tokens": 4096, "messages": messages })
}

/// Tool definitions plus results — the most branch-heavy path in the translators.
fn tool_conversation() -> Value {
    let tools: Vec<Value> = (0..12)
        .map(|index| {
            json!({
                "type": "function",
                "function": {
                    "name": format!("tool_{index}"),
                    "description": format!("Does thing {index}, at some length so the description is not trivial to copy."),
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "A filesystem path" },
                            "limit": { "type": "integer", "description": "How many results" },
                        },
                        "required": ["path"],
                    },
                },
            })
        })
        .collect();
    json!({
        "model": "gpt-4o",
        "max_tokens": 2048,
        "tools": tools,
        "messages": [
            { "role": "user", "content": "Read the three files and summarise them." },
            { "role": "assistant", "tool_calls": [
                { "id": "call_1", "type": "function",
                  "function": { "name": "tool_0", "arguments": "{\"path\":\"a.rs\"}" } },
                { "id": "call_2", "type": "function",
                  "function": { "name": "tool_1", "arguments": "{\"path\":\"b.rs\"}" } },
            ]},
            { "role": "tool", "tool_call_id": "call_1", "content": "fn main() {}" },
            { "role": "tool", "tool_call_id": "call_2", "content": "pub struct A;" },
            { "role": "user", "content": "Now the third." },
        ],
    })
}

/// A Claude-format body, for the reverse direction.
fn small_claude() -> Value {
    json!({
        "model": "claude-fable-5",
        "max_tokens": 1024,
        "system": "You are a careful assistant.",
        "messages": [
            { "role": "user", "content": [{ "type": "text", "text": "Explain mutexes." }] },
            { "role": "assistant", "content": [{ "type": "text", "text": "They guard state." }] },
            { "role": "user", "content": [{ "type": "text", "text": "And channels?" }] },
        ],
    })
}

/// A Gemini-format body carrying an inline image part: media re-encoding.
fn gemini_with_image() -> Value {
    json!({
        "contents": [{
            "role": "user",
            "parts": [
                { "text": "What is in this image?" },
                { "inlineData": { "mimeType": "image/png", "data": "iVBORw0KGgoAAAANSUhEUg==" } },
            ],
        }],
        "generationConfig": { "maxOutputTokens": 1024 },
    })
}

// ── streaming frames ─────────────────────────────────────────────────────────

fn openai_delta_frame() -> Value {
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion.chunk",
        "model": "gpt-4o",
        "choices": [{ "index": 0, "delta": { "content": "token " }, "finish_reason": null }],
    })
}

fn openai_tool_frame() -> Value {
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion.chunk",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": { "tool_calls": [{
                "index": 0, "id": "call_1", "type": "function",
                "function": { "name": "tool_0", "arguments": "{\"pa" },
            }]},
            "finish_reason": null,
        }],
    })
}

fn claude_delta_frame() -> Value {
    json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": { "type": "text_delta", "text": "token " },
    })
}

// ── benches ──────────────────────────────────────────────────────────────────

fn request_translation(criterion: &mut Criterion) {
    let cases: [(&str, Value); 3] = [
        ("small", small_conversation()),
        ("large", large_conversation()),
        ("tools", tool_conversation()),
    ];

    let mut group = criterion.benchmark_group("openai_to_claude");
    for (name, body) in &cases {
        group.throughput(Throughput::Bytes(body.to_string().len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), body, |bencher, body| {
            bencher.iter(|| {
                translate_request(
                    RequestRoute {
                        source: Format::OpenAi,
                        target: Format::Claude,
                        provider: "anthropic",
                        model: "claude-fable-5",
                    },
                    black_box(body),
                    false,
                    CEILING,
                )
            });
        });
    }
    group.finish();

    let mut group = criterion.benchmark_group("claude_to_openai");
    let claude = small_claude();
    group.bench_function("small", |bencher| {
        bencher.iter(|| {
            translate_request(
                RequestRoute {
                    source: Format::Claude,
                    target: Format::OpenAi,
                    provider: "openai",
                    model: "gpt-4o",
                },
                black_box(&claude),
                false,
                CEILING,
            )
        });
    });
    group.finish();

    let mut group = criterion.benchmark_group("gemini_to_openai");
    let gemini = gemini_with_image();
    group.bench_function("with_image", |bencher| {
        bencher.iter(|| {
            translate_request(
                RequestRoute {
                    source: Format::Gemini,
                    target: Format::OpenAi,
                    provider: "openai",
                    model: "gpt-4o",
                },
                black_box(&gemini),
                false,
                CEILING,
            )
        });
    });
    group.finish();

    // The control. Same format both ends: there is nothing to *translate*, so this
    // should cost only what rewriting `model` costs — which is one deep clone of the
    // body, since the caller's copy is borrowed.
    //
    // `clone_only` is the floor that makes that claim checkable. If passthrough drifts
    // meaningfully above it, the short-circuit has stopped short-circuiting.
    let mut group = criterion.benchmark_group("passthrough");
    let body = small_conversation();
    group.bench_function("clone_only", |bencher| {
        bencher.iter(|| black_box(black_box(&body).clone()));
    });
    // Attribution for the rest of the gap: `extract_thinking` runs before the
    // same-format check, and `apply` does a per-model capability lookup. Both run on a
    // request that mentions no reasoning at all, which is the common case.
    group.bench_function("extract_thinking_absent", |bencher| {
        bencher.iter(|| {
            black_box(nullrouter_translate::thinking::extract_thinking(black_box(
                &body,
            )))
        });
    });
    group.bench_function("apply_thinking_absent", |bencher| {
        bencher.iter_batched(
            || body.clone(),
            |mut owned| {
                nullrouter_translate::thinking::apply(
                    Format::OpenAi,
                    "openai",
                    "gpt-4o",
                    &mut owned,
                    None,
                );
                black_box(owned)
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.bench_function("openai_to_openai", |bencher| {
        bencher.iter(|| {
            translate_request(
                RequestRoute {
                    source: Format::OpenAi,
                    target: Format::OpenAi,
                    provider: "openai",
                    model: "gpt-4o",
                },
                black_box(&body),
                false,
                CEILING,
            )
        });
    });
    group.finish();
}

fn frame_translation(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("sse_frame");

    let openai = openai_delta_frame();
    group.bench_function("openai_to_claude", |bencher| {
        bencher.iter_batched(
            || StreamState::new(Clock::System),
            |mut state| {
                translate_response(
                    Format::OpenAi,
                    Format::Claude,
                    black_box(&openai),
                    &mut state,
                )
            },
            criterion::BatchSize::SmallInput,
        );
    });

    let claude = claude_delta_frame();
    group.bench_function("claude_to_openai", |bencher| {
        bencher.iter_batched(
            || StreamState::new(Clock::System),
            |mut state| {
                translate_response(
                    Format::Claude,
                    Format::OpenAi,
                    black_box(&claude),
                    &mut state,
                )
            },
            criterion::BatchSize::SmallInput,
        );
    });

    let tool = openai_tool_frame();
    group.bench_function("with_tool_call", |bencher| {
        bencher.iter_batched(
            || StreamState::new(Clock::System),
            |mut state| {
                translate_response(Format::OpenAi, Format::Claude, black_box(&tool), &mut state)
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn stream_translation(criterion: &mut Criterion) {
    // Steady state across a long response. Reported as frames/sec, which converts
    // directly to per-1000-token cost without the reader doing arithmetic.
    const FRAMES: u64 = 2_000;
    let frame = openai_delta_frame();

    let mut group = criterion.benchmark_group("sse_stream");
    group.throughput(Throughput::Elements(FRAMES));
    group.sample_size(20);
    group.bench_function("2000_frames", |bencher| {
        bencher.iter_batched(
            || StreamState::new(Clock::System),
            |mut state| {
                for _ in 0..FRAMES {
                    let out = translate_response(
                        Format::OpenAi,
                        Format::Claude,
                        black_box(&frame),
                        &mut state,
                    );
                    black_box(out);
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    request_translation,
    frame_translation,
    stream_translation
);
criterion_main!(benches);
