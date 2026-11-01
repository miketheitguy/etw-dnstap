# Single-pass decoder: current contract

> **Status:** Current · **Describes:** `main` at commit `efb47af` (decoder added in `e59425f`) · **Evidence dated:** not applicable (reference; local tests only) · **Reviewed:** 2026-11-01

This page states the rules that the code enforces today. It covers the fast path
in `src/etw/decode.rs`, the schema compiler and fallback in `src/etw/schema.rs`,
and the encoder bounds in `src/dnstap.rs`. For ownership of pooled events, read
[Pooled ETW intake](pooled-intake.md). The
[implementation plan and completion record](../history/single-pass-decoder-plan.md)
is historical.

## Which events are decoded

- The parser decodes only events 256 and 257 from the DNS Server provider. All
  other events are not captured.
- For event 256, the peer address is the `Source` property. For event 257, it is
  the `Destination` property.
- The parser caches at most 8 schemas. Each schema is keyed by event ID and
  version. When the cache is full, the parser clears it.

## Fast path and fallback

Each schema is either compiled into a plan (fast path) or left without a plan
(fallback). The decision is made once, when the schema is first loaded.

A schema uses the fallback when its plan does not compile. This happens when any
of these is true:

- The schema has more than 256 properties, or a property name has more than
  256 UTF-16 units.
- A required name appears more than once. The required names are `TCP`, `Port`,
  `InterfaceIP`, the peer name, and `PacketData`. The check covers the whole
  schema, not only the prefix described below.
- A property in the prefix is an array (count other than 1), a structure, a
  custom schema, or has a flag other than `PropertyParamLength` or
  `PropertyParamFixedLength`. Setting both of these flags also selects the
  fallback.
- A property in the prefix has a layout that the grammar below does not allow.

In the fallback, the parser reads each property with TDH property calls. Packet
bytes are copied into parser storage.

Two cases do not use the fallback:

- **Metadata load failure.** If the manifest metadata cannot be loaded (for
  example, the provider or event schema does not match, or the size is invalid),
  the event is rejected. The parser remembers the failed key, for up to 8 keys.
  Later events with that key are rejected without a TDH call.
- **Malformed payload under a plan.** A payload that does not decode under a
  compiled plan is rejected. The parser never retries it through TDH.

## Grammar of the fast path

The plan includes the properties from the first one through the last required
property (the prefix). Properties after the prefix are not checked for layout.

| Native type | Accepted layout |
| --- | --- |
| `UINT8`, `INT8` | 1 byte (length 0 or 1) |
| `UINT16`, `INT16` | 2 bytes (length 0 or 2) |
| `UINT32`, `INT32`, `BOOLEAN` | 4 bytes (length 0 or 4) |
| `ANSISTRING` | Terminated by the first zero byte (length 0) |
| `UNICODESTRING` | Terminated by the first 16-bit zero unit (length 0) |
| `BINARY`, fixed width | Nonzero length, or length 0 with the fixed-length flag |
| `BINARY`, length from a property | Length names an earlier unsigned scalar (`UINT8`, `UINT16` or `UINT32`) |

Each required property has these further rules:

- `TCP` and `Port` must be unsigned integers or `BOOLEAN`. A `Port` with the
  port output type must be `UINT16`.
- `InterfaceIP` and the peer must be a string whose output type is not IPv4 or
  IPv6. Alternatively, they must be a fixed-width binary of 4 bytes with the IPv4
  output type, or 16 bytes with the IPv6 output type. Any other binary is
  rejected, which selects the fallback.
- `PacketData` must be binary.

A length that comes from a property must refer to an earlier property. A forward
or self reference is rejected.

## Decoding rules

- The decoder reads the payload once, in order. It uses checked slices and does
  not allocate.
- Scalars are little-endian. `Port` is read as big-endian (network byte order).
- A string is read up to its terminator. A UTF-16 scan starts at the field's own
  offset and moves in 2-byte units, even when that offset is odd. A missing
  terminator is an error.
- Decoding stops after the last required property. Bytes after `PacketData` are
  never read as packet content.
- The decoder forbids unsafe code (`#![forbid(unsafe_code)]`).

## Checks after decoding

The fast path and the fallback use the same checks after the fields are read:

- `TCP` must be 0 or 1.
- `Port` must fit in 16 bits. A value of 0 means that the port is absent.
- An address is at most 512 bytes. The text before any `%` scope suffix must
  parse as an IP address. An unspecified address (`0.0.0.0` or `::`) means that
  the endpoint is absent.
- The timestamp must convert to a time after the Unix epoch.
- The packet length must be 12 through 65,535 bytes. The encoder checks this.
- The DNS QR bit must match the event: a query for 256, a response for 257. The
  encoder checks this.

## Bounds

| Bound | Value | Where it is enforced |
| --- | --- | --- |
| Schemas cached per parser | 8 | `src/etw/schema.rs` |
| Failed schema keys remembered | 8 | `src/etw/schema.rs` |
| Properties per plan | 256 | `src/etw/decode.rs`, `src/etw/schema.rs` |
| Property name length | 256 UTF-16 units | `src/etw/schema.rs` |
| Address length | 512 bytes | `src/etw/schema.rs` |
| Fallback property scratch | 512 bytes | `src/etw/schema.rs` |
| Fallback text scratch | 768 bytes | `src/etw/schema.rs` |
| DNS packet length | 12 through 65,535 bytes | `src/dnstap.rs` |
| Identity | 1,024 bytes | `src/dnstap.rs` |
| Encoded output | 70 KiB (71,680 bytes) | `src/dnstap.rs` |

## Ownership and allocation

- On the fast path, `PacketData` borrows the pooled event payload. It is copied
  once into the final output. The pooled event is released before the TCP write.
- A supported schema-cache hit makes no TDH metadata or property calls.
- In the focused tests, warmed successful decoding and encoding allocate nothing.
- Cold metadata loads, fallback reads, and error construction do allocate.

## Validation scope

The local tests check:

- Both recorded event layouts (256 and 257). Their fields and protobuf output
  match the fallback.
- Malformed payloads, which are rejected under a plan without TDH calls.
- Unsupported layouts, which use the fallback.
- A process-local manifest that compares decoded fields with real TDH extraction
  for synthetic records. It does not capture live DNS Server traffic.

These tests do not measure live throughput or CPU use. They do not resolve the
capture deficit in the
[four-CPU test results](../history/singlepass-four-core-test-results.md).
Those values remain unmeasured.
