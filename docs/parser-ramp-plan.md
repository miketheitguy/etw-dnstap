# Parser optimization DNS ramp plan

This plan is for the deployment operator and evidence reviewer. It defines a bounded DNS load ramp to compare client sends and responses with collector observations after deploying the parser and encoder optimization. It does not plan service durable metrics, a Windows Performance Recorder (WPR) or CPU campaign, production Rust changes, historical guard changes, or a commit or push.

**Aborted by user; do not resume this campaign.** The user stopped testing
after the authoritative 48k short stage. Its client sent and completed
2,818,519 queries in 60 seconds (46,975.32 achieved QPS), with a clean SCM
stop. The planned higher stages and 300-second high-rate confirmations were
canceled. Existing evidence remains preserved. Cleanup restored the receiver
and Windows DNS settings and retained candidate `487FFD…A1AE` Running; see
`target/performance/parser-ramp-20261030/aborted-by-user.json`,
`cleanup/normal-receiver-ready.json`, and
`windows/cleanup-Restore-20261030T032912664Z.json` under that campaign root.
The procedures below record the canceled plan. The separately authorized
[filtered ramp plan](filtered-ramp-plan.md) starts a different campaign.

## Roles and build artifact

The current assignment is:

| Work | Model and effort |
|---|---|
| Build and this plan | Luna, low |
| Windows deployment and ramp orchestration | Sol, medium |
| Receiver work | Luna, medium |
| Count accounting review | Luna, medium |
| Review and synthesis | Root |

The production build succeeded with `cargo build --locked --release`, using the repository's pinned nightly toolchain and build configuration without environment overrides. The deployable artifact is `target/x86_64-pc-windows-msvc/release/etw-dnstap.exe` (1,065,472 bytes), SHA-256 `487FFD782B730B7E1A849626AF68BCB6C8A0EF2057ACB7D8E5D932B7EA49A1AE`. Its three optimized source hashes match the optimization validation receipt, and its hash matches the prior validated binary. Build evidence is `target/performance/parser-ramp-20261030/build/build-20261030T023020Z.json`.

## Before deployment

1. Reattach the existing detached, verified Ubuntu virtual disk (64 GiB VHDX) for the receiver role. Use the retained Windows guest (2 CPUs, 4 GiB) for the candidate service. Keep the collector YAML unchanged.
2. Run the campaign inventory and protected backup actions. The Windows guest baseline executable is `C:/ProgramData/EtwDnstap/performance/parser-ramp-20261030/baseline.exe`; the campaign also captures registry, policy, and DNS snapshots. Verify the backup before deployment.
3. Deploy the reviewed candidate with the existing campaign script. Preserve the service account, ACLs, and configuration. Confirm the candidate path and hash before service start. Do not edit production Rust code or alter historical harness guards for this campaign.
4. On the Ubuntu receiver, configure controlled recursive upstream activity and narrowly scoped, owned nftables counters using the established harness. Limit collector-output changes to temporary stdout and stderr redirection. Cleanup removes temporary forwarding and counter state, restores output redirection, and retains the reviewed candidate in its required running state.

The Windows operator uses `scripts/performance/windows-parser-campaign.ps1` for `Inventory`, `Backup`, and `Deploy`. The deploy action receives `-CandidatePath target/x86_64-pc-windows-msvc/release/etw-dnstap.exe`; the script checks the candidate hash before copying it. Each action writes a timestamped JSON receipt under `target/performance/parser-ramp-20261030/windows/`. The protected guest baseline and snapshots are stored under the guest path above. The `Prepare` action creates the owned `rec.stress.test` conditional forwarder and requires an explicit upstream IPv4 address; obtain that address from the receiver operator before running it. Do not start load until the receiver reports ready and deployment verification is complete. The restore action verifies the saved DNS settings and service policy before leaving the reviewed candidate running.

## Run the ramp

Use 5,000 A names, two workers, 256 outstanding requests per worker (512 total), and a single 2,000-millisecond request timeout for each stage. Rotate the A-name queries as in the established client workload. Keep the collector YAML fixed throughout.

First run the two 300-second baselines:

| Query type | Rate | Duration |
|---|---:|---:|
| Authoritative | 1,000 queries per second (QPS) | 300 seconds |
| Recursive | 1,000 QPS | 300 seconds |

Then run each stage for 60 seconds. Stop that mode's higher-rate stages at its first verified client-type delivery deficit in the client or collector counts:

| Query type | Rates (QPS) |
|---|---|
| Authoritative | 8k, 16k, 24k, 32k, 40k, 48k, 56k, 64k, 80k, 96k, 112k, 128k |
| Recursive | 8k, 16k, 24k, 28k, 32k, 40k, 48k, 56k, 64k, 80k, 96k, 112k, 128k |

If a stage has an actual client-type delivery deficit, stop higher-rate stages for that mode. Run the last clean rate for a 300-second confirmation and report both requested and achieved rates. A clean delivery confirmation means the client and collector checks pass; qualify the load separately and do not imply that requested QPS was achieved when the measured rate is lower. If that confirmation fails, allow at most one fallback run at the previous clean rate, again recording requested and achieved rates. An achieved/requested ratio below 98% by itself is an offered-rate shortfall, not a delivery deficit; continue the planned higher requested rates up to the 128k cap while achieved QPS still increases. Stop the offered-load search if achieved QPS increases by less than 2% at each of two successive higher requested rates, and classify this as an offered-load limit with no loss boundary discovered. Do not change the thread or in-flight settings. Treat 128k QPS as the bounded limit of this experiment; it does not establish a universal service ceiling.

Run each stage with `scripts/performance/windows-parser-stage.ps1` as an isolated lifecycle: confirm `READY`, start the service through Service Control Manager (SCM), run the client, wait three seconds after client completion, stop the service through SCM, then let the receiver establish its final boundary and reconcile the stream. Preserve each lifecycle receipt. The Windows stage runner requires a `READY` receipt with `run`, `mode`, `configured_global_qps`, `duration_seconds`, `receiver_ready`, `start_marker_ready`, and `ready_utc` fields; it rejects a stage without `-MatrixApproved`. Pass the mode, run ID, rate, duration, receipt path, and matrix approval as the runner's required parameters.

The client sends DNS queries to the Windows DNS guest, not the separate Ubuntu collector receiver. It uses `--server DNS_SERVER_IP:53`, the appropriate authoritative or recursive name list, `--mode`, `--threads 2`, `--duration`, `--inflight 256`, `--timeout-ms 2000`, `--qps`, and `--output-dir`. Replace `DNS_SERVER_IP` with the Windows DNS guest address recorded by the stage preflight. The 256 in-flight setting is per worker, for 512 total outstanding requests. The stage runner records `preflight.json`, `invocation.json`, `guards.jsonl`, `client-process.jsonl`, `client-exit.json`, `service-stop.json`, and `completion.json`; client output includes `summary.json`, `metrics.jsonl`, stdout, and stderr. These are campaign-script outputs, not service durable metrics.

The Ubuntu control program is `scripts/performance/ubuntu-parser-ramp-control.py`. Its `prepare` command writes `ready.json` and `start.json`; the Windows stage runner requires the `ready.json` receipt. Its `monitor` command writes `resources.jsonl`. After the Windows sender has stopped, its `finish` command writes `end.json`; `archive` writes `receiver-raw.jsonl.gz` and `archive-manifest.json`. The stage evidence also includes the reviewed `reconciliation-v2.json` ledger. Preserve these receipts in the matching receiver stage directory and transfer verified evidence to the campaign stage directory.

## Compare client and collector counts

For every stage, record the client send and response totals, errors, achieved rate, start and stop times, service lifecycle results, and collector counts. Separate collector `CLIENT_QUERY` from client sends and collector `CLIENT_RESPONSE` from client responses received. Report resolver-generated query and response counts in their own fields, with actual upstream activity identified separately. Do not infer a fixed number of resolver records per client query.

Calculate and label each directional difference explicitly:

- Client sends versus collector `CLIENT_QUERY` records.
- Client responses received versus collector `CLIENT_RESPONSE` records.
- Resolver-generated records, reported separately from client-type records.

An aggregate difference cannot identify whether loss occurred in ring-buffer capture, Event Tracing for Windows (ETW), or receiver processing. Equal totals do not prove that each sent query uniquely matches a captured query. Preserve raw stage data so reviewers can inspect stage boundaries and record types.

If achieved load falls below 98% of the requested rate, record an offered-rate shortfall and investigate generator and target limits; the ratio alone does not identify which one constrained the rate. Continue higher requested rates unless an actual delivery deficit occurs, the two-step achieved-rate plateau occurs, or the 128k cap is reached. Assess client-type delivery deficits separately because an offered-rate shortfall can coexist with collector loss. Distinguish a DNS flag-validation problem from an actual delivery deficit by checking the validation result and matching client-type count before declaring loss. Apply the stop-on-first-deficit rule only after that verification.

The approved authoritative baseline ledger is `target/performance/parser-ramp-20261030/stages/parser-auth-1000-300s-1/receiver/reconciliation-v2.json`. At a requested 1,000 QPS over 300 seconds, it records 299,906 client sends, 299,906 completed responses, 299,906 `CLIENT_QUERY` records, and 299,906 `CLIENT_RESPONSE` records, with clean client DNS-path and receiver reconciliation results. Use this ledger as the baseline evidence; do not present it as a final campaign report.

## Evidence and review gates

Store evidence under `target/performance/parser-ramp-20261030/`. Raw receipt filenames, exact status strings, and output schemas are pending confirmation from the deployment and receiver operators; do not invent them. Preserve raw evidence and write derived summaries separately.

The final evidence package should include:

- Build provenance, artifact hash, and protected backup and deployment verification.
- Per-stage client output and raw collector stream or count receipts, with lifecycle timestamps and status.
- Separate client-type and resolver counts, requested and achieved rates, directional differences, and stop reason.
- A summary table of both baselines, every completed ramp stage, the first verified deficit or offered-rate shortfall, and any confirmation or fallback stage.
- Cleanup results showing the temporary recursive forwarder was removed, collector configuration restored, and reviewed candidate service retained in the running state.
- A reviewer synthesis that states supported conclusions and measurement limits without attributing aggregate deficits to an unobserved subsystem.

Do not begin a ramp stage until deployment hash verification and the prior isolated lifecycle are complete. Stop the affected mode on a verified delivery deficit, stop offered-load search on the two-step plateau, and stop all activity on a failed service lifecycle or unstable collector state. Do not claim a passing stage above the last clean 300-second confirmation unless the required confirmation and any permitted fallback also pass; state requested and achieved rates for both.

## Verify and clean up

Before closing the campaign, verify that every started stage has a matching client receipt, collector receipt, and stop/settle result. Verify the evidence files are readable and that their artifact hash, stage rate, query type, and timestamps agree. Restore the collector configuration, remove temporary forwarding state, and confirm the reviewed candidate remains running. Record cleanup failures as blockers; do not treat missing evidence as a pass.
