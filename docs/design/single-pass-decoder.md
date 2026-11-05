# Single-pass decoder: current contract

> **Status:** Current · **Describes:** `src/etw/decode.rs`, `src/etw/schema.rs`, and the encoder bounds in `src/dnstap.rs` on branch `spool-encryption` as of 2026-11-05 · **Evidence dated:** not applicable (reference, local tests only) · **Reviewed:** 2026-11-05

This page states the rules that the decoder and the encoder enforce. For the ownership of
pooled events, read [Pooled ETW intake](pooled-intake.md).

## Which events are decoded

- The parser decodes events 256 (query received), 257 (response success), and 258
  (response failure) from the DNS Server provider.
- For event 256, the peer address is the `Source` property. For events 257 and 258, it is
  the `Destination` property. Event 258 has a `Reason` string before the peer, which the
  plan walks past and discards.
- The parser caches schemas keyed by event ID and version, up to the limit in
  [Bounds](#bounds). When the cache is full, the parser clears it.

## Supported schemas

A compiled plan decodes every captured event. TDH runs only when a schema key misses the
cache: two `TdhGetEventInformation` calls read the manifest metadata for the plan. A
schema is unsupported when its plan does not compile, because:

- The schema has more properties than the limit in [Bounds](#bounds), or a property name
  is longer than its limit.
- A required name is missing or appears more than once anywhere in the schema. The
  required names are `TCP`, `Port`, `InterfaceIP`, the peer name, and `PacketData`.
- A property in the prefix (the first property through the last required one) is an
  array (count other than 1), a structure, or a property with a custom schema.
- A prefix property has a flag other than `PropertyParamLength` or
  `PropertyParamFixedLength`, has both, or has a layout outside the
  [grammar](#grammar-of-the-fast-path).

Each rejected event counts in `rejected`. The causes are:

- **Unsupported schema.** The event also counts in `schema_rejected`. The parser logs a
  warning at the load and remembers the key, so later events with that key are rejected
  without a TDH call. When the remembered list is full, the parser clears it, and a
  forgotten key loads and warns again.
- **Metadata load failure.** A `TdhGetEventInformation` call returns a Win32 error. The
  event counts the same way, and the key is remembered for `SCHEMA_RETRY_EVENTS` (65,536)
  rejected events, a budget that needs no clock. The next event then loads the schema
  again. A successful load moves the key into the cache. A failed load counts in
  `schema_rejected` and starts a new budget. Unusable retried metadata makes the key an
  unsupported key. The warning is logged for the first four failures of each key, then
  for the 61st, 121st, and so on.
- **Malformed payload under a plan.** The event is rejected with no TDH retry.

## Grammar of the fast path

The plan covers the prefix. A property after it stays as a name-only entry, whatever its
layout, and the decoder never reads its bytes.

| Native type | Accepted layout |
| --- | --- |
| `UINT8`, `INT8` | 1 byte (length 0 or 1) |
| `UINT16`, `INT16` | 2 bytes (length 0 or 2) |
| `UINT32`, `INT32`, `BOOLEAN` | 4 bytes (length 0 or 4) |
| `ANSISTRING`, `UNICODESTRING` | Terminated by the first zero unit: one byte for `ANSISTRING`, 16 bits for `UNICODESTRING` (length 0) |
| `BINARY`, fixed width | Nonzero length, or length 0 with the fixed-length flag |
| `BINARY`, length from a property | Length names an earlier unsigned scalar (`UINT8`, `UINT16`, or `UINT32`) |

Each required property has these further rules:

- `TCP` and `Port` must be unsigned integers or `BOOLEAN`. A `Port` with the port output
  type must be `UINT16`.
- `InterfaceIP` and the peer must be a string whose output type is not IPv4 or IPv6, or a
  fixed-width binary of 4 bytes (IPv4 output type) or 16 bytes (IPv6 output type). Any
  other binary makes the schema unsupported.
- `PacketData` must be binary.

A length that comes from a property must name an earlier property. A forward or self
reference is rejected.

## Decoding rules

- The decoder reads the payload once, in order, with checked slices and no allocation.
- Scalars are little-endian. A `Port` field is read as big-endian only when its output type
  is `PORT`, which requires `UINT16`. The DNS Server schema declares `Port` as `UINT32` with
  output `UNSIGNEDINT`, so it is read little-endian.
- A string is read up to its terminator. A UTF-16 scan starts at the offset of the field
  and moves in 2-byte units, even when that offset is odd. A missing terminator is an
  error.
- Decoding ends after the last required property. Bytes after `PacketData` are never read
  as packet content.
- The decoder forbids unsafe code (`#![forbid(unsafe_code)]` in `src/etw/decode.rs`).

## Checks after decoding

The parser applies these checks after it reads the fields:

- `TCP` must be 0 or 1.
- `Port` must fit in 16 bits. A value of 0 means that the port is absent.
- An address must be within the limit in [Bounds](#bounds). The text before any `%` scope
  suffix must parse as an IP address. An unspecified address (`0.0.0.0` or `::`) means that
  the endpoint is absent.
- The timestamp must convert to a time at or after the Unix epoch.

The encoder then rejects the record when any of these is true:

- The event ID is unsupported.
- The DNS packet length is outside the bound in [Bounds](#bounds).
- The DNS QR bit disagrees with the event. Event 256 needs a query. Events 257 and 258
  need a response.
- The local and peer addresses are in different families.
- The identity or the encoded frame is larger than its bound.

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

In the focused tests, warmed successful decoding and encoding allocate nothing, while cold
metadata loads and error construction allocate. The packet copy and the buffer reuse are
in [Nonblocking admission and allocation scope](pooled-intake.md#nonblocking-admission-and-allocation-scope).

## Validation scope

The local tests cover the layouts, the rejection rules, the retry rules, and golden
protobuf output. They do not measure live throughput or CPU use, and they leave the
capture deficit in the [four-CPU test results](../history/singlepass-four-core-test-results.md)
open.
