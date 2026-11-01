# Single-pass ETW decoder implementation plan

> **Status:** Current (implementation reference for the single-pass decoder added in `e59425f`) · **Evidence dated:** not applicable (local validation scope is stated in the document) · **Commit:** `e59425f` · **Reviewed:** 2026-11-01

This plan guides implementation and verification of a decoder that removes all
ten per-event TDH property calls from the supported, cached-schema path. TDH
still supplies schema metadata on a cache miss. The existing property-based
decoder remains the fallback for layouts the new decoder cannot prove safe.

## Stage 1: Preserve the baseline and define the contract

Preserve the dirty working tree, including direct packet encoding and shutdown
fixes. Baseline files, hashes, and the preexisting diff are recorded under
`target/single-pass-decoder/baseline/`. Do not change service configuration,
filters, pool/channel capacity, network behavior, metrics, or build hardening.
No deployment, VM workload, commit, or push is part of this task.

Use a safe private `src/etw/decode.rs` module. Normalize native field metadata
into owned cold-path descriptors. The initial shared interface is:

```rust
pub(super) struct FieldSpec {
    pub(super) name: String,
    pub(super) input: i32,
    pub(super) output: i32,
    pub(super) flags: u32,
    pub(super) count: u16,
    pub(super) length: u16,
}

impl DecodePlan {
    pub(super) fn compile(fields: &[FieldSpec], peer_name: &str) -> Option<Self>;
    pub(super) fn decode<'a>(&self, payload: &'a [u8])
        -> Result<Decoded<'a>, DecodeError>;
}

pub(super) struct Decoded<'a> {
    pub(super) tcp: &'a [u8],
    pub(super) port: &'a [u8],
    pub(super) local: &'a [u8],
    pub(super) peer: &'a [u8],
    pub(super) packet: &'a [u8],
}
```

`length` and `count` retain the validated metadata union values, interpreted
according to their flags. The integration must not interpret non-struct union
members for structured/custom fields. Unsupported or ambiguous metadata yields
no plan and uses the original fallback. Owners must agree before changing this
interface and notify the verification owner.

## Stage 2: Compile a bounded schema plan

At schema load, retain field order, input/output types, flags, count, and length
dependencies, not observed event offsets. Accept only supported scalar layouts
and bounded metadata. Plan size and any dependency storage have explicit limits;
larger or unsupported layouts use fallback. Reject ambiguous duplicate target
names, invalid/forward length dependencies, unsupported arrays/structures,
custom schemas, and unknown layout-affecting flags from the fast path.

Support the saved event 256 and 257 templates, including their preceding ANSI
and UTF-16 strings, fixed-width integers, and binary PacketData whose length
comes from BufferSize. Never assume addresses occupy 4 or 16 event bytes when
the schema says text. Select Source or Destination by the existing event rule.
Known binary addresses can be supported only with proven fixed widths; other
representations use fallback. Schema compatibility, not a guessed OS offset,
determines eligibility.

## Stage 3: Decode and integrate

Walk the payload once using checked byte slices and little-endian scalar reads.
Find ANSI/UTF-16 terminators within the supplied payload; UTF-16 scans advance
in two-byte units relative to the field start, without assuming pointer alignment.
Resolve binary lengths only from validated earlier scalar dependencies. Read
only through the last required field; do not mistake bytes after PacketData
for packet content. No per-event allocation, native TDH property call, or
intermediate packet vector is allowed on the supported success path.

Return borrowed field slices, retaining terminators where the existing address
validator expects them. Reuse existing integer, address, unavailable-endpoint,
timestamp, packet-length, and QR checks. Keep the 512-byte address bound,
12..=65,535-byte packet bound, and 70 KiB encoded-output bound. Fill the final
protobuf destination by copying the borrowed packet slice once. The pooled
event must remain alive through that copy and be released before TCP output.

A malformed event under a supported plan is rejected; it must not silently
switch to fallback. An unsupported schema uses the original TDH extraction
path. Move schema ownership into Parser if needed to borrow plans without a
per-event Arc allocation/refcount operation. Preserve the bounded cache and
all source filters, ledger meanings, and failure clearing behavior.

## Parallel ownership and model selection

| Task | Model and effort | Files | Required return |
| --- | --- | --- | --- |
| Native metadata adapter and parser integration | Sol, medium | `src/etw.rs` (now `Parser` in `src/etw/schema.rs`) | Safe metadata normalization, fallback routing, pooled lifetime, zero-call integration evidence |
| Safe plan compiler and decoder | Sol, medium | `src/etw/decode.rs` | Supported grammar, bounds, rejection rules, allocation behavior |
| Independent fixtures and regression coverage | Sol, medium | `src/etw/decode_tests.rs` and test fixtures | Independent encoded event fixtures, field/protobuf parity, malformed and schema rejection coverage |
| Maintained documentation | Sol, low | README and parser references, after interfaces settle | Accurate supported/fallback distinction and evidence limits |
| Independent security/correctness review | Sol, high | Read-only integrated review | Severity-ranked findings, fixes required, final review disposition |

The decoder owner declares `#[cfg(test)] #[path = "decode_tests.rs"] mod tests;`
in the decoder module. The integration owner may add parent-module integration
tests. All Cargo commands belong to the coordinator to avoid conflicting runs.

## Stage 4: Verify against the contract

Require these checks before completion:

- Both recorded 256/257 layouts produce correct fields and byte-identical
  protobuf output against an independent reference across query/response,
  IPv4/IPv6 text, UDP/TCP, variable string lengths, and repeated buffer reuse.
- Packet bytes are borrowed from the pooled payload and copied directly into
  final output. Warmed successful decoding/encoding does not allocate.
- Instrument TDH wrappers in tests only to prove cached supported events call
  neither property API nor metadata API. Prove unsupported layouts route to
  fallback and supported malformed payloads do not.
- Cover truncation at meaningful boundaries, unterminated strings, odd UTF-16
  tails, zero/oversized/inconsistent packet lengths, invalid scalar values,
  invalid dependency indices and flags, duplicate required fields, and bounds.
- Exercise schema normalization and reject malformed native metadata without
  out-of-bounds access. Verify pool recycling and stale-output rejection.
- Where supported locally, use a process-local TDH test manifest to compare
  valid fixtures against real TDH extraction. Keep it isolated from live
  services, unload it, and label fixture scope. If unavailable, report that
  limitation explicitly rather than claiming native equivalence.

Run locked tests, strict all-target Clippy, formatting and diff checks, focused
unwind-enabled release tests, and focused AddressSanitizer tests. Use the
installed ASan runtime through the test shell PATH. Build the ordinary hardened
release separately to verify production compilation. Preserve logs under
`target/single-pass-decoder/`. Obtain independent review, fix required findings,
and repeat affected checks.

## Stage 5: Document and close

Update maintained parser documentation with the fast-path grammar, fallback,
remaining copies, and validation scope. Preserve historical plans/results and
link this plan as the current contract. Record each acceptance gate and its
actual evidence here. Local tests establish implementation behavior, not live
throughput or resolution of the 100,000-QPS campaign's capture deficit.

## Completion record

All five stages completed on October 30, 2026. Implementation and independent
fixture work ran in parallel on Sol at medium effort; documentation used Sol at
low effort. Sol at high effort performed the independent correctness, security,
and performance review. Its required fixes are implemented, regression-tested,
and approved. No review findings remain unresolved.

| Acceptance gate | Result and evidence |
| --- | --- |
| Preserve existing work and scope | Baseline hashes confirm `src/dnstap.rs`, `src/pipeline.rs`, and `src/network.rs` are unchanged from task entry. Service settings, filtering, queue capacities, and build hardening are unchanged. |
| Bounded safe decoder | `src/etw/decode.rs` forbids unsafe code. Plans have at most 256 fields, checked slice offsets, bounded string scans, and validated scalar length dependencies. Native normalization bounds metadata and property names before compiling a plan. |
| Eliminate ten property calls | `cached_supported_parser_has_zero_tdh_calls_and_no_allocations` asserts zero metadata, size, and property calls on supported cache hits, including repeated warmed parsing. Cold metadata loading and unsupported-schema fallback remain. |
| Field and protobuf equivalence | `process_local_manifest_tdh_matches_borrowed_fields_and_parser_output` compares required fields against actual TDH extraction and full protobuf bytes against the original fallback for both event layouts. Independent fixtures also cover text and binary addresses, UDP/TCP, variable prefixes, and buffer reuse. |
| Ownership and allocation | Successful warmed decoding and encoding allocate nothing in the focused tests. Packet bytes borrow the pooled payload until the single copy into final output; integration tests verify lease recycling. |
| Malformed input and fallback | Tests cover truncation, string termination, invalid schema flags/dependencies, duplicate names, native bounds, invalid scalars and addresses, packet length, QR direction, timestamps, and output reset. Supported malformed events reject without TDH; unsupported layouts retain fallback. A subsequent valid event still encodes correctly. |
| Review fixes | Unspecified zero-length binary fields now disable plan compilation. Unsupported trailing fields retain names without interpreting native unions, allowing safe prefix decoding while still rejecting duplicate target names. Both have regression coverage. |
| Maintained documentation | README, pooled-intake documentation, and the documentation index describe the current path. Historical parser plans link to this contract and retain their original evidence scope. |

The native differential test loads and unloads a process-local manifest with
the saved 256/257 field layouts and test event version 250. It exercises real
TDH APIs using synthetic records; it does not capture live DNS Server traffic
or modify provider registration or service configuration.

| Verification | Result |
| --- | --- |
| `cargo test --locked` | 80 passed; 1 manual intake benchmark ignored |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed |
| `cargo fmt --check` | Passed |
| `git diff --check` | Passed |
| `cargo test-release --lib etw::` | 41 passed; 1 ignored; 39 filtered out |
| `cargo asan --lib etw::` | 41 passed; 1 ignored; 39 filtered out |
| `cargo build --release --locked` | Passed for the ordinary production profile |
| Production PE inspection | x64, static CRT, ASLR, high-entropy VA, NX, CFG, CET, security cookie, and EH continuation metadata verified |

AddressSanitizer covers the instrumented Rust test graph and rebuilt standard
library. Windows system DLLs, including TDH, are not instrumented. The release
test profile uses unwinding for tests; the separately built production binary
retains the configured abort behavior.

Logs, baseline evidence, PE inspection, source hashes, and a copy of the
production binary are under `target/single-pass-decoder/`. The binary's SHA-256
is `EB70865D5D36A6E19868E87F2F7A4CAEE2D260B588DD39983097EA51E52803ED`.
These local artifacts are build output, not committed documentation assets.

The binary has not been deployed as part of this task. No VM stress test was
run. CPU savings, maximum sustainable QPS, and any change to the earlier
collector deficit remain unmeasured.
