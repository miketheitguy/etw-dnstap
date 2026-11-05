# Single-pass decoder: current contract

> **Status:** Current · **Describes:** `src/etw/decode.rs`, `src/etw/schema.rs`, and the encoder bounds in `src/dnstap.rs` on branch `spool-encryption` as of 2026-11-05 · **Evidence dated:** not applicable (reference, local tests only) · **Reviewed:** 2026-11-05

This page states the rules that the code enforces today. For the ownership of pooled
events, read [Pooled ETW intake](pooled-intake.md). The
[implementation plan and completion record](../history/single-pass-decoder-plan.md) is
historical. Earlier builds decoded unsupported schemas with per-event TDH property calls.
The [CHANGELOG](../../CHANGELOG.md) records that removal.

## Which events are decoded

- The parser decodes events 256 (query received), 257 (response success), and 258
  (response failure) from the DNS Server provider. It captures no other events.
- For event 256, the peer address is the `Source` property. For events 257 and 258, it is
  the `Destination` property. Event 258 has a `Reason` string before the peer. The plan
  walks past it and keeps none of it.
- The parser caches schemas keyed by event ID and version. The cache limit is in
  [Bounds](#bounds). When the cache is full, the parser clears it.

## Supported schemas

Every captured event is decoded by a compiled plan. There is no per-event TDH reader. TDH
runs only when a schema key misses the cache. Then two `TdhGetEventInformation` calls read
the manifest metadata that the plan is compiled from. The decision is made when the schema
loads.

A schema is unsupported when its plan does not compile. That happens when any of these is
true:

- The schema has more properties than the limit in [Bounds](#bounds), or a property name
  is longer than its limit.
- A required name appears more than once, or is missing. The required names are `TCP`,
  `Port`, `InterfaceIP`, the peer name, and `PacketData`. The repeat check covers the whole
  schema, not only the prefix described below.
- A property in the prefix is an array (count other than 1), a structure, or a property with a
  custom schema. A property in the prefix that has a flag other than
  `PropertyParamLength` or `PropertyParamFixedLength` also makes the schema unsupported.
  Setting both of these flags also makes it unsupported. The prefix runs from the first
  property through the last required property.
- A property in the prefix has a layout that the grammar below does not allow.

Events are rejected in these cases:

- **Unsupported schema.** The event is rejected and counted in `schema_rejected` and in
  `rejected`. The same metadata gives the same result, so the parser does not retry the
  key. The parser logs a warning when it loads the schema, and it remembers the key. Later
  events with a remembered key are rejected without a TDH call. The list of remembered keys
  is bounded. When it is full, the parser clears it, and a forgotten key loads and warns
  again on its next event.
- **Metadata load failure.** A `TdhGetEventInformation` call returns a Win32 error. The
  event is rejected and counted the same way. The key is remembered with a retry budget of
  `SCHEMA_RETRY_EVENTS` (65,536) rejected events. Each rejected event of that key uses one
  event of the budget. The first event after the budget is spent loads the schema again. The
  budget counts events, not time, so the path reads no clock. If the load succeeds, the key
  leaves the remembered list and the schema enters the cache. If the load fails again, the
  key gets a new budget, and the failed load counts in `schema_rejected`. If the retried
  metadata cannot be used by this program, the key becomes an unsupported key. The warning for
  a load failure is logged for the first four failures of each key, then for the 61st, 121st,
  and so on.
- **Malformed payload under a plan.** A payload that does not decode under a compiled plan is
  rejected. There is no TDH retry.

## Grammar of the fast path

The plan includes the properties from the first one through the last required property.
Properties after that point are not checked for layout. An unsupported trailing layout
stays as a name-only entry, and the decoder never reads its bytes.

| Native type | Accepted layout |
| --- | --- |
| `UINT8`, `INT8` | 1 byte (length 0 or 1) |
| `UINT16`, `INT16` | 2 bytes (length 0 or 2) |
| `UINT32`, `INT32`, `BOOLEAN` | 4 bytes (length 0 or 4) |
| `ANSISTRING`, `UNICODESTRING` | Terminated by the first zero unit: one byte for `ANSISTRING`, 16 bits for `UNICODESTRING` (length 0) |
| `BINARY`, fixed width | Nonzero length, or length 0 with the fixed-length flag |
| `BINARY`, length from a property | Length names an earlier unsigned scalar (`UINT8`, `UINT16`, or `UINT32`) |

Each required property has these further rules:

- `TCP` and `Port` must be unsigned integers or `BOOLEAN`. A `Port` with the port output type
  must be `UINT16`.
- `InterfaceIP` and the peer must be a string whose output type is not IPv4 or IPv6.
  Alternatively, they must be a fixed-width binary of 4 bytes with the IPv4 output type, or
  16 bytes with the IPv6 output type. Any other binary is rejected, which makes the schema
  unsupported.
- `PacketData` must be binary.

A length that comes from a property must name an earlier property. A forward or self
reference is rejected.

## Decoding rules

- The decoder reads the payload once, in order. It uses checked slices and does not allocate.
- Scalars are little-endian. A `Port` field is read as big-endian only when its output type is
  `PORT`, which requires `UINT16`. The DNS Server schema declares `Port` as `UINT32` with output
  `UNSIGNEDINT`, so it is read little-endian.
- A string is read up to its terminator. A UTF-16 scan starts at the offset of the field and
  moves in 2-byte units, even when that offset is odd. A missing terminator is an error.
- Decoding ends after the last required property. Bytes after `PacketData` are never read as
  packet content.
- The decoder forbids unsafe code (`#![forbid(unsafe_code)]` in `src/etw/decode.rs`).

## Checks after decoding

The parser applies these checks after it reads the fields:

- `TCP` must be 0 or 1.
- `Port` must fit in 16 bits. A value of 0 means that the port is absent.
- An address must be within the limit in [Bounds](#bounds). The text before any `%` scope
  suffix must parse as an IP address. An unspecified address (`0.0.0.0` or `::`) means that
  the endpoint is absent.
- The timestamp must convert to a time at or after the Unix epoch.
- The packet length must be within the DNS packet bound in [Bounds](#bounds). The encoder
  checks this.
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

The local tests cover the layouts, the rejection rules, the retry rules, and golden protobuf
output. They do not measure live throughput or CPU use. The capture deficit in the
[four-CPU test results](../history/singlepass-four-core-test-results.md) is not resolved by
these tests.
