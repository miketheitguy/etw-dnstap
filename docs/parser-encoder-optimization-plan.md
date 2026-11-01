# Parser and encoder optimization plan

This historical plan records the earlier reusable packet-vector implementation
for parser and encoder maintainers. Its frozen APIs, baseline hashes, and
validation results apply to that implementation. The
[single-pass decoder plan](single-pass-decoder-plan.md) describes the maintained
interface: production parsing consumes the pooled event and fills the final
protobuf packet range synchronously through `Encoder::encode_with`, without a
standalone packet vector. Supported schemas borrow packet bytes from the pooled
payload; unsupported schemas retain TDH extraction. The historical
[direct packet encoding plan](direct-packet-encoding-plan.md) records the earlier
TDH direct-fill transition. The evidence retained here does not validate either
later implementation or establish a live performance gain.

Use this implementation plan if you maintain the DNS Server Event Tracing for
Windows (ETW) parser and dnstap encoder. It defines the changes, ownership
contract, and local evidence needed to reduce repeated allocation while
preserving packet bytes, wire output, and the TCP service behavior.

This is an implementation plan, not evidence of measured throughput gains.
Windows performance counters V2 remain a separate documentation-only proposal;
do not implement its provider or expand its minimal nine-counter scope here.

## Preserve the starting worktree

The starting worktree contains uncommitted TCP-only service changes across Rust,
documentation, diagrams, and performance scripts. Preserve all of them: required
TCP receiver, no file output, no JSONL metrics, and no foreground CLI mode.
Do not reset files, commit, push, deploy, run a collector, or start VM workloads.

The implementation baseline is HEAD
`2c62a9ffe789e15f95f77e3d429c4bd625ca480f` plus those uncommitted edits.
The following SHA-256 hashes identify selected files before this plan is added:

| File | SHA-256 |
| --- | --- |
| `src/etw.rs` | `41508CE60A8DEBC48DEB197DE83D0EBCF445D9A88632B89C2E43B208455AA49A` |
| `src/dnstap.rs` | `5E55E8C13CF5D11B95C5F5B0A78AEE8457F30ABEED698C3C746AC76E2629A1E1` |
| `src/pipeline.rs` | `0E721737713E54BE21D3AFA6C9DD23EDAF6B971E488AFA921F9A5569D2D7F742` |
| `docs/windows-performance-counters-v2-plan.md` | `68A41F731EE647E9EFA29C13BCED709F8D13FE082961CB7249DE2038418BA293` |

Before implementation, retain `git status --short`, a diff of affected files,
and a file/hash inventory outside the tracked sources. Compare against that
snapshot during final review to distinguish task edits from existing work.

## Confirm the allocation sources

The parser calls `TdhGetPropertySize` and `TdhGetProperty` for TCP, Port,
InterfaceIP, Source or Destination, and PacketData. Each extraction allocates a
fresh vector. Unicode address conversion allocates a UTF-16 vector and string;
ANSI address conversion copies borrowed UTF-8 into a string.

The encoder consumes an owned packet vector, allocates address vectors and an
identity copy, constructs a Prost message, and calls `encode_to_vec` for each
event. The packet already moves into the message without an intermediate copy.
Preserve that property while reclaiming its allocation after serialization.

## Freeze the shared interface

These are the implementation interfaces, rather than descriptions of existing
APIs. The encoder owner defines `RawDnsEvent` in `src/dnstap.rs` with its existing
metadata fields and types, removing only `packet_data`. Keep `Transport` there.
The TDH owner imports those shared types and implements the parser interface.

The parser exposes the following API:

```rust
impl Parser {
    pub(crate) fn new() -> Self;
    pub(crate) fn parse(
        &mut self,
        event: RawEtwEvent,
        packet: &mut Vec<u8>,
    ) -> anyhow::Result<Option<RawDnsEvent>>;
}
```

The encoder exposes the following API:

```rust
impl Encoder {
    pub(crate) fn new(identity: &[u8]) -> Result<Self, EncodeError>;
    pub(crate) fn encode(
        &mut self,
        event: RawDnsEvent,
        packet: &mut Vec<u8>,
    ) -> Result<&[u8], EncodeError>;
}
```

The returned slice borrows only the encoder and remains valid until its next
mutable use. It must not borrow the event, native ETW record, or caller packet.
The caller owns the packet vector throughout the loop. On successful parse,
the vector contains that event's packet. At parser entry and on `Err` or
`Ok(None)`, clear its length so callers cannot reuse an earlier valid packet.

Encoding preserves the packet vector's contents and allocation on both success
and failure. Validate the envelope and metadata before moving the vector into
the active Prost field. Use `mem::swap` or equivalent ownership transfer, encode,
then restore that same vector before returning, including any encoding error.
Do not clone the packet or copy it into another intermediate packet buffer.
Prost's write into the serialized output remains necessary.

## Assign disjoint implementation ownership

The work starts only after this planning file is written. Use parallel agents
for the two independent implementation areas, then integrate and review them.
Choose model and effort for the ownership/FFI complexity of each task; record
the actual selection and rationale in the implementation receipt.

The following assignments prevent simultaneous edits to shared files:

| Owner | Exclusive files | Deliverable |
| --- | --- | --- |
| TDH parser agent | `src/etw.rs` | Reusable parser storage, address conversion, parser tests |
| Prost encoder agent | `src/dnstap.rs` | Metadata type, reusable encoder, encoder tests |
| Integrator | `src/pipeline.rs`, supporting test infrastructure if required | Persistent parser/encoder/packet and shared allocation measurement |
| Plan/review owner | This plan, optional `docs/README.md` link | Contract, evidence, scope review |

Neither implementation agent edits the other's files or changes Cargo features,
dependencies, callback logic, pooled intake, network sink, or service behavior.
Send proposed interface changes to the integrator before implementing them.
Run Cargo checks after integration to avoid reporting expected transitional API
errors as final failures or competing over formatting other agents' files.

## Implement reusable TDH storage

1. Introduce `Parser` storage on the existing parser thread; preserve the schema
   cache's manifest-name/type validation and `(event ID, version)` key.
2. Replace fresh small property vectors with initialized bounded storage.
   A reusable 512-byte scratch area covers existing address limits and the
   four-byte integer limits; consume each borrowed value before reuse.
3. Preserve both TDH calls and all returned-size/status checks. Pass exactly
   the validated length to TDH, never capacity, and expose no bytes on failure.
4. Read PacketData directly into the caller's reusable initialized vector after
   validating the existing 65,535-byte maximum. Preserve its 12-byte minimum.
   Avoid geometric growth beyond the permitted retention bound; reserve exact
   additional storage when growth is needed.
5. Preserve the event lease through all native calls and return it when parsing
   completes. Keep native views scoped; do not cache borrowed native pointers.
6. Parse ANSI addresses from borrowed UTF-8. Decode Unicode addresses with
   reusable bounded scratch or an initialized stack buffer, without allocating
   a fresh string or UTF-16 vector per event.
7. Preserve binary IPv4/IPv6 support, trailing-NUL validation, invalid UTF-16
   rejection, zone suffix removal, unspecified-address absence, and zero-port
   absence. Validate the full encoded text before removing the opaque zone
   suffix. Preserve baseline behavior: an interior NUL in the IP prefix is
   invalid; a validly encoded NUL inside the ignored zone suffix is tolerated.

Keep the existing 512-byte property limit and bounded schema cache. No unsafe
uninitialized vector length, alignment casts, speculative fixed offsets,
additional threads, callback parsing, telemetry, or hardcoded event layout.
Update every changed unsafe comment to cover initialization, length, ownership,
and the synchronous native call's lifetime.

## Implement reusable Prost storage

1. Validate and copy identity once in `Encoder::new`. Preserve the 1,024-byte
   limit and omit empty identity rather than emitting an empty optional field.
2. Retain one Prost top-level message, nested DNS message, serialized output
   vector, and reusable endpoint address storage. Keep Prost's generated
   serialization and all existing schema tags, enum values, and fixed32 fields.
3. Clear output length at each encode entry. Reset every per-event optional
   scalar, timestamp, address, and packet field before reuse so query/response
   alternation and unknown endpoints cannot leak earlier values.
4. Populate IPv4/IPv6 octets directly into reusable address vectors. Reclaim
   vectors when optional fields become absent; don't drop capacity on every
   present/absent transition.
5. Move the caller packet into exactly one active message field, serialize into
   retained output with `Message::encode`, then return the packet as specified
   by the shared contract. Keep the inactive packet field absent.
6. Bound retained storage: packet length at 65,535 bytes, identity at 1,024,
   address vectors at 16 each, and output at a conservative 70 KiB ceiling.
   Check `encoded_len` before reservation; use bounded exact reservation for
   growth. Test actual retained capacities and document allocator caveats.
7. Preserve existing validation for unsupported IDs, QR direction, packet
   bounds, timestamps, mixed address families, transport, and optional presence.

## Integrate the parser loop

1. Construct one parser, encoder, and reusable packet vector before draining the
   queue. Propagate identity-constructor errors through the existing result path.
2. Parse into the packet, encode accepted metadata, then call `write_frame`
   with the borrowed encoded slice before the next encoder mutation.
3. Preserve all existing counter increments, rejection log limits, ignored
   event behavior, error propagation, periodic flush, and STOP/FINISH exchange.
4. Remove obsolete allocating production helpers and adapt existing tests to
   the frozen API. Keep any independent reference encoder inside tests only.

The synchronous buffered TCP writer consumes its payload borrow during
`write_frame`; it must never retain the borrowed output after that call.

## Verify semantics and allocation behavior

Add focused tests that exercise reuse, rather than duplicating implementation:

- Compare independent golden wire bytes and decoded messages for every supported
  event ID, UDP/TCP, IPv4/IPv6, unknown endpoints, zero timestamps, and identity
  presence. Retain the existing fixed32 nanosecond wire golden.
- Encode a mixed sequence with one encoder: query, response, absent addresses,
  IPv6, validation failure, then a valid query. Assert exact packet bytes,
  optional-field absence, clean timestamps, and recovery after each failure.
- Exercise packet sizes 11, 12, 65,535, and 65,536, unsupported events, bad QR,
  mixed families, pre-epoch time, and oversized identity. Assert packet pointer,
  capacity, and bytes survive encoding, including rejected inputs.
- Exercise bounded TDH extraction success/failure and stale-buffer cleanup
  through a test seam if native DNS manifests aren't available locally. Keep
  production TDH calls unchanged; label seam coverage separately from native TDH.
- Exercise ANSI, UTF-16, binary addresses, zone suffixes, malformed lengths,
  terminators, interior NULs, invalid encoding, and alternating small/large data.
- Measure allocation/reallocation counts after warmup for encoder reuse and
  TDH scratch/address conversion separately. Reuse the existing thread-local
  counting allocator through a small test-only seam; don't add a second global
  allocator. Keep fixture creation, decoding, assertions, and diagnostics outside
  the measured region. Report growth and error-path allocations separately.

Run `cargo fmt --check`, `cargo check --locked`, `cargo test --locked`,
`cargo clippy --locked --all-targets -- -D warnings`, `cargo test-release`,
and `cargo asan`. The changed initialized native output buffers require the
ASan pass; state its actual instrumentation and native-fixture limitations.
Then run `cargo build --locked --release` to restore the production abort build
and inspect its PE for DEP, ASLR, high-entropy VA, CFG, CET compatibility,
security cookie, and EH continuation metadata using the established toolchain.

## Review and record the result

Review resulting code for extra packet copies, hidden per-event allocation,
stale optional fields, failed buffer restoration, native bounds/lifetimes, and
scope drift. Fix findings and rerun checks affected by those fixes. Record exact
commands, outcomes, test counts, allocation measurements, unavailable checks,
and reviewed file changes in the final receipt.

A complete single-pass schema decoder is a gated future candidate. It requires
schema proof across supported versions, independent equivalence tests, and a
representative benchmark before implementation. This work retains per-property
TDH lookup and makes no claim that lookup overhead or real workload throughput
improves. Local allocation evidence supports only the measured allocation claim.

## Implementation and local validation record

The parser, encoder, and integration changes implement the frozen interfaces.
The TDH and Prost agents ran in parallel using GPT-6.1 Sol at medium effort.
Their separate file ownership kept the native buffer work and Prost ownership
work independent; the integrator connected the APIs and ran one validation
sequence. An independent GPT-6.1 Sol reviewer at high effort approved the final
source checkpoint with no remaining correctness or security findings, including
the zone-suffix NUL compatibility correction.

The TDH implementation retains five size calls and five extraction calls.
It reuses initialized 512-byte property and 768-byte text scratch arrays and
fills the caller packet vector directly. The encoder retains identity, endpoint
vectors, message fields, and serialized output. It restores the caller's packet
after serialization, including the tested post-transfer error case.

Review clarified one compatibility detail in address conversion: validate all
UTF-8/UTF-16 before discarding the zone suffix, while preserving the old parser's
tolerance of validly encoded NULs inside that opaque suffix. Prefix NULs and
invalid encoded suffixes remain rejected. Tests cover both cases.

The following checks pass on the pinned Windows x64 toolchain:

| Command | Result |
| --- | --- |
| `cargo fmt --check` | Pass |
| `cargo check --locked` | Pass |
| `cargo clippy --locked --all-targets -- -D warnings` | Pass after empty-assert lint fixes and deriving `Copy` for metadata |
| `cargo test --locked` | 49 passed, zero failed, one ignored manual benchmark |
| `cargo test-release` | 49 passed, zero failed, one ignored manual benchmark |
| `cargo asan` | 49 passed, zero failed, one ignored manual benchmark |
| `cargo build --locked --release -v` | Pass; ordinary production abort profile rebuilt after unwind/ASan tests |
| `git diff --check` | Pass |

Logs are under `target/parser-encoder-optimization-validation`. ASan uses the
configured instrumentation for Rust dependencies and rebuilt standard library.
Its runtime directory comes from the installed Visual Studio Build Tools x64
compiler discovered through `vswhere`; the PATH change is scoped to that command
and restored afterward. Native Windows TDH internals remain uninstrumented, and
the extraction seam tests do not execute the actual DNS provider's TDH calls.

The allocation tests observe zero allocations, reallocations, and deallocations
for warmed property scratch extraction, mixed ANSI/Unicode address conversion,
reused packet extraction at 12/65,535/12 bytes, and the warmed encoder fixture.
The packet's initial 65,535-byte storage growth observes one allocation and
zero reallocations. Encoder golden/equivalence tests separately cover mixed
query/response, address families, absent fields, identity presence, and errors.
Error-path and cold-schema allocation totals are not measured; they remain
outside the zero-operation claims. No real throughput benchmark runs here.

The production executable is 1,065,472 bytes with SHA-256
`487FFD782B730B7E1A849626AF68BCB6C8A0EF2057ACB7D8E5D932B7EA49A1AE`.
`dumpbin /headers /loadconfig /dependents` observes x64, high-entropy VA,
ASLR, NX, CFG with 757 function targets, a security cookie, CET compatibility,
and 11 EH continuation entries. It shows no CodeView/PDB entry or ASan runtime
dependency. These artifact markers do not prove runtime CET enforcement or
instrumentation of every function.

The following hashes identify the locally verified Rust sources:

| File | SHA-256 |
| --- | --- |
| `src/etw.rs` | `6F67326178D2CEE56A8FCF181E0D73968C2AE893753D154FD879CB6B71084625` |
| `src/dnstap.rs` | `3AC788D62B80D382E7422CAB9EFE636165FE8E2F9E11CCC0D83BA79BEC3DE678` |
| `src/pipeline.rs` | `DF1F817732842A65309F480647DD449002F15E4E9B077DA0C46451D9F23CB910` |

The Windows V2 counter proposal retains its initial hash and remains
documentation-only. No VM, collector, service deployment, performance campaign,
commit, or push forms part of this validation.
