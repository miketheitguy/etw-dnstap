# Response failure capture plan

> **Status:** Historical (executed and lab-validated 2026-11-01) · **Evidence dated:** 2026-11-01 · **Commit:** planned on top of `19aa979` · **Reviewed:** 2026-11-01

This plan adds the DNS Server `RESPONSE_FAILURE` event (ID 258) to the capture.
It also removes the level filter from the ETW subscription. DNS over HTTPS events
stay out of scope.

## Decisions

| Item | Today | After |
| --- | --- | --- |
| `MatchAnyKeyword` | `QUERY_RECEIVED \| RESPONSE_SUCCESS` (`0x3` on the lab VM) | `QUERY_RECEIVED \| RESPONSE_SUCCESS \| RESPONSE_FAILURE` (`0x7` on the lab VM), each bit resolved by name |
| `MatchAllKeyword` | `0` | `0` (unchanged) |
| Level | 4 (`TRACE_LEVEL_INFORMATION`) | `0xFF`: no level filter |
| `EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0` | set | set (unchanged) |
| ETW event-ID filter and callback allowlist | 256, 257 | 256, 257, 258 |
| Keyword fallback | `0x8000000000000000` | unchanged; the ID filter still limits the events |
| dnstap mapping | 256 → `CLIENT_QUERY`, 257 → `CLIENT_RESPONSE` | adds 258 → `CLIENT_RESPONSE` |

Notes:

- `Level` is a maximum. An event passes when its level is at most the value.
  `0xFF` therefore passes every level. Event 258 has level Error (2), which
  level 4 already passed, so the level change does not change the captured set
  today.
- The keywords also enable events that the ID filter excludes. The EDNS
  variants are 284, 285, and 287, and `DNSSEC_VALIDATION_FAILURE` is 288. DNS
  over HTTPS is 597 to 599. ETW applies the ID filter after the provider builds
  the event (Microsoft Learn, `EnableTraceEx2` remarks). These events therefore
  cost `dns.exe` CPU when they occur, but they never reach the service.

## Evidence from the lab VM

From the provider manifest (`Get-WinEvent -ListProvider
Microsoft-Windows-DNSServer`) and `logman query providers` on DNSTap-WS2025,
2026-11-01:

- `RESPONSE_FAILURE` is `0x4`, `QUERY_RECEIVED` is `0x1`, and
  `RESPONSE_SUCCESS` is `0x2`. Each event also carries the Analytical channel
  bit `0x8000000000000000`.
- Event 258, version 0, level Error, has 17 fields in this order: TCP
  (UInt8), InterfaceIP (AnsiString), Reason (UnicodeString), Destination
  (AnsiString), QNAME (AnsiString), QTYPE (UInt32), XID (UInt32), RCODE
  (UInt32), Port (UInt32), Flags (UInt32), Zone (UnicodeString), PolicyName
  (UnicodeString), BufferSize (UInt32), PacketData (Binary), AdditionalInfo
  (UnicodeString), ElapsedTime (UInt32), GUID (UnicodeString).
  - The peer is `Destination`, as in 257.
  - The packet length is `BufferSize`, followed by `PacketData`.
  - `Reason` is a terminated UTF-16 string before the peer field.

A probe session with keywords `0x7` and level `0xFF` recorded these results:

- Queries that ended in SERVFAIL (RCODE 2, `Reason=System`) produced event
  258. Its `PacketData` was the response sent to the client: the QR bit was
  set, and it was 34 bytes.
- NXDOMAIN answers from a hosted zone, and NXDOMAIN for `.invalid`, produced
  event 257 (RESPONSE_SUCCESS). They are already captured.
- None of 284, 285, 287, or 288 appeared.

## Code changes

1. `src/etw/keywords.rs`:
   - Add `KEYWORD_RESPONSE_FAILURE = "RESPONSE_FAILURE"`.
   - The narrowed choice needs all three names. It ORs their bits.
   - It falls back if any name is missing or malformed, or if any two names
     share a bit. That is the same rule as today, extended to three names.
   - Update the doc comments ("events 256 and 257").
2. `src/etw.rs`: `SUPPORTED_EVENT_IDS = [256, 257, 258]`. The session's
   event-ID filter is sized from this array (`src/etw/session.rs`). Check that
   the struct and size computation follow from the array length.
3. `src/etw/session.rs`: the enable call uses level `0xFF`. Use a named
   constant with a doc comment that says it is a maximum and passes every level.
   Update any test that asserts level 4.
4. `src/etw/schema.rs`: the peer field for 258 is `Destination`. Today the code
   picks `Source` only for 256 and `Destination` otherwise; make that explicit
   for 256, 257, and 258. The schema plan compiler already supports terminated
   UTF-16 strings. Confirm that 258's prefix (TCP, InterfaceIP, Reason,
   Destination, … BufferSize, PacketData) compiles, and that `Reason` is walked
   but not kept.
5. `src/dnstap.rs`: map 258 to `MessageType::ClientResponse` with
   `is_response = true`. The existing direction check rejects a 258 packet whose
   QR bit is clear (`EncodeError::IncorrectDirection`). Keep that check.
6. `src/etw/decode.rs` and `src/etw/fuzzing.rs`: the saved v0 field-order
   helpers for 256 and 257 gain 258. Extend the fuzz entry point so it covers
   258.
7. Comments and docs that say "256 and 257" across `src/` must be updated
   where they describe the captured set.
8. No change to the pool, the channel, the performance counters, or the ETW
   callback's hot path, beyond the allowlist array.

## Tests

- Keyword selection:
  - all three names present, which gives `0x7` from the fixture bits
  - any one missing, which falls back
  - two sharing a bit, which falls back
  - the fallback reason text
- The event-ID filter contains exactly 256, 257, and 258.
- The level constant is `0xFF`.
- The schema loads 258 from a fixture that matches the field list above.
  `src/etw/testdata/decoder-fixture.man` is the existing fixture source; add a
  258 event to it.
- 258 decoding:
  - the peer address and port come from `Destination` and `Port`
  - the packet comes from `BufferSize`/`PacketData`
  - a long or empty `Reason` is handled
  - truncated payloads are rejected
- dnstap: 258 encodes as `CLIENT_RESPONSE`, with `response_message` set from
  the packet. A 258 packet with QR clear is rejected with
  `IncorrectDirection(258)`.
- Existing golden or oracle tests (`src/dnstap/test_oracle.rs`,
  `golden_tests.rs`) gain a 258 case where the pattern allows it.

## Documentation

- `docs/guide/configuration.md`, "Subscription":
  - the three keywords
  - no level filter
  - the ID filter 256, 257, 258
  - that 258 is a failed response (for example SERVFAIL) and is sent as
    `CLIENT_RESPONSE`
  - that NXDOMAIN is a successful response (257)
  - that 284, 285, 287, 288, and 597 to 599 are enabled by the keywords but
    excluded by the ID filter
- `README.md`, "What is captured", and `docs/design/pooled-intake.md`
  ("Subscription and event selection"): the same facts, in short form.
- `CHANGELOG.md` `[Unreleased]`: an `Added` entry for capturing failed responses.
  The receiver now gets `CLIENT_RESPONSE` messages for failed responses. The
  local versioning policy puts the dnstap output in the contract, so decide
  under that policy whether the entry is marked **Breaking.**, and say why.
- `docs/README.md`: a plans index row for this plan.

## Checks

- `cargo build --locked --release --target x86_64-pc-windows-msvc`
- `cargo test --locked --target x86_64-pc-windows-msvc`, and with
  `--features diag-timing`
- `cargo clippy --locked --all-targets --target x86_64-pc-windows-msvc -- -D warnings`,
  and the `diag-timing` and `fuzzing` variants
- `cargo fmt --check`
- `cargo check --locked` in `fuzz/`
- `scripts/ci/verify-pe.ps1` on the release executable
- `python -I scripts/ci/linkcheck.py`
- the host-path check from `CLAUDE.md`

## Lab check

This step is run by the orchestrator.

1. Deploy the new build.
2. `capture started` shows `match_any_keyword=0x7`.
3. Send SERVFAIL-producing queries (a non-recursive query for a name that the
   server does not host) and normal queries.
4. Confirm:
   - `received` and `encoded` include the 258 events, with 0 `rejected`
   - the receiver gets them as `CLIENT_RESPONSE`
   - a 10,000 queries-per-second run with normal queries still loses nothing

## Lab results

Run on 2026-11-01 on DNSTap-WS2025 with the LocalService restricted-SID
profile, default buffer settings, and a plaintext receiver.

- **Startup:** `capture started` showed `match_any_keyword=0x7` and
  `keyword_fallback=false`.
- **Count check:** an independent ETW session with the same keywords and level
  recorded 800 events 256, 400 events 257, and 400 events 258 while the service
  ran. The traffic was 200 lookups with answers and 200 that ended in SERVFAIL;
  each lookup sent A and AAAA queries. The service's `capture stopped` showed
  `received`, `dequeued`, and `encoded` at 1,600 each, with 0 `rejected` and 0
  `ring_dropped`.
- **Failure types:**

  | Query | Server action | Event | Packet |
  | --- | --- | --- | --- |
  | Non-recursive query for a name the server does not host | SERVFAIL, RCODE 2, Reason `System` | 258 | response, QR set |
  | Name matched by a DENY query resolution policy | REFUSED, RCODE 5, Reason `Policy` | 258 | response, QR set |
  | Name matched by an IGNORE query resolution policy | no response; the client timed out | 259 | none |
  | Name with no record in a hosted zone | NXDOMAIN | 257 | response |

  Event 259 has no packet and nothing is sent to the client, so it stays out of
  the capture. The query is still captured as 256.
- **Regression:** a 60-second run at 10,000 queries per second with normal
  queries captured all 1,200,000 events. It had 0 ring drops, 0 `etw_lost`, and
  0 rejected, and the receiver got 103.015 bytes per frame. The rate warning did
  not fire.

## Review outcome

An independent code review found no blocker or major issue. These were
addressed:

- If only `RESPONSE_FAILURE` cannot be resolved, the service narrows to
  `QUERY_RECEIVED | RESPONSE_SUCCESS` and warns that failed responses are not
  captured. It no longer falls back to the whole Analytical channel.
- A decoder test now puts `Reason` at an odd payload offset.
- The documentation and changelog wording now match the observed behavior.
