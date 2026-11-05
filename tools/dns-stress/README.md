# DNS stress client

Windows host utility: pipelined UDP A/IN queries to a lab DNS server, one connected UDP socket per worker. Run it on the Hyper-V host to keep its cost off the DNS VM.

### Quick Start

From the repository root, with names files from the [pinned workload procedure](../../datasets/performance/README.md#generate-the-workload) and a fresh output directory:

```powershell
$dnsStress = 'tools/dns-stress/target/x86_64-pc-windows-msvc/release/dns-stress.exe'
& $dnsStress --server 172.27.215.163:53 `
  --names target/performance/workload/auth-names.txt `
  --mode authoritative --threads 2 --duration 300 --inflight 256 `
  --timeout-ms 2000 --qps 1000 `
  --output-dir target/performance/auth-2threads
```

### Steps / Configuration

#### Build and verify

Windows x64 host with the [build prerequisites](../../docs/guide/build.md#build-the-executable). From the repository root:

```powershell
cargo fmt --manifest-path tools/dns-stress/Cargo.toml --check
cargo test --manifest-path tools/dns-stress/Cargo.toml --locked
cargo clippy --manifest-path tools/dns-stress/Cargo.toml --all-targets `
  --locked -- -D warnings
cargo build --manifest-path tools/dns-stress/Cargo.toml --release --locked
```

Own lockfile. The executable imports Windows system DLLs only; inspect its imports after changes.

#### Run a measured scenario

- Verify the DNS server, zone, and recursive upstream with independent address and TTL probes. The example targets the recorded DNS VM `172.27.215.163`: verify its identity first.
- It sets a **global 1,000 QPS ceiling** across two workers, as in the campaigns ([review](../../docs/history/performance-review-2026-10-28.md), [pooled VM review](../../docs/performance/pooled-vm-performance-review.md)). Achieved QPS can be lower.
- Recursive: recreate the `rec.stress.test` forwarder and Ubuntu upstream from the [performance test plan](../../docs/history/performance-test-plan.md), then use `rec-names.txt`, `--mode recursive`, a new `--output-dir`.
- Authoritative sends RD=0 and checks AA=1. Recursive sends RD=1 and checks RA=1. Both check the echoed RD bit.
- Success: RCODE=NOERROR, at least one answer, expected flags, TC=0. Record bounds are checked. Answer address and TTL aren't, so run independent fixture probes.
- `--qps` splits equally across workers. Slot `k` is due `ceil(k * threads * 1e9 / qps)` ns after the shared start. A late worker sends all due slots, at most 64 per loop pass, within `--inflight`. If the oldest due slot is over 100 ms late, it drops older slots, counts them in `pacing_slots_skipped`, and continues at the latest.
- **No `--qps` = uncapped.** Tune `--threads` and `--inflight` first.
- Sending stops at `start + duration`. Pending requests drain for at most `--timeout-ms`. No warm-up.
- Afterward check `summary.json` and `metrics.jsonl` ([Interpret the output](#interpret-the-output)).

| Flag | Default or range | Meaning |
| --- | --- | --- |
| `--server IP:PORT` | Required | IPv4 or `[IPv6]` |
| `--names PATH` | Required | UTF-8, one per line, `#` comments |
| `--mode MODE` | Required | `authoritative`, `recursive` |
| `--threads COUNT` | `1`, 1–64 | Workers |
| `--duration SECONDS` | `300`, 1–3600 | Sending time |
| `--inflight COUNT` | `256`, 1–8192 | Pending per worker |
| `--timeout-ms MILLISECONDS` | `2000`, 1–60000 | Request timeout |
| `--qps RATE` | None, 1–1000000000 | Global ceiling |
| `--output-dir PATH` | Required | Gets `summary.json`, `metrics.jsonl` |
| `--help`, `--version` | Optional | Usage, package version |

Names: lowercased; labels are ASCII letters, digits, hyphens, underscores, one optional trailing dot. Max 16 MiB and 100000 names. Each worker cycles the list.

#### Interpret the output

- `summary.json`, schema version 1: `start_unix_ms`, `sending_end_unix_ms` (UTC window), `end_unix_ms` (with drain), `elapsed_with_drain_ns` (monotonic), `measured_send_duration_ns` (configured sending interval). Average sending QPS = `counts.sent` / duration.
- `metrics.jsonl`: cumulative per-worker records about once per second, plus `final_sample=true`. Fields: worker, `elapsed_ns`, `outstanding`, `counts`, `latency`. Worker order is unspecified. Difference consecutive records for interval rates.

| Counter | Meaning |
| --- | --- |
| `sent` | Accepted by send call |
| `completed` | Valid matching response by deadline |
| `successful` | Completed, checks pass |
| `timeouts` | No valid response by deadline |
| `send_errors`, `receive_errors` | Local socket errors |
| `malformed` | Failed wire, header, or A-IN validation |
| `mismatched` | ID used, name differs |
| `unsolicited` | ID never sent on this socket |
| `late` | Response after timeout |
| `duplicate` | Response after completion |
| `bytes_sent`, `bytes_received` | DNS bytes only |
| `rcode` | Completions by 4-bit RCODE |
| `aa_missing`, `ra_missing`, `rd_mismatch` | Flag violations |
| `truncated` | TC=1 completions (no TCP retry) |
| `noerror_without_answers` | NOERROR, zero answers |
| `peak_outstanding` | Worker peak (aggregate: sum) |
| `sockets_used` | Current and retired sockets |
| `rotation_count` | Completed socket rotations |
| `rotation_wait_us` | Microseconds paused for ID-exhaustion drain |
| `pacing_slots_skipped` | Slots over 100 ms behind (0 uncapped) |

- Drained run: `sent = completed + timeouts`, `outstanding_final = 0`. Paced run: `sent` plus `pacing_slots_skipped` is about ceiling times duration.
- A worker error shows in `workers[].error`. The process exits unsuccessfully after writing partial results.
- Latency: just before send to matching valid response, so it includes client scheduling delay. Completed RCODEs count. Timeouts and late packets don't.
- `p50_ns_upper`, `p95_ns_upper`, `p99_ns_upper`: histogram upper bounds (bin width at most about 3.125%). `max_ns`: observed maximum. Empty histogram: null percentiles.
- 2048 `latency_histogram_bins`, 32 per power of two: index `floor(log2(max(ns,1))) * 32 + floor((ns - 2^power) * 32 / 2^power)`.

#### Response identity and resource bounds

- The OS filters replies to the server IP and port. The client validates ID, question name, QTYPE=A, QCLASS=IN, QR=1, opcode=0, record lengths, bounded name compression.
- Each socket uses every 16-bit ID once. After 65536 sends it drains and rotates to a new local endpoint. Retired sockets stay bound until all workers finish, so delayed packets can't match a later generation.
- Retention: `floor(8192 / threads)` sockets per worker. Exhausting it fails the worker. Windows ephemeral-port limits can be lower.
- `retired_socket_packets_unobserved` flags packets on retired sockets. Late and duplicate counts are lower bounds.
- Bounds: 65536 entries and 8192 pending queries per worker, the socket budget, fixed histograms, the metrics channel, the names input.
- Workers poll nonblocking sockets and yield when idle. Count host CPU separately.
- Safe Rust. See the [UDP socket API](https://doc.rust-lang.org/std/net/struct.UdpSocket.html) and [C runtime linkage](https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes).

### Common Gotchas

- **Existing result files cause an error.** Use a fresh `--output-dir`.
- **A zero exit code doesn't show DNS validity.** Check `successful`, errors, flags, RCODEs.
- **A timeout isn't proof of packet loss.** The client reports no drop count.
- **ID exhaustion stalls workers.** One unanswered request can pause a worker for nearly its timeout (periodic dips). Check `rotation_count` and `rotation_wait_us`. Waiting fraction = aggregate wait / `threads * duration * 1000000`. Live metrics include an ongoing pause. Final counters stop at the sending deadline.
- **Slow output storage can delay workers.**
- **Closed loop.** Slow responses and full windows reduce offered load. Nonzero `pacing_slots_skipped` means under-offering.
- **No coordinated-omission correction.** Read achieved QPS, timeouts, and server resource use with percentiles.
- **Client counters don't show dnstap delivery.** Pair them with the [service and receiver gates](../../docs/design/measurement-terms.md#independent-result-gates).
