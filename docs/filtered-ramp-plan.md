# Filtered DNS ramp plan

This plan helps the Windows deployment operator and evidence reviewer deploy
the reviewed client-event filter and run a bounded DNS ramp. The campaign
compares client sends and matched responses with collector client-type counts.
It stores evidence under `target/performance/filtered-ramp-20261030/` and keeps
the aborted parser-ramp campaign unchanged.

## Capture contract

The reviewed `src/etw.rs` SHA-256 is
`2D601DF9EA49679FFCA021757E5733B849C49A3347513AF0AF45C09D8BEFF3FE`.
`EnableTraceEx2` requests level 4 and match-any keyword
`0x8000000000000000`; the callback admits only event IDs 256 and 257.
The service therefore emits `CLIENT_QUERY` and `CLIENT_RESPONSE`. Resolver
records are excluded even when the DNS workload uses recursion. Controlled
upstream packet counters describe actual recursion; they are not expected
collector record counts for this artifact.

The service keeps its default queue capacity 8,192 and jumbo capacity 64.
Run it through Service Control Manager (SCM) with its normal TCP receiver
arguments. Service JSONL metrics and Windows Performance Recorder (WPR) are
outside this counts-only campaign.

## Owners and release gate

The operators have these responsibilities:

| Work | Owner | Model and effort |
|---|---|---|
| Release build, commit, and push | Release agent | Luna, low |
| Windows scripts, deployment, and serial load | Windows operator | Sol, medium |
| Receiver storage, boundaries, and upstream | Receiver operator | Luna, medium |
| Release gate and interpretation | Coordinator | Root |

Complete these steps in order:

1. Validate the reviewed source and build the locked production release.
2. Commit and push the reviewed changes. Record the commit, pushed remote,
   build receipt, and executable SHA-256 in the campaign evidence.
3. Obtain the coordinator's release gate confirming that push succeeded.
4. Preserve a protected Windows baseline before deployment.
5. Deploy the verified executable and verify its installed hash and ACL.
6. Obtain a fresh matching receiver READY receipt before each load stage.

Read-only preflight is permitted before the release gate. Deployment and load
wait for the gate. The executable SHA-256 comes from the release build receipt;
the source-file hash is not an executable hash.

The locked release candidate is 1,065,472 bytes with executable SHA-256
`6065E8DFD185D3793BA34F8DD994F62F15992FD9066FD49BF85EDE700419C27F`.
Build provenance is `target/performance/filtered-ramp-20261030/build/receipt.json`.

## Preserve and deploy the Windows baseline

Use Windows PowerShell 5.1 and PowerShell Direct to the retained
`DNSTap-WS2025` VM, GUID `24bafedb-0998-4430-a0c6-7e90350cd9ac`, with two
virtual CPUs and fixed 4 GiB memory. Live preflight identifies the guest at
`172.27.215.163` and the collector destination at `172.27.215.60:6000`.
Verify these addresses before acting.

The baseline service is Running as LocalSystem with unrestricted service SID
and only `SeChangeNotifyPrivilege`. Its installed executable hash is
`487FFD782B730B7E1A849626AF68BCB6C8A0EF2057ACB7D8E5D932B7EA49A1AE`.
The normal service arguments are:

```text
"C:\Program Files\EtwDnstap\etw-dnstap.exe" --receiver 172.27.215.60:6000 --session-name EtwDnstap-Lab
```

Use `scripts/performance/windows-parser-campaign.ps1` with
`-CampaignId filtered-ramp-20261030`. Its `Inventory`, `Backup`, `Deploy`,
`Prepare`, `Stop`, `Status`, `Restore`, and `Fetch` actions keep receipts in
the campaign's `windows/` directory. Pass the release executable hash through
`-ExpectedHash` for deployment, stage execution, and restoration.

The `Backup` action refuses an existing guest campaign directory. It protects
the baseline executable, service registry settings, original arguments,
executable ACL, SCM ACL, SID type, privileges, and DNS recursion, cache, and
forwarder settings under
`C:/ProgramData/EtwDnstap/performance/filtered-ramp-20261030/`.
The `Deploy` action checks host and guest hashes, stops the service, and
preserves the executable ACL. It leaves the candidate stopped for the first
receiver boundary.

For recursive traffic, wait for the receiver operator's verified controlled
upstream receipt. Then run `Prepare` with the explicit upstream IPv4 address
to create only the campaign-owned `rec.stress.test` conditional forwarder.
Do not run DNS inventory or management during load.

## Run the serial matrix

Use the unchanged Rust stress client with 5,000 rotating A names, two workers,
256 outstanding requests per worker, and a 2,000-millisecond response timeout.
The client sends to the Windows DNS guest on port 53. Keep the thread count,
outstanding limit, artifact, and VM resources fixed throughout the campaign.

Run each mode at these requested rates, starting at 48,000 QPS:

| Mode | Requested QPS | Sending duration |
|---|---|---|
| Authoritative | 48k, 56k, 64k, 80k, 96k, 112k, 128k | 60 seconds each |
| Recursive | 48k, 56k, 64k, 80k, 96k, 112k, 128k | 60 seconds each |

Run the authoritative sequence and its confirmation before the recursive
sequence. Stop a mode at its first verified client delivery deficit or
collector `CLIENT_QUERY`/`CLIENT_RESPONSE` deficit. If the initial 48k stage
has a deficit, report the observed boundary as at or below 48k; do not add a
lower-rate test. Also stop at 128k or after two successive higher requested
rates each produce less than a 2% achieved-rate increase. Report that latter
case as an offered-load limit with no observed loss boundary.

An achieved rate below 98% of the requested rate qualifies the stage but does
not by itself stop escalation. Preserve protocol flag failures, including
RA=false, separately from missing DNS responses or missing collector records.

Here, a clean rate means client delivery and collector directional counts
align; protocol flags and qualified shutdown failures remain separate.
If a clean stage exists, confirm the highest achieved clean rate for 300
seconds, subject to receiver storage guards. If that confirmation fails,
permit one fallback at the previous clean requested rate of at least 48k.
Keep confirmation lifecycle results separate from delivery results.

Use `scripts/performance/windows-parser-stage.ps1` with explicit
`-CampaignId`, `-ExpectedHash`, `-Mode`, `-RunId`, `-Qps`, `-Duration`,
`-ReceiverReadyReceipt`, `-CapturePolicy client_only`, and `-MatrixApproved`.
Use unique run IDs such as `parser-filtered-auth-48000-60s-1`; never overwrite
an existing Windows stage.

## Bound each stage and reconcile records

Follow this lifecycle for every stage:

1. Confirm that the Windows service is stopped and the previous receiver
   archive and ledger are complete.
2. Obtain the matching receiver READY receipt and start marker.
3. Start the candidate through SCM and run the selected client workload.
4. Allow the client response drain, wait three seconds, and stop through SCM.
5. Freeze the receiver only after final publication settles and the collector
   stops cleanly.
6. Verify the immutable raw boundary, compressed archive, and roundtrip hash.
7. Compare client sends with `CLIENT_QUERY` and matched client responses with
   `CLIENT_RESPONSE`, independently.

The Windows runner requires matching run, mode, QPS, duration, receiver-ready,
start-marker-ready, and UTC freshness fields. It preserves invocation hashes,
client output, resource guards, SCM stop state, and failure events. Host free
space must be at least 20 GiB; guest free space must be at least 8 GiB and
available memory must be at least 1 GiB before and during load. Cleanup actions remain
available when disk guards fail.

Use the exclusive fixture zone and isolated stage to separate workload
records from unrelated traffic. Sampled UDP endpoints are incomplete after
client socket rotation; do not claim per-request tuple matching from those
samples. Equal counts do not establish one-to-one identity. A deficit
establishes end-to-end observability loss and cannot identify ring admission,
ETW, TCP, or collector processing without independent counters.

Preserve a postload STOP/FINISH error 10054 as a strict lifecycle failure.
The coordinator permits qualified continuation for that same error only after
exact stable record counts, verified archives, and clean receiver shutdown.
Pause for any different error, in-load failure, unresolved backlog, invalid
boundary, or record deficit. Never change deadlines to make shutdown pass.

## Restore and close the campaign

After the last authorized stage, stop owned clients and stage helpers. Restore
the receiver's normal output and remove only owned upstream and firewall state.
Wait for explicit normal receiver readiness before running Windows `Restore`.
That action removes the owned conditional forwarder, verifies baseline DNS
settings and service policy, retains the reviewed executable, and starts it
with the original arguments when the baseline was Running.

Confirm through readbacks that both VMs remain Running, the normal service
hash and arguments match the retained candidate, and temporary state is gone.
Preserve every started stage and any failure or cancellation. Report requested
and achieved rates, directional record differences, protocol flags, lifecycle
outcomes, the stop reason, and cleanup blockers independently.
