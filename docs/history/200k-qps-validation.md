# 200,000 QPS validation

> **Status:** Historical · **Evidence dated:** 2026-11-01 (guest local time) · **Commit:** `fcecb56` (lab build) · **Reviewed:** 2026-11-01

This record covers two lab runs at 200,000 queries per second (QPS) with the
largest ETW buffer size and the largest channel. It records how many ETW
buffers and channel slots the service used. The recommended settings for
200,000 QPS in the
[configuration reference](../guide/configuration.md#settings-for-200000-queries-per-second)
come from this record.

The raw run data is lab evidence. It is not in the repository.

## Setup

| Item | Value |
| --- | --- |
| Guest | DNSTap-WS2025: 6 vCPUs, 4 GiB, Windows Server 2025 (10.0.26100). Rebooted before the runs. |
| Service build | `fcecb56`, release, SHA-256 `0C3673E0801FD5431B2F44BF16734A4351DEA88E46C1BF642F7105593E486DCC` |
| Service options | `--plaintext --etw-buffer-kib 128 --etw-max-buffers 256 --capacity 262144 --stats-interval-seconds 5`. The other options were the defaults. |
| Receiver | DNS-collector 3.2.3, plaintext. Its output went to the `devnull` logger, and its stdout and stderr went to null. It wrote nothing to disk. |
| Client | `dns-stress` at `6d49577` on the host. Authoritative load, 16 threads, 1,024 in flight per thread, `--qps 200000`, 50 s. |

The service restarted before each run, so its counters started at zero. A
sampler in the guest read the ETW session every 250 ms with
`ControlTraceW(EVENT_TRACE_CONTROL_QUERY)`. It recorded `NumberOfBuffers`,
`FreeBuffers`, `EventsLost`, and `RealTimeBuffersLost`, and the service's private
bytes. The sampler kept its samples in memory. The `capture stats` and
`output stats` lines came every 5 s. The receiver's TCP `bytes_received` was
read with `ss -tin` after the client stopped.

## Results

| Item | Run 1 | Run 2 |
| --- | --- | --- |
| Achieved QPS | 199,999.5 | 192,044 |
| Queries sent / answered | 9,999,975 / 9,992,398 | 9,602,209 / 9,600,230 |
| Client timeouts | 7,577 (0.08%) | 1,979 (0.02%) |
| `received` = `encoded` | 19,992,373 | 19,202,439 |
| `ring_dropped`, `pool_exhausted_*` | 0 | 0 |
| `etw_lost`, `etw_buffers_lost` | 0 | 0 |
| Receiver bytes per encoded frame | 103.01 | 103.01 |
| `etw_buffers_delivered` | 53,570 | 51,459 |
| Service CPU in 50 s | 17.1 s | 16.7 s |
| `dns.exe` CPU in 50 s | 146.7 s | 143.0 s |
| Private bytes | 683 to 684 MiB | 683 to 690 MiB |

- `received` was exactly the queries sent plus the responses. The service
  captured every query event, also for the queries that timed out.
- The receiver got 103.01 bytes for each encoded frame, so every encoded frame
  reached it.
- In run 2, the client stalled for about 2 s in second 44. It skipped 397,757
  send slots, so the achieved rate was 192,044 QPS. In every other second it
  sent about 200,000 queries.
- The service used about 0.34 of one CPU. The DNS server used about 2.9 CPUs.

## ETW buffers

The session had 128 KiB buffers. The service requested at most 256 buffers
(32 MiB). It requests a minimum of 4 buffers, and ETW raised the minimum to 2
per logical processor: 12 on this guest.

| Item | Run 1 | Run 2 |
| --- | --- | --- |
| Buffers at start | 12 (1.5 MiB) | 12 (1.5 MiB) |
| Most buffers allocated | 29 (3.6 MiB) | 27 (3.4 MiB) |
| Time to reach 16 buffers | 5.7 s | 3.4 s |
| Time to reach the peak | about 50 s (end of the run) | 4.4 s |
| Buffers in use (`NumberOfBuffers` − `FreeBuffers`) | 6 in most samples, 7 at most | 6 in most samples, 7 at most |
| Fewest free buffers | 8 | 8 |
| Events per delivered buffer | about 373 | about 373 |

- Six buffers in use is one buffer per logical processor that ETW fills. At
  most one full buffer waited for delivery at a sample time.
- ETW delivered about 1,070 buffers each second. Delivery was continuous. It
  was not one batch each `--etw-flush-seconds`.
- ETW allocated at most 29 of the 256 buffers that the service allowed (11%).
- ETW added buffers during the run and did not free them before the run
  ended. The allocated count is therefore higher than the count in use.
- A 250 ms sample can miss a short peak.

## Channel

| Item | Run 1 | Run 2 |
| --- | --- | --- |
| Capacity | 262,144 | 262,144 |
| `queue_depth_max` | 6,948 (2.7%) | 21,622 (8.2%) |
| Snapshot with the peak | first 5 s snapshot | first 5 s snapshot |

- Both peaks occurred when the load started. The peak did not increase after
  the first snapshot.
- The parser samples `queue_depth_max`, so it is a lower bound.
- The service allocates the pool for 262,144 slots at startup. Private bytes therefore
  stayed at about 684 MiB from the first sample.

## How the recommendation follows from the data

- **Buffer size.** The lab tested 128 KiB, and ETW lost nothing. Each buffer held about
  373 events.
Each recommended value is a small margin above the largest observed value.

| Setting | Largest observed | Recommended |
| --- | --- | --- |
| `--etw-buffer-kib` | 128 KiB, no loss | 128 |
| `--etw-max-buffers` | 29 allocated, 7 in use | 32 (4 MiB) |
| Minimum ETW buffers | 7 in use of ETW's minimum of 12 | ETW's minimum. The service has no option for it. |
| `--capacity` | `queue_depth_max` 21,622 | 24,576 |

- **Maximum buffers.** 32 is 3 buffers above the peak of 29. ETW allocates
  buffers only on demand, so a lower maximum does not save memory until ETW
  needs the buffers. It limits how far ETW can grow.
- **Minimum buffers.** ETW's minimum of 2 per logical processor was enough. At
  most 7 of 12 buffers were in use at the same time.
- **Channel capacity.** 24,576 is 14% above the peak of 21,622. The parser
  samples `queue_depth_max`, so the true peak can be slightly higher than the
  sampled value. The margin covers that difference. The service allocates the
  whole pool at startup, so the capacity sets most of the private bytes. The
  counted storage is 68.60 MiB at 24,576 and 654.04 MiB at 262,144.

These values match a steady 200,000 QPS. The lab did not measure 128 KiB
buffers at 30,000 to 50,000 QPS. In that range ETW delivered events in
batches in earlier runs. A 65,536-slot channel filled in one run at
49,400 QPS with 64 KiB buffers (see
[ETW flush removal validation](flush-removal-validation.md)).

## Not tested

- Other machines and vCPU counts.
- Recursive or mixed workloads.
- Runs longer than 50 s.
- A TLS receiver.
- The recommended `--etw-max-buffers 32` and `--capacity 24576` at 200,000 QPS.
  The runs used 256 and 262,144.
- 128 KiB buffers at 30,000 to 50,000 QPS.
