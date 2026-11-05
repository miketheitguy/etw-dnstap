# DNS stress client

This Windows host utility sends pipelined UDP A/IN queries to a lab DNS server. Each worker owns a connected UDP socket. Run it on the Hyper-V host to keep its cost outside the DNS VM.

## Before you begin

- Use a Windows x64 host with the [project build prerequisites](../../docs/guide/build.md#build-the-executable).
- Generate the names files with the [pinned workload procedure](../../datasets/performance/README.md#generate-the-workload).
- Verify the DNS server, zone, and recursive upstream with independent address and TTL probes.
- Use a fresh output directory for each run. Existing result files cause an error.

## Build and verify

Run from the repository root so Cargo finds the repository toolchain and hardening configuration:

```powershell
cargo fmt --manifest-path tools/dns-stress/Cargo.toml --check
cargo test --manifest-path tools/dns-stress/Cargo.toml --locked
cargo clippy --manifest-path tools/dns-stress/Cargo.toml --all-targets `
  --locked -- -D warnings
cargo build --manifest-path tools/dns-stress/Cargo.toml --release --locked
```

The package has its own lockfile. The executable is `tools/dns-stress/target/x86_64-pc-windows-msvc/release/dns-stress.exe`. It imports Windows system DLLs only. Inspect its imports after changes.

## Run a measured scenario

The example uses the recorded DNS VM at `172.27.215.163`. Verify its identity first. It requests a **global 1,000 QPS ceiling** across two workers. Achieved QPS can be lower.

```powershell
$dnsStress = 'tools/dns-stress/target/x86_64-pc-windows-msvc/release/dns-stress.exe'
& $dnsStress --server 172.27.215.163:53 `
  --names target/performance/workload/auth-names.txt `
  --mode authoritative --threads 2 --duration 300 --inflight 256 `
  --timeout-ms 2000 --qps 1000 `
  --output-dir target/performance/auth-2threads
```

For the recursive scenario, recreate the `rec.stress.test` conditional forwarder and Ubuntu upstream from the [performance test plan](../../docs/history/performance-test-plan.md). Then use `rec-names.txt`, `--mode recursive`, and a new `--output-dir`.

Authoritative mode sends RD=0 and checks AA=1. Recursive mode sends RD=1 and checks RA=1. Both check the echoed RD bit. A successful response has RCODE=NOERROR, at least one answer record, the expected flags, and TC=0. The client checks record bounds. It does not check the answer address or TTL, so run independent fixture probes.

The campaigns used two workers. See the [original performance review](../../docs/history/performance-review-2026-10-28.md) and the [pooled VM review](../../docs/performance/pooled-vm-performance-review.md).

`--qps` sets the global ceiling, split equally across workers. Send slot `k` is due `ceil(k * threads * 1e9 / qps)` ns after the shared start. A late worker sends all due slots, at most 64 per loop pass, within the `--inflight` window. If the oldest due slot is over 100 ms late, the worker drops the older slots, counts them in `pacing_slots_skipped`, and continues at the latest slot. **Omit `--qps` for uncapped mode:** each worker fills its window as fast as it can. Tune `--threads` and `--inflight` before the measured run.

Sending stops at `start + duration`. Pending requests then drain for at most `--timeout-ms`. The client adds no warm-up time.

After the run, check that `summary.json` and `metrics.jsonl` exist and read the counts, worker errors, and rate in [Interpret the output](#interpret-the-output). A zero exit code does not show DNS validity.

| Flag | Required or default | Meaning |
| --- | --- | --- |
| `--server IP:PORT` | Required | Numeric IPv4 or bracketed IPv6 |
| `--names PATH` | Required | UTF-8, one name per line, `#` comments |
| `--mode MODE` | Required | `authoritative` or `recursive` |
| `--threads COUNT` | `1` | Workers, 1–64 |
| `--duration SECONDS` | `300` | Sending time, 1–3600 |
| `--inflight COUNT` | `256` | Pending per worker, 1–8192 |
| `--timeout-ms MILLISECONDS` | `2000` | Request timeout, 1–60000 |
| `--qps RATE` | No ceiling | Global ceiling, 1–1000000000 |
| `--output-dir PATH` | Required | Receives `summary.json`, `metrics.jsonl` |
| `--help` | Optional | Usage |
| `--version` | Optional | Package version |

Names are lowercased. Labels take ASCII letters, digits, hyphens, and underscores, with one optional trailing dot. Labels over 63 bytes and names over 255 wire bytes are rejected. Input is limited to 16 MiB and 100000 names. Each worker cycles through the list.

## Interpret the output

`summary.json` uses schema version 1. `start_unix_ms` and `sending_end_unix_ms` mark the sending window (UTC). `end_unix_ms` includes the drain. `elapsed_with_drain_ns` is monotonic. Divide `counts.sent` by the duration for average sending QPS. `measured_send_duration_ns` is the configured sending interval.

`metrics.jsonl` holds cumulative records per worker about once per second, plus a `final_sample=true` record. Each has a worker number, `elapsed_ns`, `outstanding`, `counts`, and `latency`. Worker order is unspecified. Difference consecutive records of one worker for interval rates. Slow output storage can delay workers.

| Counter | Meaning |
| --- | --- |
| `sent` | Datagrams accepted by the send call |
| `completed` | Valid matching responses before deadline |
| `successful` | Completed and passing answer and flag checks |
| `timeouts` | Sent without a valid response by deadline |
| `send_errors`, `receive_errors` | Local socket errors |
| `malformed` | Failed wire, header, or A-IN validation |
| `mismatched` | ID used, question name differs |
| `unsolicited` | ID never sent on this socket |
| `late` | Responses to requests already timed out |
| `duplicate` | Responses to requests already completed |
| `bytes_sent`, `bytes_received` | DNS datagram bytes only |
| `rcode` | Completions by four-bit RCODE |
| `aa_missing`, `ra_missing`, `rd_mismatch` | Flag violations |
| `truncated` | Completions with TC=1 (no TCP retry) |
| `noerror_without_answers` | NOERROR with zero answers |
| `peak_outstanding` | Worker peak (aggregate: sum of peaks) |
| `sockets_used` | Current and retired sockets |
| `rotation_count` | Completed socket rotations |
| `rotation_wait_us` | Microseconds paused for ID-exhaustion drain |
| `pacing_slots_skipped` | Slots dropped over 100 ms behind (0 when uncapped) |

A timeout does not prove packet loss, and the client reports no drop count. In a drained run, `sent = completed + timeouts` and `outstanding_final = 0`. A worker error appears in `workers[].error`, and the process exits unsuccessfully after writing partial results. Check `successful`, errors, flags, and RCODEs.

Latency runs from just before the send call to receipt of a matching valid response, so it includes client scheduling delay. Completed RCODEs contribute. Timeouts and late packets are excluded. `p50_ns_upper`, `p95_ns_upper`, and `p99_ns_upper` are histogram upper bounds (bin width at most about 3.125%). `max_ns` is the observed maximum. Empty histograms give null percentiles. The 2048 `latency_histogram_bins` use 32 subdivisions per power of two: index `floor(log2(max(ns,1))) * 32 + floor((ns - 2^power) * 32 / 2^power)`.

The load is a bounded closed loop: slow responses and full windows reduce offered load. A nonzero `pacing_slots_skipped` shows the client under-offered. In a paced run, `sent` plus `pacing_slots_skipped` is about the ceiling times the duration. The histogram does not correct for coordinated omission. Read achieved QPS, timeouts, and server resource use with the percentiles.

## Response identity and resource bounds

The OS filters replies to the connected server IP and port. The client validates the ID, question name, QTYPE=A, QCLASS=IN, QR=1, opcode=0, record lengths, and bounded name compression.

Each socket sends every 16-bit ID once. After 65536 sends it drains and rotates to a new local endpoint. Retired sockets stay bound until all workers finish, so delayed packets cannot match a later generation. Retention is `floor(8192 / threads)` sockets per worker, and exhausting it fails the worker. Windows ephemeral-port limits can be lower.

After ID exhaustion, one unanswered request can pause a worker for nearly its timeout, which causes periodic throughput dips. Check `rotation_count` and `rotation_wait_us`. Divide the aggregate wait by `threads * duration * 1000000` for the fraction of worker time spent waiting. Live metrics include an ongoing pause. Final counters stop at the sending deadline.

`retired_socket_packets_unobserved` marks runs with packets on retired sockets. Late and duplicate counts are lower bounds. State is bounded to 65536 entries and 8192 pending queries per worker, the socket budget, fixed histograms, the metrics channel, and the names input.

Workers poll nonblocking sockets and yield while idle. Count host CPU separately from the DNS VM.

The implementation is safe Rust. See the [Rust UDP socket API](https://doc.rust-lang.org/std/net/struct.UdpSocket.html) and the [C runtime linkage reference](https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes).

Pair client results with the [service and receiver gates](../../docs/design/measurement-terms.md#independent-result-gates). Client counters alone do not show dnstap delivery.
