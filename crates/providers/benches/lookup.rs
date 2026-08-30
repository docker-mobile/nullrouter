//! Per-request registry lookups.
//!
//! Each of these runs at least once per request, several times for a combo, so they
//! belong in nanoseconds. They are here for attribution rather than for the comparative
//! claim: when a release gets slower, this says whether a lookup grew.
//!
//! `capability/explicit_row` against `capability/fallback_by_name` is the pair worth
//! watching. The fallback path exists because upstream matches a capability row by model
//! *name* when a provider has no row of its own, and it is strictly more work — a
//! regression that moves common models onto the fallback path would show up here as the
//! two converging.

use criterion::{Criterion, criterion_group, criterion_main};
use nullrouter_providers::{Format, capabilities_for_model, max_output};
use std::hint::black_box;

fn capability_lookup(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("capability");

    // A model with its own row in the capability table.
    group.bench_function("explicit_row", |bencher| {
        bencher.iter(|| {
            black_box(capabilities_for_model(
                black_box("openai"),
                black_box("gpt-4o"),
            ))
        });
    });

    // A model with no row of its own, resolved by name. Upstream's own fallback, and
    // the path a newly-released model takes before the registry is regenerated.
    group.bench_function("fallback_by_name", |bencher| {
        bencher.iter(|| {
            black_box(capabilities_for_model(
                black_box("openai-compatible-local"),
                black_box("gpt-4o-2099-preview"),
            ))
        });
    });

    // An unknown provider and model: the miss path, which must not be slow either.
    group.bench_function("miss", |bencher| {
        bencher.iter(|| {
            black_box(capabilities_for_model(
                black_box("no-such-provider"),
                black_box("no-such-model"),
            ))
        });
    });

    // Called on every request to clamp `max_tokens`.
    group.bench_function("max_output", |bencher| {
        bencher.iter(|| {
            black_box(max_output(
                black_box("anthropic"),
                black_box("claude-fable-5"),
            ))
        });
    });
    group.finish();
}

fn model_resolution(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("model");

    // `ds/deepseek-v4-pro` → canonical provider id. Every request with a prefix.
    group.bench_function("alias_resolution", |bencher| {
        bencher.iter(|| black_box(nullrouter_providers::resolve_provider_id(black_box("ds"))));
    });

    // Parsing the client's model string, which carries the provider prefix and may
    // carry a `(effort)` suffix.
    group.bench_function("parse_prefixed", |bencher| {
        bencher.iter(|| {
            black_box(nullrouter_providers::parse_model(black_box(
                "deepseek/deepseek-v4-pro(high)",
            )))
        });
    });
    group.finish();
}

fn transport_selection(criterion: &mut Criterion) {
    // Added with multi-transport support and previously unmeasured. Runs once per
    // attempt, and a combo makes several attempts.
    let mut group = criterion.benchmark_group("transport");

    group.bench_function("format_lookup_hit", |bencher| {
        bencher.iter(|| {
            black_box(nullrouter_providers::runtime_transport(
                black_box("deepseek"),
                black_box("deepseek-v4-pro"),
                black_box(Format::Claude),
            ))
        });
    });

    // A provider with no alternatives: the common case, and the one that must be
    // cheapest since it is what most requests hit.
    group.bench_function("format_lookup_miss", |bencher| {
        bencher.iter(|| {
            black_box(nullrouter_providers::runtime_transport(
                black_box("openai"),
                black_box("gpt-4o"),
                black_box(Format::Claude),
            ))
        });
    });

    // The per-model guard, which reads a model's own `supportedFormats`.
    group.bench_function("model_guard", |bencher| {
        bencher.iter(|| {
            black_box(nullrouter_providers::model_serves_format(
                black_box("opencode-go"),
                black_box("glm-5.2"),
                black_box(Format::Claude),
            ))
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    capability_lookup,
    model_resolution,
    transport_selection
);
criterion_main!(benches);
