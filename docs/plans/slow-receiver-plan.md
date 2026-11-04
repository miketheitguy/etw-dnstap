# Slow and saturated receiver plan

> **Status:** Proposed; Waves 1 to 3 complete on 2026-11-03, gate G1 open · **Describes:** `main` at commit `a06f6fd` · **Evidence dated:** not applicable (source analysis; no lab run) · **Reviewed:** 2026-11-03

This plan covers a receiver that is connected but slow, and a link that carries less
than the capture rate. It records what the code does today and the cost of each
behavior. It lists the tasks that detect the condition and handle it, the agent and
effort for each task, and the decision gates between them.

A dead receiver is already handled: frames go to the spool and replay after the
reconnect. This plan does not change that path.

## Terms

| Term | Meaning |
| --- | --- |
| R_in | Frame bytes per second that the parser produces. |
| R_link | Bytes per second that the receiver and the network path accept. |
| Slow | R_link drops below R_in for a short time, or one write blocks for a long time. The receiver stays connected. |
| Saturated | R_link stays below R_in. The backlog grows for the whole period. |
| Stalled | The receiver accepts no bytes, but the TCP session stays open. |

## Current behavior

The `dnstap-sender` thread owns the link and the spool. It does every socket write
and every spool append (`src/sender.rs`, `SenderThread::run`). Socket writes are
blocking. Each write of up to 64 KiB has its own 2-second deadline
(`IO_TIMEOUT`, `DeadlineTcp`, `NetworkSink::write_framed` in `src/network.rs`).

`SenderThread::destination` sends a block to the spool when one of these is true:

- The link is down.
- The spool holds a backlog.
- The send queue holds 96 or more of its 128 blocks (`SPILL_BLOCKS`).

After the first spill, every new block goes to the spool until the backlog is empty.
This keeps the frames in capture order.

### Capacity at the 103-byte lab average

The send queue holds 128 blocks of about 64 KiB, about 8 MiB. The spill threshold
leaves 32 blocks, about 2 MiB, between the spill point and a full queue.

| Queries per second | R_in | Queue lasts | Headroom after the spill threshold |
| --- | --- | --- | --- |
| 10,000 | 2.06 MB/s | about 4.1 s | about 1.0 s |
| 50,000 | 10.3 MB/s | about 0.81 s | about 0.20 s |
| 200,000 | 41.2 MB/s | about 0.20 s | about 0.05 s |

These figures are calculations, not measurements.

## Findings

### F1. A slow write blocks the spill path

The spill check runs only between writes. While a socket write blocks, the sender
thread routes no blocks, so it cannot spill. One block can take 2 or 3 chunks of up
to 2 seconds each. One replay step of 256 KiB can take 4 or 5 chunks. A receiver
that accepts a little less than 64 KiB every 2 seconds never hits the deadline, but
it blocks the sender thread for seconds at a time.

At 50,000 queries per second, the 2 MiB headroom fills in about 0.2 seconds.
The queue then drops blocks in `queue_dropped`, while the spool still has room.
The spool cannot protect against this loss today. **This is the main defect.**

### F2. The service cannot tell a slow receiver from a dead one

A chunk that takes 2 seconds ends the session. The frames in the write buffer and in
the chunk are lost and counted in `disconnected_dropped`. The reconnect then waits
for the backoff and the TLS and Frame Streams handshakes. A collector pause of 2.5
seconds, for example a garbage-collection pause, therefore costs frames and causes a
reconnect. If the stall repeats, the reconnects repeat.

### F3. A saturated link turns the spool into a long delay line

When R_link stays below R_in, the backlog grows at R_in minus R_link. Every frame
goes through the disk. When the backlog reaches `--spool-max-mib`, the spool removes
its oldest segments. In the steady state:

- Loss is R_in minus R_link. It is counted in `spool_dropped` as old data.
- Delivered data is about `backlog / R_link` seconds old. At the 4,096 MiB default
  and 5 MB/s, that is about 14 minutes.
- The disk writes at R_in and reads at R_link at the same time.

No counter or log event says that the link is saturated. An operator must infer it
from `spool_bytes` growing for the whole run.

After a short slow period, the backlog drains in `B / (R_link - R_in)` seconds.
During that time all live data goes through the disk, even when R_link is only a
little above R_in.

### F4. The detection signals are weak

- `write_max_us` is the largest write since the start. It never resets, so it does
  not show the current state.
- `slow_writes` counts writes of 5 ms or more. It does not separate a receiver that
  closes its TCP window from a congested network path.
- No counter gives bytes sent, queue depth, the age of the oldest spooled frame, or
  the drain rate.
- The service does not read the TCP state of the socket. Windows provides
  `SIO_TCP_INFO`, which reports the round-trip time, retransmissions, and the time the
  connection was limited by the receiver window, the congestion window, or the
  sender. See [R1 results](#r1-results) for the versions.

### F5. A fixed send buffer can limit throughput on a long path

`Connection::connect` sets `SO_SNDBUF` to 1 MiB. On Windows, an explicit
`SO_SNDBUF` turns off dynamic send buffering (confirmed by R1). With one 1 MiB buffer, the most a
path can carry is about 1 MiB per round trip: about 10 MiB/s at 100 ms. That is below
R_in at 50,000 queries per second.

### F6. A frame counted as lost can still arrive

Found by task C1. When a socket write times out, the link counts the frames of the
chunk, and the frames written since the last flush, in `disconnected_dropped`. A
Windows send that timed out can still finish in the kernel, so some of those frames
reach the receiver. In the C1 run, 48 frames were offered and 48 arrived, and
`disconnected_dropped` was 1. No frame arrives twice, and the order is kept.
`disconnected_dropped` is therefore an upper bound on the loss, not an exact count. The
guides say a counted frame is lost. Task W corrects them. H1 removes the timed-out
send, because a non-blocking send reports exactly what it queued. Bytes that the kernel
accepted stay unknown at a failure.

### Behavior that is correct today

- Shutdown is bounded under a stalled link (`SHUTDOWN_LINK_BUDGET`, existing tests).
- Frames keep capture order while the spool holds a backlog.
- Each lost frame is counted in exactly one of `queue_dropped`,
  `disconnected_dropped`, and `spool_dropped`. A counted frame can still have
  arrived (F6).

Every change in this plan must keep these three properties.

## Options

### Detection (no change to data flow)

Detection does not drive routing. With H1, the depth of the link FIFO is the only
spill trigger, and the spill is passive. The counters tell the operator what a
passive spill hides: a link that never catches up turns the spool into a delay line
that removes its oldest data at the cap.

- D1. Add counters: `bytes_sent`, `queue_depth_max` (reset each stats interval), and
  `spool_oldest_age_ms`.
- D2. Report `write_max_us` per stats interval, not since the start.
- D3. Optional and deferred. Sample `SIO_TCP_INFO` once per stats interval. Report the round-trip time,
  retransmitted bytes, and the share of time limited by the receiver window, the
  congestion window, and the sender. This explains why the link is slow, for
  troubleshooting with the collector owner. No decision depends on it.
- D4. Add a `link_saturated` state. Set it when the backlog has grown for N
  consecutive intervals while the link is up. Log one event when it changes.

### Handling

- H1. **Keep the sender thread free of socket waits.** Two designs:
  - H1a. A link-writer thread. The sender thread routes blocks and owns the spool.
    It passes link blocks to a small FIFO that a new `dnstap-link` thread writes.
    When that FIFO is deep, the sender thread spills to the spool. Replay batches go
    into the same FIFO, so the order is kept. Socket I/O stays blocking.
  - H1b. Time-sliced writes. The socket is non-blocking. The sender thread writes
    for at most a short slice, keeps the unwritten tail, and routes queued blocks
    between slices. There is no new thread, but it needs a partial-write state
    machine, also through rustls.
- H2. **Liveness from progress.** End the session only when the receiver has accepted
  no bytes for a stall timeout, for example 10 to 30 seconds, not when one chunk
  takes 2 seconds. This needs H1, because the spool must absorb data during the
  wait. The shutdown budget stays as it is.
- H3. **A policy for a saturated link.** Choices:
  - H3a. Keep today's strict order, with the oldest data removed. This is the
    default.
  - H3b. Live first. Send new frames to the link, and replay the backlog only when
    the link has spare capacity. Frames arrive out of order.
  - H3c. Shed load by a rule, for example drop `CLIENT_QUERY` before
    `CLIENT_RESPONSE`, with a counter.
  Gate G2 chose H3a (see [Decisions](#decisions)).
  - Order and no loss together are possible only when the slow period ends before
    the spool fills: the spool cap must be at least the deficit (R_in minus R_link)
    times the duration.
  - While the link stays slower than R_in, every policy loses frames at
    R_in minus R_link. The policies differ only in which frames they lose, and
    whether order and freshness survive.
  - H3c, and sampling 1 in N, would need the `link_saturated` trend to drive data
    flow. That would reverse the decision that detection only observes.
- H4. **Send buffer.** Remove the fixed `SO_SNDBUF`, or use it only to raise a
  smaller autotuned value. Depends on R1.

## Tasks

The tasks run in waves. A wave starts when its inputs are ready. Code tasks run in
the main working tree, one at a time, so they do not conflict.

| ID | Wave | Task | Agent | Effort | Output |
| --- | --- | --- | --- | --- | --- |
| R1 | 1 | Research the Windows TCP facts for F4 and F5: `SIO_TCP_INFO` versions and fields, the minimum Windows Server version for each, the `windows-sys` features each needs, and how an explicit `SO_SNDBUF` affects send autotuning and `SIO_IDEAL_SEND_BACKLOG_QUERY`. Cite Microsoft Learn. | `general-purpose` | medium | Section "R1 results" in this file. |
| C1 | 1 | Add characterization tests that show F1, F2, and F3 with today's code: a write stall that drops frames in `queue_dropped` while the spool has room; a stall longer than 2 s that disconnects and counts in-flight frames in `disconnected_dropped`; and a sustained slow receiver that keeps every block on the spool path. The tests assert today's behavior and say which assertion H1 and H2 will change. | `voltagent-lang:rust-engineer` | high | New test file under `src/sender/`. Section "C1 results". |
| D | 2 | Implement detection D1, D2, and D4 in the stats log and the performance counters. Read only state the service already has. No change to data flow, and no new `windows-sys` feature. | `voltagent-lang:rust-engineer` | high | Code, tests, and guide updates in `docs/guide/monitoring.md` and `docs/guide/troubleshooting.md`. |
| A | 2 | Write the design for H1 and H2: compare H1a and H1b on ownership, order, accounting, shutdown bound, TLS partial writes, and test effort. Recommend one. Give the new constants and the state machine. Use the C1 tests as the acceptance tests. | `general-purpose` | high | `docs/design/slow-receiver.md` (status: proposed). |
| V | 3 | Review the C1 and D changes for correctness, loss accounting, and the project rules. | `voltagent-qa-sec:code-reviewer` | high | Findings to the coordinator. |
| G1 | 3 | Gate: the coordinator checks R1, C1, D, A, and V, then asks the user to approve H1 and H2. | coordinator | — | Decision recorded in this file. |
| G2 | 3 | Gate: the user chooses the saturated-link policy, H3a, H3b, or H3c. | user | — | Decision recorded in this file. |
| I1 | 4 | Implement the approved H1 and H2 design. Flip the C1 assertions. | `voltagent-lang:rust-engineer` | xhigh | Code and tests. |
| I2 | 4 | Implement H4: remove the fixed `SO_SNDBUF`, so Windows send autotuning stays on. Update the comment in `src/network/sockopt.rs`, or remove the function. Never set the buffer to 0. No H3 work: G2 kept H3a. | `voltagent-lang:rust-engineer` | medium | Code and tests. |
| L | 5 | Lab run. Limit the rate and add delay on the Ubuntu receiver with `tc` (`tbf` and `netem`). Run at 10,000 and 50,000 queries per second, before and after. Record loss by counter, spool growth, and delivered latency. Needs explicit authorization: it changes lab state. | `general-purpose` | high | `docs/history/slow-receiver-validation.md`. |
| T | deferred | Optional. Implement D3 with `TCP_INFO_v0`, and add the `SndLim*` shares only where `TCP_INFO_v1` is available (see R1). Start it only if an operator needs to know whether the receiver or the network path limits the link. | `voltagent-lang:rust-engineer` | medium | Code and tests. |
| W | 5 | Correct the counter meaning for F6 in `docs/guide/monitoring.md` and `docs/design/data-path.md`. Update `docs/design/data-path.md`, `docs/guide/configuration.md`, `docs/guide/tuning.md`, and the animation if stage 5 changes. | `voltagent-dev-exp:documentation-engineer` | medium | Guide changes. |

## Acceptance criteria

After Wave 4:

1. A socket write that blocks for any time does not cause `queue_dropped` while the
   spool accepts blocks.
2. A receiver that accepts no bytes for less than the stall timeout keeps its
   session, and loses no frames.
3. The stats log and the performance counters show when the link is saturated,
   and how old the oldest spooled frame is.
4. Order, loss accounting, and the shutdown bound stay as in
   [Behavior that is correct today](#behavior-that-is-correct-today).
5. `cargo test`, `cargo clippy`, and the CI checks pass.

## Out of scope

- Several TCP connections to one receiver. See
  [Parser and TCP threading analysis](parser-tcp-threading-analysis.md).
- Async I/O.
- Changes to the dead-receiver path, except where H2 changes when a session ends.

## Results

Gate G2 is closed: H3a stays the default, with H4. Gate G1 waits for the user's approval of
the design for task I1.

### R1 results

Task R1 ran on 2026-11-03 from Microsoft Learn and the `windows-sys` 0.61 source. The
coordinator checked the `windows-sys` gates and the install guide.

- **No minimum Windows version is documented.** `docs/guide/install.md` records only
  the Windows Server 2025 lab host. The minimum is an open question for gate G1.
- **`SIO_TCP_INFO` with `TCP_INFO_v0`** needs Windows Server 2016 or later. It gives
  `RttUs`, `MinRttUs`, `BytesInFlight`, `Cwnd`, `SndWnd`, `BytesOut`, `BytesRetrans`,
  `FastRetrans`, and `TimeoutEpisodes`. It has no field for "sender limited". A
  receiver-window limit shows only as a snapshot: `SndWnd` is 0, or `BytesInFlight`
  is at least `SndWnd`.
- **`TCP_INFO_v1`** needs build 20348 (Windows Server 2022). It adds the cumulative
  `SndLimTime*`, `SndLimBytes*`, and `SndLimTrans*` fields for the receiver window,
  the congestion window, and the sender. A v1 request on an older build is expected to
  fail with `WSAEINVAL`. Probe v1 once for each connection, then fall back to v0.
- **Cost:** one synchronous IOCTL, with no privilege check and no walk of the
  connection table. A call once per stats interval is negligible.
- **`windows-sys`:** the `SIO_TCP_INFO` constant and the `TCP_INFO_v0` and
  `TCP_INFO_v1` structs need only `Win32_Networking_WinSock`, which the crate
  enables. `WSAIoctl` also needs `Win32_System_IO`, which it does not enable today.
  `TCP_INFO_v*::TimestampsEnabled` is a Rust `bool`. Read into a local `#[repr(C)]`
  copy with a `u8` field, so a byte other than 0 or 1 cannot cause undefined behavior.
- **`SO_SNDBUF`:** Microsoft Learn says that dynamic send buffering is on "unless an
  application sets the SO_SNDBUF socket option". The fixed 1 MiB value therefore
  turns autotuning off (F5 confirmed). Microsoft's advice is to leave `SO_SNDBUF`
  unset. Never set it to 0, because a blocking send can then wait for a 200 ms delayed
  acknowledgment. `windows-sys` does not define the `SIO_IDEAL_SEND_BACKLOG_*`
  constants. `..._CHANGE` blocks, so it must not run on the writer thread.
- **Timed-out send:** Microsoft Learn says that after a blocking send times out, "the
  connection is in an indeterminate state and should be closed". Ending the session
  on any write error is therefore correct. H2 must not keep a session after a send
  times out. Its liveness rule must use short writes that do not time out, or
  non-blocking writes.

Recommendations: D3 uses `TCP_INFO_v0`, sampled between writes on the thread that
owns the socket. H4 removes the fixed `SO_SNDBUF`, and the lab compares the two.

### C1 results

Task C1 ran on 2026-11-03. It added `src/sender/saturation_tests.rs`, with three tests
that pass on today's code. Each assertion that H1 or H2 changes has a
`Changes with H1:` or `Changes with H2:` comment. The only production-file change is a
`#[cfg(test)]` copy of `IO_TIMEOUT` in `src/network.rs`. `src/sender/sender_tests.rs`
shares its helpers with the new file and adds a per-frame receiver delay.

| Test | Shows | Measured |
| --- | --- | --- |
| `a_stalled_write_overflows_the_queue_while_the_spool_has_room` | F1 | 196 frames offered, `queue_dropped` 51, `spooled` 0 at the drop. After the receiver resumed, all 128 queued blocks went to the spool and replayed. |
| `a_receiver_that_accepts_nothing_for_the_io_deadline_loses_the_session` | F2, F6 | The session ended after 2.11 s, with one disconnect and one reconnect. `disconnected_dropped` was 1, but all 48 frames arrived. |
| `after_a_spill_every_later_block_takes_the_spool_path_until_the_backlog_is_empty` | F3 | 157 frames offered, 17 went direct, then 140 took the spool path while connected. None was lost. |

The agent ran the tests 18 times on an idle machine, and 8 times with every core
busy. All runs passed. The coordinator ran them 3 more times, and they passed in about
2.3 s each. The whole library suite passes, and `cargo clippy --all-targets` and
`cargo fmt --check` are clean.

Known issue, not caused by C1: with every core busy, the existing test
`shutdown_with_a_stalled_link_and_queued_blocks_is_bounded_and_accounted` failed 4 of
20 whole-suite runs ("FINISH header read"). It passed 6 of 6 alone. Task V checks
whether it is a real defect or a timing margin in the test.

### D results

Task D ran on 2026-11-03. It only observes: no routing or data-flow code changed.

| Value | Meaning | Performance counter |
| --- | --- | --- |
| `bytes_sent` | Framed bytes the link accepted, live and replayed. | 61, and the rate 62 |
| `send_queue_depth_max` | The most blocks in the send queue in the last 60 s. `queue_depth_max` already names the event channel. | 63 |
| `write_max_recent_us` | The longest socket write in the last 60 s. `write_max_us` and `slow_writes` keep their meaning. | 64 |
| `spool_oldest_age_ms` | The age of the oldest unsent segment, 0 with no backlog. It is an upper bound on the age of the oldest frame. | 65 |
| `link_saturated` | 1 after 10 consecutive 1-second samples of growth in the unread backlog with the link up. It clears when the backlog is empty, or after 10 samples with no growth. | 66 |

- The sender thread keeps both peaks in 60 one-second buckets on its own clock. The
  stats log and the performance counters only read them, so neither reader resets a
  value for the other.
- Each change of `link_saturated` logs one event, with the backlog, the oldest age,
  and the bytes sent per second.
- New files: `src/sender/monitor.rs`, and its tests.

The coordinator checked these on 2026-11-03:
- `cargo test --lib`: 477 passed, 7 ignored.
- `cargo clippy --all-targets` and `cargo fmt --check` are clean.
- The changed and new files contain no host path.
- The regenerated `packaging/perfcounters/etw-dnstap-perf.rc` only adds the six
  counter strings.

### A results

Task A wrote [Slow receiver: link writer and progress liveness](../design/slow-receiver.md)
on 2026-11-03. It recommends H1a with a link writer driven by socket readiness.

- A new `dnstap-link` thread owns the session and writes with non-blocking sends, so
  no data send can time out (the R1 rule). The router keeps the spool and never waits
  on the socket.
- The spill is passive. A full 4-block link FIFO puts live blocks in a 96-block hold
  in memory. Only a full hold spills to the spool. This keeps today's 6 MiB of jitter
  slack.
- H2: a session ends after `--receiver-stall-seconds` (default 15, range 2 to 300) with
  no bytes accepted. `IO_TIMEOUT` covers only connect, the handshakes, and STOP/FINISH.
- Memory grows from 17.3 MiB to 30.7 MiB of blocks. The shutdown bound drops from
  about 5 s to about 3.2 s.

Coordinator review: the first draft spilled when the FIFO was full, which cut the jitter
slack to about 256 KiB. The revision added the hold. The revision is accepted.

The second revision added B1, the hand-back at a link failure:
- The link thread clears `up`. The router sends a `Fence` into the FIFO and writes
  nothing to the spool until `HandBackDone` arrives. The link thread returns the cut
  frame, the rest of the block in hand, and the live FIFO blocks.
- When the failure comes soon after a spill, the returned blocks are older than the
  backlog. They wait on an in-memory front list (at most 3 blocks) and are sent first
  after the reconnect. If shutdown comes first, they are lost and counted.
- With TLS, rustls is fed in pieces of at most 16 KiB. Only the frames of the
  unconfirmed piece can be lost.
- The pool is 229 blocks (30.4 MiB).

Coordinator review: a hand-back sends at most 4 messages, and the return channel holds
4. The handshake therefore cannot deadlock. The second revision is accepted. Lab task L
must confirm that the lab collector discards a frame cut by a failure.

### V results

Task V reviewed the C1 and D changes on 2026-11-03.
- **Verdict:** ship as observe-only. Nothing blocks.
- **Verified:** routing, loss accounting, the spool cursor and crash rules, and the
  shutdown bound are unchanged. Counter IDs 61 to 66 and string IDs 344 to 366 are
  added after the existing ones, so no existing ID moved. The hot path adds no
  allocation, lock, or system call per frame.

| Severity | Finding | Fix |
| --- | --- | --- |
| Major (confirmed by the coordinator) | `link_saturated` needs growth at 10 consecutive samples. Replay reads in 256 KiB batches, so some samples are flat on a link that is only a little too slow. The flag can then stay 0 while the spool fills and deletes old data. | Use a trend over 10 samples instead of a streak. |
| Minor | A reconnect inside one sample period hides the outage, and the outage growth can count as saturation. | Reset the window when `disconnects` changes. |
| Minor | `spool_oldest_age_ms` uses the wall clock for segments of this run. | Use a monotonic clock for this run. |
| Minor | One assertion in a C1 test can pass on a stale peak. | Wait for the exact value. |
| Minor, and 3 nits | The guides mix bytes and frames, and describe some gauges as rates. | Correct the wording. |

The flaky test `shutdown_with_a_stalled_link_and_queued_blocks_is_bounded_and_accounted`
has a timing margin in the test, not a product defect. It fails on `HEAD` too (16 of
30 runs with 48 busy loops). Under load, `stall_the_sender` mistakes a sender that is
waiting for the CPU for a stalled socket. The product stays bounded and counts
correctly.

The task D agent fixed all the findings and the flaky test on 2026-11-03:
- `link_saturated` now follows a trend over 11 samples. It sets when the newest
  backlog is larger than the oldest and at least 7 of the 10 steps rose. It clears
  when the backlog has not grown over the window, or at once when the backlog is
  empty. A lost session, an outage, or paused reads restart the window. A change is
  not undone within 10 samples.
- `spool_oldest_age_ms` uses a monotonic clock for the segments of this run.
- `stall_the_sender` now also requires 512 KiB accepted by the link, and no progress
  for 100 ms. The shutdown test accepts both outcomes that are correct. In both, it
  checks the shutdown bound and the loss accounting.
- Under heavy load (48 busy loops), the shutdown test passed 20 of 20, and the C1
  tests 20 of 20.

The coordinator checked the result: `cargo test --lib` passes (482 passed, 7
ignored), `cargo clippy --all-targets` and `cargo fmt --check` are clean, and the
changed files contain no host path.

### Decisions

- 2026-11-03, user: spilling to the spool stays passive. It is driven by
  backpressure on the link (H1), not by TCP measurements. D3 (`SIO_TCP_INFO`) moves
  to the deferred, optional task T.
- 2026-11-03, user: the stall timeout default of 15 seconds and the flag name
  `--receiver-stall-seconds` are accepted.
- 2026-11-03, user (gate G2): a saturated link keeps H3a, strict capture order with
  the oldest data deleted at the cap. H4 is added: remove the fixed `SO_SNDBUF`. No
  load-shedding policy is built. `link_saturated` stays observe-only, and it is the
  alert that the spool is filling.
- 2026-11-03, user: unsent data at a link failure follows option B1. The link thread
  gives unsent live data back to the router: the rest of the block in hand, and the
  live blocks in the link FIFO. The router appends it to the spool before the hold and
  every newer block. The link FIFO shrinks to 2 blocks, and at most one replay batch
  is in it at a time. Only the unsent rest of one replay batch can be lost, and only
  while a backlog drains. Re-reading replay batches (B2) is deferred, because it
  changes the spool's cursor and segment delete rules.
