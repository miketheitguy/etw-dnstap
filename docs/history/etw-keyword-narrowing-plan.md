# ETW keyword narrowing plan

> **Status:** Historical · **Evidence dated:** not recorded · **Commit:** not recorded · **Reviewed:** 2026-11-01

**Status:** Stage K0 is recorded for Windows Server 2025 build 26100 (October
30, 2026). Stages K1–K3 were completed on October 31, 2026. Narrowing passed
live validation on that build. Record K0 for every other target build before
deploying there.

## Goal

Make DNS Server skip generating events this program never uses. Today the
session enables `MatchAnyKeyword` `0x8000000000000000` (the Analytical channel)
and relies on the ETW runtime event-ID filter to drop everything except 256
and 257. The provider still builds every analytic event, including failures,
recursion, and transfers, before ETW discards it. Microsoft documents that
level and keyword filtering happens in provider code before an event is
generated, while event-ID filtering happens after generation
([EnableTraceEx2 remarks](https://learn.microsoft.com/windows/win32/api/evntrace/nf-evntrace-enabletraceex2)).

## Evidence so far

These facts come from public sources. Neither is from a manifest whose Windows
Server version is confirmed.

- A Microsoft-Windows-DNSServer manifest dump
  ([jdu2600 gist](https://gist.githubusercontent.com/jdu2600/e246c4b72a23ac95033b1a66fff0296f/raw),
  reportedly Windows Server 2019) defines provider keywords in bits 0–46,
  including `QUERY_RECEIVED` `0x1` (event 256), `RESPONSE_SUCCESS` `0x2` (event 257),
  `RESPONSE_FAILURE` `0x4` (258), `RECURSE_QUERY_OUT` `0x10` (260), and
  `RECURSE_RESPONSE_IN` `0x20` (261). Events 284 and 286 reuse `0x2` and `0x1`.
  No event has keyword 0.
- Published `logman query providers` output
  ([Elastic](https://www.elastic.co/docs/reference/integrations/microsoft_dnsserver))
  maps channel bit 63 to `Microsoft-Windows-DNSServer/Analytical` and bit 62 to
  `/Audit`. Real event samples
  ([NXLog](https://docs.nxlog.co/integrate/dns-monitoring-windows.html)) show
  keyword values that combine a channel bit with a provider bit.
- Widening the mask to `0xE000000000000000` cannot reduce admitted events. Per
  [EVENT_DESCRIPTOR](https://learn.microsoft.com/windows/win32/api/evntprov/ns-evntprov-event_descriptor),
  an event passes when its keyword is 0, or when
  `(Keyword & MatchAny) != 0 && (Keyword & MatchAll) == MatchAll`.
  Widening would add Audit-channel generation, so it was rejected.

## Stage K0: Verify provider metadata (operator, lab VM)

Run these on each Windows Server version that will host the service, starting
with the lab DNS VM:

```powershell
wevtutil gp Microsoft-Windows-DNSServer /ge /gm:true /f:xml > "$env:TEMP\dnsserver-provider.xml"
logman query providers Microsoft-Windows-DNSServer
```

Record the following in this plan, per Windows Server build:

1. The keyword names and masks for events 256 and 257, and for 284 and 286.
2. The channel keyword bits. In particular, confirm that bit 63 is
   Analytical.
3. Whether any event at level 4 or below has keyword 0.
4. Whether any other event carries `QUERY_RECEIVED` or `RESPONSE_SUCCESS`.

If the masks differ between builds, Stage K1 must resolve them at runtime by
name rather than use constants. The design below does that regardless.

## Stage K1: Resolve and apply narrowed keywords

**Model:** Sonnet at high effort. The stage adds unsafe TDH FFI under the
crate's strict lints.

**Decisions (October 31, 2026).**

- **Names.** K0 metadata names the keywords `QUERY_RECEIVED` (mask `0x1`) and
  `RESPONSE_SUCCESS` (mask `0x2`) with empty `message` attributes, so TDH
  returns those names. Match them exactly.
- **`MatchAllKeyword` stays 0.** Step 2 below allowed bit 63 only if K0
  covered every target build, and so far it covers only build 26100.
  - The narrowed `MatchAnyKeyword` alone already excludes the failure,
    recursion, and transfer events.
  - Every event that shares the two bits is Analytical on build 26100, and
    the event-ID filter removes the rest.
  - Requiring bit 63 would gain nothing measurable. On a build whose channel
    bits differ, it would drop every event.
- **The keyword-0 guard is enabled.**

Files: `src/etw.rs` (plus `README.md` and the subscription docs for wording).

1. **Resolve keywords at startup by name.** Call
   `TdhEnumerateProviderFieldInformation(&PROVIDER, EventKeywordInformation, ...)`
   for the installed provider. Look up `QUERY_RECEIVED` and `RESPONSE_SUCCESS`.
   Require each to be a single bit within the provider-defined range
   (`0x0000FFFFFFFFFFFF`). Use a two-call size query with u64-aligned storage
   and bounds checks, following `load_schema`. Every `unsafe` block needs a
   `// SAFETY:` comment.
2. **Narrowed subscription.** If both keywords resolve, set
   `MatchAnyKeyword = QUERY_RECEIVED | RESPONSE_SUCCESS`. Set
   `MatchAllKeyword = 0x8000000000000000` only if K0 confirmed bit 63 is
   Analytical on every target build; otherwise leave it at 0.
3. **Fallback.** If lookup fails or either name is missing, keep today's
   `MatchAnyKeyword` `0x8000000000000000` and `MatchAllKeyword` 0. Emit one
   `tracing::warn!` that names the reason. Capture must not stop because
   metadata is missing.
4. **Keyword-0 guard.** Set `EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0` (`0x10`)
   in `ENABLE_TRACE_PARAMETERS.EnableProperty`. It is supported on Windows 10
   version 1507 and later. Confirm the constant's name in windows-sys
   0.61.2.
5. **Keep both event-ID layers.** Keep the ETW event-ID filter and the
   callback and parser `supported()` checks. Events 284 and 286 share the
   narrowed keywords, so the event-ID filter still matters. The parser's peer
   selection is valid only for 256 and 257.
6. **Observability.** Log the resolved masks and whether the fallback was used
   in the existing "capture started" `tracing::info!`. Add no per-event
   counters.
7. **Tests.**
   - Unit-test the pure mask builder: two valid single bits, a missing name, a
     multi-bit value, a value in the reserved high range, and duplicate bits.
   - The DNS Server provider is not installed on development machines, so add
     a unit test proving that lookup failure selects the fallback.
   - Add an `#[ignore = "requires the DNS Server role"]` test that resolves the
     real keywords and prints them.
8. **Documentation.**
   - In `README.md` and `docs/etw-subscription-filter-plan.md`, update the
     subscription description.
   - Correct the plans' claim that keyword-0 events still reach the callback.
     With the event-ID filter and the keyword-0 guard they should not.

## Stage K2: Review

An Opus reviewer at high effort checks the TDH metadata FFI, fallback
behavior, mask validation, and documentation accuracy. A Sonnet agent at
medium effort fixes the findings.

## Stage K3: Live validation

K3 runs in the same lab session as
[live load Stage L5](../plans/live-load-validation-plan.md#stage-l5-heavy-load-and-buffer-ceiling-comparison),
which defines the candidate and baseline binaries. The lab's recursive
fixture was removed after the original campaign. K3 therefore uses an
authoritative workload plus failure and recursion probes. It does not use a
recursive load.

1. **Resolution on the real provider.** Copy the test binary to the VM, as
   for the elevated reclaim tests. Run the ignored DNS-role test and record
   the resolved names and masks.
2. **Startup log.** The candidate's "capture started" Event Log entry (ID 2)
   must show the resolved masks and no fallback.
3. **Isolation precondition.** DNS Server could gate analytic generation on
   bit 63 alone. If it does, the narrowed session receives nothing. Any other
   session that enables bit 63 would hide that, including the Analytical
   event-log channel. Before steps 4 and 5, and before every N run:
   - `wevtutil gl Microsoft-Windows-DNSServer/Analytical` must show
     `enabled: false`;
   - `logman query -ets` must show no other session that enables the
     provider.
4. **Provider admission probe.** Run two passes, one at a time and never in
   parallel. Each pass starts one `logman` trace session for the provider at
   level 4:
   - Pass 1: `KwNarrow` with keyword `0x3`.
   - Pass 2: `KwBroad` with keyword `0x8000000000000000`.

   In each pass:
   1. Send the probe queries from the VM:
      - 20 successful authoritative queries;
      - 20 NXDOMAIN queries (`nx-NNNNN.auth.stress.test`);
      - 20 queries with RD=1 for names outside the lab zones, which start
        recursion.
   2. Stop the session, convert it with `tracerpt`, and count events by ID.

   Pass:
   - `KwNarrow` contains at least 20 each of 256 and 257, and only IDs in
     {256, 257, 284, 287, 597, 598}.
   - `KwBroad` shows which failure and recursion IDs the probe generated.

   If `KwNarrow` contains no 256 or 257 events, stop. A bit-63 gate makes
   narrowing unusable. The remedy is an operator override that forces the
   fallback selection, not a heuristic.
5. **Capture parity and CPU.**
   1. Generate `target/live-checks-20261031/nx-names.txt` with 5,000 names
      `nx-00001.auth.stress.test` and onward.
   2. Run two `dns-stress` clients concurrently for 60 seconds:
      - authoritative names at `--qps 20000`;
      - NX names at `--qps 10000`. The client counts NXDOMAIN as
        unsuccessful, which is expected.
   3. Run in the order B1 (baseline), N1 (candidate), B2, N2. Sample `dns.exe`
      and `etw-dnstap.exe` CPU as in Stage L5.

   Pass:
   - All loss counters are zero in every run.
   - The ratio of `encoded` to total responses received is the same for the
     baseline and candidate runs.

   Report the CPU difference with a caveat: it comes from two repetitions.

## Completion record

### Stage K0: Windows Server 2025 build 26100 (lab VM `DNSTap-WS2025`)

These results come from `wevtutil gp Microsoft-Windows-DNSServer /ge /gm:true`
and `logman query providers`. The evidence is in
`target/live-checks-20261030/dnsserver-provider.xml` and
`dnsserver-logman.txt`.

1. **Events 256 and 257.** Event 256 has keyword `0x8000000000000001`
   (`QUERY_RECEIVED`) and event 257 has `0x8000000000000002`
   (`RESPONSE_SUCCESS`), both at level 4.
2. **Channels.** The provider defines `Microsoft-Windows-DNSServer/Analytical`
   (channel 16) and `/Audit` (17). Events 256 and 257 carry bit 63, the
   Analytical channel bit.
3. **Keyword 0.** No event at level 4 or below has keyword 0.
4. **Shared bits.** Events 284 and 598 also carry `RESPONSE_SUCCESS`, and
   events 287 and 597 also carry `QUERY_RECEIVED`. Events 597 and 598 are the
   DNS-over-HTTPS query and response events.

On this build, event 286 is `RECURSE_ALIAS_FAILURE` (`0x800000000000`). That
differs from the Windows Server 2019 dump cited above, so provider metadata
does vary by build. This supports resolving keywords by name at runtime. A
narrowed mask would admit 256, 257, 284, 287, 597, and 598, so the event-ID
filter remains necessary.

### Stage K1 (Sonnet, high effort)

- **Lookup.** `src/etw.rs` resolves keywords with
  `TdhEnumerateProviderFieldInformation`.
  - The call uses the two-call size query and `u64` storage capped at 1 MiB.
  - `parse_keyword_array` parses the buffer with checked offsets.
  - The pure builder `select_keywords` returns `Narrowed { match_any }` or
    `Fallback { reason }`.
- **Subscription.** `MatchAllKeyword` is 0. `EnableProperty` is
  `EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0`. Both event-ID layers are
  unchanged.
- **Logging.** The "capture started" entry logs `match_any_keyword`,
  `match_all_keyword`, and `keyword_fallback`.
- **Tests.** Unit tests cover the plan's mask cases and malformed metadata
  images. An ignored test, `resolves_real_dns_server_keywords`, needs the DNS
  Server role.

### Stage K2 (Opus review at high effort; fixes by Sonnet at medium effort)

The review found no blockers. It confirmed the TDH protocol, the
`NameOffset` base, the bounds checks, and the fallback behavior against
Microsoft Learn.

**Major finding.** The original K3 probe ran a broad session in parallel,
which could hide a provider-side gate on bit 63. An empty trace would also
have passed. K3 now requires isolation, sequential passes, and at least 20
each of events 256 and 257.

**Fixes applied:**

- The README no longer overstates the generation savings.
- `query_provider_keywords` takes a GUID. A new non-ignored test queries an
  unregistered GUID and asserts the fallback.
- The attribution and "superseded" notes in the subscription and recovery
  plans are corrected.
- The SAFETY wording is corrected.
- Keyword resolution moved before `StartTraceW`.
- Status codes are formatted as OS errors.

**Checks.** `cargo test --locked --lib` reports 119 passed and 5 ignored.
fmt and strict Clippy are clean.

### Stage K3

Run on October 31, 2026 with Stage L5. Details are in
`target/live-checks-20261031/keyword-results.md`.

1. **Resolution: pass.** On the VM, `resolves_real_dns_server_keywords` resolved
   `QUERY_RECEIVED = 0x1` and `RESPONSE_SUCCESS = 0x2`
   and selected `match_any 0x3`, `match_all 0`, no fallback.
2. **Startup log: pass.** The candidate's "capture started" entry showed
   `match_any_keyword=0x3 match_all_keyword=0x0 keyword_fallback=false`.
3. **Isolation: pass.** The Analytical channel was `enabled: false` and no other
   session was listed before each probe pass and each N run.
4. **Admission probe: pass.** `KwNarrow` (`0x3`) held 72 events 256 and 60 events 257
   and no other provider IDs. `KwBroad` held 60 and 60 plus 20 each of 260 and 261
   (recursion). Bit 63 is not a gate.
5. **Parity and CPU: pass.** B1, N1, B2, N2 ran two concurrent clients
   (about 30k queries/s total) for 60 s. All loss counters were 0, and
   `encoded` / total responses was 2.000000 in all four runs. CPU (dns /
   etw-dnstap / total): B1 100.5 / 3.7 / 46.1, N1 100.4 / 4.5 / 45.6,
   B2 102.8 / 3.6 / 46.2, N2 105.3 / 4.3 / 46.7. No saving in DNS Server CPU
   was visible, and etw-dnstap used about 0.8 points more. Two repetitions per
   binary: indicative only. This workload generates only events 256 and 257, so it
   cannot show the generation saving from narrowing.
