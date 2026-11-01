# Service shutdown validation plan

> **Status:** Historical · **Evidence dated:** not recorded · **Commit:** not recorded · **Reviewed:** 2026-11-01

This plan gives the Windows and Ubuntu operators a bounded comparison of
DNS-collector connection-close policies using the reviewed shutdown diagnostics.
The user authorized deployment and this comparison after reviewing the
[implementation plan](service-stop-fix-plan.md). The broad stress ramp remains
stopped.

## Fixed scope

Use campaign `service-stop-20261030-0633` and keep evidence under
`target/performance/service-stop-20261030-0633/`. Preserve earlier campaigns.
Deploy the same verified production binary for both arms. Retain the ETW
filter, queue and pool capacities, two-CPU/four-GiB VMs, 5,000 authoritative A
names, two client threads, 256 outstanding requests per thread, and
2,000-millisecond timeout from the failed filtered authoritative stage.

Run only these two arms, in order:

| Run ID | Requested QPS | Duration | Receiver policy |
|---|---|---|---|
| `stop-reset-auth-80000-60s-2` | 80,000 | 60 seconds | Existing inherited `reset-conn: true` |
| `stop-graceful-auth-80000-60s-1` | 80,000 | 60 seconds | Explicit `reset-conn: false` |

There are no recursive stages, upstream fixtures, five-minute confirmations,
or further rate increases in this comparison. A baseline STOP/FINISH reset is
an expected comparison outcome; preserve it and continue to the graceful arm
after evidence is stable. Pause on a different failure, invalid capture
boundary, or resource-guard breach.

## Delegation and release gate

The operators have these responsibilities:

| Owner | Model and effort | Deliverable |
|---|---|---|
| Release agent | Luna, low | Locked release, source and artifact hashes, preserved previous artifact, PE hardening receipt |
| Windows operator | Sol, medium | Protected deployment baseline, both client runs, SCM and Event Log evidence, normal-service restoration |
| Receiver operator | Luna, medium | Configuration backup and policy hashes, bounded packet captures, directional record ledgers, receiver restoration |
| Packet analyst | Sol, medium | Sequence-aware control-frame evidence, capture integrity checks, and explicit framing limits |
| Results and helper corrections | Sol, low | Evidence-backed results and focused offline recovery fixtures |
| Independent reviewer | Sol, high | Review of operational helpers, corrections, and results |
| Coordinator | Root | Verify build gate, inspect capture contracts, interpret evidence and review final changes |

Only the verified executable hash authorizes deployment. Both arms must bind
to that hash. The receiver must be ready with the expected configuration hash
and matching run marker before each load starts. Configuration guards must
accept the explicitly selected arm hash; never substitute a baseline hash in
an evidence marker for a changed configuration.

## Evidence and limits

Capture the TCP exchange between the Windows guest and collector port 6000
using a bounded packet-capture file set. Retain the end of the stream through
service stop, record capture drop statistics, and inspect STOP, FINISH, FIN,
and RST ordering using stream sequence numbers. A capture that omits relevant
packets cannot prove their absence.

Reconcile client sends and matched responses with collector CLIENT_QUERY and
CLIENT_RESPONSE counts separately. Resolver records remain excluded by the
service filter. Preserve the raw stream, marker boundaries, archive hashes,
achieved rate, and service stop result. Equal counts do not prove one-to-one
message identity or explain where a deficit originated.

Keep the prior raw evidence intact on the existing data VHDX. Check available
space before each arm and retain existing disk and memory guards. After a
writer is closed, an archive and host roundtrip are verified, and counts are
recorded, only this campaign's guest raw file may be removed to recover space.
Do not run heavy archival or counting work during load.

## Final state

Leave the reviewed binary running with normal service arguments. Retain
`reset-conn: false` if the collector starts correctly and validation reveals
no configuration regression; otherwise restore the protected configuration
and report why. Restore normal collector output, stop temporary capture
helpers, and detach the existing data disk intact after closing its writers.
Verify both VM identities, service states, configuration hashes, and absence
of owned load processes. Do not claim the intermittent defect is eliminated
from a single successful comparison.

## Completion record

Deployment, both comparison arms, and restoration are complete. Both service
stops succeeded. Packet captures show reset-style receiver close in the
baseline and a FIN exchange without observed resets with `reset-conn: false`.
Both arms have directional collector-record deficits; neither is a clean
capture result. The earlier intermittent failure was not reproduced.
See [validation results](service-stop-validation-results.md) for counts,
packet evidence, review, and cleanup receipts.

The original baseline setup, `stop-reset-auth-80000-60s-1`, declared
`all_supported` instead of the required `client_only` capture policy. The
pre-load guard rejected it before starting the service or stress client.
Its receipts and empty captures are preserved as `setup_rejected_no_load`;
the replacement uses a fresh run ID. The matrix still contains two actual
load runs.
