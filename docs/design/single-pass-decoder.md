# Single-pass decoder: current contract

> **Status:** Current · **Describes:** the code at commit `58fde72` (decoder added in `e59425f`, TDH fallback removed in `ee94de6`) · **Evidence dated:** not applicable (reference, local tests only) · **Reviewed:** 2026-11-03

This page states the rules that the code enforces today. It covers the fast path in
`src/etw/decode.rs`, the schema compiler in `src/etw/schema.rs`, and the encoder bounds
in `src/dnstap.rs`. For ownership of pooled events, read
[Pooled ETW intake](pooled-intake.md). The
[implementation plan and completion record](../history/single-pass-decoder-plan.md) is
historical.

## Which events are decoded

- The parser decodes only events 256 (query received), 257 (response success), and 258
  (response failure) from the DNS Server provider. It does not capture other events.
- For event 256, the peer address is the `Source` property. For events 257 and 258, it
  is the `Destination` property. Event 258 has a `Reason` string before the peer. The
  plan walks it and does not keep it.
- The parser caches schemas keyed by event ID and version, up to the limit in
  [Bounds](#bounds). When the cache is full, the parser clears it.

## Supported schemas

Every captured event is decoded by a compiled plan. There is no per-event TDH reader.
TDH runs only when a schema key is not in the cache. Then two `TdhGetEventInformation`
calls read the manifest metadata that the plan is compiled from. The decision is made
when the schema loads.

A schema is unsupported when its plan does not compile. That happens when any of these
is true:

- The schema has more properties than the limit in [Bounds](#bounds), or a property
  name is longer than its limit.
- A required name appears more than once, or is missing. The required names are `TCP`,
  `Port`, `InterfaceIP`, the peer name, and `PacketData`. The repeated-name check covers
  the whole schema, not only the prefix described below.
- A property in the prefix is an array (count other than 1), a structure, a custom
  schema, or has a flag other than `PropertyParamLength` or `PropertyParamFixedLength`.
  Setting both of these flags also makes the schema unsupported.
- A property in the prefix has a layout that the grammar below does not allow.

Events are rejected in these cases:

- **Unsupported schema.** The event is rejected and counted in `schema_rejected` and in
  `rejected`. The same metadata gives the same result, so the parser does not retry the
  key. The parser logs one warning for each key and remembers failed keys up to the
  limit in Bounds. When the list is full, the parser clears it. A dropped key can load
  and warn again on its next event. Later events with a remembered key are rejected
  without a TDH call.
- **Metadata load failure.** A `TdhGetEventInformation` call returns a Win32 error. The
  event is rejected and counted in the same counters. The key is remembered with a retry
  budget of `SCHEMA_RETRY_EVENTS` events. Each later rejected event of the key uses one
  event of the budget. The first event after the budget is spent loads the schema again.
  The budget counts events, so no clock is read. If the load succeeds, the key leaves the
  list and the schema enters the cache. If it fails again, the key gets a new budget, and
  the failed load counts in `schema_rejected`. If the retry finds metadata that this
  program cannot use, the key becomes an unsupported key. The warning for a load failure
  is logged for the first four failures of each key, then for the 61st, 121st, and so on.
- **Malformed payload under a plan.** A payload that does not decode under a compiled plan
  is rejected. There is no TDH retry.

Earlier builds decoded unsupported schemas with per-event TDH property calls. The
[CHANGELOG](../../CHANGELOG.md) records the removal.

## Grammar of the fast path

The plan includes the properties from the first one through the last required property
(the prefix). Properties after the prefix are not checked for layout. Unsupported
trailing layouts can stay as names-only sentinels, and the decoder does not inspect
their bytes.

| Native type | Accepted layout |
| --- | --- |
| `UINT8`, `INT8` | 1 byte (length 0 or 1) |
| `UINT16`, `INT16` | 2 bytes (length 0 or 2) |
| `UINT32`, `INT32`, `BOOLEAN` | 4 bytes (length 0 or 4) |
| `ANSISTRING`, `UNICODESTRING` | Terminated by the first zero unit: one byte for `ANSISTRING`, 16 bits for `UNICODESTRING` (length 0) |
| `BINARY`, fixed width | Nonzero length, or length 0 with the fixed-length flag |
| `BINARY`, length from a property | Length names an earlier unsigned scalar (`UINT8`, `UINT16` or `UINT32`) |

Each required property has these further rules:

- `TCP` and `Port` must be unsigned integers or `BOOLEAN`. A `Port` with the port output
  type must be `UINT16`.
- `InterfaceIP` and the peer must be a string whose output type is not IPv4 or IPv6.
  Alternatively, they must be a fixed-width binary of 4 bytes with the IPv4 output type,
  or 16 bytes with the IPv6 output type. Any other binary is rejected, which makes the
  schema unsupported.
- `PacketData` must be binary.

A length that comes from a property must refer to an earlier property. A forward or self
reference is rejected.

## Decoding rules

- The decoder reads the payload once, in order. It uses checked slices and does not
  allocate.
- Scalars are little-endian. A `Port` field is read as big-endian only when its output
  type is `PORT`, which requires `UINT16`. The DNS Server schema declares `Port` as
  `UINT32` with output `UNSIGNEDINT`, so it is read little-endian.
- A string is read up to its terminator. A UTF-16 scan starts at the offset of the field
  and moves in 2-byte units, even when that offset is odd. A missing terminator is an
  error.
- Decoding ends after the last required property. Bytes after `PacketData` are never read
  as packet content.
- The decoder forbids unsafe code (`#![forbid(unsafe_code)]`).

## Checks after decoding

The parser applies these checks after the fields are read:

- `TCP` must be 0 or 1.
- `Port` must fit in 16 bits. A value of 0 means that the port is absent.
- An address must be within the limit in Bounds. The text before any `%` scope suffix
  must parse as an IP address. An unspecified address (`0.0.0.0` or `::`) means that the
  endpoint is absent.
- The timestamp must convert to a time at or after the Unix epoch.
- The packet length must be within the DNS packet bound in Bounds. The encoder checks
  this.
- The DNS QR bit must match the event. Event 256 needs a query. Events 257 and 258 need a
  response. The encoder checks this.

## Bounds

| Bound | Value | Where it is enforced |
| --- | --- | --- |
| Schemas cached per parser | 8 | `src/etw/schema.rs` |
| Failed schema keys remembered | 8 | `src/etw/schema.rs` |
| Rejected events before a failed schema load is retried (`SCHEMA_RETRY_EVENTS`) | 65,536 | `src/etw/schema.rs` |
| Properties per plan | 256 | `src/etw/decode.rs`, `src/etw/schema.rs` |
| Property name length | 256 UTF-16 units | `src/etw/schema.rs` |
| Address length | 512 bytes | `src/etw/schema.rs` |
| Address text scratch | 768 bytes | `src/etw/schema.rs` |
| DNS packet length | 12 through 65,535 bytes | `src/dnstap.rs` |
| Identity | 1,024 bytes | `src/dnstap.rs` |
| Encoded output | 70 KiB (71,680 bytes) | `src/dnstap.rs` |

## Ownership and allocation

- `PacketData` borrows the pooled event payload. It is copied once into the final output.
  The pooled event is released before the TCP write.
- A schema-cache hit makes no TDH call.
- In the focused tests, warmed successful decoding and encoding allocate nothing. Cold
  metadata loads and error construction do allocate.

## Validation scope

The local tests cover the layouts, rejection, and retry rules above, with golden protobuf
output. They do not measure live throughput or CPU use. The capture deficit in the
[four-CPU test results](../history/singlepass-four-core-test-results.md) is not resolved
by these tests.
