use std::hint::black_box;
use std::path::Path;

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use flint_core::{Algorithm, Limiter, MultiCheckItem};
use tempfile::TempDir;

fn configured_limiter(algorithm: Algorithm, rate: u64, per: &str) -> (TempDir, Limiter) {
    let dir = TempDir::new().expect("temp dir");
    let limiter = Limiter::open(dir.path()).expect("open limiter");
    limiter
        .limit("api:user-42", rate, per, algorithm)
        .expect("configure limit");
    (dir, limiter)
}

fn seed_limiter(path: &Path, keys: usize, checks_per_key: usize) {
    let limiter = Limiter::open(path).expect("open limiter");
    for idx in 0..keys {
        let key = format!("api:user-{idx}");
        limiter
            .limit(&key, 1_000_000, "1h", Algorithm::TokenBucket)
            .expect("configure limit");
        for _ in 0..checks_per_key {
            limiter.check(&key).expect("check limit");
        }
    }
}

fn bench_check_hot_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("check_hot_path");
    group.sample_size(20);

    for algorithm in [
        Algorithm::TokenBucket,
        Algorithm::SlidingWindowLog,
        Algorithm::FixedWindowCounter,
    ] {
        let name = format!("{algorithm:?}");
        group.bench_function(name, |b| {
            let (_dir, limiter) = configured_limiter(algorithm, 1_000_000, "1h");
            b.iter(|| {
                black_box(limiter.check("api:user-42").expect("check limit"));
            });
        });
    }

    group.finish();
}

fn bench_cost_and_multi_limit(c: &mut Criterion) {
    let mut group = c.benchmark_group("cost_and_multi_limit");
    group.sample_size(20);

    group.bench_function("token_bucket_cost_10", |b| {
        let (_dir, limiter) = configured_limiter(Algorithm::TokenBucket, 1_000_000, "1h");
        b.iter(|| {
            black_box(
                limiter
                    .check_cost("api:user-42", black_box(10))
                    .expect("cost check"),
            );
        });
    });

    group.bench_function("check_all_three_limits", |b| {
        let dir = TempDir::new().expect("temp dir");
        let limiter = Limiter::open(dir.path()).expect("open limiter");
        for key in ["user:42", "org:acme", "route:/v1/chat"] {
            limiter
                .limit(key, 1_000_000, "1h", Algorithm::TokenBucket)
                .expect("configure limit");
        }

        b.iter(|| {
            black_box(
                limiter
                    .check_all(vec![
                        MultiCheckItem {
                            key: "user:42".to_string(),
                            cost: 1,
                        },
                        MultiCheckItem {
                            key: "org:acme".to_string(),
                            cost: 10,
                        },
                        MultiCheckItem {
                            key: "route:/v1/chat".to_string(),
                            cost: 1,
                        },
                    ])
                    .expect("multi check"),
            );
        });
    });

    group.finish();
}

fn bench_many_keys(c: &mut Criterion) {
    let mut group = c.benchmark_group("many_keys");
    group.sample_size(10);

    for keys in [1_000_usize, 10_000] {
        group.throughput(Throughput::Elements(keys as u64));
        group.bench_with_input(
            BenchmarkId::new("configure_keys", keys),
            &keys,
            |b, &keys| {
                b.iter_batched(
                    TempDir::new,
                    |dir| {
                        let dir = dir.expect("temp dir");
                        let limiter = Limiter::open(dir.path()).expect("open limiter");
                        for idx in 0..keys {
                            limiter
                                .limit(format!("api:user-{idx}"), 100, "1m", Algorithm::TokenBucket)
                                .expect("configure limit");
                        }
                        black_box(limiter.list().expect("list limits"));
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_replay_and_recovery(c: &mut Criterion) {
    let mut group = c.benchmark_group("replay_and_recovery");
    group.sample_size(10);

    for events in [1_000_usize, 10_000] {
        let dir = TempDir::new().expect("temp dir");
        seed_limiter(dir.path(), 10, events / 10);

        group.throughput(Throughput::Elements(events as u64));
        group.bench_with_input(
            BenchmarkId::new("reopen_from_aof", events),
            &events,
            |b, _| {
                b.iter(|| {
                    let limiter = Limiter::open(dir.path()).expect("reopen limiter");
                    black_box(limiter.doctor().expect("doctor"));
                });
            },
        );
    }

    group.finish();
}

fn bench_compaction(c: &mut Criterion) {
    let mut group = c.benchmark_group("compaction");
    group.sample_size(10);

    for events in [1_000_usize, 10_000] {
        group.throughput(Throughput::Elements(events as u64));
        group.bench_with_input(
            BenchmarkId::new("compact_aof", events),
            &events,
            |b, &events| {
                b.iter_batched(
                    || {
                        let dir = TempDir::new().expect("temp dir");
                        seed_limiter(dir.path(), 10, events / 10);
                        dir
                    },
                    |dir| {
                        let limiter = Limiter::open(dir.path()).expect("open limiter");
                        limiter.compact().expect("compact");
                        black_box(limiter.doctor().expect("doctor"));
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_check_hot_path,
    bench_cost_and_multi_limit,
    bench_many_keys,
    bench_replay_and_recovery,
    bench_compaction
);
criterion_main!(benches);
