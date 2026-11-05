# Single-pass decoder: current contract

> **Status:** Current · **Describes:** `src/etw/decode.rs`, `src/etw/schema.rs`, and the encoder bounds in `src/dnstap.rs` on branch `spool-encryption` as of 2026-11-05 · **Evidence dated:** not applicable (reference, local tests only) · **Reviewed:** 2026-11-05

### Quick Start

```text
key (event ID, version) -> cached plan, or 2 x TdhGetEventInformation -> compile or reject
payload -> one pass over the prefix -> parser checks -> encoder checks -> dnstap frame
```

Event ownership: [Pooled ETW intake](pooled-intake.md).

### Steps / Configuration

#### Decoded events

Events 256 (query received), 257 (response success), 258 (response failure). Peer: `Source`
for 256, `Destination` for 257 and 258; the plan skips the `Reason` string before the peer in 258.

#### Supported schemas

TDH runs only on a cache miss. A schema compiles to a plan when:

- property count and name lengths are within [Bounds](#bounds);
- each required name (`TCP`, `Port`, `InterfaceIP`, the peer, `PacketData`) appears exactly once anywhere in the schema;
- each prefix property (first through last required) is a single item (count 1), not a structure, with no custom schema;
- each prefix property carries at most one flag, `PropertyParamLength` or `PropertyParamFixedLength`, and fits the [grammar](#grammar-of-the-fast-path).

Every rejected event counts in `rejected`:

- **Unsupported schema:** also `schema_rejected`; one warning at load; the key is remembered, so TDH is skipped.
- **Metadata load failure** (Win32 error from `TdhGetEventInformation`): counted the same; reload after `SCHEMA_RETRY_EVENTS` (65,536) rejected events, a clockless budget. Success caches the key; failure counts `schema_rejected` and restarts the budget; unusable metadata marks it unsupported. Warns on failures 1 to 4, then 61, 121, and so on.
- **Malformed payload under a plan:** rejected, no TDH retry.

#### Grammar of the fast path

The plan covers the prefix; later properties stay name-only and unread.

| Native type | Accepted layout |
| --- | --- |
| `UINT8`, `INT8` | 1 byte (length 0 or 1) |
| `UINT16`, `INT16` | 2 bytes (length 0 or 2) |
| `UINT32`, `INT32`, `BOOLEAN` | 4 bytes (length 0 or 4) |
| `ANSISTRING`, `UNICODESTRING` | To the first zero unit (1 byte, or 16 bits), length 0 |
| `BINARY`, fixed | Nonzero length, or 0 with the fixed-length flag |
| `BINARY`, length property | An earlier `UINT8`, `UINT16`, or `UINT32` |

- `TCP`, `Port`: unsigned integer or `BOOLEAN`; `Port` with the port output type is `UINT16`.
- `InterfaceIP`, peer: a string with a non-IP output type, or fixed binary of 4 bytes (IPv4 output) or 16 bytes (IPv6 output).
- `PacketData`: binary.

#### Decoding rules

- One in-order pass, checked slices, no allocation, `#![forbid(unsafe_code)]` (`src/etw/decode.rs`).
- Little-endian scalars; `Port` is big-endian only with output type `PORT` (`UINT16`). DNS Server declares `Port` `UINT32`, output `UNSIGNEDINT`: little-endian.
- Strings run to the terminator; UTF-16 scans step 2 bytes from the field offset, odd or even.
- Decoding stops after the last required property; bytes after `PacketData` stay unread.

#### Checks after decoding

Parser:

- `TCP` is 0 or 1.
- `Port` fits 16 bits; 0 means absent.
- Address within [Bounds](#bounds); text before any `%` parses as IP; `0.0.0.0` or `::` means absent.
- Timestamp at or after the Unix epoch.

Encoder:

- Supported event ID.
- DNS packet length within [Bounds](#bounds).
- QR bit matches: query for 256, response for 257 and 258.
- Local and peer addresses in one family.
- Identity and frame within bounds.

#### Bounds

| Bound | Value | File |
| --- | --- | --- |
| Schemas cached per parser | 8 | `schema.rs` |
| Failed schema keys remembered | 8 | `schema.rs` |
| `SCHEMA_RETRY_EVENTS` | 65,536 | `schema.rs` |
| Properties per plan | 256 | `decode.rs`, `schema.rs` |
| Property name length | 256 UTF-16 units | `schema.rs` |
| Address length | 512 bytes | `schema.rs` |
| Address text scratch | 768 bytes | `schema.rs` |
| DNS packet length | 12 through 65,535 bytes | `dnstap.rs` |
| Identity | 1,024 bytes | `dnstap.rs` |
| Encoded output | 70 KiB (71,680 bytes) | `dnstap.rs` |

### Common Gotchas

- A full schema cache or failure list is cleared; a forgotten unsupported key loads and warns again.
- An unterminated string is an error, and so is a forward or self length reference.
- Warm decoding and encoding allocate nothing in the focused tests; cold metadata loads and error construction allocate ([allocation scope](pooled-intake.md#nonblocking-admission-and-allocation-scope)).
- Local tests cover layouts, rejection, retry, and golden protobuf output, not live throughput or CPU. The [four-CPU capture deficit](../history/singlepass-four-core-test-results.md) stays open.
