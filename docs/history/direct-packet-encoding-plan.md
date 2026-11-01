# Direct PacketData encoding plan

> **Status:** Superseded by [Single-pass ETW decoder](../design/single-pass-decoder.md). This is the historical transition from an intermediate packet vector to direct encoding. · **Evidence dated:** not recorded · **Commit:** not recorded · **Reviewed:** 2026-11-01

This historical plan records the transition from an intermediate packet vector
to direct filling of the final protobuf packet range. The
[single-pass decoder contract](../design/single-pass-decoder.md) supersedes its parser
extraction path: supported schemas borrow packet bytes from the pooled payload
and copy them into that range; unsupported schemas retain TDH extraction.
Its test counts and results remain evidence for the earlier implementation,
not validation of the single-pass decoder or a live performance gain.

This plan gives maintainers the implementation and verification requirements
for removing the intermediate DNS packet vector. TDH must write `PacketData`
directly into the final dnstap protobuf output buffer. The existing pooled
copy made by the ETW callback remains necessary for asynchronous ownership.

## Behavior and interface

The encoder accepts decoded event metadata, a validated packet length, and a
synchronous filling callback with an initialized, exactly sized mutable byte
slice. The callback performs `TdhGetProperty` directly into that slice. It
must not retain the destination pointer. The encoder returns a borrowed output
slice only after extraction and validation succeed.

Use this private interface unless the owners agree on a justified adjustment:

```rust
fn encode_with(
    &mut self,
    event: RawDnsEvent,
    packet_length: usize,
    fill: impl FnOnce(&mut [u8]) -> anyhow::Result<()>,
) -> anyhow::Result<&[u8]>;
```

The parser accepts the encoder by mutable reference and returns an optional
slice borrowed only from the encoder. It retains the pooled event throughout
all metadata and packet TDH calls, then releases that event before the network
write. There is no standalone packet vector in the production pipeline.

Perform these operations for each accepted event:

1. Decode metadata using the existing schema cache and property scratch space.
2. Validate that `PacketData` is binary and obtain its length through TDH.
3. Validate metadata, packet bounds, and total protobuf size using checked
   arithmetic. Reserve all final output space before passing any slice to TDH.
4. Encode the outer envelope and nested metadata with the correct lengths.
   Prefer existing Prost encoding for metadata where it avoids duplicating the
   schema; handle the packet field and enclosing lengths directly.
5. Initialize the packet's destination range and invoke the filling callback
   exactly once. Do not allocate a packet-sized temporary or move packet bytes
   afterward to repair a length prefix.
6. Validate the DNS QR bit in place. Preserve the 12..=65,535-byte packet limit,
   1,024-byte identity limit, 70 KiB output bound, timestamp checks, endpoint
   mapping, optional-field presence, and all existing supported encoder IDs.
7. Return the complete output on success. Clear partial output on every error;
   no partial protobuf frame may reach the network.

Preserve byte-for-byte output compatibility with the independent Prost
reference where possible, including field order. Keep nanoseconds as fixed32.
Do not change the callback filter, ring/pool sizes, TCP behavior, shutdown
diagnostics, metrics, service configuration, or receiver configuration.

## Parallel ownership

The three implementation tasks use separate files:

| Task | Model and effort | Ownership | Return to coordinator |
|---|---|---|---|
| Direct encoder | Sol, medium | `src/dnstap.rs` production implementation and adaptation of its existing tests | Final helper contract, bounds and error behavior, exact copy removal, remaining allocations |
| TDH and pipeline integration | Sol, medium | `src/etw.rs`, `src/pipeline.rs`, their existing tests | Proof of pooled-event lifetime, exact native destination, unchanged counters and filters |
| Independent regression coverage | Sol, medium | New `src/dnstap/direct_tests.rs` | Independent wire reference, destination-pointer proof, error recovery, boundary and warmed-allocation checks |

The encoder owner declares the new test module and coordinates its interface
with both peers before edits depend on it. Preserve independent reference
serialization rather than rewriting the reference to duplicate the new path.
Do not erase existing validation coverage merely to accommodate the new API.

## Verification and independent review

Verify these properties locally:

- Query and response bytes match the independent Prost reference across IPv4,
  IPv6, unavailable endpoints, TCP/UDP, empty/maximum identity, supported event
  mappings, and packet/nested-message varint-length boundaries.
- The fill callback receives initialized storage whose address is inside the
  returned protobuf buffer. Its payload remains at that location unchanged.
- Invalid metadata or sizes reject before filling; partial fill plus error,
  wrong QR, and alternating failure/success cannot publish stale output.
- Repeated warmed encoding reuses the final output allocation. Test-only
  fixtures may own packets; production code must not retain the old packet
  vector or introduce an equivalent temporary buffer.
- Pooled storage survives the complete synchronous TDH extraction and is
  recycled on success, ignored events, and failures.

After integrating the parallel changes, run formatting, locked unit tests,
and strict Clippy with the pinned toolchain. Run the focused tests in the
repository's unwind-enabled release test profile and address sanitizer profile
where the existing local setup supports them. Record exact scope and limits
under `target/direct-packet-encoding/`; do not claim synthetic tests establish
live TDH allocation behavior or throughput.

Assign Sol at high effort to review the combined change independently. Review
native buffer initialization and bounds, borrowing and recycling, arithmetic,
wire tags and lengths, error publication, and test independence. Delegate any
required corrections to their owner, repeat affected checks, and obtain final
review approval.

## Documentation and operational scope

After implementation, update the maintained parser/encoder documentation to
describe direct filling and remaining copies. Preserve historical reports and
the preexisting uncommitted shutdown work. This task does not deploy a binary,
restart either VM service, change collector settings, or resume stress testing.

## Completion record

Implementation and local verification completed on October 30, 2026. Three
Sol agents at medium effort implemented the encoder, integrated TDH and the
pipeline, and wrote independent regression tests in parallel. A separate Sol
agent at high effort approved the combined code and security review with no
blocking findings. A Sol agent at low effort updated the maintained references.

Production parsing no longer owns a separate packet vector. Metadata uses
Prost, while the encoder writes the enclosing lengths and packet field framing.
TDH receives the initialized final packet range. This removes one packet-sized
temporary and the subsequent serialization copy. The callback's ownership copy,
TDH extraction copy, and destination initialization remain. Final output storage
can still grow until warmed; native TDH allocations and throughput are unmeasured.

The following local checks passed with the pinned toolchain:

| Check | Result | Local evidence |
| --- | --- | --- |
| `cargo fmt --check` and `git diff --check` | Passed | Coordinator output |
| `cargo test --locked` | 63 passed; one manual benchmark ignored | `target/direct-packet-encoding/tests.log` |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed, including production and test targets | `target/direct-packet-encoding/clippy.log` |
| `cargo test-release --lib dnstap::` | 11 passed | `target/direct-packet-encoding/release-encoder-tests.log` |
| Release scoped native-record lifetime test | One passed | `target/direct-packet-encoding/release-lease-test.log` |
| `cargo asan --lib dnstap::` | 11 passed | `target/direct-packet-encoding/asan-encoder-tests.log` |
| ASan scoped native-record lifetime test | One passed | `target/direct-packet-encoding/asan-lease-test.log` |

The initial timestamp fixture subtracted 1 ns from the Unix epoch. Windows
truncated that subtraction to zero; the corrected fixture uses one representable
100 ns tick. Production validation was unchanged. Strict Clippy also prompted
four test assertion formatting corrections.

The first ASan execution failed at startup with `STATUS_DLL_NOT_FOUND`. Adding
the installed MSVC runtime directory to the test shell's `PATH` resolved it:
`C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.51.36231/bin/Hostx64/x64`.
Both ASan checks then executed successfully. The release test alias enables
unwinding; it is not the distributable release configuration. These focused
tests use synthetic extraction callbacks and do not exercise live native TDH
extraction or instrument Windows system DLLs.

The independent review confirmed wire compatibility, checked lengths, initialized
native destinations, event ownership through synchronous extraction, and output
clearing on errors. It suggested optional coverage that primes an earlier encoded
payload before an ignored-event parser call. The parser already clears that
output and the pipeline never publishes ignored or failed events; no production
defect was found. That additional test was not added.

The worktree retains the earlier shutdown changes. No deployment, service
restart, collector change, or VM workload was performed for this change.
