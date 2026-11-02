//! Criterion benchmark of the per-event hot path: the deep copy into a pooled
//! slot (ETW thread), then decode, encode, and their production composition
//! (parser thread). It needs the `bench` feature and a standard library built
//! for unwinding, which the alias in `.cargo/config.toml` provides:
//!
//! ```text
//! cargo bench-hot-path
//! ```
//!
//! The inputs are deterministic: a 60-byte A query (event 256), a 100-byte
//! response (event 257), and a 4,096-byte response that needs a jumbo slot.
//! Every measured parse is a schema-cache hit.

use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use etw_dnstap::bench::{Pipeline, Plan, Pool, Record, copy_record};

/// Leases outstanding at once in `parse_and_encode`. The benchmark pool holds
/// 2,048 regular slots, so a batch never exhausts it.
const BATCH: u64 = 1_024;

fn bytes(slice: &[u8]) -> u64 {
    u64::try_from(slice.len()).expect("slice lengths fit u64")
}

/// `copy_record`: acquire a slot, copy the user data, and (on drop) recycle the
/// slot. Production splits the copy and the recycle across the ETW and parser
/// threads; here one iteration pays for both.
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

/// `DecodePlan::decode` with the plan compiled from the saved template.
fn decode_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode");
    for (name, record) in [
        ("query_256", Record::query()),
        ("response_257", Record::response()),
    ] {
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
    for (name, record) in [
        ("query_256", Record::query()),
        ("response_257", Record::response()),
    ] {
        let event = copy_record(&pool, &record);
        let decoded = pipeline.parse(&event);
        group.bench_function(name, |b| {
            b.iter(|| black_box(pipeline.encode(black_box(&decoded))).len());
        });
    }
    group.finish();
}

/// `pipeline::parse_and_encode` on a schema-cache hit. The copy that makes each
/// input runs in the batch setup, outside the measurement.
fn parse_and_encode_group(c: &mut Criterion) {
    let pool = Pool::new();
    let mut pipeline = Pipeline::new();
    let mut group = c.benchmark_group("parse_and_encode");
    group.throughput(Throughput::Elements(1));
    for (name, record) in [
        ("query_256", Record::query()),
        ("response_257", Record::response()),
    ] {
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

criterion_group!(
    benches,
    copy_record_group,
    decode_group,
    encode_group,
    parse_and_encode_group
);
criterion_main!(benches);
