# DNS stress client

Use this Windows host utility to send pipelined UDP A/IN queries to an explicitly
selected DNS laboratory server. Each worker owns a connected UDP socket and its
request state. Run the executable on the Hyper-V host to keep its CPU and memory
cost outside the DNS VM.

## Before you begin

Prepare the following before sending load:

- Use a Windows x64 host with the toolchain and Build Tools required by the
  [project build instructions](../../docs/guide/install.md#build-the-executable).
- Generate the names files with the
  [pinned workload procedure](../../datasets/performance/README.md#generate-the-workload).
- Verify the selected lab DNS server, authoritative zone, and recursive upstream
  fixtures with independent address and TTL probes. The files alone do not
  provision a DNS server.
- Choose a fresh output directory for each run. Existing result files cause
  an error.

## Build and verify

Run these commands from the repository root so Cargo discovers the repository's
nightly toolchain, static CRT, rebuilt standard library, and Windows hardening
configuration:

```powershell
cargo fmt --manifest-path tools/dns-stress/Cargo.toml --check
cargo test --manifest-path tools/dns-stress/Cargo.toml --locked
cargo clippy --manifest-path tools/dns-stress/Cargo.toml --all-targets `
  --locked -- -D warnings
cargo build --manifest-path tools/dns-stress/Cargo.toml --release --locked
```

The standalone package has its own lockfile. The release executable is
`tools/dns-stress/target/x86_64-pc-windows-msvc/release/dns-stress.exe`.
It needs Windows system DLLs and does not need an installed Visual C++ runtime.
Inspect each production binary's imports to verify this property after changes.

## Run a measured scenario

The examples use the recorded Windows DNS VM at `172.27.215.163`. Verify its
identity and fixture readiness before using that address. Both examples request
a **global 1,000 QPS ceiling**, shared across two workers. Achieved QPS can be lower.
For the authoritative scenario, run:

```powershell
$dnsStress = 'tools/dns-stress/target/x86_64-pc-windows-msvc/release/dns-stress.exe'
& $dnsStress --server 172.27.215.163:53 `
  --names target/performance/workload/auth-names.txt `
  --mode authoritative --threads 2 --duration 300 --inflight 256 `
  --timeout-ms 2000 --qps 1000 `
  --output-dir target/performance/auth-2threads
```

Before rerunning the recursive scenario, recreate the temporary
`rec.stress.test` conditional forwarder and Ubuntu upstream from the
[performance test plan](../../docs/history/performance-test-plan.md). The original
campaign's cleanup removed this recursive fixture and retained the 5,000
authoritative records. Verify both fixture states before another run.
Then change the names file and mode:

```powershell
& $dnsStress --server 172.27.215.163:53 `
  --names target/performance/workload/rec-names.txt `
  --mode recursive --threads 2 --duration 300 --inflight 256 `
  --timeout-ms 2000 --qps 1000 `
  --output-dir target/performance/rec-2threads
```

Authoritative mode sends RD=0 and checks AA=1. Recursive mode sends RD=1 and
checks RA=1. Both modes check the echoed RD bit. RA can also be set on
authoritative replies. A successful response has RCODE=NOERROR, at least one
answer record, the expected flags, and TC=0. The client validates record bounds
but does not verify that an answer is an A record with the configured fixture
address or TTL. Run independent fixture probes to establish answer correctness.

The historical pilot comparison selected two workers for subsequent campaigns.
See the [original performance review](../../docs/history/performance-review-2026-10-28.md)
for that selection and the [pooled VM review](../../docs/performance/pooled-vm-performance-review.md)
for the later capped ramp and confirmations. The thread count remains configurable.

Change `--qps` to select a different global sending ceiling. Each worker receives
an equal share. **Omitting `--qps` selects uncapped mode:** each worker fills its outstanding window
as quickly as send and receive processing permit. Tune `--threads` and
`--inflight` during calibration before selecting the measured load.

The shared monotonic start follows worker setup. Sending stops at
`start + duration`. Each send checks this deadline. Pending requests then drain
until their own response deadlines, at most `--timeout-ms` after the sending
interval. Scheduler and I/O delays can affect achieved throughput. The program
does not insert warm-up time. Use a different output directory for each run.
Existing result files cause an error.

After the run, confirm that `summary.json` and `metrics.jsonl` exist in the
selected output directory. Inspect the final counts, worker errors, and actual
rate as described in [Interpret the output](#interpret-the-output). A zero exit
code alone is not a passing DNS-validity result.

The following flags control each run:

| Flag | Required or default | Bounds and meaning |
| --- | --- | --- |
| `--server IP:PORT` | Required | Numeric IPv4 or bracketed IPv6. Nonzero port |
| `--names PATH` | Required | UTF-8 file, one name per line. `#` starts comments |
| `--mode MODE` | Required | `authoritative` or `recursive` |
| `--threads COUNT` | `1` | 1–64 workers |
| `--duration SECONDS` | `300` | 1–3600 seconds of sending |
| `--inflight COUNT` | `256` | 1–8192 pending queries per worker |
| `--timeout-ms MILLISECONDS` | `2000` | 1–60000 milliseconds per request |
| `--qps RATE` | No ceiling | 1–1000000000 global queries/second ceiling |
| `--output-dir PATH` | Required | Creates `summary.json` and `metrics.jsonl` |
| `--help` | Optional | Displays usage |
| `--version` | Optional | Displays package version |

Names are normalized to lowercase. Labels accept ASCII letters, digits, hyphens,
and underscores. One trailing dot is accepted. Empty labels, labels exceeding
63 bytes, and names exceeding 255 wire bytes are rejected. Input is limited to
16 MiB and 100000 names. Each worker cycles through the supplied list.

## Interpret the output

`summary.json` uses schema version 1. Its `start_unix_ms` and
`sending_end_unix_ms` identify the measured sending window in UTC Unix
milliseconds. `end_unix_ms` includes the drain and output collection.
`measured_send_duration_ns` is the configured sending interval.
`elapsed_with_drain_ns` uses the monotonic clock. Divide `counts.sent` by the
configured duration to calculate average achieved sending QPS.

`metrics.jsonl` contains cumulative records from each worker about once per
second, plus a record with `final_sample=true` at shutdown. Each record has a
worker number, `elapsed_ns` from the shared start, `outstanding`, `counts`, and
`latency`. Record ordering across workers is unspecified. Difference consecutive
records for the same worker to calculate interval rates, using their actual
elapsed timestamps. Do not sum cumulative samples across time. Metric writes
use a bounded channel. Slow output storage can delay workers.

Counter meanings are as follows:

| Counter | Meaning |
| --- | --- |
| `sent` | Complete datagrams accepted by the local send call |
| `completed` | Valid matching responses received before their deadlines |
| `successful` | Completed responses satisfying the answer and flag checks |
| `timeouts` | Sent requests without a matching valid response by their deadline |
| `send_errors`, `receive_errors` | Local socket operation errors |
| `malformed` | Replies failing DNS wire/header/A-IN question validation |
| `mismatched` | Reply ID was used, but its question name did not match |
| `unsolicited` | Reply ID has never been sent on this socket |
| `late` | Matching responses to requests already classified as timed out |
| `duplicate` | Matching responses to requests already completed |
| `bytes_sent`, `bytes_received` | DNS datagram bytes. No IP/UDP/Ethernet overhead |
| `rcode` | Array indexed by the basic four-bit DNS RCODE, for completions |
| `aa_missing`, `ra_missing`, `rd_mismatch` | Expected flag violations |
| `truncated` | Completed responses with TC=1. No TCP retry |
| `noerror_without_answers` | NOERROR completions with zero answer records |
| `peak_outstanding` | Worker peak. Aggregate is the sum of worker peaks |
| `sockets_used` | Current and retired sockets across generations |
| `rotation_count` | Completed socket rotations |
| `rotation_wait_us` | Cumulative microseconds paused for ID-exhaustion drain during the sending window |

A timeout does not prove network packet loss. This client cannot measure kernel
or network dropped datagrams, so it reports no fabricated drop count. In a
successful drained run, `sent = completed + timeouts` and
`outstanding_final = 0`. Worker errors retain outstanding requests and appear
in `workers[].error`. The process exits unsuccessfully after writing partial
results. A successful exit alone does not imply that all DNS responses passed
the answer checks: inspect `successful`, errors, flags, and RCODEs.

Latency is measured from immediately before the send call to application
receipt of a matching valid response. It includes client scheduling delay.
All completed RCODEs contribute. Timeouts and late packets do not. `p50_ns_upper`,
`p95_ns_upper`, and `p99_ns_upper` are histogram upper bounds with at most
approximately 3.125% bin width. `max_ns` is the observed maximum. Empty
histograms produce null percentiles. The 2048 `latency_histogram_bins` use
32 equal subdivisions per power of two in nanoseconds: index
`floor(log2(max(ns,1))) * 32 + floor((ns - 2^power) * 32 / 2^power)`.

The load is a bounded closed loop. Slow responses and full windows reduce the
offered load. Pacing never catches up with a burst after a stall. The histogram
does not correct coordinated omission and does not represent latency for
queries that could have been offered during stalls. Compare achieved QPS,
timeouts, and server/service resource measurements alongside the percentiles.

## Response identity and resource bounds

The operating system filters replies to the connected server IP and port.
The client then validates the ID, normalized question name, QTYPE=A, QCLASS=IN,
QR=1, opcode=0, record lengths, and bounded name compression.

Each socket sends every 16-bit ID at most once. After 65536 sends, it drains
pending requests and rotates to a new local endpoint. Retired sockets remain
bound until all workers finish, so arbitrarily delayed packets cannot be
reassigned to a later ID generation. Socket retention is bounded to
`floor(8192 / threads)` sockets per worker. Exhausting this budget fails the
worker rather than reusing an endpoint. Windows ephemeral-port availability
can impose a lower limit. Rotation can temporarily reduce sending throughput,
especially when outstanding requests must time out.

Inspect `rotation_count` and `rotation_wait_us` in each worker's cumulative
metrics to quantify this client policy. A single unanswered request can pause
that worker for nearly its full timeout after ID exhaustion, creating periodic
throughput dips. This can limit achieved QPS independently of server capacity.
Live metrics include an ongoing pause. Final counters exclude time after the
sending deadline. The aggregate wait is summed across workers, so divide it by
`threads * duration * 1000000` to calculate the fraction of available worker
time spent waiting. The counter excludes the brief socket creation and state
reset operations. Wait durations are rounded down to microseconds per pause.

Packets arriving on retired sockets after their drain are not polled.
`retired_socket_packets_unobserved` identifies runs where this limitation
applies. Late and duplicate counts are observed lower bounds. These limits
bound the state:

- 65536 entries per worker, and at most 8192 pending queries per worker.
- The socket budget.
- Fixed histograms.
- The bounded metrics channel and the bounded names input.

Workers poll nonblocking sockets and yield while idle. Account for
host CPU use separately from the DNS VM.

The implementation uses safe Rust throughout. Build decisions follow the
[Rust UDP socket API](https://doc.rust-lang.org/std/net/struct.UdpSocket.html),
the [Rust C runtime linkage reference](https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes),
and the repository hardening configuration.

For capture fidelity and service resource use, pair the client results with the
[service and receiver gates](../../docs/design/measurement-terms.md#independent-result-gates).
Client latency and success counters alone do not establish dnstap record delivery.
