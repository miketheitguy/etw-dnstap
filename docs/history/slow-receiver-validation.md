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
   an unshaped run had 0. The cause is not known. A WPR trace of the service
   threads is the next step if it recurs.
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
   run, also unshaped. Not investigated.

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
