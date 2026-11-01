# Live load validation plan

> **Status:** Proposed (execution is not recorded in this index; confirm before relying on it) · **Evidence dated:** October 30, 2026 (deferred checks) · **Commit:** not recorded · **Reviewed:** 2026-11-01

This plan unblocks and runs the load checks deferred by the October 30, 2026
live validation in the [source review remediation plan](src-review-remediation-plan.md).
Stages L1–L3 used a low-cost model at low effort. Stages L4 and L5 use the
models named in each stage.

The checks were blocked for three reasons:

- The service's loss counters only reach stderr, which a service does not
  have (review finding 1).
- The Ubuntu receiver's root filesystem is full.
- No load stage had been defined.

## Lab identity (verify before every stage)

- Windows DNS VM: Hyper-V `DNSTap-WS2025`, VM ID
  `24bafedb-0998-4430-a0c6-7e90350cd9ac`, address `172.27.215.163`.
  - Reach it with PowerShell Direct:
    `New-PSSession -VMId <id> -Credential (Import-Clixml target/lab-windows/private/administrator.clixml)`.
  - The session runs elevated.
- Ubuntu receiver VM: `ssh -F target/lab-ubuntu/ssh_config dnstap-ubuntu`
  (`172.27.215.60`, passwordless `sudo -n`). The DNS-collector listens on
  `172.27.215.60:6000`.
- Original service state to restore:
  - binary SHA-256 `CE4314678BCBEDE625AC536FCC9096C1401A45F93E770DBE69484B789CF0B656`;
  - `PathName` `"C:\Program Files\EtwDnstap\etw-dnstap.exe" --receiver 172.27.215.60:6000 --session-name EtwDnstap-Lab`;
  - state Stopped.
  - Backups are in `C:\ProgramData\EtwDnstap\live-checks-20261030\`
    (`service-original.exe`, `service-original.json`).

## Stage L1: Counters to the Application Event Log (finding 1)

Files: `src/service.rs`, `src/pipeline.rs`, `README.md`.

1. In `src/service.rs`, add `pub(crate) struct EventLogWriter;` implementing
   `tracing_subscriber::fmt::MakeWriter<'a>`.
   - `make_writer_for(meta)` returns a line writer that buffers bytes. When it
     is dropped, it reports them with `ReportEventW` to the already registered
     `EtwDnstap` source.
   - Map levels to event types and IDs:
     - `ERROR` → `EVENTLOG_ERROR_TYPE`, event ID 4;
     - `WARN` → `EVENTLOG_WARNING_TYPE`, event ID 3;
     - otherwise → `EVENTLOG_INFORMATION_TYPE`, event ID 2.

     Event ID 1 stays reserved for `report_failure`.
   - Register the event source once, lazily, in a process-lifetime
     `OnceLock`. If registration fails, drop messages silently. A logging
     failure must never stop capture.
   - Wrapping the raw `HANDLE` needs `unsafe impl Send` and `Sync` with a
     `// SAFETY:` comment. Event log handles may be used from any thread.
2. Share one pure text sanitizer with `report_failure`. It trims trailing
   CR/LF, caps the text at 8,192 UTF-16 units, replaces NUL with U+FFFD, and
   appends a terminator. Unit-test the sanitizer and the level-to-type/ID
   mapping. Tests must never call `ReportEventW` or write to the real Event
   Log.
3. In `pipeline::run_capture`, change the tracing initialization so output
   goes to both stderr and the Event Log:
   `.with_writer(std::io::stderr.and(crate::service::EventLogWriter))`, using
   `tracing_subscriber::fmt::writer::MakeWriterExt`. Also set
   `.with_ansi(false)` and `.with_max_level(tracing::Level::INFO)`. Message
   volume is low: start and stop, at most four rejected-event warnings, and
   at most one reclaim warning per run.
4. README: state that the "capture started" and "capture stopped" lines,
   including all counters, appear in the Application log under source
   `EtwDnstap` with IDs 2, 3, and 4, and that ID 1 is a fatal failure.
5. Checks: `cargo fmt`, `cargo fmt --check`,
   `cargo clippy --locked --all-targets -- -D warnings`,
   `cargo test --locked --lib` (starting count: 108 passed, 4 ignored), and
   `git diff --check` on the touched files.

## Stage L2: Ubuntu receiver disk cleanup

Runs on `dnstap-ubuntu` and the host. It must not stop or reconfigure
`dnscollector` or `dnstap-lab-filter`.

1. Confirm no campaign is running: no `tcpdump`, `capture-wrapper.py`, or
   `cpu-sampler.py` processes. Record `df -h /`.
2. Truncate the syslog with `sudo -n truncate -s 0 /var/log/syslog`, and
   delete any rotated `/var/log/syslog.[0-9]*` files.
3. For each campaign directory in `/home/dnstaplab`, copy, verify, then delete.
   The directories are `direct-packet-100000-20261030-070305`,
   `direct-packet-64000-20261030-064831`, `filtered-ramp-20261030`,
   `parser-ramp-20261030`, `performance`, `performance-1000qps`,
   `performance-pooled-vm-20261029-0942`, `performance-ringfix-20261029`,
   `service-stop-20261030-0633`, and `singlepass-4cpu-100000`.
   1. On the guest, write a manifest of every regular file: relative path,
      byte size, and SHA-256. Copy it to
      `target/performance/guest-archive-20261030/<dir>.manifest.tsv`.
   2. Copy the whole directory to
      `target/performance/guest-archive-20261030/<dir>/`, for example with
      `scp -r -F target/lab-ubuntu/ssh_config` or a tar stream. Do not
      deduplicate against older host copies; a complete copy is simpler and
      auditable.
   3. On the host, compute the SHA-256 and size of every copied file and
      compare them with the manifest. Write
      `target/performance/guest-archive-20261030/<dir>.verified.json` with
      the file count, total bytes, and any mismatches.
   4. Delete the guest directory with `sudo -n rm -rf` only when there are
      zero mismatches and the file count matches. On any mismatch, keep the
      guest directory and report it.
4. Leave everything else in `/home/dnstaplab` untouched, including the
   collector and filter files, the scripts, and the small root-level `.pcap`
   and `.jsonl` files.
5. Record `df -h /` afterward. Target: at least 20 GB free.

## Stage L3: Load checks

Prerequisites: L1 and L2 are complete.

**Load generator.** `tools/dns-stress` runs on the Hyper-V host. Its release
binary is `tools/dns-stress/target/x86_64-pc-windows-msvc/release/dns-stress.exe`.
Read `tools/dns-stress/README.md` first. Use authoritative mode with
`target/performance/workload/auth-names.txt` against `172.27.215.163:53`.
Each run needs a fresh `--output-dir`.

**Setup.**

1. Build the release service binary: `cargo build --locked --release`.
   Record its SHA-256.
2. Verify the fixture. On the VM, `Get-DnsServerZone -Name auth.stress.test`
   must exist. From the host, resolve three names from `auth-names.txt` with
   `Resolve-DnsName -Server 172.27.215.163 -DnsOnly`; each must return
   NOERROR with an answer. If the fixture is missing, stop and report; do not
   recreate zones.
3. On the receiver, check `df -h /`. Before each scenario, if less than 10 GB
   is free, truncate `/var/log/syslog` again. Collector output grows quickly
   under load.
4. Deploy the new binary to `C:\Program Files\EtwDnstap\etw-dnstap.exe`. The
   service must be stopped; the backup already exists.

**Scenario procedure.** For each scenario:

1. Set `PathName` to the original command plus the scenario flags, using
   `Invoke-CimMethod ... Change`.
2. Start the service and wait 5 seconds.
3. Run the load from the host.
4. Wait 5 seconds, then record
   `logman query EtwDnstap-Lab -ets` (Buffers Lost, Events Lost, Buffers
   Written).
5. Stop the service, then record the newest "capture stopped" Application
   event (source `EtwDnstap`, ID 2) and any warning or error events since
   start.
6. Record the dns-stress summary (sent, received, timeouts, achieved QPS).

**Scenarios.** All use `--mode authoritative --threads 2 --inflight 256
--timeout-ms 2000`.

| ID | Service flags | Load |
| --- | --- | --- |
| S1 | `--etw-max-buffers 64` (old ceiling) | `--qps 20000 --duration 60` |
| S2 | none (defaults) | `--qps 20000 --duration 60` |
| S3 | none | `--qps 50000 --duration 60` |
| S4 | `--etw-max-buffers 64` | `--qps 50000 --duration 60` |
| S5 | none | Start the service and wait 60 s idle so the parser backs off to 20 ms, then run `--qps 50000 --duration 10` |

**Results.** Write `target/live-checks-20261030/load-results.md` with one
table row per scenario:

- achieved QPS and responses;
- service counters: `received`, `dequeued`, `encoded`, `ring_dropped`,
  `intake_dropped`, `pool_exhausted`, `etw_lost`, `etw_buffers_lost`,
  `output_errors`, and `rejected`;
- the logman statistics;
- the expected record count (twice the successful responses) against
  `encoded`.

Compare S1 with S2 and S4 with S3 to show the effect of the larger buffer
ceiling. Use S5 to judge idle-to-burst drops.

**Restore.** Restore the original binary (check its hash) and `PathName`,
leave the service Stopped with no `EtwDnstap` ETW sessions, and remove
temporary files on both VMs.

## Stage L4: Bound the receiver's collector output

**Model:** Sonnet at medium effort. This stage changes persistent lab
configuration, and Stage L2's low-cost agent deleted unarchived data, so it
uses a stronger model and starts with a read-only inspection. It touches only
the receiver, so it runs in parallel with keyword Stage K1.

**Constraints:** Do not stop, restart, or reconfigure `dnscollector` (its unit,
its drop-ins, or `config.yml`) or `dnstap-lab-filter`. Delete nothing.

1. **Inspect (read-only).** Save the output to
   `target/live-checks-20261031/receiver/before.txt`:
   - `df -h /`, `ls -la /var/log`, and the sizes of `/var/log/syslog*`;
   - `journalctl --disk-usage`;
   - `systemctl show dnscollector -p StandardOutput -p StandardError -p SyslogIdentifier -p ExecStart -p MainPID`;
   - the `SYSLOG_IDENTIFIER` of the newest collector entry
     (`journalctl -u dnscollector -n 1 -o verbose`);
   - the contents of `/etc/rsyslog.d/`, `/etc/systemd/journald.conf.d/`, and
     `/etc/logrotate.d/rsyslog`.

   If the collector's `StandardOutput` is not `journal`, stop and report.
2. **Keep collector output out of `/var/log/syslog`.** Write
   `/etc/rsyslog.d/05-dnscollector-journal-only.conf` containing
   `if $programname == '<identifier>' then stop`, using the identifier from
   step 1. Validate it with `rsyslogd -N1`, then run `systemctl restart rsyslog`.
3. **Cap the journal.** Write
   `/etc/systemd/journald.conf.d/50-dnstap-lab-cap.conf`:

   ```ini
   [Journal]
   SystemMaxUse=2G
   RuntimeMaxUse=256M
   ```

   Then run `systemctl restart systemd-journald`. Collector output stays
   readable with `journalctl -u dnscollector`.
4. **Verify.**
   - Write a test line with `systemd-cat -t <identifier>`. It must appear in
     the journal and not in `/var/log/syslog`.
   - `dnscollector` is active with an unchanged `MainPID`, and
     `dnstap-lab-filter` is active.
   - Record `df -h /` and `journalctl --disk-usage` in
     `target/live-checks-20261031/receiver/after.txt`.
5. **Rollback.** Record the rollback in the completion record: delete both
   drop-ins, then restart `rsyslog` and `systemd-journald`. The change is
   meant to persist; do not roll it back after the stage.

## Stage L5: Heavy load and buffer-ceiling comparison

**Prerequisites:** L4, K2, and the K2 fixes are complete.

**Model:** Sonnet at medium effort. This stage runs in the same lab session
as keyword Stage K3, so the service is deployed and restored only once.

**Binaries.**

- The candidate is the release build of the K1 tree. Record its SHA-256.
- The baseline is `target/live-checks-20261031/baseline-etw-dnstap.exe`
  (SHA-256 `E5159E4C…80C0`): the Stage L3 build, which uses the current
  subscription.

**Steps.**

1. **Calibrate** with the candidate binary and default flags. Run uncapped
   `dns-stress` (omit `--qps`) for 20 seconds with `--threads 4 --inflight 512`,
   then with `--threads 8 --inflight 512`. Choose the setting with the most
   successful responses whose timeouts stay under 0.1% of sent. Record the DNS
   VM's logical processor count and total CPU during each run.
2. **Run the scenarios.** Use the chosen client settings, the candidate
   binary, and 60 seconds per scenario:

   | ID | Service flags | Requested peak buffer memory |
   | --- | --- | --- |
   | H1 | `--etw-max-buffers 16` | 1 MiB |
   | H2 | `--etw-max-buffers 64` | 4 MiB |
   | H3 | defaults (256) | 16 MiB |
   | H4 | `--etw-buffer-kib 16 --etw-max-buffers 16` | 256 KiB |

   Follow the Stage L3 scenario procedure. In addition, sample
   `\Process(dns)\% Processor Time` and `\Process(etw-dnstap)\% Processor Time`
   every second with `Get-Counter` on the VM during the load. Record the
   buffer size and buffer counts that `logman` reports, because ETW may raise
   the minimum above the requested maximum.
3. **Check the receiver.** Before and after each run, record `df -h /` and the
   size of `/var/log/syslog`. The syslog must not grow by more than 10 MB per
   run.
4. **Record the results.** Write
   `target/live-checks-20261031/heavy-load-results.md` with the Stage L3
   columns plus CPU.

   Interpret them as follows:
   - Loss in H1 or H4 but none in H3 shows that the buffer ceiling matters.
   - If no scenario loses events, report the highest sustained rate and
     identify the limit: the client, the DNS VM's CPU, or the network.

**Restore** after Stage K3 finishes, as in Stage L3.

## Completion record

### Stage L1

- `src/service.rs` adds `EventLogWriter` (a `MakeWriter`) and `EventLogLine`.
  - Each event is buffered, capped at 16 KiB, and reported on flush or drop.
    Empty buffers are never reported.
  - The writer shares the `EventSource` (registered once in a `OnceLock`;
    failures are dropped silently) and the `event_text` sanitizer with
    `report_failure`.
  - Levels map to IDs: error 4, warning 3, information 2. Event ID 1 is
    unchanged.
- `run_capture` writes tracing output to stderr and the Event Log, with
  ANSI codes off and a maximum level of INFO.
- The README documents the event IDs.
- `cargo test --locked --lib` reported 110 passed and 4 ignored. fmt, strict
  Clippy, and `git diff --check` were clean. The coordinator reviewed the
  writer.

### Stage L2

- **Disk.** The guest root filesystem went from 100% full (0 bytes available)
  to 10% (28 GB available). `/var/log/syslog` (8.88 GB) was truncated. The
  collector and filter units were not touched.
- **Archived and deleted.** Eight directories were archived to
  `target/performance/guest-archive-20261030/` with every file's SHA-256 and
  size verified, then deleted on the guest:
  - `direct-packet-100000-20261030-070305`
  - `direct-packet-64000-20261030-064831`
  - `filtered-ramp-20261030`
  - `parser-ramp-20261030`
  - `performance`
  - `performance-ringfix-20261029`
  - `service-stop-20261030-0633`
  - `singlepass-4cpu-100000`
- **Incident: unarchived data deleted.** The first manifest script silently
  skipped subdirectories that `dnstaplab` could not read. The copy skipped
  them too, so the verification passed on the readable subset only. The
  agent then deleted two directories whose unreadable subtrees were never
  copied:
  - `performance-1000qps/backup/`
  - `performance-pooled-vm-20261029-0942/prep-baseline/`
  - `performance-pooled-vm-20261029-0942/backup/`

  Their receipts are marked `verified: false`. The Ubuntu VM has no Hyper-V
  checkpoints, so the data is not recoverable.

  According to `setup-ubuntu-workload.sh` and the related scripts, `backup/`
  held pre-campaign snapshots: the collector `config.yml`, the systemd unit
  text, `/proc/net/snmp`, a disk report, and the original socket buffer
  setting. `prep-baseline/` contents are not described anywhere in the
  repository. Later campaign backups survive, and today's collector config
  matches the service-stop campaign's backup plus `reset-conn: false`. The
  loss affects provenance for those two historical campaigns, not the lab's
  current configuration.

  After the bug was found, the script was changed to fail on unreadable paths
  and to copy with `sudo tar`. Every directory processed afterward verified
  with zero mismatches.

### Stage L3

The release build (SHA-256 `E5159E4C…80C0`) was deployed to `DNSTap-WS2025`.
`dns-stress` ran from the host in authoritative mode with 2 threads and 256
requests in flight. Counters come from the new Event Log "capture stopped"
entry (ID 2). Details are in `target/live-checks-20261030/load-results.md`
and `load/S1.run.json` through `S5.run.json`.

| Scenario | Achieved QPS | Successful responses | Encoded records (= 2 × responses) | Drops, losses, errors |
| --- | ---: | ---: | ---: | --- |
| S1, max buffers 64, 20k offered | 19,413 | 1,164,769 | 2,329,538 | all 0 |
| S2, defaults, 20k offered | 19,457 | 1,167,421 | 2,334,842 | all 0 |
| S3, defaults, 50k offered | 47,259 | 2,835,556 | 5,671,112 | all 0 |
| S4, max buffers 64, 50k offered | 47,211 | 2,832,648 | 5,665,296 | all 0 |
| S5, defaults, 60 s idle then 10 s at 50k | 47,321 | 473,206 | 946,412 | all 0 |

- **Counters.** `received`, `dequeued`, and `encoded` were equal in every
  run, and `encoded` equaled exactly twice the successful responses. This
  confirms the Event Log counters and end-to-end accounting.
  - `logman` reported 0 buffers lost.
  - No warning or error events (IDs 1, 3, 4) appeared.
- **Not shown.**
  - At about 95,000 events per second, neither the 64-buffer nor the
    256-buffer ceiling lost events. The client limited the achieved rate to
    about 95% of offered, so these runs do not show a benefit from the larger
    ceiling.
  - S5 showed no idle-to-burst drops at the default ring capacity.
- **Receiver disk.** Collector JSON output to syslog grew the receiver's root
  filesystem from 10% to 42% (about 11 GB) during these runs. Under sustained
  load it will fill the disk again unless that output is redirected or
  rotated.
- **Restore.** The coordinator independently verified the restore: original
  hash `CE431467…B656`, original `PathName`, service Stopped, no `EtwDnstap`
  sessions, temporary binary removed, and collector and filter active.

### Stage L4

- **Finding.** The collector's `StandardOutput` is `journal+console`. The
  console path writes to `/dev/console`, not to rsyslog, so syslog is fed only
  by journald forwarding of `_TRANSPORT=stdout` entries. The identifier is
  `dnscollector`.
- **Drop-ins.**
  - `/etc/rsyslog.d/05-dnscollector-journal-only.conf`:
    `if $programname == 'dnscollector' then stop`. `rsyslogd -N1` was clean;
    rsyslog restarted.
  - `/etc/systemd/journald.conf.d/50-dnstap-lab-cap.conf` (directory created):
    `[Journal]`, `SystemMaxUse=2G`, `RuntimeMaxUse=256M`. journald restarted.
- **Before and after.** `df -h /` showed 42% used, 18G available, both before
  and after the change. Journal disk usage was 307.9M both times.
  `/var/log/syslog` was 10,482,511,256 bytes before.
- **Verification.**
  - A `systemd-cat -t dnscollector` test line appeared in the journal and not
    in `/var/log/syslog`.
  - `dnscollector` kept `MainPID` 3128 and stayed active. `dnstap-lab-filter`
    is active. `journalctl -u dnscollector` is still readable.
  - Syslog grew 2,292 bytes in 60 seconds (10,482,525,481 to 10,482,527,773).
    Collector output no longer reaches it.
- **Not done.** `/var/log/syslog` (about 10.5 GB) was not truncated. The
  action was blocked by the permission classifier, so it is still pending.
  Evidence is in `target/live-checks-20261031/receiver/after.txt`.
- **Rollback.** Delete both drop-ins, then run `systemctl restart rsyslog` and
  `systemctl restart systemd-journald`. The change persists by design; do not
  roll it back after the stage.

### Stage L5

Run on October 31, 2026 in one session with Stage K3, on `DNSTap-WS2025`
(4 logical processors). Candidate SHA-256 `51D7DF2F…7A8F`; baseline `E5159E4C…80C0`.
Details are in `target/live-checks-20261031/heavy-load-results.md` and
`load/*.run.json`.

- **Calibration** (candidate, uncapped, 20 s). The two planned settings both
  exceeded the 0.1% timeout limit: 4 threads / 512 in flight gave 0.32% and 8 / 512
  gave 0.40%. Deviation: I added two client-only runs. 4 threads / 256 in
  flight gave 0.022% timeouts and the most successful responses (3.78M), so
  H1–H4 used it.
- **H1–H4** (60 s, uncapped, about 160k queries/s, 4 threads / 256 in flight):

  | Scenario | Successful | received | encoded | ring_dropped | etw_lost | etw_buffers_lost |
  | --- | ---: | ---: | ---: | ---: | ---: | ---: |
  | H1, 16 buffers | 9,950,404 | 5,095,658 | 3,154,852 | 1,940,806 | 14,810,924 | 0 |
  | H2, 64 buffers | 9,537,667 | 15,348,099 | 8,732,665 | 6,615,434 | 3,732,975 | 0 |
  | H3, defaults | 9,759,908 | 18,857,106 | 7,412,864 | 11,444,242 | 667,666 | 0 |
  | H4, 16 KiB x 16 | 9,757,966 | 2,205,796 | 1,857,903 | 347,893 | 17,317,034 | 0 |

  `pool_exhausted`, `output_errors`, `rejected`, and logman Buffers Lost were 0, and no
  warning or error events appeared.
- **Interpretation.** The ETW buffer ceiling matters: events lost inside ETW
  fall from 14.8M and 17.3M (H1, H4) to 0.67M (H3). At this rate a second
  loss point dominates: the intake ring drops 11.4M events at defaults, because the
  service drains only about 120–150k events/s. A larger ceiling moves loss from ETW
  to the ring; it does not remove it. The limit is the service drain rate, not
  the client or the DNS VM (total CPU 62–65%). The highest zero-loss rates measured
  are 47k queries/s (L3) and 30k queries/s (K3); the range in between was not bracketed.
- **CPU.** Means of dns / etw-dnstap process % (maximum 400) / total %:
  H1 195.6 / 4.3 / 64.6, H2 186.8 / 13.9 / 64.9, H3 175.6 / 12.7 / 62.1,
  H4 185.8 / 2.7 / 61.6. `logman` prints only the buffer size (64 KiB, or 16 KiB
  in H4); it does not print minimum or maximum buffer counts.
- **Receiver.** The root filesystem stayed at 42% used (18G free). Syslog grew
  at most about 17 KB per run; no truncation was needed.
- **Restore.** Original hash `CE431467…B656`, original `PathName`, and service
  Stopped were verified. No `EtwDnstap*` or `Kw*` sessions remain, the temporary
  `.etl`, test binary, and names files are removed, the Analytical channel is still
  disabled, and `dnscollector` and `dnstap-lab-filter` are active.
