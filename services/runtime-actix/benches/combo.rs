//! Combo model ordering, uncontended and under contention.
//!
//! Round-robin ordering takes a lock on shared rotation state, because a cursor only
//! means anything relative to the previous request. That makes it the one per-request
//! path in the router with real contention, and `contended_8_threads` is the bench that
//! matters — the uncontended figures exist to give it a scale.
//!
//! `fill_first` and `fusion` are here precisely because they should *not* take the lock:
//! both keep the configured order, so a regression that started locking on them would
//! show up as their cost approaching round-robin's.

use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use nullrouter_runtime::RotationBench;
use std::hint::black_box;

fn uncontended(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("combo");
    let bench = RotationBench::new(5);

    group.bench_function("fill_first_5", |bencher| {
        bencher.iter(|| black_box(bench.fill_first()));
    });
    group.bench_function("fusion_5", |bencher| {
        bencher.iter(|| black_box(bench.fusion()));
    });
    group.bench_function("round_robin_5_sticky_3", |bencher| {
        bencher.iter(|| black_box(bench.round_robin(3)));
    });
    // A sticky limit of 1 rotates on every request: the worst case for cursor churn.
    group.bench_function("round_robin_5_sticky_1", |bencher| {
        bencher.iter(|| black_box(bench.round_robin(1)));
    });
    group.finish();
}

fn contended(criterion: &mut Criterion) {
    // Eight threads on one combo's cursor, which is what a busy router with a single
    // popular combo actually does.
    const THREADS: usize = 8;
    const PER_THREAD: usize = 250;

    let mut group = criterion.benchmark_group("combo");
    group.sample_size(20);
    group.throughput(criterion::Throughput::Elements(
        (THREADS * PER_THREAD) as u64,
    ));
    group.bench_function("contended_8_threads", |bencher| {
        bencher.iter_batched(
            || Arc::new(RotationBench::new(5)),
            |bench| {
                let handles: Vec<_> = (0..THREADS)
                    .map(|_| {
                        let bench = Arc::clone(&bench);
                        std::thread::spawn(move || {
                            for _ in 0..PER_THREAD {
                                black_box(bench.round_robin(1));
                            }
                        })
                    })
                    .collect();
                for handle in handles {
                    // A poisoned lock would be a real defect, so the bench stops rather
                    // than quietly reporting a number for a broken run.
                    handle.join().expect("rotation thread panicked");
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(benches, uncontended, contended);
criterion_main!(benches);
