# Parser and decoder performance review

> **Status:** Historical · **Evidence dated:** not recorded · **Commit:** not recorded · **Reviewed:** 2026-11-01

This review helps the Rust pipeline maintainer choose concrete changes to the
Event Tracing for Windows (ETW) parser and decoder. Start with a single
schema lookup on cache hits, then reduce the compiled decoder's dependency
bookkeeping. These changes remove identifiable repeated work while preserving
the existing bounded, allocation-free fast path. Their speedups remain
unmeasured. This is a recommendation report; it changes no application source
and performs no build, test run, workload, VM operation, or file-hash check.

## Evidence and limits

The performance evidence is the
[symbolized release WPR report](symbolized-release-wpr-results.md), checked
against its retained
[summary](../../target/analysis/symbol-wpr/executed-100000/analysis/summary.json) (local artifact)
and [exclusive CPU report](../../target/analysis/symbol-wpr/executed-100000/analysis/cpu-exclusive.txt) (local artifact).
Recommendations also use the working-tree source inspected on 2026-10-30.
The profiled artifact includes recorded working-tree modifications; its commit
ID alone is not the source identity. Source inspection does not establish that
every statement below has its own measured cost.

The parser uses **9.127795 seconds of DPC/ISR-exclusive CPU in a 57.994060-second
window**, or 15.74% of one logical CPU. Its regular sampled weight is 8.752382
seconds. Neither measurement establishes sustained CPU saturation. Device
interrupt execution, Ready time, and input waits are separate from parser
execution. This review does not analyze query counts, achieved rates, drops,
or collector counts.

The relevant regular sampled buckets are:

| Function or path | Leaf weight | Inclusive weight | Interpretation |
| --- | ---: | ---: | --- |
| `Parser::parse` | 1.325727 s | 5.552938 s | Large investigation target; includes inlined work, not just cache lookup |
| `DecodePlan::decode` | 1.317523 s | 1.615444 s | Strongest direct decoder target; no instruction-level breakdown |
| `Encoder::encode_with` | 0.335055 s | 1.122512 s | Adjacent work; resolved instantiation identifies the fast-path closure |
| Prost encoding paths | Not separately partitioned | 0.532239 s | Overlaps encoder inclusive weight |
| `etw::address` | 0.205468 s | Not separately partitioned | Address conversion deserves a later experiment |
| `read_ipv4_addr` | 0.212355 s | Not separately partitioned | Standard text parsing has observable CPU cost |
| `BuildHasher::hash_one` | 0.123906 s | Not separately partitioned | Supports investigating redundant hashing |
| Hash writer | 0.097740 s | Not separately partitioned | Does not establish the fraction removable by one lookup |
| `HashMap::contains_key` | 0.070623 s | Not separately partitioned | Additional evidence of the explicit cache-hit lookup |

Inclusive weights overlap and must not be added. Leaf buckets describe sampled
instructions under resolved symbols, not complete source operations. Inlining,
identical-code folding, incomplete stacks, and profiling overhead limit
attribution. The report records 1.280486 seconds of overlapping parser samples
containing trace stack-walk work. Microsoft distinguishes these sampled
estimates from scheduling-based precise CPU in its
[CPU analysis reference](https://learn.microsoft.com/en-us/windows-hardware/test/wpt/cpu-analysis).

For scale, hypothetical elimination of the entire decoder leaf bucket would
remove weight representing **15.05% of regular parser samples**, equivalent to
2.27% of one CPU over the window. A particular decoder rewrite removes only
some work, and related instructions can appear under other symbols. This is an
observed budget, not a predicted saving or a complete upper bound for every
decoder-related instruction. The parser leaf bucket similarly represents
15.15% of regular samples; it cannot all be assigned to cache lookup. These
figures do not justify an aggregate throughput or Amdahl speedup prediction.

## Existing fast path and cold paths

The design already avoids several common costs. Preserve these properties
rather than proposing them as improvements:

| Path | Existing behavior | Consequence for optimization |
| --- | --- | --- |
| Warm supported schema | `Parser::parse` borrows an owned cached plan; `DecodePlan::decode` returns payload slices; no TDH property calls | Do not add an `Arc`, clone a schema, or materialize a packet `Vec` per event |
| Schema miss | `load_schema` queries TDH, validates aligned and bounded metadata, normalizes fields, and compiles a plan | Name allocation and metadata validation are cold costs; optimize the repeated walk first |
| Unsupported layout | Cached schema retains `plan: None`; `parse_fallback` uses TDH property calls | This can be a steady path for an unsupported schema, even after metadata is cached; measure it separately |
| Malformed supported payload | Fast decode or semantic validation returns an error without retrying TDH | Never turn malformed data into an expensive fallback retry |
| Output ownership | Packet bytes remain borrowed from the pooled event until copied into the encoder's final range | The returned output borrows the encoder, and the event lease ends before return |

See `Parser::parse` and `parse_fast` (`src/etw.rs`),
`DecodePlan` (`src/etw/decode.rs`), and
`Encoder::encode_with` (`src/dnstap.rs`). The parser's text and property
scratch arrays are initialized at construction and reused. There is no
per-event text allocation to remove. The encoder also retains address and
output capacity after warmup.

## Ranked code changes

The ordering balances confidence in the repeated work, implementation risk,
and measured relevance. It is an experiment order, not a predicted ranking by
speedup.

### 1. Use one schema lookup on a warm hit

In `src/etw.rs`, `contains_key(&key)` is followed by
`get(&key)`. Replace that hit path with one `get` and dispatch using the
borrowed `Schema`. Put metadata loading, capacity handling, insertion, and the
post-insertion lookup on a separate miss path. A small helper that accepts
`&Schema`, the event, and disjoint mutable scratch fields can keep fast and
fallback dispatch shared without cloning the schema.

Keep the default hasher for this first experiment. Rust's
[HashMap documentation](https://doc.rust-lang.org/std/collections/struct.HashMap.html)
also provides an entry API, but an entry-based refactor must handle eviction
without retaining an entry borrow across `clear`. Do not clear a full cache
before checking for a hit: the existing policy clears only after a successful
schema load on a miss at the 32-entry limit. A metadata error must not evict
usable schemas.

Preserve the provider filter, IDs 256/257, the complete `(ID, version)` key,
cached unsupported schemas, and initial output clearing. This is the lowest
risk change with direct code and sample evidence. It removes a lookup at the
source level; it does not imply that hashing CPU is halved.

### 2. Compile only actual length dependencies

In `src/etw/decode.rs`, the walk initializes
`[0u32; 256]`, then decodes and stores **every unsigned field**, regardless of
whether a later binary field uses it. For the
saved query/response fixtures (`src/etw/decode_tests.rs`), the required
prefix contains 11/17 steps and performs 7/10 such stores. Only `BufferSize`
supplies a binary length in those fixtures. These counts describe fixtures,
not the observed runtime schema distribution.

Change compilation to gather all `BinaryFrom` sources in the required prefix,
assign each distinct source a dense slot, and record a save operation only on
those source steps. Encode the validated width as a one-, two-, or four-byte
little-endian read operation. Replace `Step::unsigned` with an optional
dependency-save slot and make `BinaryFrom` refer to the assigned slot. Repeated
consumers share a slot. A TCP or Port field used as a length source must still
save its raw unsigned length value independently of its semantic conversion.

For the saved fixtures, this changes dependency saves from seven or ten to
one. It does not eliminate string scans, payload bounds checks, or target
extraction. Retain validation of every prefix field before optimizing its
operation; a skipped unsupported layout must not become accepted.

Treat dependency storage as a separate experiment. Dense slot numbering alone
does not remove a full `[0; 256]` initialization. One safe option is an
initialized, bounded array owned by `Parser`, passed mutably to `decode` and
reused without per-event clearing. The compiler must prove that every consumed
slot has a strictly earlier source step, and that step writes the slot before
any consumer executes. A failed traversal exits before later reads. This
written-before-read invariant must hold across alternating schemas and errors;
stale initialized values must never substitute for a missing source.

That option moves roughly 1 KiB of scratch into the parser object rather than
reducing total storage. A small initialized inline buffer with a bounded
general path is another option if object footprint matters, but adds more
cases. Neither option requires uninitialized storage or unsafe indexing.
Inspect generated code before claiming that the original compiler emits a
1 KiB clear on every call, or that a proposed layout removes it. The source
initialization is visible; its machine cost is not isolated by the trace.

### 3. Reduce compiled walk operations

After dependency-only recording is measured, coalesce adjacent fixed fields
that are neither targets nor length sources into a `SkipFixed` operation.
The fixture's `QTYPE`/`XID` pair is a concrete example; response fields provide
additional opportunities. Use a checked sum during compilation and checked
payload slicing during execution. Preserve truncation rejection at any point
in the required prefix. Never coalesce across a variable string, binary
dependency, required target, or length source.

Then consider compiling TCP width, Port width/byte order, and address kind into
explicit operations. A typed result could return validated TCP and Port
values alongside borrowed endpoint and packet slices, avoiding the generic
`integer` dispatch in `parse_fast`. Keep Boolean validation (`0` or `1`), the
`u16` port bound, and the distinction between little-endian scalar lengths and
network-order `TDH_OUTTYPE_PORT`. Microsoft documents the relevant
[manifest input and output representations](https://learn.microsoft.com/en-us/windows/win32/wes/eventmanifestschema-inputtype-complextype).

These changes have more compiler-state and compatibility risk than the first
two recommendations. They need separate measurements. Do not replace the plan
with offsets learned from one payload: QNAME, endpoint text, and response
Scope/Zone/PolicyName lengths vary. The existing compiler already limits work
to the prefix ending at the last required field, while checking all names for
duplicate targets. Keep that behavior. TDH property flags select length-index
and union semantics; zero metadata length is not permission to guess offsets.
See Microsoft's
[EVENT_PROPERTY_INFO contract](https://learn.microsoft.com/en-us/windows/win32/api/tdh/ns-tdh-event_property_info).

### 4. Evaluate a bounded direct schema index

If lookup cost remains material after recommendation 1, the accepted IDs and
eight-bit versions permit a 512-entry index. An initialized `[u8; 512]` can use
zero for absent and 1–32 for entries in bounded owned schema storage. After
provider and ID validation, the index is `(id - 256) * 256 + version`. Clearing
the schema storage must clear the index in the same operation. This retains
the existing 32-schema cap and covers every version without hashing.

The index itself costs exactly 512 bytes, plus schema storage and its capacity.
Do not replace the cache with 512 full schemas or separately allocated plans
without explicitly accepting the larger memory and lifetime policy. A
last-schema accelerator uses less indexing storage but adds invalidation and
can miss frequently when query/response IDs alternate. It must store a safe
key/slot, not a raw pointer into a map that can move or clear. No observed
locality data establishes that it beats the direct index or the simpler map.

### 5. Specialize address conversion conservatively

`address` (`src/etw.rs`) already handles schema-declared four-byte IPv4
and sixteen-byte IPv6 values directly. Keep these representations and exact
length checks. Do not reinterpret text as binary based only on its length.
ANSI text already borrows payload bytes and uses the standard IP parser.

The UTF-16 path converts to reusable UTF-8 scratch before standard parsing.
An ASCII common path could validate code units and copy ASCII bytes directly
into that scratch, retaining the existing Unicode path when non-ASCII units
appear. Continue using safe UTF-8 validation and standard IP parsing initially.
Avoid introducing a custom IP grammar merely to remove a conversion pass.

Preserve validation of the **entire** string before stripping the opaque `%`
routing suffix. Valid non-ASCII suffix text is accepted by the existing path;
malformed UTF-8 or UTF-16 in the suffix is rejected. An ASCII shortcut that
stops validation at `%` changes behavior. Also retain odd-length rejection,
terminator rules, interior-NUL behavior, size bounds, unspecified-address
omission, and mixed-family rejection.

Terminator search is another independent experiment. The query/response
fixtures scan three/six strings, including fields that are skipped semantically
but still determine later offsets. An ANSI `memchr(0, ...)` may help long
strings; its own [documentation](https://docs.rs/memchr/latest/memchr/)
reports a latency tradeoff for tiny inputs. A byte search for `00 00` is not
automatically a UTF-16 terminator search: matches must align to code units
relative to the property's start, which can be an odd payload offset.
Keep the existing safe Unicode scan until a replacement proves parity.

## Smaller or deferred opportunities

The following candidates have weaker isolated evidence or a wider change surface:

| Candidate | Concrete possible change | Why it follows the parser work |
| --- | --- | --- |
| Timestamp representation | Carry validated Unix seconds/nanoseconds from `timestamp` to the encoder instead of constructing `SystemTime` and immediately recovering a duration | The round trip is visible in the timestamp conversion in `src/etw.rs` and in `src/dnstap.rs`, but no isolated timestamp cost is established; constant divisions and remainders may already be combined or strength-reduced |
| Fast-path packet copy | Add a borrowed-slice encoding entry point that reserves the bounded final output, appends packet bytes safely, and appends the suffix | `fill_message` resizes with zeros before the fast closure overwrites the packet; whether the zeroing survives optimization and matters is unmeasured |
| Metadata field encoding | Reduce repeated length/field dispatch only if encoder benchmarks identify it | Prost inclusive weight is smaller and overlaps the broader encoder bucket; the existing handwritten envelope already places packet bytes directly in the final output |
| Function layout or inlining | Inspect optimized assembly and experiment only on a demonstrated call/layout issue | Inlined frames and folded symbols can change attribution without changing total work; Rust's [inline attributes are hints](https://doc.rust-lang.org/reference/attributes/codegen.html) |
| Profile-guided optimization | Use representative compiler instrumentation after simpler changes stabilize | [Rust PGO](https://doc.rust-lang.org/rustc/profile-guided-optimization.html) requires an instrument/run/merge/rebuild workflow; a WPR sample export is not that input |

A timestamp refactor must preserve epoch rejection, checked range handling,
100 ns precision, and non-ETW encoder callers. Do not describe repeated source
arithmetic as repeated hardware division without inspecting assembly.

A slice-specific encoder can use safe `extend_from_slice` while keeping the
initialized `&mut [u8]` fill contract for TDH fallback. Retain full size
reservation before filling, DNS length and QR checks, error clearing, and the
pooled lease lifetime. Shared assembly logic should prevent two wire formats
from drifting. Do not use `set_len`, unchecked writes, or uninitialized slices
to remove zeroing. Keep proto2 presence, fixed32 nanoseconds, field numbers,
field order, direction, and byte-for-byte reference output. A broad custom
protobuf rewrite is not justified by this trace.

Do not prioritize removal of the duplicate `Vec::clear`: the early parser
clear protects schema/filter failures, and its cost is not established.
Likewise, defer a general endpoint cache, faster third-party hasher, SIMD
intrinsics, forced inlining, unchecked indexing, and extra workers. Each adds
complexity or changes resource behavior without evidence of a larger benefit
than the simpler candidates.

## Follow-up implementation and validation plan

The following stages describe future work; no tests or measurements are run
for this recommendation-only review. Before implementation, preserve the
reviewed baseline and use the existing fixtures and native manifest test as
correctness references.

1. **Establish focused measurements.** Add separate benchmarks for a warmed
   compiled decode, warmed parser plus encoder, cold schema compilation, and
   cached unsupported-schema fallback. Prepare payloads and plans outside the
   timed warm loops, retain reusable output capacity, and consume results so
   optimization cannot discard work. Measure elapsed time per synthetic
   operation and allocation activity without network output. Cover IDs 256/257,
   UDP/TCP, IPv4/IPv6, ANSI/Unicode/binary endpoints, varying QNAME and response
   field lengths, and small/large packets. Include mixed versions and malformed
   cases separately; do not let cold setup dominate the warm comparison.
2. **Implement the single-lookup cache change.** Verify same-key hits, alternating
   IDs/versions, cached `plan: None`, failed metadata loads, and the 32-entry
   boundary. If a direct index is evaluated later, exercise all 512 keys and
   invalidation on eviction. Compare this change independently of decoder work.
3. **Implement dependency-only operations.** Test one-, two-, and four-byte
   sources, shared sources, several independent dependencies, zero lengths,
   forward/self references, invalid sources, and the 256-field limit. For
   reusable scratch, alternate plans and force errors immediately before a
   dependency write, then verify a subsequent valid decode. Confirm that stale
   data never affects output. Measure before changing the operation walk.
4. **Implement fixed-run coalescing and typed operations separately.** Compare
   borrowed field slices and final bytes with the existing implementation.
   Truncate at every byte boundary in representative payloads. Test varying
   string lengths, odd-aligned Unicode starts, missing terminators, overflowed
   lengths, unsupported prefix layouts, unsupported trailing metadata, duplicate
   targets, host/network-order ports, and Boolean/range errors. Keep unsupported
   layouts on fallback and malformed supported payloads off TDH.
5. **Evaluate later candidates individually.** For address work, include valid
   and malformed full suffixes, IPv6 scope text, UTF-16 surrogate pairs, lone
   surrogates, invalid UTF-8, interior NULs, and exact binary widths. For encoder
   work, retain the independent byte-exact baseline, mixed-size reuse, failure
   recovery, absent endpoints, direction, timestamp, and output-size tests.
6. **Validate integrated behavior and CPU cost.** Run relevant existing tests,
   formatting, and strict lint checks for implemented changes. Confirm zero TDH
   calls and no heap operations on warmed supported parser paths, then run the
   process-local native manifest parity test against TDH. Only after those
   checks pass, compare optimized assembly and repeat controlled benchmark
   samples. In a subsequent authorized WPR campaign, compare regular parser
   samples and DPC/ISR-exclusive CPU using the same interval definitions; keep
   Ready/Wait and profiling overhead separate. Require a reproducible benefit
   without correctness or allocation regressions before retaining each change.

The existing coverage includes
`cached_supported_parser_has_zero_tdh_calls_and_no_allocations` (`src/etw.rs`),
`supported_malformed_rejects_without_tdh_but_unsupported_schema_uses_fallback` (`src/etw.rs`),
`process_local_manifest_tdh_matches_borrowed_fields_and_parser_output` (`src/etw.rs`),
the [decoder fixtures](../../src/etw/decode_tests.rs), and the
encoder's independent reference and reuse tests (`src/dnstap.rs`).
These are inspected coverage, not newly reported test passes. Extend them to
verify behavior at the changed boundaries, rather than asserting only that
the proposed internal operation list has a particular shape.

Microbenchmarks distinguish decoder, cache, conversion, and encoding costs
without the observed scheduler and interrupt interference. A later WPR
comparison checks whether those isolated changes matter in the integrated
pipeline. Neither alone supports an unmeasured throughput promise.
