# Remove the duplicate schema lookup

Implement recommendation 1 from the [parser/decoder review](parser-decoder-performance-review.md): use one schema lookup on a warm cache hit in `Parser::parse`. Preserve the existing working tree. Do not deploy, change VM state, run a workload, or perform file-hash checks.

## Stage 1: preserve the contract

The coordinator saves the pre-change `src/etw.rs` under `target/analysis/schema-single-lookup/baseline/`. The implementation must preserve:

- Provider and event ID filters, the complete `(ID, version)` key, and output clearing before any failure or ignored event.
- Owned schemas, the default hasher, supported fast decoding, and cached unsupported-layout fallback.
- A 32-entry cache: a hit never clears it; a miss loads successfully before clearing and inserting. Failed metadata loading preserves existing entries.
- Borrowed schema and payload access, pooled event lifetime, zero TDH calls and no allocations on warmed supported paths.
- Existing errors, packet/protobuf validation, and service behavior.

Decoder instructions, cache representation, output ownership, and other recommendations remain outside this change.

## Stage 2: implement and verify behavior

Delegate implementation and focused regression coverage to **Sol, low effort**. The agent owns `src/etw.rs` and any narrowly required test module. On a hit, perform one `get` and dispatch using the borrowed schema. Keep metadata loading, capacity handling, insertion, and any post-insertion lookup on the miss path. Share fast/fallback dispatch through a small helper if borrowing requires it; do not clone schemas or add reference counting or unsafe code.

Extend existing fixtures only where coverage is missing. Verify full-cache hits, alternating ID/version keys, failed metadata load at capacity, successful replacement at capacity, and cached unsupported schemas. Reuse the native manifest parity test where practical rather than adding production loader abstractions solely for tests. Existing allocation/TDH and malformed-recovery tests remain acceptance gates.

Return the exact change summary, tests added, behavior preserved, and any uncertainty. Keep all Cargo commands under this agent's ownership until handoff; preserve logs under `target/analysis/schema-single-lookup/`.

## Stage 3: run local checks

Run `cargo fmt`, `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, and `cargo test --locked`. Run the existing focused release test alias for the ETW module if needed to confirm optimized-path behavior. No VM stress test, deployment, or performance-gain claim belongs to this task.

## Stage 4: independent review

Delegate review to **Astra, high effort** after the implementation is ready. Compare against the saved baseline and recommendation 1. Review cache hit/miss semantics, eviction ordering, errors and stale output, borrowing/lease lifetime, fast/fallback parity, test strength, and scope. Confirm the warm hit contains one lookup. Return severity-ranked findings and an approval disposition. The implementer fixes required findings; rerun affected checks and obtain final review.

The coordinator verifies the review and check receipts, records completion here, and reports the local changes. Neither a source-level lookup reduction nor passing tests establishes measured CPU savings.

## Completion record

Completed on 2026-10-30. Sol at low effort implemented the change in
`src/etw.rs`; Astra at high effort reviewed the task delta against the saved
baseline and recommendation 1. The coordinator inspected the delta and check
logs. No application files other than `src/etw.rs` were changed for this task.

The warm path performs one `get` and calls a shared borrowed-schema dispatch
helper. The miss path retains successful metadata loading before cache clearing,
insertion, and a post-insertion lookup. The default hasher, complete key, filters,
initial output clearing, fast/fallback behavior, and event lifetime are preserved.

Regression coverage verifies a full cache across alternating IDs and versions
with different TCP field widths, no TDH calls or allocations on those warm hits,
preservation after a failed metadata load, recovery, and replacement after a
successful miss at capacity. The native manifest test retains byte-exact
fast/fallback parity. Review identified an initial test that varied payloads
without varying schemas; distinct unsigned 8-bit and 32-bit TCP layouts resolve
that finding. Final review approved the change with no outstanding findings.

| Check | Result |
| --- | --- |
| `cargo fmt` and `cargo fmt --check` | Passed |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed |
| `cargo test --locked` | 81 passed, 1 manual benchmark ignored |
| `cargo test-release etw::tests::` | 29 passed, 1 ignored, 52 filtered out |
| `git diff --check` | Passed |

Logs and the review receipt are retained under
`target/analysis/schema-single-lookup/`. No deployment, VM workload, commit,
push, or file-hash check was performed. CPU savings have not been measured.
