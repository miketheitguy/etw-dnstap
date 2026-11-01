# Single-pass decoder: current contract

> **Status:** Current · **Describes:** `main` at commit `dde7188` (decoder added in `e59425f`, TDH fallback removed in `69db97d`) · **Evidence dated:** not applicable (reference, local tests only) · **Reviewed:** 2026-11-01

This page states the rules that the code enforces today. It covers the fast path
in `src/etw/decode.rs`, the schema compiler in `src/etw/schema.rs`,
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

## Supported schemas

Every captured event is decoded by a compiled plan. There is no per-event TDH
reader. TDH is used only when a schema key is not in the cache. Then two
`TdhGetEventInformation` calls read the manifest metadata that the plan is
compiled from. The decision is made when the schema is loaded.

A schema is unsupported when its plan does not compile. This happens when any of
these is true:

- The schema has more than 256 properties, or a property name has more than
  256 UTF-16 units.
- A required name appears more than once. The required names are `TCP`, `Port`,
  `InterfaceIP`, the peer name, and `PacketData`. The check covers the whole
  schema, not only the prefix described below.
- A property in the prefix is an array (count other than 1), a structure, a
  custom schema, or has a flag other than `PropertyParamLength` or
  `PropertyParamFixedLength`. Setting both of these flags also makes the
  schema unsupported.
- A property in the prefix has a layout that the grammar below does not allow.

Events are rejected in these cases:

- **Unsupported schema or metadata load failure.** The event is rejected and
  counted in `schema_rejected` and in `rejected`. The cause can be one of these:
  - A schema that does not compile.
  - A schema that is not from a manifest.
  - A provider or event schema that does not match.
  - A TDH call that fails.
  - An invalid size.

  One warning is logged for each key. The parser remembers up to 8 keys. When
  that list is full, the parser clears it. Later events with a remembered key
  are rejected without a TDH call.
- **Malformed payload under a plan.** A payload that does not decode under a
  compiled plan is rejected. There is no TDH retry.

Earlier builds decoded an unsupported schema through per-event TDH property
calls instead of rejecting it. The CHANGELOG records the removal.

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
  rejected, which makes the schema unsupported.
- `PacketData` must be binary.

A length that comes from a property must refer to an earlier property. A forward
or self reference is rejected.

## Decoding rules

- The decoder reads the payload once, in order. It uses checked slices and does
  not allocate.
- Scalars are little-endian. A `Port` field is read as big-endian only when its
  output type is `PORT`, which requires `UINT16`. The DNS Server schema declares
  `Port` as `UINT32` with output `UNSIGNEDINT`, so it is read little-endian.
- A string is read up to its terminator. A UTF-16 scan starts at the field's own
  offset and moves in 2-byte units, even when that offset is odd. A missing
  terminator is an error.
- Decoding stops after the last required property. Bytes after `PacketData` are
  never read as packet content.
- The decoder forbids unsafe code (`#![forbid(unsafe_code)]`).

## Checks after decoding

The parser applies these checks after the fields are read:

- `TCP` must be 0 or 1.
- `Port` must fit in 16 bits. A value of 0 means that the port is absent.
- An address is at most 512 bytes. The text before any `%` scope suffix must
  parse as an IP address. An unspecified address (`0.0.0.0` or `::`) means that
  the endpoint is absent.
- The timestamp must convert to a time at or after the Unix epoch.
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
| Address text scratch | 768 bytes | `src/etw/schema.rs` |
| DNS packet length | 12 through 65,535 bytes | `src/dnstap.rs` |
| Identity | 1,024 bytes | `src/dnstap.rs` |
| Encoded output | 70 KiB (71,680 bytes) | `src/dnstap.rs` |

## Ownership and allocation

- `PacketData` borrows the pooled event payload. It is copied once into the
  final output. The pooled event is released before the TCP write.
- A schema-cache hit makes no TDH call.
- In the focused tests, warmed successful decoding and encoding allocate nothing.
- Cold metadata loads and error construction do allocate.

## Validation scope

The local tests check:

- Both recorded event layouts (256 and 257), with golden protobuf output.
- Malformed payloads, which are rejected under a plan without TDH calls.
- Unsupported layouts, which are rejected and counted.
- A process-local manifest: a plan compiled from real TDH metadata produces the
  same output as the plan compiled from the fixture template. It does not
  capture live DNS Server traffic.
- The parser's failed-key list stays bounded and does not reallocate.

The ignored `parse_encode_benchmark` test is an optional manual timing of the
warmed parse and encode path. It also prints the size of `Parser`.

These tests do not measure live throughput or CPU use. They do not resolve the
capture deficit in the
[four-CPU test results](../history/singlepass-four-core-test-results.md).
Those values remain unmeasured.
