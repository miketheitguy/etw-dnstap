# Performance lab scripts

These scripts drive the etw-dnstap lab: Hyper-V guest `DNSTap-WS2025` (the forwarder under test), Ubuntu receiver `DNSTap-Ubuntu2604`, and a DNS client on the Hyper-V host. Evidence goes under `target/performance/`.

**These scripts change real VMs.** Some format a disk, remove DNS zones, replace the installed service binary, or detach virtual disks. Read the [destructive steps](#destructive-steps) first. Run them only on the dedicated lab host.

## Prerequisites

- Windows host with the **Hyper-V** role, in an elevated **Windows PowerShell 5.1** session. PowerShell 7 returns deserialized VM objects that break `Remove-VMHardDiskDrive` and the identity checks.
- The two VMs, with the names and GUIDs in [`lab.json`](lab.json). Every script verifies the VM GUID first. The Windows guest needs the DNS Server role, the `EtwDnstap` service, and WPR/logman. The Ubuntu guest needs `dnscollector`, `dnsmasq`, `nft`, `tcpdump` and `python3`.
- Windows Performance Toolkit (`xperf.exe`, `wpaexporter.exe`) on the host for the `export-*` scripts.
- `tools/dns-stress` built for `x86_64-pc-windows-msvc`.
- Python 3.11+ on the host (`pip install -r requirements.txt`). Guest Python uses the system `python3` and the standard library. `archive/ubuntu-service-stop-config.py` also needs `apt install python3-yaml`.

### Guest administrator credential (`administrator.clixml`)

PowerShell Direct needs a credential. Create it once, as the user that runs the scripts (DPAPI-protected, per user and machine):

```powershell
$path = 'target/lab-windows/private/administrator.clixml'
New-Item -ItemType Directory -Force (Split-Path $path) | Out-Null
Get-Credential Administrator | Export-Clixml -LiteralPath $path
```

### SSH to the Ubuntu VM

The Ubuntu controllers use `ssh -F target/lab-ubuntu/ssh_config dnstap-ubuntu`. Create that file with a `Host dnstap-ubuntu` entry: `HostName`, `User dnstaplab`, `IdentityFile`, `IdentitiesOnly yes`, and `UserKnownHostsFile` set to a lab-only file. Set `StrictHostKeyChecking yes` (or `accept-new` for the first connection) and pin the host key.

### Lab constants

[`lab.json`](lab.json) holds VM names and GUIDs, receiver `172.27.215.60:6000`, DNS server `172.27.215.163`, install paths, the `EtwDnstap-Lab` session name, zones, and the SSH alias. `LabCommon.psm1`, [`lab_config.py`](lab_config.py), and the shell scripts read it. Per-campaign values (campaign IDs, binary hashes, disk identity) are parameters.

## Run order

1. **Workload (host, once):** `Get-DnsWorkloadSource.ps1` downloads the pinned SecLists names and runs `New-DnsWorkload.ps1`.
2. **Inspect:** `windows-inventory.ps1`, optionally `windows-smoke-queries.ps1`.
3. **Ubuntu fixture (guest, root):** copy `lab.json`, `lab_config.py`, `monitor-ubuntu-workload.py`, `tune-upstream-buffer.py` and `rec-names.txt` next to the script, then `sudo ./setup-ubuntu-workload.sh`. It installs into the root-owned `/usr/local/lib/etw-dnstap-lab`. Undo with `sudo ./restore-ubuntu-workload.sh`.
4. **Optional data disk:** `ubuntu-performance-data-disk.ps1 -Action Attach -CampaignId <id> -WhatIf` (host), then `prepare-ubuntu-data-disk.py` (guest, dry run first). Later: `unmount-ubuntu-data-disk.py`, then `ubuntu-performance-data-disk.ps1 -Action Detach`.
5. **Parser-ramp campaign:** `windows-parser-campaign.ps1` (`Inventory`, `Backup`, `Deploy`, `Prepare`), `ubuntu-parser-ramp-upstream.py` and `ubuntu-parser-ramp-control.py`, one `windows-parser-stage.ps1` per stage, then `windows-parser-campaign.ps1 -Action Restore`/`Fetch`, `windows-final-proof.ps1`, and the verify, reconcile, and report scripts. Optional: `windows-startup-capacity.ps1 -WhatIf`, then without `-WhatIf`, to check that the service reaches Running at `--capacity 262144`.
6. **Per-run helpers:** `windows-run-client.ps1` (load), `windows-capture.ps1` (counters, WPR, clock).
7. **Analysis (host, offline):** see [Python](#python).

Archived harnesses live in [`archive/`](archive/). Their `Backup/Prepare/Deploy/Start` actions are disabled. Cleanup and evidence actions work.

## Destructive steps

| Script | Changes | Guard |
| --- | --- | --- |
| `prepare-ubuntu-data-disk.py` | `mkfs.ext4`, mount, collector drop-in | Dry run unless `--apply`. Explicit `--device`/`--mount`. Checks size, serial/WWN, label, mount state, root disk, partitions, host receipt. `--wipe-existing-signatures` to override `wipefs` |
| `unmount-ubuntu-data-disk.py` | `umount` of the data filesystem | Dry run unless `--apply`. Re-verifies UUID, serial/WWN, label, open descriptors |
| `remove-verified-guest-archives.py` | Deletes guest archive duplicates | Dry run unless `--apply`. Needs host hash/count proofs |
| `ubuntu-performance-data-disk.ps1` | Creates, attaches, or detaches a VHDX | `-WhatIf`/`-Confirm`. GUID, OS-disk, free-space checks |
| `Remove-LabOwnedZone` | `Remove-DnsServerZone -Force` | Only zones in the campaign's `owned-zones.txt` |
| `windows-*campaign*.ps1` Deploy/Restore | Replaces the service binary and SCM command line | Hash and ACL checks. Backup refuses to reuse a campaign ID |
| `windows-startup-capacity.ps1` | Changes the SCM command line (`--capacity`) and optionally the binary, starts and stops the service | `-WhatIf` runs the preflight only. Hash and SDDL checks. Restore in `finally` |
| `setup-ubuntu-workload.sh` | Edits collector config, installs units and an nft table | Root only. Refuses if a backup, drop-in, or log exists |

Run Python destructive scripts without `--apply` first and read the plan.

## Receiver STOP/FINISH check

At a requested stop the service sends a Frame Streams STOP and the receiver answers FINISH. The service exits 0 either way, so `windows-parser-stage.ps1` asks the receiver through `receiver_stop_finish.py` and the `*-LabReceiverStopCapture` functions in `LabCommon.psm1`.

- It runs `sudo -n tcpdump` over ssh on the receiver around the stop. The filter keeps TCP port 6000 packets with a 1 to 16 byte payload and FIN/RST packets. Output goes to `stages/<run>/receiver-stop/`. Print the command with `python -I scripts/performance/receiver_stop_finish.py --print-command`.
- `Assert-LabReceiverStopCaptureReady` needs `python` on the host, and `tcpdump` with passwordless sudo on the receiver.
- Verdict `receiver_stop_finish`: `confirmed` (STOP, then FINISH on the same connection), `not-seen` (fails the strict stage pass), or `not-checked` (no capture; `windows-startup-capacity.ps1` records it). It appears in `completion.json`, `stage-summary.json`, the ledger, and both reports.
- Offline: `python -I scripts/performance/receiver_stop_finish.py target/performance/<id>/stages/<run>`. Test: `python -I scripts/performance/test_receiver_stop_finish.py`.

## Campaign IDs

`-CampaignId` (1-64 letters, digits, `_`, `-`) is mandatory for the campaign scripts. `Backup` and `Prepare` refuse to start when the host evidence directory or guest campaign directory exists. Fetches verify each file's SHA-256.

## Script reference

### PowerShell (host)

| Script | Purpose | Inputs |
| --- | --- | --- |
| `LabCommon.psm1` | Shared module: config, sessions, `Invoke-LabGuest`, `Receive-LabFile`, `Initialize-LabDnsZones` | `lab.json` |
| `Get-DnsWorkloadSource.ps1` | Download names, run `New-DnsWorkload.ps1` | `-OutputDirectory` |
| `New-DnsWorkload.ps1` | Build name lists, zone file, provenance | `source-names.txt`, `source-commit.json` |
| `windows-inventory.ps1` | Read-only guest inventory | none |
| `windows-smoke-queries.ps1` | Resolve smoke names, check TTLs | `-ProofName`, `-EvidenceRoot` |
| `windows-final-proof.ps1` | Prove service and zones are back to normal | `-EvidenceRoot` |
| `windows-capture.ps1` | Counters, bounded WPR, clock bounds | `-Action`, `-RunId`, `-EvidenceRoot` |
| `windows-run-client.ps1` | Run and sample `dns-stress` | `-Mode`, `-RunId`, `-Qps`, `-Duration` |
| `windows-startup-capacity.ps1` | Startup check at maximum `--capacity` (no receiver or campaign ID needed) | `-Capacity`, `-CandidatePath`, `-ExpectedHash`, `-ProofName`, `-EvidenceRoot` |
| `windows-parser-campaign.ps1` | Backup/Deploy/Prepare/Start/Stop/Restore/Fetch | `-Action`, `-CampaignId`, `-ExpectedHash`, `-CandidatePath`, `-UpstreamAddress` |
| `windows-parser-stage.ps1` | One isolated parser stage | `-Mode`, `-CampaignId`, `-RunId`, `-Qps`, `-Duration`, `-ReceiverReadyReceipt`, `-ExpectedHash`, `-MatrixApproved` |
| `ubuntu-performance-data-disk.ps1` | Attach or detach the temporary VHDX | `-Action`, `-CampaignId` |
| `export-cpu-precise.ps1`, `export-pooled-evidence.ps1` | Run `xperf`/`wpaexporter` on a trace | `-TracePath`, window, output dir |

`windows-startup-capacity.ps1` appends `--capacity` to the service `PathName`, can replace the binary (`-CandidatePath`, `-ExpectedHash`), samples SCM status until `RUNNING`, then restores path, binary, hash and SDDL in `finally`. It writes `<EvidenceRoot>/<ProofName>.json`.

### Shell (guest, root)

- `setup-ubuntu-workload.sh`: install the dnsmasq fixture, nft counters, collector telemetry and monitor.
- `restore-ubuntu-workload.sh`: remove the fixture and restore the collector config.

### Python

Guest-side (root): `prepare-ubuntu-data-disk.py`, `unmount-ubuntu-data-disk.py`, `remove-verified-guest-archives.py`, `archive-ubuntu-run.py`, `ubuntu-storage-budget.py`, `tune-upstream-buffer.py`, `monitor-ubuntu-workload.py`, `receiver-evidence.py`, `summarize-ubuntu-workload.py`, `ubuntu-parser-ramp-control.py`, `ubuntu-parser-ramp-upstream.py`. Copy them with `lab.json`, `lab_config.py`, `guest_common.py`, `parser_ramp_receipts.py` to one root-owned directory. `.gitattributes` keeps `*.sh` as LF.

Host-side (each has `--help`):

- Per run: `analyze-host.py`, `analyze-windows.py`, `aggregate-cpu-precise.py`, `measure-ubuntu-clock.py`.
- Pooled runs: `summarize-pooled-broad.py`, `-native.py`, `-stacks.py`, `-stage.py`, `-trace-gates.py`, `check-pooled-summary-gates.py`, `audit-pooled-management.py`, `build-pooled-report.py`, `export-pooled-graph-data.py`, `plot-pooled-wpr.py`.
- Parser ramp: `build-parser-ramp-report.py`, `reconcile-parser-ramp.py`, `verify-parser-ramp-archive.py`.
- Receiver archive: `count-receiver-archive.py`, `verify-receiver-archive.py`.
- `pooled_intake_run.py` runs the Rust callback benchmark. It, `plot-pooled-wpr.py` and `export-pooled-graph-data.py` hash their own source into provenance records.
- Shared: `metrics_ledger.py`, `perf_common.py`, `parser_ramp_receipts.py`, `lab_config.py`, `guest_common.py`.
- `plot-tier-runs.py` draws a PNG per lab run and per QPS tier from folders holding `run.json` and `timeseries.csv`. Setup and run:

```powershell
python -m venv target/plot-venv
target/plot-venv/Scripts/pip install -r scripts/performance/requirements.txt
target/plot-venv/Scripts/python scripts/performance/plot-tier-runs.py --runs <runs-root> --out docs/assets/performance [--tier 10000]
```

Loss percent is lost events over events offered (received plus `etw_lost`). A run with `"tier_summary": false` in `run.json` stays out of the tier PNG. Test: `python -I scripts/performance/test_plot_tier_runs.py` in the same venv.

## Tests

```powershell
# Prints one line per syntax error
Get-ChildItem scripts/performance -Recurse -Include *.ps1,*.psm1 | ForEach-Object {
    $errors = $null
    [void][Management.Automation.Language.Parser]::ParseFile($_.FullName, [ref]$null, [ref]$errors)
    $errors | ForEach-Object { "$($_.Extent.File):$($_.Extent.StartLineNumber): $($_.Message)" }
}
```

```bash
bash -n scripts/performance/setup-ubuntu-workload.sh scripts/performance/restore-ubuntu-workload.sh
python -I -m py_compile scripts/performance/*.py scripts/performance/archive/*.py
python -I scripts/performance/test_metrics_ledger.py
python -I scripts/performance/test_pooled_intake_run.py
python -I scripts/performance/archive/test_archived_analyzers.py   # also run by CI
```

## File moves and renames

| Old | New |
| --- | --- |
| `metrics-ledger-test.py` | `test_metrics_ledger.py` |
| `pooled-intake-run-test.py` | `test_pooled_intake_run.py` |
| `pooled-intake-run.py` | `pooled_intake_run.py` |
| `windows-1000qps-campaign.ps1` | `archive/windows-1000qps-campaign.ps1` |
| `windows-pooled-campaign.ps1` | `archive/windows-service-campaign.ps1 -Variant Pooled` |
| `windows-ring-fix-campaign.ps1` | `archive/windows-service-campaign.ps1 -Variant RingFix` |
| `windows-pooled-stage.ps1`, `windows-pooled-wpr.ps1`, `windows-control.ps1` | same names under `archive/` |
| `windows-pooled-diagnostics.ps1`, `-final-proof`, `-inventory`, `-process-check`, `-size-watch` | deleted |
| `ubuntu-service-stop-config.py`, `ubuntu-service-stop-pcap.py`, `count-parser-ramp-diagnostic.py`, `analyze-ring-fix-cpu.py`, `analyze-metrics-phases.py`, `analyze-send-window.py`, `summarize-receiver-results.py` | same names under `archive/` |

## Known limits

- The Ubuntu controllers default `--campaign-id` to their dated campaigns and embed its filesystem UUIDs. Pass the campaign explicitly.
- `-ExpectedHash` is mandatory. `windows-service-campaign.ps1` Fetch keeps a differing host file.
