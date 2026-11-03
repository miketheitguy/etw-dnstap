# TCP sink spool plan

> **Status:** Active · **Branch:** `tcp-sink-spool` · **Base:** `main` at commit `0f6db1e` · **Reviewed:** 2026-11-03

This plan stops the TCP sink from losing frames while the dnstap receiver is
down. Today `NetworkSink` drops every frame that arrives while it is
disconnected and counts it in `disconnected_dropped`. After this plan, a new
`dnstap-sender` thread owns the connection and a disk spool. Frames that
arrive while the receiver is down or slow go to append-only segment files and
are sent, oldest first, after the receiver reconnects.

Every agent reads [Rules for every agent](#rules-for-every-agent), the
[Decisions](#decisions), the [Design](#design), and then only its own role
section.

| Role | Section | Agent |
| --- | --- | --- |
| Orchestrator | [Orchestration](#orchestration) | The main session |
| Coding, stage C1 | [C1: Spool storage](#c1-spool-storage) | Rust coding agent |
| Coding, stage C2 | [C2: Sender thread and queued sink](#c2-sender-thread-and-queued-sink) | Rust coding agent |
| Coding, stage C3 | [C3: Configuration, wiring, and counters](#c3-configuration-wiring-and-counters) | Rust coding agent |
| Testing, stage T1 | [T1: Integration, property, and fuzz tests](#t1-integration-property-and-fuzz-tests) | Testing agent |
| Documentation, stage D1 | [D1: Documentation](#d1-documentation) | Documentation agent |
| Review, stage R | [R: Review](#r-review) | Review agent, once per stage |

## Decisions

The user made these decisions on 2026-11-02. Do not change them. If the code
cannot meet one, stop and report it.

1. **The spool is on by default.** The default folder is
   `C:\ProgramData\EtwDnstap\spool`, built from the `ProgramData`
   environment variable, falling back to `C:\ProgramData`. `--no-spool` turns
   it off. When the folder is missing or unsafe, the service logs a warning
   and runs without a spool, as it behaves today. It never fails to start
   because of the spool.
2. **Drop oldest when full.** At the size cap or the free-space floor, the
   spool deletes its oldest segment and counts the frames and bytes it
   discarded.
3. **Replay leftovers.** At startup, segments left by an earlier run are
   queued ahead of live data and sent after the receiver connects.
4. **At-most-once delivery.** A frame is never sent twice. A frame that was
   handed to the connection when the connection failed is lost and counted.
   A frame read from the spool is marked consumed (in the cursor file)
   before it is written to the socket.

## Design

### Threads and data flow

```text
dnstap-parser thread                         dnstap-sender thread
Encoder ─► QueuedSink::write_frame           ┌─► Link (NetworkSink): connect, reconnect,
          (append framed bytes to a Block)   │     write pre-framed bytes, STOP/FINISH
          ─► crossbeam bounded channel ──────┤
          ◄─ ArrayQueue of free Blocks ◄─────┘─► Spool: segment files on disk
```

- **Block.** `struct Block { bytes: Vec<u8>, frames: u32 }`. It holds
  complete Frame Streams data frames: a 4-byte big-endian length, then the
  payload. Capacity is `BLOCK_CAPACITY` (136 KiB). The parser sends a block
  when its length reaches `BLOCK_SEND_BYTES` (64 KiB), and on the parser's
  existing flush timers (100 ms idle, 1 s busy). A frame always fits: the
  block holds less than 64 KiB before the append, and an encoded frame is at
  most `MAX_ENCODED_LENGTH` (70 KiB, `src/dnstap.rs`) plus 4 bytes.
- **Send queue.** A `crossbeam_channel::bounded(SEND_QUEUE_BLOCKS)` with
  `SEND_QUEUE_BLOCKS = 128`. Free blocks come back through a
  `crossbeam_queue::ArrayQueue` sized `SEND_QUEUE_BLOCKS + 2`, all allocated
  at startup (about 17 MiB). The parser never blocks and never allocates. If
  `try_send` fails because the queue is full, the parser discards the block's
  frames, counts them in `queue_dropped`, clears the block, and reuses it.
- **Sender thread.** It owns the link and the spool. It is the only thread
  that does socket or spool I/O. The parser thread no longer does socket I/O.

### Routing on the sender thread

For each block received:

1. A spool exists, and any of these holds: the spool has a backlog, the link
   is not connected, or the channel holds at least `SPILL_BLOCKS` (96 of
   128) blocks: append the block to the spool. This keeps frames in order,
   and a slow but connected receiver spills to the spool instead of filling
   the queue. A paused spool refuses the block and counts it in
   `spool_dropped`.
2. Link connected and no backlog: write the block to the link.
3. Otherwise (no spool, link down): discard and count the frames in
   `disconnected_dropped`, as today.

Then return the block to the free queue. (Rules 1 and 3 changed after the C2
review, 2026-11-02.)

Between blocks, the sender polls reconnects (the existing backoff, 250 ms to
30 s, on the existing `dnstap-reconnect` helper thread). While connected with
a backlog, it replays. Before each replay step it moves every block waiting
in the channel to the spool (at most `SEND_QUEUE_BLOCKS`), so live data never
waits behind replay. A replay step reads up to `REPLAY_BATCH_BYTES`
(256 KiB) of whole data frames from the spool head, records the new read
position in the cursor file, and then writes the batch to the link in chunks
of at most 64 KiB, each with its own I/O deadline. It uses `try_recv` while a
backlog is draining and `recv_timeout(SENDER_POLL)` (50 ms) otherwise. It
flushes the link when it has written data and the channel is empty.

A failed link write counts the batch, or the block, plus the frames not yet
flushed, in `disconnected_dropped`. This keeps today's rule in
`NetworkSink::connection_lost`.

### Spool on disk

- **Folder.** One folder, by default `C:\ProgramData\EtwDnstap\spool`. The
  installer creates it with owner Administrators, no inherited ACEs, full
  control for SYSTEM and Administrators, and Modify for
  `NT SERVICE\EtwDnstap`. The service never creates the folder. At startup
  it checks that the folder exists, that it and its parent are not reparse
  points, and that its owner is SYSTEM (`S-1-5-18`) or Administrators
  (`S-1-5-32-544`). A failed check logs one warning that names the reason
  and runs without a spool.
- **Segments.** Files named `seg-<20-digit sequence>.fstrm`, created with
  `create_new`. Each segment is a valid unidirectional dnstap Frame Streams
  file: START with content type `protobuf:dnstap.Dnstap`, data frames, then
  STOP when the segment is sealed. A segment is sealed at
  `SEGMENT_BYTES = min(64 MiB, max_bytes / 8)` and at shutdown. New writes
  always start a new segment; the service never appends to a segment from an
  earlier run.
- **No flush to disk.** No `FlushFileBuffers` and no write-through. The spool
  protects against a receiver outage, not against a power loss.
- **Cursor file.** `cursor.bin` holds the read position: magic `b"ETDC"`,
  sequence `u64`, byte offset `u64`, and a check value
  `sequence ^ offset ^ 0x5bd1_e995_5bd1_e995`, all little-endian (28 bytes).
  It is overwritten in place before each replay batch is written to the
  link. The cursor is a high-water mark and is never deleted: when the spool
  empties, it is set to `(next sequence, START length)`. At open, every
  segment with a sequence below the cursor's was read and is deleted
  uncounted; a missing cursor segment means reading starts at the next
  segment's START. `next_sequence` is at least the cursor's sequence, so
  sequence numbers never repeat. A missing file means no partial read. A
  file that fails its check means the oldest segment is treated as fully
  read: it is deleted and counted as dropped (at-most-once). A segment file
  that cannot be deleted stays counted in `spool_bytes` and is retried later.
  (Changed after the C1 review, 2026-11-02.)
- **Reader.** Reads whole data frames only. A zero length starts a control
  frame: it reads the control length (4 to 512), skips START, and ends the
  segment at STOP. A data length of 0 or over `MAX_ENCODED_LENGTH`, or an
  end of file inside a frame, ends the segment: the remaining bytes are
  counted in `spool_dropped_bytes`, and a cut-off final frame is counted in
  `spool_dropped`. A missing or wrong START deletes the segment and counts
  its bytes as dropped. For the segment that is still being written, the
  reader stops at the writer's length known in memory.
- **Caps.** `max_bytes` (`--spool-max-mib`, default 4096) and
  `min_free_bytes` (`--spool-min-free-mib`, default 2048). Before each
  append, if the total segment bytes plus the block would exceed
  `max_bytes`, the spool deletes the oldest segment until it fits. At each
  new segment, it reads the volume's free space (`GetDiskFreeSpaceExW`). If
  free space is below `min_free_bytes + SEGMENT_BYTES`, it deletes the
  oldest segments until it is not. If only the active segment is left, the
  spool stops accepting blocks for 10 s, counting them in `spool_dropped`,
  and then checks again. An evicted segment from this run adds its unread
  frames to `spool_dropped`. An evicted leftover segment adds only bytes to
  `spool_dropped_bytes`, because its frame count is not known.
- **Empty spool.** When the reader reaches the writer's position, the active
  segment is closed and deleted, the cursor moves to the next sequence, and
  routing returns to direct writes.
- **I/O errors.** A spool I/O error counts `spool_errors`, discards the
  block (`spool_dropped`), closes the active segment, and suspends the spool
  for 10 s. Errors are logged at most 4 times, then every 60th time, as in
  `poll_reconnect`.

### Shutdown

`QueuedSink::finish` sends the last partial block, drops the channel sender,
joins the sender thread, and returns the thread's result. On a closed
channel, the sender:

1. routes the blocks already received, as above;
2. if connected with no backlog, runs the existing STOP/FINISH exchange;
3. if a backlog exists, seals the active segment, saves the cursor, and then
   runs STOP/FINISH if connected. It does not drain the backlog at shutdown:
   the 10 s shutdown watchdog (`src/pipeline/watchdog.rs`) must not fire;
4. if disconnected, seals the active segment and logs today's warning.

### Counters

Add these fields to `SinkStats` (`src/sink.rs`). Update the doc comment that
says every counter is monotonic except `connected`.

| Field | Kind | Meaning |
| --- | --- | --- |
| `queue_dropped` | counter | Frames discarded on the parser thread because the send queue was full. |
| `spooled` | counter | Frames appended to the spool. |
| `replayed` | counter | Frames read from the spool and handed to the link. |
| `spool_dropped` | counter | Frames removed from the spool unsent, or refused by it: eviction, corrupt or cut-off data, floor suspension, I/O errors. |
| `spool_dropped_bytes` | counter | Bytes removed from the spool unsent, including leftover segments whose frame count is unknown. |
| `spool_errors` | counter | Spool I/O errors. |
| `spool_bytes` | gauge | Bytes in segment files now. |
| `spool_segments` | gauge | Segment files now. |
| `spool_available` | flag (`AtomicBool`) | True while the spool is usable. |

`disconnected_dropped` keeps its meaning: frames lost in the sink because the
connection failed with them in flight, or because they arrived while the
receiver was down and no spool was usable.

### Command line

| Option | Default | Range | Meaning |
| --- | --- | --- | --- |
| `--spool-dir PATH` | `C:\ProgramData\EtwDnstap\spool` | an absolute path | Spool folder. Conflicts with `--no-spool`. |
| `--no-spool` | off | flag | Run without a spool. |
| `--spool-max-mib N` | `4096` | 64 to 1,048,576 | Cap on segment bytes. |
| `--spool-min-free-mib N` | `2048` | 0 to 1,048,576 | Free space to keep on the volume. |

## Rules for every agent

- **Work in the main checkout**, on branch `tcp-sink-spool`. Do not use a
  worktree. Never commit, stash, reset, checkout, restore, rebase, or clean.
  Preserve uncommitted work by other stages.
- **Touch only the files your stage lists.** If you need another file, stop
  and report why.
- **No host directory paths** in anything you write: code, comments, tests,
  docs, or reports that will be pasted into the repository. Read
  `CLAUDE.md`. Tests that need a folder build it at run time from
  `std::env::temp_dir()`.
- **Do not** deploy, run elevated commands, start real ETW sessions, run
  `#[ignore]`d tests, run lab scripts, or change service, ETW, or registry
  state. Do not add a crate dependency. `windows-sys` features may be added
  where a stage says so.
- **Style.** Match the surrounding code: comment density, `anyhow`
  `ensure!`/`bail!`, `thiserror` where the module already uses it,
  `#[expect(..., reason = "...")]` for intentional lint exceptions, and a
  `// SAFETY:` comment on every `unsafe` block. Unsafe code lives in a small
  submodule with `#[allow(unsafe_code)]` on its `mod` line, as
  `src/network/sockopt.rs` does. Lints are strict: warnings denied, Clippy
  pedantic, `unwrap_used` denied. Tests live in `<module>/<name>_tests.rs`
  files included with `#[cfg(test)] #[path = ...] mod ...;`, as
  `src/network.rs` does.
- **Hot path.** `QueuedSink::write_frame` must not allocate, lock, or make a
  system call, except the channel send once per block.
- **Checks before you report.** Run, from the repository root:
  `cargo fmt`, `cargo fmt --check`,
  `cargo clippy --locked --all-targets -- -D warnings`,
  `cargo clippy --locked --all-targets --features diag-timing -- -D warnings`,
  `cargo test --locked`, and `git diff --check`. The baseline is recorded in
  the [completion record](#completion-record).
- **Report back** to the orchestrator in this form, and nothing else:

  ```text
  Stage: <id>
  Files changed: <list>
  Summary: <what you built, 5 to 15 lines>
  Deviations from the plan: <none, or each one with the reason>
  Checks: <each command and its result; test counts passed/failed/ignored>
  Open questions: <none, or a list>
  ```

## Orchestration

The orchestrator (the main session) runs the stages in this order and reviews
each report before the next stage starts.

| Step | Who | Runs after |
| --- | --- | --- |
| 1. C1 | Rust coding agent | Baseline recorded |
| 2. R on C1 | Review agent | C1 report |
| 3. C2 | Rust coding agent | C1 fixes accepted |
| 4. R on C2 | Review agent | C2 report |
| 5. C3 | Rust coding agent | C2 fixes accepted |
| 6. R on C3 | Review agent | C3 report |
| 7. T1 and D1, in parallel | Testing agent, documentation agent | C3 fixes accepted |
| 8. R on T1 and on D1 | Review agent | T1 and D1 reports |
| 9. Release gates | Orchestrator | All fixes accepted |
| 10. Lab outage test | Orchestrator, only with user approval | Step 9 |

For each review, the orchestrator reads the diff itself, compares it with the
review agent's findings, and sends the fixes it accepts back to the stage's
agent. That agent fixes them, reruns its checks, and reports again.

Step 9 runs the checks that need PowerShell or the release build:
`cargo build --locked --release` with its log captured to an ignored `*.log`
file, `scripts/ci/verify-link-exceptions.ps1`, `scripts/ci/verify-pe.ps1`,
`cargo deny --locked check`, `python -I scripts/ci/linkcheck.py .`,
`cargo test-release`, `cargo asan`, and the host-path check from `CLAUDE.md`
on the full diff.

Step 10 stops the receiver on the lab for 60 s at 1,000 QPS and then at
50,000 QPS, restarts it, and confirms from `ss -tin` bytes received that
`encoded` equals the frames received plus every drop counter. Collector
output goes to null.

## C1: Spool storage

**Skills:** load `rust-best-practices`, `rust-skills`, and
`rust-security-hardening` (for the unsafe Win32 calls) before you write code.

**Files:** `src/spool.rs` (new), `src/spool/segment.rs` (new),
`src/spool/cursor.rs` (new), `src/spool/fsinfo.rs` (new, unsafe),
`src/spool/test_support.rs` (new), `src/spool/spool_tests.rs` (new),
`src/spool/segment_tests.rs` (new), `src/framestream.rs`, `src/lib.rs` (the
`mod` line only), `Cargo.toml` (`windows-sys` features
`Win32_Storage_FileSystem` and `Win32_Security_Authorization` only, with a
comment naming the calls that need them).

Build the [Spool on disk](#spool-on-disk) design as a synchronous,
single-thread type. Nothing outside this stage uses it yet.

1. `src/framestream.rs`: make `CONTROL_START`, `CONTROL_STOP`, and
   `FIELD_CONTENT_TYPE` `pub(crate)`. Add `FrameStreamWriter::write_framed(&mut self, framed: &[u8])`,
   which writes bytes that already hold complete data frames, with the same
   poison rule as `write_frame`. Add a unit test next to the existing ones.
2. `src/spool/segment.rs`: `SegmentWriter` (create with `create_new`, write
   START, append framed bytes, seal with STOP, report its length) and
   `SegmentReader` (open at an offset, read whole data frames into a caller
   buffer up to a byte limit, report frames and bytes read, report the end
   reason: STOP, writer position, end of file, cut-off frame, or corrupt
   length).
3. `src/spool/cursor.rs`: read, write in place, and delete `cursor.bin` in
   the format above.
4. `src/spool/fsinfo.rs`: `available_bytes(dir) -> io::Result<u64>` with
   `GetDiskFreeSpaceExW`, and `check_spool_dir(dir) -> Result<(), SpoolDirError>`
   (exists, is a directory, it and its parent are not reparse points, owner
   is SYSTEM or Administrators). Read the owner with `GetNamedSecurityInfoW`
   and free the descriptor with `LocalFree`. Split the owner rule into a pure
   function `owner_is_trusted(&[u8] /* SID bytes */) -> bool` so it can be
   tested with the two well-known SIDs and a user SID.
5. `src/spool.rs`: `SpoolConfig { dir, max_bytes, min_free_bytes }`,
   `Spool::open(config, stats, free_space)` where `free_space` is a
   `fn(&Path) -> io::Result<u64>` (production passes
   `fsinfo::available_bytes`; tests pass a fake). `open` does not call
   `check_spool_dir`; the caller does. Methods: `append(&mut self, bytes, frames)`,
   `has_backlog()`, `read_batch(&mut self, out: &mut Vec<u8>, limit) -> io::Result<u32>`
   (frames read; it writes the cursor before it returns), `seal()`, and the
   eviction, floor, suspension, and empty-spool rules above. Update the
   `SinkStats` fields only through a `&SinkStats` passed in. **Do not edit
   `src/sink.rs`:** C2 adds the fields. Until then, define a
   `pub(crate) struct SpoolStats` in `src/spool.rs` with the spool fields
   from the [Counters](#counters) table, and C2 will move them.
6. Tests (`spool_tests.rs`, `segment_tests.rs`), using a `TestDir` helper in
   `test_support.rs` that makes a unique folder under `std::env::temp_dir()`
   and removes it on drop:
   - append then read returns the same bytes in order across a rollover;
   - each sealed segment is START, data, STOP, and parses as a dnstap
     Frame Streams file;
   - the size cap evicts the oldest segment and counts frames and bytes;
   - the free-space floor (fake probe) evicts, then suspends;
   - a reopened spool replays leftovers from the cursor position, and starts
     a new segment for new writes;
   - a cut-off final frame, a corrupt length, and a missing START are each
     counted and skipped;
   - a bad cursor file drops the oldest segment (at-most-once);
   - reading to the writer position empties the spool and deletes the files.

## C2: Sender thread and queued sink

**Skills:** load `rust-best-practices`, `rust-skills`, and `build-rust`
before you write code.

**Files:** `src/sender.rs` (new), `src/sender/sender_tests.rs` (new),
`src/sink.rs`, `src/spool.rs` (to move `SpoolStats` into `SinkStats`),
`src/network.rs`, `src/network/*_tests.rs` (only where a refactor requires
it), `src/dnstap.rs` (make `MAX_ENCODED_LENGTH` `pub(crate)` only),
`src/lib.rs` (the `mod` line only).

1. `src/sink.rs`: add the [Counters](#counters) fields and fix the doc
   comments. Remove `SpoolStats` from `src/spool.rs` and point the spool at
   `SinkStats`.
2. `src/network.rs`: give `NetworkSink` the methods the sender needs:
   `is_connected`, `poll(now)` (today's `poll_reconnect`),
   `write_framed(bytes, frames)` (write pre-framed bytes; on failure apply
   `connection_lost` with these frames), `flush`, and `finish`. Keep the
   `FrameSink` impl for `NetworkSink` and all existing network tests passing
   unchanged. Add `Connection::write_framed` through
   `FrameStreamWriter::write_framed`.
3. `src/sender.rs`: `Block`, the block pool, `QueuedSink` (implements
   `FrameSink` on the parser thread: `write_frame`, `flush` sends a partial
   block, `finish` as in [Shutdown](#shutdown)), and the `dnstap-sender`
   thread with the [routing](#routing-on-the-sender-thread) and
   [shutdown](#shutdown) rules. `QueuedSink::start(link: NetworkSink, spool: Option<Spool>, stats)`
   spawns the thread. The validation in `write_frame` matches
   `FrameStreamWriter::write_frame` (1 byte to `MAX_ENCODED_LENGTH`; return
   `InvalidInput` without changing the block).
4. Tests in `sender_tests.rs`, using the fake receivers in
   `src/network/test_support.rs` (make helpers `pub(crate)` if needed) and
   the spool `TestDir`:
   - connected, no spool: frames arrive in order, nothing dropped;
   - receiver down with a spool: frames are spooled, then replayed in order
     after it comes back, before newer frames, with no duplicates;
   - receiver down without a spool: frames count in `disconnected_dropped`;
   - a full send queue counts `queue_dropped` and never blocks the parser
     (use a receiver that never reads);
   - shutdown with a backlog seals the segment and keeps it for the next run;
   - shutdown while connected with no backlog completes STOP/FINISH.

## C3: Configuration, wiring, and counters

**Skills:** load `rust-best-practices` and `rust-skills` before you write
code.

**Files:** `src/pipeline/config.rs`, `src/pipeline/config_tests.rs`,
`src/pipeline/cli_tests.rs`, `src/pipeline/supervisor.rs`,
`src/pipeline/supervisor_tests.rs`, `src/perfcounters.rs`,
`packaging/perfcounters/etw-dnstap.man`,
`packaging/perfcounters/etw-dnstap-perf.rc`.

1. Add the [Command line](#command-line) options to `CaptureArgs` and
   `PipelineConfig`, with range checks in the style of the existing options
   and tests in `config_tests.rs` and `cli_tests.rs`. `--spool-dir` must be
   absolute.
2. In `run_capture`, build the sink: `NetworkSink::start` as today, then
   `check_spool_dir` and `Spool::open` unless `--no-spool`, then
   `QueuedSink::start`. A failed check or open logs one warning with the
   reason and continues without a spool. Keep the sink factory seam that the
   supervisor tests use.
3. Add the new fields to the `output stats` and `output stopped` events in
   `emit_counts!`, and update the counter-relationship comment above it.
4. Add the counters to the PerfLib V2 provider. Follow the existing layout in
   `src/perfcounters.rs` and the manifest exactly: append new counter IDs
   after the last one, never renumber. Gauges use a raw-count type. Update
   the layout tests.

## T1: Integration, property, and fuzz tests

**Skills:** load `rust-best-practices` and `rust-skills` (testing rules)
before you write tests.

**Files:** new test files under `src/spool/` and `src/sender/` only, the
`fuzzing` entry point (`src/etw/fuzzing.rs` pattern; add the spool one in a
new `src/spool/fuzzing.rs` with its `mod` line in `src/spool.rs`),
`src/lib.rs` (re-export in the existing `fuzzing` module only), and
`fuzz/fuzz_targets/spool_segment.rs` with its `[[bin]]` entry in
`fuzz/Cargo.toml`. Do not change production code. Report any defect you find
instead of fixing it.

1. Read the C1, C2, and C3 code and tests first. Do not duplicate tests that
   exist.
2. Property tests (`proptest`, already a dev-dependency): any sequence of
   appends and batch reads, with random cap and random cut-off at reopen,
   yields frames in order, never a frame twice, and
   `appended = read + spool_dropped + left in spool`.
3. End-to-end tests through `QueuedSink` with a fake receiver: several
   outages and reconnects in one run; an outage longer than the cap
   (oldest dropped, newest delivered); restart with leftovers and a cursor
   in the middle of a segment; a receiver that accepts slowly, so blocks
   spool and then drain; at-most-once when the link fails mid-replay
   (no frame received twice).
4. A libFuzzer target over the segment reader with arbitrary file bytes. It
   must never panic. Build it only if `cargo fuzz` is installed; otherwise
   check that `cargo clippy --locked --all-targets --features fuzzing -- -D warnings`
   passes and say so.
5. Run the full check list in [Rules](#rules-for-every-agent), plus
   `cargo test --locked --features bench` and
   `cargo clippy --locked --all-targets --features bench -- -D warnings`.

## D1: Documentation

**Skills:** load `technical-documentation` and `asd-ste100` before you write.
The existing guides use short, direct sentences; keep that style.

**Files:** `docs/design/data-path.md`, `docs/guide/configuration.md`,
`docs/guide/install.md`, `docs/guide/upgrade.md`,
`docs/guide/uninstall.md`, `docs/guide/monitoring.md`,
`docs/guide/troubleshooting.md`, `docs/guide/tuning.md`, `SECURITY.md`,
`CHANGELOG.md`. Read the code from C1 to C3 for exact names; the code wins
over this plan.

1. `data-path.md`: stage 5 becomes the queued sink, the sender thread, and
   the spool; update the thread list, the startup and shutdown order, and the
   loss table (`queue_dropped`, `spool_dropped`, `disconnected_dropped`). Do
   not regenerate the GIF; add one sentence that it shows the earlier
   single-thread sink.
2. `configuration.md`: the four options; a new "Spool" section with the
   routing, ordering, drop-oldest, replay, and at-most-once rules; correct
   the "Removed options" sentence that says the service writes no capture
   files.
3. `install.md`: a step that creates the spool folder with the protected
   folder functions already in the guide, then grants `NT SERVICE\EtwDnstap`
   Modify on the spool folder only. Explain why the service does not create
   the folder.
4. `upgrade.md`: create the folder on upgrade; until it exists, the service
   warns and runs without a spool. Re-register the counter manifest if the
   guide already describes that step for counter changes.
5. `uninstall.md`: delete the spool folder; it holds DNS data.
6. `monitoring.md`: the new counters, the new log fields, and their
   relationships. `tuning.md`: a sizing table (about 103 bytes per frame at
   minimum, 2 frames per query) for choosing `--spool-max-mib`.
   `troubleshooting.md`: what happens and what to check when the receiver is
   down, and the spool warnings.
7. `SECURITY.md`: the spool stores client addresses and query names on disk
   without encryption; who can read it; `--no-spool`.
8. `CHANGELOG.md`: one entry in the style of the existing ones.
9. Run `python -I scripts/ci/linkcheck.py .` and the host-path check.

## R: Review

**Skills:** for code, load `rust-security-hardening`, `rust-best-practices`,
and `windows-service-security`. For documentation, load
`technical-documentation`.

**Files:** none. The review agent only reads. It does not edit, build into
the shared `target` folder, or run tests; it may run
`git diff` and `git status`.

For the stage named in its brief, check the diff against this plan and
report findings ranked by severity. Look first for:

- loss or duplication paths that break the [Decisions](#decisions);
- frames counted twice or not counted;
- blocking, allocation, or a system call on the parser thread per frame;
- shutdown paths that could exceed the 10 s watchdog;
- unsafe code: buffer sizes, ownership of the security descriptor,
  `LocalFree`, wide-string termination;
- spool folder trust: reparse points, the owner check, `create_new`, and
  files that are not segments;
- disk-full and free-space behavior;
- tests that do not test what they claim, or that are flaky by timing;
- for documentation: wrong names, defaults, or ranges against the code.

Report in this form:

```text
Stage reviewed: <id>
Findings:
1. [severity: high|medium|low] <file>:<line> — <defect>. Failure: <inputs → wrong result>. Fix: <suggestion>.
...
Plan deviations noticed: <none, or a list>
```

## Completion record

Each stage appends a subsection here when the orchestrator accepts it.

### Baseline

Recorded by the orchestrator before C1, at `0f6db1e` on branch
`tcp-sink-spool`: `cargo test --locked` gives 276 passed, 0 failed,
7 ignored in the library tests, and 0 tests in the binary and doc-test
targets. The release-style link prints the expected LNK2047 lines for ring's
NASM objects (see `.cargo/config.toml`); they are not failures.

### C1: Spool storage

Accepted on 2026-11-02 after one review round. The review agent and the
orchestrator found one high defect: a consumed segment whose delete failed
could be replayed after a restart. They also found medium defects:
sequence overflow under `panic = "abort"`, name collisions that stall the
spool, the free-space floor deleting the whole backlog, and sharing
violations deleting data. Fixes F1 to F15 made the cursor a high-water mark,
added a pending-delete list, case-insensitive names with collision retry,
transient read-error pauses, and a DACL check on the spool folder
(`ace_is_safe`, `dacl_is_safe`). Accepted deviations: `next_sequence` is
above the cursor sequence, `FILE_DELETE_CHILD` counts as a dangerous right,
and `read_batch` returns `Ok(0)` with a backlog while reads are paused.
`MAX_DATA_FRAME` in `src/spool/segment.rs` stays until C2. Checks
re-run by the orchestrator: `cargo fmt --check`, Clippy, and
`cargo test --locked` with 347 passed, 0 failed, 7 ignored.

### C2: Sender thread and queued sink

Accepted on 2026-11-02 after one review round. The review found one high
defect: during a replay the sender took one live block per 256 KiB batch, so
the queue overflowed in catch-up. The orchestrator found that a slow but
connected receiver never spilled to the spool. Fixes G1 to G13:

- Queued blocks drain to the spool before each replay step.
- A spill rule at 96 queued blocks.
- A real shutdown bound: a 1 s link budget that starts in `finish` and
  `Drop`, about 5 s worst case.
- A one-listener fake receiver for the tests.
- `send_timeout` in `finish`.
- Routing rule 3 simplified: with a spool and the link down, blocks always
  go to the spool.
- Replay writes in 64 KiB chunks of whole frames.

Accepted deviations: the sender tests use their own fake receiver, the spool
`test_support` is `pub(crate)`, `QueuedSink::start` returns `io::Result`, and
a cap of 3 idle reads in a row. Checks re-run by the orchestrator:
`cargo fmt --check`, Clippy, and `cargo test --locked` with 361 passed,
0 failed, 7 ignored. The 12 sender tests passed in 5 repeated runs.

### C3: Configuration, wiring, and counters

Accepted on 2026-11-03 after one review round. The review found one high
defect: a network or device `--spool-dir` could block the startup path
longer than the startup watchdog and make the service fail to start.
Fixes H1 to H9:

- Only a fixed local disk is accepted, both at parse time
  (`check_path_shape`) and at run time (`GetDriveTypeW`). A `ProgramData`
  value that fails the check falls back to `C:\ProgramData`.
- Corrected help text for counters 2, 11, and 18, with the `.rc`
  regenerated by `ctrpp`. The orchestrator confirmed that the `.rc` is
  identical to `ctrpp` output.
- A test seam for the spool folder check and an outage test through
  `build_sink_with`.
- `spool_available` is also false during the free-space-floor pause.
- `.` and `..` are refused at parse time.

New performance counters 52 to 60 are appended without renumbering.
Accepted deviations: the size options also conflict with `--no-spool`, the
outage test drives `QueuedSink` directly rather than through
`run_pipeline`, and a doc comment edit in `src/sink.rs`. Checks re-run by
the orchestrator: `cargo fmt --check`, Clippy, `cargo test --locked` with
387 passed, 0 failed, 7 ignored, and the host-path check.

### T1: Integration, property, and fuzz tests

Accepted on 2026-11-03 after one review round. T1 found no production
defects. The review found a rare flake in the several-outages test (a probe
from an earlier round could end a wait early) and an at-most-once upper
bound that could not fail. Fixes J1 to J9:

- Tighter loss bounds.
- Conservation kept after a cut, with a cut-segment budget.
- A drop-oldest check in every property test.
- Noisy-cursor and free-space-floor property tests.
- A slow-accept fake receiver mode.
- A restart test that resumes from a cursor left in the middle of a replay.

The libFuzzer build of `spool_segment` fails in build-std on the nightly
toolchain, as it does for the existing `decoder` target. The entry point
is exercised by unit tests and by `cargo check` of the fuzz crate.

### Documentation-review code fixes (K1 to K3)

Accepted on 2026-11-03. The documentation review found gaps that needed
code changes:

- The spool folder check now rejects read-type rights for other accounts,
  and applies to inherit-only entries, with an inherit-only CREATOR OWNER
  entry allowed.
- The two receiver warnings in `src/network.rs` now say that frames go to
  the spool.
- Discarded unreadable segment data is logged, rate-limited.

Checks re-run by the orchestrator after T1 and K: `cargo fmt --check`; Clippy
with no features, `diag-timing`, `fuzzing`, and `bench`; and
`cargo test --locked` with 413 passed (418 with `diag-timing`, 417 with
`bench`), 0 failed, 7 ignored.

### D1: Documentation

Accepted on 2026-11-03 after one review round. The review found:

- an uninstall step that ran a recursive delete through a folder the service
  can write;
- a missing **Breaking.** changelog entry;
- slow-receiver, shutdown, in-flight-loss, and `--spool-dir` statements that
  did not match the code;
- stale statements outside the D1 file list.

Fixes L1 to L19 also cover `README.md`, `docs/design/measurement-terms.md`,
`docs/guide/receiver.md`, a superseded note in
`docs/plans/parser-tcp-threading-analysis.md`, and an index entry in
`docs/README.md`. The install grant is split: `(RX,W)` on the spool folder and
`(OI)(CI)(IO)(M)` on its contents, so the service cannot delete the folder.
The lab has not run this grant under the restricted token. A malformed
explicit `--spool-dir` fails command-line parsing. Only run-time folder
problems fall back to running without a spool.

### Release gates (step 9)

Run by the orchestrator on 2026-11-03. All pass:

- `cargo build --locked --release`.
- `scripts/ci/verify-link-exceptions.ps1`: 6 checks, 0 failed.
- `scripts/ci/verify-pe.ps1`: 21 checks, 0 failed.
- `cargo deny --locked check`.
- `cargo test-release`: 413 passed.
- `cargo asan`: 413 passed, with the ASan runtime put on `PATH` as the CI
  `sanitizers` job does.
- `python -I scripts/ci/linkcheck.py .`: 0 broken.
- The host-path check over the diff and the new files.

Not done yet: the lab outage test (step 10), which needs user approval, and
the status-header updates in the edited guides, which happen at commit time.
