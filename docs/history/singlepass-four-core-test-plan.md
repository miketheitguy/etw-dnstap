# Test the single-pass decoder with four CPUs

> **Status:** Historical · **Evidence dated:** October 30, 2026 UTC · **Commit:** not recorded (predates `e59425f`) · **Reviewed:** 2026-11-01

This plan is for the lab operators running one authoritative DNS test with
100,000 requested queries per second (QPS). It defines the deployment, readiness
gates, measurement window, receipt reconciliation, and final VM state. It does
not authorize additional load runs or attribute a count deficit to a component.

## Assign the work

| Owner | Model and effort | Responsibility |
| --- | --- | --- |
| Windows operator | Sol, medium | Windows VM baseline, graceful resize, artifact deployment, workload, CPU sampling, and final service verification |
| Linux operator | Sol, medium | Collector VM baseline, graceful resize, capture preparation, CPU sampling, delayed ledger validation, and collector restoration |
| Plan and analysis reviewer | Sol, low | Staged plan, independent directional count reconciliation, and CPU calculations |
| Coordinator | Root | Verify evidence, release readiness gate, coordinate the single run, and report results |

Keep campaign evidence under `target/performance/singlepass-4cpu-100000/`, with
Windows and Linux receipts in `windows/` and `linux/`. Preserve preexisting
artifacts, captures, and unrelated workspace changes.

## Prepare both VMs

1. Identify the existing Windows and Ubuntu lab VMs from live Hyper-V identity
   records. Record their IDs, names, state, virtual processor count, memory
   configuration, guest addresses, service state, and deployed artifact hashes.
2. Gracefully shut down both guests. Verify each VM is off before changing its
   processor allocation to four. Retain each VM's existing memory configuration.
3. Restart both VMs. Verify that Hyper-V and each guest report four logical CPUs,
   unchanged memory, the expected identity, and usable management connectivity.
4. Deploy `target/single-pass-decoder/etw-dnstap.exe` to the Windows service's
   established executable path while the service is stopped. Verify SHA-256
   `EB70865D5D36A6E19868E87F2F7A4CAEE2D260B588DD39983097EA51E52803ED`
   on the source and deployed executable. Preserve service security settings.
5. Prepare an isolated collector capture with a unique campaign/run identity.
   Preserve the earlier capture and normal collector configuration. Retain the
   established source filter, console JSON format, and `reset-conn: false`.
6. Check guest storage against the prior multi-gigabyte capture and retain
   enough free space for the complete run, drain, and evidence. Record resource
   guard limits and permit only lightweight health/storage checks during load.
7. Verify both service configurations, the collector connection, workload binary,
   5,000-name authoritative fixture, and guest clocks. Record UTC clock brackets
   before load and after load for both guests relative to the workload clock.

## Gate and execute one load window

1. Prepare coordinated one-second total guest CPU collection on both VMs.
   Retain all four individual CPU instances and a total value, raw timestamps,
   counter validity, and sampling intervals. Capture idle samples before load
   and retain sampling through the end of the sending interval.
2. Return readiness receipts from both operators. The coordinator verifies the
   four-CPU topology, unchanged memory, artifact hash, capture boundary,
   configuration, clock mapping, storage, sampler activity, and service health.
   Start load only after both readiness gates pass.
3. Run the workload on the Windows host, as in the preceding campaign, under
   the Windows operator's ownership. Run exactly one 60-second authoritative
   workload with a global requested
   ceiling of 100,000 QPS. Reuse the preceding workload: 5,000 names, two client
   threads, 256 outstanding requests per thread, and a 2,000 ms timeout. Retain
   the client summary and actual sending-window timestamps. Do not ramp, retry,
   or add a validation workload.
4. Wait for the entire 60-second sending interval and the bounded client drain.
   Do not run post-capture counting, parsing, hashing, compression, or transfer
   during that interval. The planned collector console stream is compressed
   inline to fit the available storage. Record the actual sent count and
   achieved QPS; 6,000,000 is a
   nominal target rather than a guaranteed count.
5. After the sending interval, stop the Windows service gracefully and allow a
   bounded collector drain. Record the drain duration and completion evidence.
   Close the collector and compression writers and create an end marker with the immutable byte
   boundary, file identity, size, timestamps, and SHA-256. If drain or closure
   fails, preserve the evidence and report validation as incomplete.
6. Verify the closed gzip stream's CRC and decompression success. Compute and
   retain compressed artifact SHA-256 and size plus decompressed SHA-256 and
   byte count. Adapt the streaming ledger to the decompressed stream without
   expanding a multi-gigabyte raw file onto the guest's limited storage.
   Stream-validate only the closed capture. Bind the ledger to matching start
   and end markers and the run identity; verify unchanged size and modification
   time, full-line termination, byte count, and hash. Scope directional counts
   to the isolated run and `auth.stress.test` zone. Report malformed JSON/DNS,
   diagnostic lines, unknown operations, unrelated records, resolver records,
   and observed duplicate indicators separately.

## Analyze counts and CPU

Compare client sent queries with collector `CLIENT_QUERY` and client successful
responses with collector `CLIENT_RESPONSE` separately. Also report matched
responses, timeouts, errors, anomalies, and outstanding requests. Compute each
directional deficit and percentage from its own client denominator. Equal
aggregate counts do not prove message identity. A deficit does not identify
ETW loss, queue drops, network loss, or collector processing loss.

For each VM, align CPU samples to the complete client sending interval using
the recorded UTC clock brackets. Exclude uninitialized samples and reject or
report invalid counters. A sampled interval contributes in proportion to its
overlap with the 60-second window. Report overlap coverage, gaps, largest sample
interval, clock uncertainty, and sensitivity to the clock bounds. If clock
brackets do not intersect, report that limitation instead of inventing a valid
constant-offset intersection.

Report mean and maximum sampled utilization for each of the four guest CPUs
and the VM overall. Normalize the overall value across exactly four logical
CPUs; exclude Windows processor-group subtotals from that denominator. Verify
Linux CPU accounting semantics before comparing it with Windows. Label idle,
iowait, and steal treatment explicitly. The maximum is an interval average,
not an instantaneous spike. Total guest CPU includes background services and
measurement overhead; it is neither host physical CPU nor service-only CPU.
Ubuntu also performs inline capture compression, so its CPU includes that
work. The prior campaign used uncompressed console capture and two CPUs per VM;
cross-campaign comparisons therefore change more than the decoder alone.

## Restore and verify

1. Restore the collector's normal output configuration and verify the expected
   configuration hash, active process, and receiver connection.
2. Restart and verify the Windows service with the deployed single-pass build
   and required hash. Stop owned samplers and confirm no workload remains.
3. Leave both VMs running with four CPUs and their original memory settings.
   Record final VM topology, service state, executable hash, and collector state.
4. Independently reproduce directional residuals and both VM CPU summaries from
   retained evidence. Publish results with limitations and artifact links.

## Reuse prior evidence and scripts

Use the preceding campaign at
`target/performance/direct-packet-100000-20261030-070305/` as the workload and
evidence-format reference. Its `results.md`, `execution-plan.md`, client summary,
Windows stage/sampling scripts, collector-control script, and streaming ledger
provide reusable patterns. Copy and adapt campaign paths and run IDs rather
than modifying the previous campaign.

The preceding `cpu-analysis.py` hard-codes two Windows CPUs and its old campaign
path. Adapt it to four verified instances and add independent Linux analysis.
The preceding ledger compares completed responses; this campaign must also
compare successful responses explicitly. Its fixed zone and permissive fallback
when `client.run_id` is absent require review before reuse: preserve a separate
explicit run manifest that binds the client summary and capture markers.
The Ubuntu guest has approximately 8.8 GB free and retains approximately 14 GB
of historical raw captures. Preserve that evidence. Verify compressed capture
headroom and the storage guard before starting; do not rely on a particular
compression ratio or uncompressed final size.

## Completion

The single authorized run is complete. See the [test results](singlepass-four-core-test-results.md) for independent count reconciliation, both VM CPU tables, capture integrity, limitations, and final-state evidence.
