# ETW subscription and intake filter plan

This plan guides the implementation and review of the DNS service's narrower
ETW subscription. The accepted intake event IDs become 256 and 257 only.

## Required behavior

- Set `EnableTraceEx2` level to 4 (informational).
- Set `MatchAnyKeyword` to `0x8000000000000000` as a 64-bit value.
- Keep `MatchAllKeyword` at zero and retain the provider GUID, timeout,
  session ownership, and native callback ABI.
- Return immediately for every event ID except 256 and 257, before callback
  context access, counters, thread-local producer access, pool acquisition,
  payload copying, queue admission, or TDH parsing.
- Preserve the provider GUID check and defensive null-record check.
- Keep the parser's defensive allowlist consistent with the callback.
- Treat deliberately excluded events as filtered, not overload drops or errors.
  Do not add per-event metrics or logging for the filter.

The subscription level is a maximum severity level, rather than an exact-level
match. Keyword-zero events can still pass ETW's keyword selection, so the
callback event-ID check remains necessary. These semantics follow Microsoft's
[EnableTraceEx2 documentation](https://learn.microsoft.com/en-us/windows/win32/api/evntrace/nf-evntrace-enabletraceex2).

## Implementation sequence

1. Record the existing worktree and source identity. Preserve the uncommitted
   parser optimization, TCP-only changes, and stopped campaign evidence.
2. Introduce clearly named subscription constants, or use the existing SDK
   informational-level constant with its correct type. Pass the exact requested
   mask and level to the existing enable call.
3. Narrow the shared intake predicate to 256 and 257. Move the header/provider
   check immediately after the defensive record-pointer check, ahead of
   dereferencing `UserContext`. Retain the parser's early predicate check.
4. Add focused callback regression coverage: both accepted IDs enqueue; formerly
   accepted 258/260/261 and unrelated boundary IDs return without touching the
   queue, consuming pool storage, or incrementing intake/drop/error counters.
   Include excluded records with invalid payload lengths or an exhausted pool
   to prove filtering precedes copying and leasing. Reuse the existing allocator
   fixture to verify the early-return path performs no heap operations.
5. Update the current README and intake documentation to describe the emitted
   client-only event set and exclusion of 258/260/261. Preserve historical
   reports and encoder compatibility fixtures; do not remove unrelated encoder
   mappings or alter previously recorded results.
6. Run formatting, locked compilation, strict all-target Clippy, and focused
   local callback/parser tests. Review the exact incremental diff against the
   pre-change source, including pointer-access order and counter semantics.
7. Record the completed changes, checks, and any limitations in this plan.

## Delegation and scope

GPT-6.1 Sol at medium reasoning effort owns implementation, focused tests,
current-reference documentation, and the completion record. The coordinator
reviews the resulting diff and evidence. This is a bounded FFI intake change;
no pool redesign or native allocation changes are needed.

The earlier live test campaign remains stopped. This task does not deploy to
the VMs, start DNS load, change collector configuration, or commit and push.
Local checks cannot establish live provider delivery under the new keyword
mask; that requires a separately authorized deployment and observation.

## Completion record

Implementation, local validation, and coordinator review are complete. The
coordinator found no correctness or safety issues in the production constants,
callback ordering, shared predicate, or regression coverage.

- `src/etw.rs` passes typed `u8` level 4 and `u64` keyword mask
  `0x8000000000000000` to `EnableTraceEx2`. Match-all zero, timeout 5000,
  provider identity, and callback ABI remain unchanged.
- The shared predicate accepts only 256 and 257. The callback checks the
  provider and ID immediately after its defensive null-record check, before
  dereferencing `UserContext`. The parser uses the same predicate before TDH.
- Four focused regression tests cover both accepted IDs; excluded IDs 0, 1,
  255, 258, 259, 260, 261, 262, and 65535; unchanged counters and pool capacity;
  exhausted storage; malformed pointer/length pairs; reentrant TLS; defensive
  null inputs; parser rejection before TDH; and zero allocator activity in
  measured callback/parser paths.
- README event mappings and the pooled-intake reference describe client-only
  capture and intentional exclusions. The documentation index links this plan.
  Legacy encoder mappings and historical campaign evidence remain intact.
- `cargo fmt`, `cargo fmt --check`, `cargo check --locked`, and
  `cargo clippy --locked --all-targets -- -D warnings` pass. The first Clippy
  pass identified an assertion-style lint in a regression test; it was fixed
  before the successful final pass.
- Focused ETW tests passed (23 passed, one manual benchmark ignored). Final
  `cargo test --locked --lib` passed (53 passed, one manual benchmark ignored).
  `git diff --check` passed, with existing line-ending conversion warnings.

Pre-change snapshots, incremental diffs, final check logs, and source hashes
are in `target/subscription-filter-validation`. The pre-change `src/etw.rs`
SHA-256 is `6f67326178d2cee56a8fcf181e0d73968c2ae893753d154fd879cb6b71084625`;
the validated source SHA-256 is
`2d601df9ea49679ffca021757e5733b849c49a3347513af0af45c09d8beff3fe`.

No live VM, DNS-load, deployment, or benchmark work was performed. No release
artifact was produced. Local fixtures verify filtering and resource accounting;
live provider delivery with this keyword mask remains unverified.
