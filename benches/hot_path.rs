//! Criterion benchmark of the per-event hot path: the deep copy into a pooled
//! slot (ETW thread), then decode, encode, and their production composition
//! (parser thread). It needs the `bench` feature and a standard library built
//! for unwinding, which the alias in `.cargo/config.toml` provides:
//!
//! ```text
//! cargo bench-hot-path
//! ```
//!
//! The inputs are deterministic: a 60-byte A query (event 256), the same query
//! over IPv6, a 100-byte response (event 257), a 37-byte failure response
//! (event 258), and a 4,096-byte response that needs a jumbo slot. Every
//! measured parse is a schema-cache hit.
//!
//! Most groups run on one thread over a small pool that stays in cache, so
//! their numbers are best cases. `copy_record_cold` and `two_thread` use a
//! production-sized pool, and `two_thread` adds the cross-core cache traffic of
//! the real producer and consumer. `decompose` splits `parse_and_encode` into
//! its parts. See `datasets/baselines/code-review-2026-11-01/README.md` for how
//! to compare runs.

use std::{hint::black_box, thread, time::Duration, time::Instant};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use crossbeam_channel::bounded;
use etw_dnstap::bench::{
    Copied, Pipeline, Plan, Pool, Record, SocketWriter, copy_record, private_bytes,
};

/// Leases outstanding at once in the batched groups. The benchmark pool holds
/// 2,048 regular slots, so a batch never exhausts it.
const BATCH: u64 = 1_024;
/// Regular slots in the cold pool: 128 MiB of slot bytes, beyond the last-level
/// cache, and in the range the lab tested (`--capacity 65536`).
const COLD_POOL_CAPACITY: usize = 65_536;
/// Jumbo slots in the cold pool, the service default.
const COLD_POOL_JUMBO: usize = 64;
/// Records in flight between the two threads of `two_thread`.
const CHANNEL_CAPACITY: usize = 1_024;

fn bytes(slice: &[u8]) -> u64 {
    u64::try_from(slice.len()).expect("slice lengths fit u64")
}

/// The records of the decode, encode, and parse groups, by name.
fn regular_records() -> [(&'static str, Record); 4] {
    [
        ("query_256", Record::query()),
        ("response_257", Record::response()),
        ("query_256_ipv6", Record::query_ipv6()),
        ("failure_258", Record::response_failure()),
    ]
}

/// `copy_record`: acquire a slot, copy the user data, and (on drop) recycle the
/// slot. Production splits the copy and the recycle across the ETW and parser
/// threads; here one iteration pays for both. The pool is small, so the slots
/// stay in cache: this is the best case.
fn copy_record_group(c: &mut Criterion) {
    let pool = Pool::new();
    let mut group = c.benchmark_group("copy_record");
    for (name, record) in [
        ("regular_query_256", Record::query()),
        ("regular_response_257", Record::response()),
        ("jumbo_response_257", Record::jumbo_response()),
    ] {
        group.throughput(Throughput::Bytes(bytes(record.payload())));
        group.bench_function(name, |b| {
            b.iter(|| copy_record(black_box(&pool), black_box(&record)));
        });
    }
    group.finish();
}

/// The groups that need a production-sized pool. It is allocated once, and its
/// private bytes are printed for the baseline record.
fn cold_pool_groups(c: &mut Criterion) {
    let before = private_bytes();
    let pool = Pool::with_capacity(COLD_POOL_CAPACITY, COLD_POOL_JUMBO);
    let after = private_bytes();
    eprintln!(
        "pool_private_bytes regular_slots={} jumbo_slots={COLD_POOL_JUMBO} before={before} after={after} delta={}",
        COLD_POOL_CAPACITY + 2,
        after - before,
    );
    copy_record_cold_group(c, pool);
    two_thread_group(c, pool);
}

/// `copy_record` on a pool larger than the last-level cache. The free list is
/// FIFO, so each iteration leases a slot last touched 65,000 events ago, as the
/// service does at its tested capacity.
fn copy_record_cold_group(c: &mut Criterion, pool: Pool) {
    let mut group = c.benchmark_group("copy_record_cold");
    for (name, record) in [
        ("regular_query_256", Record::query()),
        ("regular_response_257", Record::response()),
    ] {
        group.throughput(Throughput::Bytes(bytes(record.payload())));
        group.bench_function(name, |b| {
            b.iter(|| copy_record(black_box(&pool), black_box(&record)));
        });
    }
    group.finish();
}

/// Production's two threads: a producer copies records into the cold pool and
/// hands them over a bounded channel (as the ETW callback does), and the
/// benchmark thread parses, encodes, and recycles them. `iter_custom` times
/// `iters` events from the first copy to the last recycle, so the time per
/// iteration is the reciprocal of the sustained events per second. The `Arc`
/// count, the free-list ends, and the record bytes all cross cores here. The
/// producer blocks on a full channel (the service drops instead) so that every
/// run moves exactly `iters` events.
fn two_thread_group(c: &mut Criterion, pool: Pool) {
    let mut group = c.benchmark_group("two_thread");
    group.throughput(Throughput::Elements(1));
    for (name, make) in [
        ("query_256", Record::query as fn() -> Record),
        ("response_257", Record::response),
    ] {
        let mut pipeline = Pipeline::new();
        // Fill the schema cache, so every measured parse is a hit.
        pipeline.parse_and_encode(copy_record(&pool, &make()));
        group.bench_function(name, |b| {
            b.iter_custom(|iters| {
                let (sender, receiver) = bounded::<Copied>(CHANNEL_CAPACITY);
                let start = Instant::now();
                thread::scope(|scope| {
                    scope.spawn(move || {
                        // A `Record` holds a raw pointer, so it is built here.
                        let record = make();
                        for _ in 0..iters {
                            sender
                                .send(copy_record(&pool, &record))
                                .expect("the consumer outlives the producer");
                        }
                    });
                    for _ in 0..iters {
                        let event = receiver.recv().expect("the producer sends `iters` events");
                        black_box(pipeline.parse_and_encode(event).len());
                    }
                });
                start.elapsed()
            });
        });
    }
    group.finish();
}

/// One socket write in a fresh deadline window on a loopback connection that a
/// second thread drains: 64 KiB is what the frame writer's buffer flushes (the
/// service makes one such write per 600 or so events), 103 bytes is one frame.
/// The difference between runs is the per-write cost of the deadline logic and
/// the `set_write_timeout` call.
fn socket_write_group(c: &mut Criterion) {
    let mut writer = SocketWriter::new();
    let mut group = c.benchmark_group("socket_write");
    for (name, length) in [("flush_64k", 64 * 1024), ("frame_103", 103)] {
        let buffer = vec![0x5a_u8; length];
        group.throughput(Throughput::Bytes(bytes(&buffer)));
        group.bench_function(name, |b| {
            b.iter(|| writer.write_window(black_box(&buffer)));
        });
    }
    group.finish();
}

/// `DecodePlan::decode` with the plan compiled from the saved template.
fn decode_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode");
    for (name, record) in regular_records() {
        let plan = Plan::new(record.event_id());
        let payload = record.payload();
        group.throughput(Throughput::Bytes(bytes(payload)));
        group.bench_function(name, |b| {
            b.iter(|| plan.decode(black_box(payload)));
        });
    }
    group.finish();
}

/// `Encoder::encode` of a view decoded once outside the measurement.
fn encode_group(c: &mut Criterion) {
    let pool = Pool::new();
    let mut pipeline = Pipeline::new();
    let mut group = c.benchmark_group("encode");
    group.throughput(Throughput::Elements(1));
    for (name, record) in regular_records() {
        let event = copy_record(&pool, &record);
        let decoded = pipeline.parse(&event);
        group.bench_function(name, |b| {
            b.iter(|| black_box(pipeline.encode(black_box(&decoded))).len());
        });
    }
    group.finish();
}

/// `pipeline::parse_and_encode` on a schema-cache hit. The copy that makes each
/// input runs in the batch setup, outside the measurement; the lease recycle
/// inside `parse_and_encode` is measured. The inputs were written just before,
/// on the same core, so they are warm.
fn parse_and_encode_group(c: &mut Criterion) {
    let pool = Pool::new();
    let mut pipeline = Pipeline::new();
    let mut group = c.benchmark_group("parse_and_encode");
    group.throughput(Throughput::Elements(1));
    for (name, record) in regular_records() {
        // Fill the schema cache, so every measured call is a hit.
        pipeline.parse_and_encode(copy_record(&pool, &record));
        group.bench_function(name, |b| {
            b.iter_batched(
                || copy_record(&pool, &record),
                |event| black_box(pipeline.parse_and_encode(black_box(event))).len(),
                BatchSize::NumIterations(BATCH),
            );
        });
    }
    group.finish();
}

/// The parts of `parse_and_encode`. `parse_encode_retained` parses and encodes
/// one record that stays leased and warm (the shape of the manual
/// `parse_encode_benchmark`); `recycle` is the lease drop alone. What
/// `parse_and_encode` costs beyond their sum is cache warmth and harness.
fn decompose_group(c: &mut Criterion) {
    let pool = Pool::new();
    let mut pipeline = Pipeline::new();
    let mut group = c.benchmark_group("decompose");
    group.throughput(Throughput::Elements(1));
    for (name, record) in regular_records() {
        let event = copy_record(&pool, &record);
        pipeline.parse_encode_retained(&event);
        group.bench_function(BenchmarkId::new("parse_encode_retained", name), |b| {
            b.iter(|| black_box(pipeline.parse_encode_retained(black_box(&event))).len());
        });
    }
    for (name, record) in [
        ("query_256", Record::query()),
        ("response_257", Record::response()),
    ] {
        group.bench_function(BenchmarkId::new("recycle", name), |b| {
            b.iter_batched(
                || copy_record(&pool, &record),
                |event| black_box(event).recycle(),
                BatchSize::NumIterations(BATCH),
            );
        });
    }
    group.finish();
}

// 200 samples of 10 s tighten the within-run interval; the 2% noise threshold
// stops `--baseline` from flagging run-to-run drift of that size as a change.
criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(200)
        .measurement_time(Duration::from_secs(10))
        .noise_threshold(0.02);
    targets =
        copy_record_group,
        cold_pool_groups,
        socket_write_group,
        decode_group,
        encode_group,
        parse_and_encode_group,
        decompose_group
}
criterion_main!(benches);
