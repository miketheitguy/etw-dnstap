# Slow receiver lab validation

> **Status:** Historical · **Evidence dated:** 2026-11-04 (timestamps in UTC) · **Commits:** `a06f6fd` ("before") and `13972d8` ("after") · **Reviewed:** 2026-11-04

This record covers task L of the
[slow and saturated receiver plan](../plans/slow-receiver-plan.md): the first lab
run of the link writer, the progress liveness rule, and the spool rewind
([design](../design/slow-receiver.md)). It answers four questions:

1. With a receiver that is slower than the capture, does the "before" build lose
   frames in `queue_dropped` while the spool has room (F1), and does it end a
   session that is only slow (F2)? Does the "after" build keep every frame and the
   session, and put the overflow in the spool?
2. On a Windows Server guest with every CPU busy, does the replay after a
   60-second outage drain faster than the capture (review V3, finding V3-M-A, and
   task B2)?
3. Does the lab collector discard a frame that a closed connection cuts (design
   risk 6)?
4. Does the receiver-side STOP/FINISH check from task F work, and does it conflict
   with the parser-ramp flow?

The raw run data is lab evidence under `target/performance/slow-receiver-L-20261104/`.
It is not in the repository.

## Summary

| Question | Result |
| --- | --- |
| F1 on "before" | Confirmed. `queue_dropped` rose while the spool held at most 190 MB of a 4,096 MiB cap: 218,681 frames with a 2 MB/s link at 50,000 QPS, 121,603 in an 8-second collector pause, and 120,346 to 136,204 at the start of each outage. |
| F2 on "before" | Confirmed for a stall. An 8-second collector pause ended the session after a 2.01 s write, with 636 frames in `disconnected_dropped`. A rate limit alone never did: the longest write was 1.12 s. |
| "After", slow and paused receiver | Held in all 7 runs with a slow or paused receiver: no `queue_dropped`, no disconnect, every frame delivered, the overflow in the spool. One control run (40 MB/s link) had 10,822 frames in `queue_dropped` at the end of the load, from a router pause that two reruns did not repeat. See [Open items](#open-items). |
| V3-M-A on Windows Server | Closed for this host. With 12 busy threads on 6 vCPUs, the "after" build delivered a 1.46 GB backlog in about 2.1 s (about 700 MB/s), while the capture went on at 10 MB/s. |
| Risk 6, cut frame | Confirmed. DNS-collector 3.2.3 printed only the whole frames in each of three cut variants, and logged a reader error. |
| Task F check | Worked on first use: 34 of 34 stops `confirmed`. No script change was needed. The parser-ramp ordering does not conflict. |

## Environment

| Item | Value |
| --- | --- |
| Guest | DNSTap-WS2025 (`24bafedb-0998-4430-a0c6-7e90350cd9ac`): 6 vCPUs, 4 GiB, Windows Server 2025 Standard Evaluation (build 26100). Checkpoint `slow-receiver-L-pre-2026-11-04` taken before any change. `lab.json` still says 2 vCPUs. |
| Service identity | `NT AUTHORITY\LocalService`, SID type restricted, recovery `restart/5000`, from the MSI. Not changed by this run. |
| Build installed before the run | MSI 0.1.0, ProductCode `{34524511-F48F-45D2-B150-3492BD4B23F3}`, installed 2026-11-03, MSI SHA-256 `2233AE1EE87C92E334D70CE90942FF54A7161E582AB836EE0EDA664C744D680C`, executable SHA-256 `5F5ED754815751A2DE9A3277A969F135904CA1CD64C59CFB852726399422A88A`. |
| "Before" build | `main` at `a06f6fd`, built with `scripts/build/Build-Msi.ps1` in a temporary worktree under `target/`. Executable SHA-256 `3A46F27B78BC3CF3FA4944E1A3DCAD0E0AE51E344561E06495CCC1DFDDB9664E`, MSI SHA-256 `ECF8E91D772AAB34B8E39FB2456F82B3341E743C98372F512621DC4F3709A25D`. |
| "After" build | `slow-receiver` at `13972d8`, built with `scripts/build/Build-Msi.ps1`. Executable SHA-256 `CAD589EE7B85B7E71D3F71D79674FB8F5A84F05B31DF3079054F28CE8AE0092E`, MSI SHA-256 `F43E0A51C4C516AA3EAA2395B65E0B8532BAB4CD8A257E1E3D0A0B341ED197D3`. Both MSIs are version 0.1.0. The link-exception and PE checks of `Build-Msi.ps1` passed for both. |
| Service options | MSI properties `RECEIVER=172.27.215.60:6000`, `SESSION_NAME=EtwDnstap-Lab`, `SERVICE_ARGS="--plaintext --stats-interval-seconds 5"`, the same on both builds. Spool at its defaults: 4,096 MiB cap, 2,048 MiB free-space floor. Stall timeout at its default of 15 s on "after". |
| Receiver | DNSTap-Ubuntu2604 (`d5ca1bde-da5c-4c08-9625-e845c1443b0b`): 4 vCPUs, kernel 7.0.0-38-generic, DNS-collector 3.2.3, plaintext on `172.27.215.60:6000`. Its pipeline `tap` (dnstap) forwards to `discard` (devnull), and the systemd drop-in sets `StandardOutput=null` and `StandardError=null`. It wrote nothing to disk. Checkpoint `slow-receiver-L-pre-2026-11-04`. |
| Client | `dns-stress` built from the current source (`2073411`, absolute pacing) into `target/`. Authoritative load, 4 threads, 256 in flight per thread, 0 timeouts in every run. The older binary at the `lab.json` path predates the pacing fix: it sent 7,400 QPS when asked for 10,000. |

### Rates

Each query makes two frames (query and response). The calibration runs gave
103.015 receiver bytes for each frame at both rates.

| QPS | Frames per second | Capture rate (R_in) |
| --- | --- | --- |
| 10,000 | 20,000 | 2.06 MB/s |
| 50,000 | 100,000 | 10.3 MB/s |

The plan brief assumed one frame per query (1.03 and 5.15 MB/s). The rate limits
below are therefore twice the brief's example values.

## Method

1. Recorded the guest state (service configuration, product, executable hash),
   the receiver's `tc qdisc show`, `tc filter show dev eth0 ingress`,
   `iptables -S` (filter and mangle), `ip6tables -S`, `nft list ruleset`, the
   link list, and the loaded traffic-control modules. Took both checkpoints.
2. Installed each build with `msiexec /i <msi> /qn` and the three properties
   above. `MajorUpgrade AllowSameVersionUpgrades` replaced the installed product.
   Each install exited 3010. The guest was restarted, and the service came up
   with the expected `ImagePath` and executable hash.
3. Ran each case with a driver script (PowerShell 5.1, `LabCommon.psm1`,
   PowerShell Direct):
   - restart the service, so its counters start at 0;
   - sample the performance counters (`typeperf "\EtwDnstap(*)\*"`, 1 s) in the
     guest, and the receiver's `ss -tin` `bytes_received` (1 s; 0.1 s in the last
     run) over ssh to the host;
   - shape the receiver ingress, then run `dns-stress` for 60 s (150 or 200 s in
     the outage cases);
   - keep the shaping for 30 s after the load, remove it, and wait until
     `spool_bytes` is 0 for three polls;
   - stop the service around the task F capture, and keep the Application log
     events of the run (`capture stats` and `output stats` every 5 s).
4. Shaping. Data arrives at the receiver, so the shaping is on ingress. A `u32`
   filter on the `eth0` ingress qdisc sends packets from the guest to TCP port
   6000 to an `ifb` device. The `ifb` device has `tbf rate <R> burst 256kb
   latency 400ms` as root, and `netem delay 100ms limit 200000` as its child when
   a delay is set. ACKs are not shaped, so the delay adds 100 ms to the round
   trip.
5. Stall. `kill -STOP` on the collector for 8 s, then `kill -CONT`.
6. Outage. A temporary `nft` table `inet slowrxL` with a rule that drops packets
   from the guest to port 6000, for 60 s (120 s in the last run). A dedicated
   table was used instead of `iptables`, so removing the table restores the
   ruleset exactly.
7. Busy host. 12 `powershell.exe` processes, each running `while($true){}` at
   normal priority, 2 for each vCPU, from 3 s before the load to the end of the
   load. In the "after" runs the guest sampled `% Processor Time` and the
   processor queue length.

Delivered frames are the receiver bytes divided by 103.015. The **gap** is the
delivered frames less `encoded - (queue_dropped + disconnected_dropped +
spool_dropped)`. A gap of +3 to +56 is the 84-byte handshake of each connection.
A negative gap is frames that no counter holds and the receiver did not get.

**Latency.** The only latency measure is `spool_oldest_age_ms` ("after" only): the
age of the oldest unsent spool segment, an upper bound on the age of the oldest
frame that waits. The table gives its largest value in the run, which is at the
end of the shaped period. The "before" build has no such counter. No per-frame
delivery latency was measured; the collector wrote nothing.

## Case 1: slow receiver, rate and delay

Rate limits: about 4 times R_in (control), about 78 % of R_in ("below"), and about
20 % of R_in ("well below"). "Rx MB/s" is the receiver rate over the 60 s load.

| Case | Build | Encoded | Gap | `queue_dropped` | `disconnected_dropped` | Spooled | Disconnects | Max spool (MB) | Max oldest age (s) | `link_saturated` | Max write wait (ms) | Rx MB/s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 10k, 8 MB/s, 0 ms | before | 1,199,992 | +3 | 0 | 0 | 0 | 0 | 0 | n/a | n/a | 16 | 1.96 |
| 10k, 8 MB/s, 0 ms | after | 1,200,000 | +3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 16 | 1.97 |
| 10k, 1.6 MB/s, 0 ms | before | 1,200,000 | +3 | 0 | 0 | 951,082 | 0 | 91 | n/a | n/a | 79 | 1.53 |
| 10k, 1.6 MB/s, 0 ms | after | 1,200,000 | +3 | 0 | 0 | 935,335 | 0 | 93 | 37.0 | 1 | 595 | 1.52 |
| 10k, 0.4 MB/s, 0 ms | before | 1,199,996 | +3 | 0 | 0 | 1,172,220 | 0 | 121 | n/a | n/a | 1,118 | 0.38 |
| 10k, 0.4 MB/s, 0 ms | after | 1,199,992 | +3 | 0 | 0 | 1,167,830 | 0 | 120 | 83.9 | 1 | 1,125 | 0.38 |
| 10k, 8 MB/s, 100 ms | before | 1,200,000 | +3 | 0 | 0 | 0 | 0 | 0 | n/a | n/a | 204 | 1.97 |
| 10k, 8 MB/s, 100 ms | after | 1,200,000 | +3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 301 | 1.97 |
| 10k, 1.6 MB/s, 100 ms | before | 1,200,000 | +3 | 0 | 0 | 1,020,996 | 0 | 92 | n/a | n/a | 204 | 1.50 |
| 10k, 1.6 MB/s, 100 ms | after | 1,200,000 | +3 | 0 | 0 | 951,344 | 0 | 90 | 37.7 | 1 | 301 | 1.51 |
| 50k, 40 MB/s, 0 ms | before | 5,999,992 | +13 | 0 | 0 | 0 | 0 | 0 | n/a | n/a | 1 | 10.11 |
| 50k, 40 MB/s, 0 ms | after | 5,993,226 | +24 | **10,822** | 0 | 215,680 | 0 | 0 | 0 | 0 | 3 | 9.85 |
| 50k, 40 MB/s, 0 ms, rerun 2 | after | 5,999,996 | +13 | 0 | 0 | 196,424 | 0 | 0 | 0 | 0 | 3 | 10.11 |
| 50k, 40 MB/s, 0 ms, rerun 3 | after | 5,985,888 | +51 | 0 | 0 | 575,156 | 0 | 22 | 0.4 | 0 | 4 | 10.09 |
| 50k, no shaping | after | 5,999,421 | +13 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 10.15 |
| 50k, 8 MB/s, 0 ms | before | 5,999,968 | +12 | 0 | 0 | 5,895,224 | 0 | 205 | n/a | n/a | 16 | 7.78 |
| 50k, 8 MB/s, 0 ms | after | 5,999,990 | +13 | 0 | 0 | 5,693,036 | 0 | 190 | 21.0 | 1 | 16 | 7.83 |
| 50k, 2 MB/s, 0 ms | before | 5,999,974 | −25 | **218,681** | 0 | 5,758,274 | 0 | 526 | n/a | n/a | 594 | 1.95 |
| 50k, 2 MB/s, 0 ms | after | 5,999,994 | +13 | 0 | 0 | 5,967,742 | 0 | 548 | 77.3 | 1 | 556 | 1.96 |
| 50k, 40 MB/s, 100 ms | before | 5,999,992 | +20 | **53,076** | 0 | 2,114,216 | 0 | 82 | n/a | n/a | 201 | 9.99 |
| 50k, 40 MB/s, 100 ms | after | 5,999,968 | +12 | 0 | 0 | 303,260 | 0 | 21 | 0 | 0 | 300 | 10.09 |
| 50k, 8 MB/s, 100 ms | before | 5,999,958 | +17 | **642** | 0 | 5,989,045 | 0 | 214 | n/a | n/a | 188 | 7.59 |
| 50k, 8 MB/s, 100 ms | after | 5,985,172 | +56 | 0 | 0 | 5,983,193 | 0 | 214 | 23.9 | 1 | 301 | 7.42 |

A 20-second "before" check run at 50k, 8 MB/s, 100 ms had 65,569 frames in
`queue_dropped`, all in the first 6 s of the spill, while the spool held under
60 MB.

Every run: `spool_dropped` 0, `spool_errors` 0, `output_errors` 0, `etw_lost` 0
(except 1,536 in one "before" outage run), and the delivered rate matched the
`tbf` rate. `ring_dropped` (intake, before the sink) was 0 to 14,812 at
50,000 QPS with the default `--capacity`, as in earlier records. It is not a sink
loss.

### Stall: the collector paused for 8 s at 50,000 QPS

| Build | Encoded | Gap | `queue_dropped` | `disconnected_dropped` | Spooled | Disconnects | Max write wait | `link_saturated` | Max oldest age |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| before | 5,999,984 | −10,450 | 121,603 | 636 | 667,477 | 1 | 2.01 s | n/a | n/a |
| after | 5,999,245 | +14 | 0 | 0 | 648,289 | 0 | 7.77 s | 0 | 3.9 s |

### Detection on "after"

- `link_saturated` was set in every "below" and "well below" run, 12 to 30 s
  after the load started, and never in a control run or the 8-second pause. It
  cleared 3 to 11 s after the load ended, when the backlog stopped growing,
  although the link was still shaped and the backlog was 14 to 472 MB. This is the
  documented rule ("no longer larger than 10 samples ago").
- `write_max_recent_us` followed the link: 3 to 16 ms with no delay at 8 MB/s and
  above, 0.3 s with 100 ms of delay, 0.6 s at 10,000 QPS with a 1.6 MB/s link,
  0.56 s at 50,000 QPS with a 2 MB/s link, 1.13 s with a 0.4 MB/s link, and the
  whole wait (7.77 s, 15.0 s) in a stall.
- `send_queue_depth_max` was 2 in the 50,000 QPS runs with an 8 or 2 MB/s link
  and no delay, drains included, 23 to 25 with 100 ms of delay, and 30 to 41 in
  two of the three 50k control runs (128 in the first; see
  [Open items](#open-items)). It did not rise during any drain (design risk 15).
  At 10,000 QPS it read 33 to 61 in every run, the control with an 8 MB/s link
  included, with no loss. The cause was not investigated.
- `spool_oldest_age_ms` reached 21 to 38 s with a "below" link and 77 to 84 s with
  a "well below" link, at the end of the 30 s shaped tail.

### Drain after the shaping was removed

With the guest idle, the spool emptied within one 1 s poll of the removal in
every run: 459 MB ("before") and 481 MB ("after") at 50,000 QPS, about 120 MB at
10,000 QPS. That is at least 400 MB/s on both builds.

## Case 2: outage on a busy host (V3-M-A, B2)

50,000 QPS for 150 s, with the receiver cut from 20 s to 80 s. The last run cut it
from 20 s to 140 s of a 200 s load, and sampled the receiver every 0.1 s. "Busy"
is 12 spinning threads on 6 vCPUs: `% Processor Time` was 100 for the whole
outage and drain, with a processor queue length of 14.5 to 15.1 on average
("after" runs; the second "before" run gave 100 and 12.9).

| Run | Build | Encoded | Gap | `queue_dropped` | `disconnected_dropped` | Spooled = replayed | Max spool (MB) | Session lost after the cut | Reconnected after the restore | Backlog delivered in |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| idle | before | 14,999,998 | −9,978 | 120,346 | 641 | 6,067,590 | 610 | 2.2 s | 1.1 s | ≤ 1.1 s (625 MB) |
| busy | before | 14,999,960 | −8,947 | 126,175 | 1,904 | 7,250,170 | 736 | 2.2 s | 12.9 s | ≤ 1.7 s (747 MB) |
| busy, rerun | before | 14,998,456 | +35 | 136,204 | 10,806 | 6,942,989 | 701 | 2.3 s | 10.0 s | ≤ 1.5 s (715 MB) |
| idle | after | 14,999,974 | −5,657 | 0 | 0 | 6,372,905 | 644 | 15.1 s | 2.8 s | ≤ 1.5 s (657 MB) |
| busy | after | 14,999,962 | −40,797 | 0 | 0 | 6,910,033 | 700 | 15.5 s | 8.3 s | ≤ 1.8 s (712 MB) |
| busy, 120 s cut, 0.1 s samples | after | 20,000,000 | −41,236 | 0 | 0 | 14,216,070 | 1,436 | 15.5 s | 20.3 s | 2.09 s (1,465 MB) |

- **Drain rate.** In the 0.1 s run the new connection carried 1,464.7 MB in the
  2.09 s after its handshake, 355 to 1,339 MB/s in each 0.1 s step, then fell to
  the live rate of 10 MB/s. That is about 700 MB/s with every CPU busy, while the
  capture went on at 10 MB/s. B2 measured 51 to 368 MB/s on the 24-CPU client host
  at the same load per CPU. The "≤" values in the table are bounded by the 1 s
  counter poll.
- **The backlog shrank while the capture went on** in every run. No frame went to
  `spool_dropped`, and nothing was evicted: the largest spool was 1,436 MB of the
  4,096 MiB cap.
- **No spill before the cut.** In the 20 s before each cut, with every CPU busy,
  `spooled` stayed 0 on "after" (two runs): the 128-block FIFO did not spill at 10.3 MB/s.
  This run did not test 41 MB/s, where design risk 9 places the margin.
- **Reconnect.** The delay after the restore (1.1 to 20.3 s) is the reconnect
  backoff, not the replay.
- **Uncounted loss at the failure.** The negative gaps are bytes that the guest's
  TCP accepted and the receiver never got. On "after", `bytes_sent` was larger
  than the receiver bytes by 4.25 MB in the 0.1 s run, which matches the gap of
  41,236 frames. With the fixed 1 MiB send buffer ("before"), the gap was 0 to
  10,450 frames (up to about 1.08 MB). With autotuning ("after", H4) it was
  5,657 frames idle and 40,797 to 41,236 frames busy (about 4.2 MB). This is the
  cost that design risk 4 predicts: the unseen loss grows with the buffer. It
  applies only to a failed connection; the slow and paused cases had none.

## Case 3: cut frame (design risk 6)

Method: a second DNS-collector 3.2.3 ran in the foreground over ssh, on
`127.0.0.1:6001`, with a `stdout` text logger. Its config and the test client
were in a memory-backed tmpfs and were deleted afterwards. Its output went to the
ssh session only, not to the receiver's disk. The production collector was not
touched by this test. A Python client sent READY, read ACCEPT, sent START and three whole
dnstap frames (`CLIENT_QUERY`, distinct names), and then ended in one of four
ways.

| Variant | Last bytes sent | Close | Lines printed | Collector log |
| --- | --- | --- | --- | --- |
| `stop` (control) | STOP, FINISH read | FIN | 3 whole frames | `framestream reset by sender` (INFO) |
| `body` | length prefix and 46 of 93 body bytes | FIN | 3 whole frames | `framestream reader error: unexpected EOF` (ERROR) |
| `header` | 2 of the 4 length bytes | FIN | 3 whole frames | `framestream reader error: unexpected EOF` (ERROR) |
| `bodyrst` | length prefix and 48 of 96 body bytes | RST | 3 whole frames | `framestream reader error: … connection reset by peer` (ERROR) |

The cut frame (`cut-0.<variant>.cut.test`) was never printed, and no garbled
record appeared. Source (DNS-collector 3.2.3 requires
`github.com/dmachard/go-framestream` v1.8.0):

- `go-framestream` v1.8.0, `framestream.go`, `readFrame`: the length and the body
  are read with `io.ReadFull`. Its error is returned with a `nil` frame.
  `io.ReadFull` returns `io.ErrUnexpectedEOF` when EOF comes after some, but not
  all, of the bytes.
- `go-dnscollector` v3.2.3, `workers/dnstapserver.go`, `HandleConn`: on an error
  from `RecvFrame` the loop logs it, flushes the batch of whole frames, and breaks.
  `frame.Data()` is reached only when the error is `nil`.

A cut that falls exactly on a frame boundary is an ordinary close (`io.EOF`). The
lab collector therefore keeps every whole frame before a cut and discards the cut
frame, which is what B1 and B2 assume.

## Case 4: the receiver STOP/FINISH check (task F)

- The driver used `Start-LabReceiverStopCapture`, `Stop-LabReceiverStopCapture`
  (`scripts/performance/LabCommon.psm1`), and `receiver_stop_finish.py` around
  every service stop: 14 stops on "before", 17 on "after", and 3 check runs.
  All 34 gave `confirmed`: STOP from the service, FINISH from the collector on the
  same port, then a FIN from each side. Every stop took 0.30 to 0.37 s and ended
  `Stopped` with exit code 0.
- No fix was needed. `python -I scripts/performance/test_receiver_stop_finish.py`
  passes (20 tests).
- `windows-parser-stage.ps1` itself was not run. It needs a parser-ramp campaign
  (a receiver READY receipt and the parser-ramp collector, whose raw output goes to
  a file, against the null rule). Its preflight also requires `StartName`
  `LocalSystem` (the MSI installs `NT AUTHORITY\LocalService`), and
  `Get-LabVm -CheckResources` fails because `lab.json` says 2 vCPUs. These checks
  are older than task F.
- **Ordering with `reconcile-parser-ramp.py`.** There is no conflict. The stage
  script runs the capture and the service stop in its `finally` block. The
  receiver's `finish` step (`ubuntu-parser-ramp-control.py`) runs later and needs
  `--sender-stopped`. It samples for a plateau with no established peer on port
  6000 (`pre_stop_publication_samples`), and only then stops the collector. The
  "graceful stop" in the reconcile error text is the collector's stop. The service
  closed its connection at every stop in this run, so no peer remained.

## Findings against the plan

| Item | Expectation | Result |
| --- | --- | --- |
| F1 | "Before": a blocked write stops the spill, so `queue_dropped` rises while the spool has room. | **Confirmed.** 218,681 frames at a 2 MB/s link, 642 to 65,569 with 100 ms of delay, 121,603 in an 8 s pause, 120,346 to 136,204 at each outage. The spool held at most 190 MB of 4,096 MiB at the time. A link at 78 % of R_in with no delay lost nothing: there the writes were short (16 ms). |
| F2 | "Before": a slow receiver looks dead, so it disconnects. | **Confirmed for a stall, not for a rate limit.** The 8 s pause ended the session (write 2.01 s). Rate limits down to 0.4 MB/s gave writes of up to 1.12 s and no disconnect. |
| F5 / H4 | The fixed 1 MiB `SO_SNDBUF` limits a long path. | **Confirmed.** At 50,000 QPS with 100 ms and a 40 MB/s link, "before" spilled 2,114,216 frames and lost 53,076; "after" kept up (10.09 MB/s) and lost none. |
| Criterion 1 | "After": no `queue_dropped` while the spool accepts blocks. | **Held** in every slow, paused, and outage run (10 runs) and in 5 of 6 control runs. One control run had 10,822; see [Open items](#open-items). |
| Criterion 2 | "After": no disconnect while bytes progress, and a stall shorter than the timeout loses nothing. | **Held.** 0 disconnects in all shaped runs and in the 8 s pause, which lost nothing (gap +14). |
| Criterion 3 | Detection counters show saturation and age. | **Held.** `link_saturated` set in every saturated run and in no control run; `spool_oldest_age_ms` up to 84 s. |
| Spool takes the overflow | "After" | **Held.** 0.6 to 14.2 million frames spooled and all replayed; `spool_dropped` 0. |
| V3-M-A (B2) | Replay drains faster than the capture on a Windows Server guest at 2 busy threads per CPU. | **Confirmed.** About 700 MB/s (1.46 GB in 2.09 s) at 100 % CPU, against a 10.3 MB/s capture. |
| Risk 6 | The collector discards a cut frame. | **Confirmed** (case 3). |
| Risk 9 | The 128-block FIFO covers a Server quantum at full rate. | **Not decided.** No spill at 10.3 MB/s with every CPU busy. 41 MB/s was not run. |
| Risk 2 | The disk can take R_in. | **Held at 10.3 MB/s**: 0 `spool_errors`, no `queue_dropped` from disk, up to 1.44 GB on disk. |
| Risk 15 | `send_queue_depth_max` rises during a drain. | **Not seen.** It stayed at 2 in the 50,000 QPS drains, and did not rise in the busy drains. |

## Open items

1. **One `queue_dropped` burst on "after" with a fast link.** In the first 50k
   control run (40 MB/s, no delay), 10,822 frames went to `queue_dropped` at the
   end of the load. In that second, `send_queue_depth_max` reached 128 (the parser
   channel was full), the receiver got 0.2 MB, `spooled` did not change, guest
   CPU was 25 to 31 %, and the service process used 1.5 to 3 % of a CPU. So the
   router took no parser block for about 1 s, and did not spill. Two reruns and
   an unshaped run had 0. Later analysis (2026-11-04, task L1D in the
   [plan](../plans/slow-receiver-plan.md#l-1-and-l-6-results)) found no cause in
   the code, and found that the whole service process ran for only about 1.5 % of
   a CPU in the second before the drops. The new counters
   `router_wait_max_recent_us` and `router_pass_max_recent_us`, and a WPR trace,
   are the next step if it recurs.
2. **A spill at the start of the load, with lab shaping on.** In 5 of 6 runs at
   50,000 QPS with an `ifb` filter and a fast link, both builds, the receiver got
   no bytes for about 2.5 s after the load started, and 0.2 to 0.6 million frames
   spilled ("after": no loss). The unshaped run had no spill. This points at the
   lab's ingress shaping, not at the service, but it was not proven.
3. **`link_saturated` during a silent outage.** With the receiver's packets
   dropped, the session stays up until the 15 s stall timeout. The backlog grows
   with the session up, so `link_saturated` was set 11.4 to 12.0 s into each
   outage, 3.4 to 3.6 s before `dnstap receiver connection lost` (three runs). The monitoring guide says
   that growth during an outage is not saturation. That holds only after the
   session ends. Task W may want to say so.
4. **Uncounted loss at a failed connection** is up to about 4.2 MB with the
   autotuned send buffer on a busy host (case 2). Design risk 4 predicted the
   direction, not the size.
5. **`send_queue_depth_max` at 10,000 QPS** read 33 to 61 with no loss in every
   run, also unshaped. Later analysis (task L1D) traced it to one ETW flush burst
   of about 2 MB waiting while the idle router wakes. It is not a defect.
6. **Follow-up runs L1 and L5 (2026-11-04).** See
   [Follow-up runs L1 and L5](#follow-up-runs-l1-and-l5-2026-11-04). Item 1: the drop did
   not recur in 14 runs, and a related stall of the event delivery did (8 runs). Risk 9
   held in every run that reached the link, but 41.2 MB/s on a fully busy host was not
   reached.

## Deviations from the plan

- Rate limits are twice the brief's example values, because each query makes two
  frames (see [Rates](#rates)).
- The outage used an `nft` table instead of `iptables`, so the receiver's
  ruleset is restored exactly.
- Added runs: the 8 s collector pause (F2 needs a stall), two reruns and an
  unshaped run of the 50k control, the idle outage on both builds, and a 120 s
  busy outage with 0.1 s receiver samples.
- The load client is `dns-stress` from the current source, not the binary at the
  `lab.json` path, which could not reach 10,000 QPS.
- The STOP/FINISH check used the `LabCommon.psm1` functions, not
  `windows-parser-stage.ps1` (see case 4).
- The first "before" busy outage has no CPU sample; it was repeated with one.

## Limits

- One run per case, except the 50k control (three runs) and the busy outage
  (two runs per build).
- Plaintext only. No TLS, so the TLS hand-back paths were not run.
- Rates of 10,000 and 50,000 QPS only. 41 MB/s (risk 9) was not run.
- Receiver bytes are sampled each second; a drain shorter than one sample has
  only an upper bound on its time, except in the 0.1 s run.
- No frame order or duplicate check at the receiver: the collector wrote
  nothing, so the check is on byte counts.
- The rate limit is a network limit (`tbf`), so TCP sees a slow path, not a
  closed receive window. Only the 8 s pause closes the window.
- No eviction at the spool cap: the largest backlog was 1.44 GB.

## Lab state after the run

- **Guest:** the "after" MSI (`13972d8`, ProductCode
  `{0FD5A881-9EF0-4835-8A01-E9DEC1B6BC49}`) is installed, with
  `RECEIVER=172.27.215.60:6000`, `SESSION_NAME=EtwDnstap-Lab`, and
  `SERVICE_ARGS="--plaintext --stats-interval-seconds 5"`. The `ImagePath`
  matches the one recorded before the run. The service is running as
  `NT AUTHORITY\LocalService`, SID type restricted, with the same recovery
  actions. The spool holds only `cursor.bin`. The major upgrades kept the
  `backup` folder; neither install was an uninstall, so no data folder was
  deleted. The busy processes, the counter samplers, and the staging folder are
  gone.
- **Receiver:** `tc qdisc show`, the `eth0` ingress filters, `iptables -S`
  (filter and mangle), `ip6tables -S`, the link list, and the loaded
  traffic-control modules match the recorded state line for line. `nft list
  ruleset` matches too (only `inet dnstap_lab`). The `ifb`, `sch_tbf`,
  `sch_netem`, `sch_ingress`, `act_mirred`, and `cls_u32` modules that the run
  loaded were unloaded. The collector config is unchanged (modified
  2026-11-02), with its output to null. The collector process is the same one
  (the pause stopped and continued it). It receives the service's frames again.
  The temporary collector files are deleted.
- The checkpoints `slow-receiver-L-pre-2026-11-04` on both guests are kept.

## Follow-up runs L1 and L5 (2026-11-04)

Two follow-up tasks on the same lab, after the router wait and pass counters
([plan](../plans/slow-receiver-plan.md#l-1-and-l-6-results)) were built:

- **L1.** Repeat the L case `after-50k-ctl-d0` (open item 1, finding L-1) with the new
  counters, a WPR trace, and guest and host counters, to find out what holds the
  service.
- **L5.** Run design risk 9: does the 128-block link FIFO cover a Windows Server
  quantum at 41.2 MB/s?

The raw data is lab evidence under `target/performance/slow-receiver-L1L5-20261104/`.
It is not in the repository. Result in one line: the L-1 drop did not recur, but a
related stall did, in 8 of 14 runs. It is a gap in the delivery of events to the
service, not a late router wake. Risk 9 held in
every run that reached the link at full rate, but the runs that tested it hardest
never reached it.

### Method differences from L

| Item | L | Follow-up |
| --- | --- | --- |
| Build | `13972d8` | `1cc66d8` (HEAD), MSI built with `scripts/build/Build-Msi.ps1`. MSI SHA-256 `BB2F578ED98F1534E5A325230EB1391707463FF89A0B24683ACEEBA72F3587E3`, executable SHA-256 `12ACC3994A003A6A78BEAD1EF616E9D57B0BDEE3359F2DF039F925D3D263A79F`. Installed with the same three MSI properties, exit 3010, guest restarted. After the restart the service ran the new hash, and `typeperf -q EtwDnstap` listed "Router Wait Peak (microseconds)" (ID 67) and "Router Pass Peak (microseconds)" (ID 68). |
| Checkpoints | `slow-receiver-L-pre-2026-11-04` | New `slow-receiver-L1L5-pre-2026-11-04` on both guests, taken before any change. All earlier checkpoints kept. |
| Client | `dns-stress`, 4 threads | The same binary. 4 threads for 50,000 QPS, 8 for 100,000, 16 for 200,000. Its `metrics.jsonl` is kept for every run. |
| Guest counters | `\EtwDnstap(*)\*`, processor, queue length, service CPU | Also at 1 s: `\Memory\Pages/sec`, `Page Faults/sec`, `Available MBytes`, `Pages Input/sec`, and for the process instance `etw-dnstap` (checked: one instance, no suffix) `Page Faults/sec`, `Working Set`, `% Processor Time`, `Thread Count`. |
| Host counters | none | Read-only, at 1 s: `\Hyper-V Hypervisor Virtual Processor(DNSTap-WS2025*)`, instances `DNSTap-WS2025:Hv VP 0` to `Hv VP 5`: `CPU Wait Time Per Dispatch`, `% Guest Run Time`, `CPU Contention Time Per Dispatch`, `CPU Wake Up Time Per Dispatch`. |
| WPR | none | Runs r02, r04, r06: `wpr -start CPU -start DiskIO` (memory mode, circular), stopped 5 s after the load ended. This did not work: the CPU profile with stacks wrote about 90 MB/s, so the buffer held only the last 5 s of an 85 s run. Those traces were deleted. Runs r07, r08, r10, r11, r13, r14: a custom profile in file mode (context switch, ready thread, page fault, hard fault, disk I/O events, no stacks, about 150 MB for a run), and in r13 and r14 also the 1 ms sampled profile with stacks. Stopped 5 s after the load ended. `wpr -cancel` on any error path. |
| ETL kept | n/a | The rule was: only for `queue_dropped` > 0 or a router peak above 250,000 us. No run met it, so I kept the file-mode traces of every run that spilled (r07, r10, r11, r13, r14, 0.77 GB in all) and deleted the others. |
| Analysis of traces | n/a | Windows Performance Toolkit `xperf` on the host, text dump of the events and `-a profile -detail` with the Microsoft symbol server. |

The L1 case is unchanged: 50,000 QPS, 4 client threads, 60 s load, `tbf rate 320mbit
burst 256kb latency 400ms` on the receiver ingress through `ifb`, no delay, 30 s tail.
The receiver collector config was checked before the first run: the devnull logger and
`StandardOutput=null`. Delivery was counted from `ss -tin` `bytes_received`
(103.015 bytes a frame), never from the collector. The driver and analysis scripts are
in `target/performance/slow-receiver-L1L5-20261104/scripts/`.

### L1 results

14 runs of the L case: 5 without WPR (r01, r03, r05, r09, r12), 9 with WPR (r02, r04,
r06 in memory mode, r07, r08, r10, r11 in file mode, r13, r14 with stacks). "Stall" is a
sampled second with events received below half of 100,000. "Spill" is the second in
which `spooled` first rose. The peaks are the largest of the 1 s counter samples.

| Run | WPR | Encoded | Delivered (gap) | `queue_dropped` | Spooled | Peak `send_queue_depth_max` | Router wait peak (ms) | Router pass peak (ms) | Write wait peak (ms) | `intake_dropped` | Min events/s, last 10 s | Min service CPU %, last 10 s | Stall, s into load | Spill, s into load |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| r01 | no | 5,999,976 | 5,999,989 (+13) | 0 | 330,964 | 42 | 65.7 | 16.7 | 3.2 | 0 | 99,887 | 3.1 | 12-13, 16 | 15 |
| r02 | memory, 5 s | 5,999,994 | 6,000,007 (+13) | 0 | 0 | 2 | 66.1 | 0.9 | 2.9 | 0 | 99,526 | 6.2 | none | none |
| r03 | no | 5,956,817 | 5,956,894 (+77) | 0 | 539,079 | 54 | 69.5 | 19.8 | 3.1 | 43,159 | 99,557 | 7.7 | 22-25 | 3, 24, 32 |
| r04 | memory, 5 s | 5,939,561 | 5,939,599 (+38) | 0 | 332,221 | 47 | 65.4 | 16.4 | 3.9 | 60,439 | 99,615 | 6.2 | 7-10 | 8 |
| r05 | no | 5,976,431 | 5,976,481 (+50) | 0 | 589,665 | 37 | 66.2 | 16.5 | 3.4 | 23,549 | 1 | 0.0 | 12-13, 54-55 | 3, 17, 59 |
| r06 | memory, 5 s | 5,960,513 | 5,960,507 (-6) | 0 | 101,498 | 13 | 65.8 | 21.2 | 4.0 | 39,447 | 99,891 | 6.1 | 23-25 | 26 |
| r07 | file | 5,999,968 | 5,999,980 (+12) | 0 | 219,785 | 42 | 68.2 | 16.4 | 3.5 | 0 | 99,036 | 12.4 | 44 (trace: 42-49) | 50 |
| r08 | file | 5,999,984 | 5,999,997 (+13) | 0 | 0 | 2 | 66.0 | 0.4 | 2.6 | 0 | 99,188 | 7.8 | none | none |
| r09 | no | 5,999,992 | 6,000,005 (+13) | 0 | 245,538 | 36 | 65.7 | 15.6 | 3.2 | 0 | 99,902 | 4.6 | none | 3 |
| r10 | file | 5,970,698 | 5,970,763 (+65) | 0 | 196,361 | 27 | 66.1 | 12.7 | 3.0 | 29,282 | 99,873 | 7.7 | none | 3 |
| r11 | file | 5,997,534 | 5,997,554 (+20) | 0 | 245,688 | 38 | 65.6 | 16.9 | 4.5 | 2,422 | 99,753 | 9.3 | none | 3 |
| r12 | no | 5,999,984 | 5,999,997 (+13) | 0 | 148,370 | 23 | 65.7 | 11.7 | 3.2 | 0 | 99,519 | 3.1 | 7 | 13 |
| r13 | file + stacks | 5,955,273 | 5,955,368 (+95) | 0 | 159,632 | 36 | 66.0 | 13.1 | 3.1 | 44,719 | 99,450 | 7.7 | none | 3 |
| r14 | file + stacks | 5,999,988 | 6,000,001 (+13) | 0 | 250,706 | 29 | 66.7 | 18.3 | 3.2 | 0 | 99,802 | 6.2 | 12-13 (trace: 11-22) | 22 |

Every run: `spool_dropped` 0, `disconnects` 0, `etw_lost` 0, stop and FINISH confirmed,
client timeouts 0, client rate 49,972 to 50,032 a second. The positive gap of +12 to
+95 is the 84-byte handshake of each connection and a few replayed frames, as in L.
The small `Min events/s` values in r05 (1) are the stall at 54 s, 6 s before the load end.

#### Findings

1. **`queue_dropped` did not recur.** It was 0 in all 14 runs (about 83 million
   frames). L-1 is not reproduced. The closest run is r05: a stall at 54 s into the
   load (events received 1 a second, service CPU 0 %), then a spill of 220,885 frames at
   59 s and 23,549 frames in `intake_dropped`. That is the L-1 pattern one step short of
   the 128-block parser channel overflowing.
2. **A delivery stall recurred in 8 of 14 runs** (r01, r03, r04, r05, r06, r07, r12,
   r14), with and without WPR, at any time in the load. In one to three consecutive
   samples the events received per second fall to 0 to 58,000 (nominal 100,000), the
   send rate to 0.0 to 3 MB/s, and the service CPU to 0 to 3 %, while the guest stays 24
   to 27 % busy and `dns-stress` sees every answer. This is the L-1 picture (events
   1,900 a second, send rate 0.2 MB/s, service CPU 1.5 %). The receiver got nothing for
   about 1.5 s in r01 and r07. The events missing in the stall (about 170,000 to
   250,000, which is 17 to 26 MB) come back later in one burst of 150,000 to 250,000
   events in a second (r01: 215,000 and 243,000; r07: 251,000; r14: 240,000).
3. **The stall is not a late router wake or a blocking router call.** The router wait
   peak was 65.4 to 69.5 ms in all 14 runs (the wait times out at 50 ms, so this is
   the normal value plus jitter). The router pass peak was 0.4 to 0.9 ms with no spill
   (r02, r08) and 11.7 to 21.2 ms in runs that spilled (the spool file writes).
   Neither moved in a stall.
4. **The service threads were idle, not held.** The traces of r07 and r14 show, in the
   stall: `etw-consumer`, `dnstap-parser`, `dnstap-sender`, and `dnstap-link` all wait
   with their normal timeouts, and no thread runs for a whole second. `etw-consumer` is
   woken about 1,600 times a second by the `System` process, the same as outside a
   stall, but it uses about 5 ms of CPU a second instead of 35 to 40. After the gap
   (2 s in r07, 1 to 2 s in r14) the events come at the normal rate of 100,000 a
   second for 5 s (r07, 44.7 to 49.8 s) to 9 s (r14), but only in once-a-second
   batches (the 1 s ETW flush interval): the threads use 2 to 5 times less CPU for the
   same rate (parser 23 to 28 ms a second instead of 90 to 130), the service
   working set is 150 MB instead of 92 MB, and the process fault counter reads about
   8,700 a second. Then one burst clears it (r07 at 49.8 s: 251,000 events, 21 hard
   faults, working set back to 92 MB, a spill). Over the load window of r07, r10, r11,
   r13, and r14 the longest wait of any service thread between ready and running was
   0.6 to 1.6 ms. So the threads never waited for a processor.
5. **Not the host, memory, or paging.** `CPU Wait Time Per Dispatch` stayed in its
   normal range (1,700 to 4,500) with no change in a stall; `CPU Contention Time` and
   `CPU Wake Up Time` were 0 in every run. `Available MBytes` stayed above 1,250 (above
   1,950 outside r02), `Pages Input/sec` was 300 to 420 at most
   (one reading of 10,724 in r02, a clean run, while WPR wrote), and the traces of r07 and r14 show
   21 hard faults in the one second of the catch-up burst and none in the stall. The process page
   fault counter read 8,700 to 12,500 a second in the spill runs while the working
   set was about 150 MB, and 5,200 to 5,300 in the clean runs r02 and r08. The traces
   show few fault events. That disagreement is not resolved, and the faults are not
   the cause: they start with the stall.
6. **The ETW buffers did not fill, and nothing was lost.** `% ETW Buffers Used` stayed
   at 6 to 11 %, `etw_lost` was 0, and `encoded` equals `received` in every run. So
   the events were delayed, not dropped, before they reached the service. The extra
   working set (about 58 MB) suggests that they wait in the memory of the service
   process, in the ETW consumer library, but this was not shown. The `dns.exe`
   process kept its CPU (about 900 ms a second) and kept calling `EtwpWriteUserEvent`
   at about the normal rate (sampled profile of r14), so the events are produced in the
   stall. The cause of the late delivery is not known. One candidate is the timestamp
   ordering that `ProcessTrace` does across the per-processor buffers, which waits for
   the 1 s flush when one buffer is quiet. This was not tested.
7. **Spills have two sources.** In r03, r05, r09, r10, r11, and r13 a spill of 150,000
   to 220,000 frames came 2.5 to 3.5 s after the load started, with no stall: this is
   finding L-2 (lab shaping at the load start), unchanged. In r01, r03, r04, r05, r06,
   r07, r12, r14 a spill came 1 to 11 s after the stall began, when the catch-up burst
   arrived: 17 to 26 MB at once is more than the parser channel (8 MiB) and the link
   FIFO (8 MiB) hold while the shaped link drains 40 MB/s, so the FIFO filled and the
   router spilled. `intake_dropped` was above 0 in 7 runs (2,422 to 60,439): four with
   a stall (r03, r04, r05, r06) and three with the load-start spill (r10, r11, r13).
8. **The stall is not caused by the lab shaping.** A 30 s probe at 200,000 QPS with no
   shaping (`l5-probe-200k`) had one stall at 31 s (events 305 a second, service CPU
   0 %) and no spill. The six unshaped L5 runs at 60 s had none, apart from the
   client's own stall noted below.
9. **Verdict on L-1.** What held the service for about 1 s was not a late wake, a long
   router pass, a processor shortage, or paging. The service had no work: events
   reached it about 2 s late, and the delay was cleared with one burst. A burst larger
   than the parser channel (128 blocks) produces `queue_dropped`, which is what L-1
   recorded once. Here the same burst produced a spill and `intake_dropped`, never
   `queue_dropped`. That the L-1 stall had this cause is likely, not proven. The
   cause of the gap in the ETW delivery is open.

The normal baseline of the new counters is in finding 3. In the L5 runs (below) the
router wait peak was 66 to 88 ms on an idle guest, 140 ms on a busy guest at 100,000 QPS,
and 195 to 216 ms with 12 busy threads at 200,000 QPS, where the pass peak stayed under
1 ms. That is what a late wake looks like (the wake lost a quantum), and it is not what
the L1 stalls looked like.

### L5 results

Design risk 9, link unshaped (no `ifb`), 60 s load. "Busy" is 12 `powershell.exe`
processes running `while($true){}` at normal priority, from 3 s before the load to its
end. Runs `l5x-` were added to find the busy level at which 41 MB/s still reaches the
link. `l5-probe-200k` is the 30 s test of the client. The client reached 100,000 and
200,000 QPS in every run (`metrics.jsonl`: mean 99,999 and 200,000 to 200,001 a second,
lowest second 99,912 and 199,883). It does not stall the rate except as noted.

| Run | Busy | QPS (client mean/s) | Encoded | Delivered (gap) | `queue_dropped` | Spooled | Peak `send_queue_depth_max` | Router wait peak (ms) | Router pass peak (ms) | Write wait peak (ms) | `link_saturated` | Rx MB/s | `etw_lost` | `intake_dropped` | Events received/s mean (min) | Guest CPU % mean (max) | Proc. queue mean (max) | Service CPU % mean |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| idle 100k | 0 | 100,000 (100,000) | 11,986,312 | 11,986,333 (+21) | 0 | 0 | 2 | 66.1 | 0.4 | 0.4 | 0 | 20.4 | 13,626 | 0 | 199,752 (196,770) | 40 (54) | 0.5 (3) | 30 |
| idle 200k | 0 | 200,000 (200,001) | 23,891,395 | 23,891,131 (-264) | 0 | 0 | 4 | 80.9 | 1.5 | 0.6 | 0 | 40.4 | 103,207 | 0 | 395,256 (236,138) | 71 (83) | 2.1 (7) | 48 |
| busy 100k | 12 | 100,000 (99,999) | 11,610,815 | 11,610,756 (-59) | 0 | 0 | 50 | 139.8 | 0.2 | 1.6 | 0 | 19.6 | 4,673 | 384,496 | 199,685 (186,272) | 100 (100) | 17.3 (27) | 10 |
| busy 200k | 12 | 200,000 (200,000) | 668,068 | 668,181 (+113) | 0 | 0 | 23 | 216.1 | 0.5 | 0.7 | 0 | 1.1 | 23,330,343 | 0 | 10,568 (0) | 100 (100) | 19.0 (45) | 1 |
| busy 200k, repeat | 12 | 200,000 (200,000) | 714,629 | 714,488 (-141) | 0 | 0 | 20 | 195.2 | 0.3 | 1.0 | 0 | 1.2 | 23,285,347 | 0 | 11,404 (0) | 100 (100) | 23.3 (42) | 1 |
| busy 6, 200k (extra) | 6 | 200,000 (196,618) | 9,454,259 | 9,454,057 (-202) | 0 | 0 | 76 | 194.5 | 1.0 | 2.0 | 0 | 16.1 | 13,680,921 | 398,820 | 159,476 (10,577) | 100 (100) | 10.8 (20) | 11 |
| busy 3, 200k (extra) | 3 | 200,000 (194,882) | 23,280,959 | 23,280,798 (-161) | 0 | 0 | 13 | 66.0 | 1.1 | 1.8 | 0 | 39.4 | 133,910 | 0 | 387,266 (30,249) | 88 (93) | 3.7 (8) | 42 |
| probe 200k, 30 s | 0 | 200,000 (199,998) | 11,931,399 | 11,931,411 (+12) | 0 | 0 | 5 | 87.8 | 2.3 | 1.3 | 0 | 38.1 | 68,243 | 0 | 382,575 (305) | 69 (81) | 1.8 (6) | 44 |

Each query makes two events, so 200,000 QPS is about 400,000 events a second and
41.2 MB/s on the wire. Every run: `spool_dropped` 0, `disconnects` 0, stop and FINISH
confirmed, `spool_errors` 0. The `etw_lost` and `intake_dropped` columns are losses
before the sink and are not risk 9. The client's own timeouts (DNS answers it did not
get) were 0 to 5,344 a run (0.04 % at most). The busy 3 run has one client second at
0 and the next at 114,029 (a client stall, 22 to 23 s): the `Events received/s` minimum
of 30,249 in that run is the client, not the service. The `Delivered (gap)` of -59 to -264
at 100,000 and 200,000 QPS is the few frames in flight at the service stop.

#### Findings

1. **Risk 9 did not fail in any run.** `spooled` was 0 and `queue_dropped` was 0 in all
   8 runs, so the link FIFO never filled. The write wait peak was 0.4 to 2.0 ms: the
   link thread never waited on the socket for long. That includes the two runs at
   full rate: 40.4 MB/s on an idle guest (71 % busy) and 39.4 MB/s with 3 busy threads
   (88 % busy). In the busy 3 run the router wait peak was 66 ms, so no thread lost a
   quantum.
2. **The hardest case was not reached.** With 12 busy threads at 200,000 QPS the
   capture path starves before the link: the guest is 100 % busy, the service uses 1 % of a
   CPU, and 97 % of the events are lost in ETW (`etw_lost` 23.3 million), so only 1.1
   to 1.2 MB/s reach the link. With 6 busy threads 16 MB/s reaches it, and 13.7 million
   events are lost in ETW. So the case "full 41.2 MB/s, and a link thread that loses a
   quantum" never happened, and I could not make it happen with CPU load alone. The
   router did lose more than a quantum in the busy 200k runs (wait peak 195 and
   216 ms against 187.5 ms), and the FIFO still had no spill, but at 1 MB/s that proves
   nothing about 41 MB/s.
3. **Busy at 100,000 QPS** (19.6 MB/s, every CPU busy, queue length 17): no spill, no
   `queue_dropped`, router wait peak 140 ms, but 384,496 events (3.2 %) lost at the intake
   ring and 4,673 in ETW: the parser does not keep up on this guest. That is not a
   link FIFO loss.
4. **No spill happened, so there is no cause to name.** A write wait that stays small
   while the FIFO fills would show the link thread not running; no run showed it.
5. **Unshaped, the ETW stall of L1 did not appear in the 60 s runs**, and appeared once
   in the 30 s probe (finding 8 of L1).

Verdict: risk 9 **held** for what the lab can generate (41 MB/s with up to 3 busy threads
on 6 vCPUs, and 20 MB/s with 12), and is **still open** for 41.2 MB/s on a fully busy
host, because the capture path loses the events before they reach the link.

### Deviations and limits

- WPR in memory mode (the built-in `CPU` and `DiskIO` profiles) held only the last
  5 s, so r02, r04, and r06 have no usable trace. The file-mode profile is lighter
  than `CPU`/`DiskIO` (no stacks except in r13 and r14), so it may perturb less, or
  differently.
- Counters are 1 s samples and the guest times move in 15.6 ms ticks. A stall of
  1 s can fall in two samples, and a burst can hide it (r07, r10).
- The cause of the late ETW delivery was not found. The traces rule out
  processor, memory, and host causes for the service threads, but they do not show where
  the events wait.
- r08 (a clean run) and r02 have no ETL. The ETL of r10, r11, and r13 shows no stall
  (their spills are the load-start spill, L-2).
- Extra runs: `l5-probe-200k`, `l5x-busy6-200k`, `l5x-busy3-200k`.

### Lab state after

- **Guest:** the HEAD MSI is installed, ProductCode
  `{81B6B84B-8F14-4885-93EF-E98F0951151E}`, MSI properties
  `RECEIVER=172.27.215.60:6000`, `SESSION_NAME=EtwDnstap-Lab`,
  `SERVICE_ARGS="--plaintext --stats-interval-seconds 5"`. The executable SHA-256 is
  `12ACC3994A003A6A78BEAD1EF616E9D57B0BDEE3359F2DF039F925D3D263A79F`. The service is
  running as `NT AUTHORITY\LocalService`, SID type restricted, recovery
  `restart/5000`, connected to the receiver. The spool holds only `cursor.bin`. The
  busy processes, the samplers, WPR (`wpr -status`: not recording), and the staging
  folder are gone.
- **Receiver:** `tc qdisc show`, the `eth0` ingress filters, `iptables -S` (filter and
  mangle), `ip6tables -S`, `nft list ruleset`, the link list, and the loaded
  traffic-control modules match the step 0 record line for line. The `ifb`, `sch_tbf`,
  `sch_ingress`, `act_mirred`, and `cls_u32` modules that the runs loaded were
  unloaded. The collector config is unchanged (same SHA-256 before and after), and its
  output still goes to null (`StandardOutput=null`, `StandardError=null`, devnull
  logger).
- **Checkpoints:** `slow-receiver-L1L5-pre-2026-11-04` on both guests, kept, with all
  earlier checkpoints.
