# Performance lab scripts

Lab: Hyper-V guest `DNSTap-WS2025` (forwarder under test), Ubuntu receiver `DNSTap-Ubuntu2604`, DNS client on the Hyper-V host. Evidence: `target/performance/`.

**These scripts change real VMs** (format disks, remove DNS zones, replace the service binary, detach VHDXs). Run them only on the dedicated lab host. See [Destructive steps](#destructive-steps).

### Quick Start

Elevated Windows PowerShell 5.1 on the host:

```powershell
.\scripts\performance\Get-DnsWorkloadSource.ps1 -OutputDirectory <dir>
.\scripts\performance\windows-inventory.ps1
```

### Steps / Configuration

#### Prerequisites

- **Hyper-V** host, elevated **Windows PowerShell 5.1**. PowerShell 7 returns deserialized VM objects that break `Remove-VMHardDiskDrive` and the identity checks.
- Two VMs with the names and GUIDs in [`lab.json`](lab.json); every script checks the GUID. Windows guest: DNS Server role, `EtwDnstap`, WPR/logman. Ubuntu guest: `dnscollector`, `dnsmasq`, `nft`, `tcpdump`, `python3`.
- `xperf.exe`, `wpaexporter.exe` for `export-*`.
- `tools/dns-stress` built for `x86_64-pc-windows-msvc`.
- Host Python 3.11+ (`pip install -r requirements.txt`). Guest: system `python3`. `archive/ubuntu-service-stop-config.py` needs `apt install python3-yaml`.
- **`administrator.clixml`** (DPAPI, per user and machine), created once:

```powershell
$path = 'target/lab-windows/private/administrator.clixml'
New-Item -ItemType Directory -Force (Split-Path $path) | Out-Null
Get-Credential Administrator | Export-Clixml -LiteralPath $path
```

- **SSH:** `ssh -F target/lab-ubuntu/ssh_config dnstap-ubuntu`. The file needs a `Host dnstap-ubuntu` entry with `HostName`, `User dnstaplab`, `IdentityFile`, `IdentitiesOnly yes`, a lab-only `UserKnownHostsFile`, `StrictHostKeyChecking yes` (`accept-new` first time), and a pinned host key.
- **Constants:** [`lab.json`](lab.json) holds VM names and GUIDs, receiver `172.27.215.60:6000`, DNS server `172.27.215.163`, install paths, session `EtwDnstap-Lab`, zones, SSH alias. `LabCommon.psm1`, [`lab_config.py`](lab_config.py), and the shell scripts read it. Campaign IDs, hashes, disk identity are parameters.

#### Run order

1. `Get-DnsWorkloadSource.ps1` (host, once): downloads the pinned SecLists names, runs `New-DnsWorkload.ps1`.
2. `windows-inventory.ps1`, optionally `windows-smoke-queries.ps1`.
3. Guest, root: copy `lab.json`, `lab_config.py`, `monitor-ubuntu-workload.py`, `tune-upstream-buffer.py`, `rec-names.txt` next to the script, then `sudo ./setup-ubuntu-workload.sh` (installs to root-owned `/usr/local/lib/etw-dnstap-lab`). Undo: `sudo ./restore-ubuntu-workload.sh`.
4. Optional data disk: `ubuntu-performance-data-disk.ps1 -Action Attach -CampaignId <id> -WhatIf` (host), `prepare-ubuntu-data-disk.py` (guest, dry run first). Later `unmount-ubuntu-data-disk.py`, then `ubuntu-performance-data-disk.ps1 -Action Detach`.
5. Campaign: `windows-parser-campaign.ps1` (`Inventory`, `Backup`, `Deploy`, `Prepare`), `ubuntu-parser-ramp-upstream.py`, `ubuntu-parser-ramp-control.py`, one `windows-parser-stage.ps1` per stage, `windows-parser-campaign.ps1 -Action Restore`/`Fetch`, `windows-final-proof.ps1`, then verify, reconcile, report. Optional: `windows-startup-capacity.ps1 -WhatIf`, then without, to check Running at `--capacity 262144`.
6. Per run: `windows-run-client.ps1` (load), `windows-capture.ps1` (counters, WPR, clock).
7. Analysis: [Python](#python).

`-CampaignId`: 1 to 64 letters, digits, `_`, `-`, mandatory for campaign scripts. `Backup` and `Prepare` refuse an existing host evidence or guest campaign directory. Fetches verify SHA-256.

`archive/`: `Backup/Prepare/Deploy/Start` disabled; cleanup and evidence work.

#### Destructive steps

| Script | Changes | Guard |
| --- | --- | --- |
| `prepare-ubuntu-data-disk.py` | `mkfs.ext4`, mount, collector drop-in | Dry run unless `--apply`; explicit `--device`/`--mount`; size, serial/WWN, label, mount, root disk, partition, host receipt checks; `--wipe-existing-signatures` overrides `wipefs` |
| `unmount-ubuntu-data-disk.py` | `umount` | Dry run unless `--apply`; re-verifies UUID, serial/WWN, label, open descriptors |
| `remove-verified-guest-archives.py` | Deletes guest duplicates | Dry run unless `--apply`; host hash/count proofs |
| `ubuntu-performance-data-disk.ps1` | Creates, attaches, detaches VHDX | `-WhatIf`/`-Confirm`; GUID, OS-disk, free-space checks |
| `Remove-LabOwnedZone` | `Remove-DnsServerZone -Force` | Only zones in `owned-zones.txt` |
| `windows-*campaign*.ps1` Deploy/Restore | Service binary, SCM command line | Hash and ACL checks; Backup refuses a reused campaign ID |
| `windows-startup-capacity.ps1` | SCM command line (`--capacity`), optional binary, service start/stop | `-WhatIf` is preflight only; hash and SDDL checks; restore in `finally` |
| `setup-ubuntu-workload.sh` | Collector config, units, nft table | Root only; refuses if a backup, drop-in, or log exists |

#### Receiver STOP/FINISH check

The service exits 0 whether or not the receiver answers STOP with FINISH. `windows-parser-stage.ps1` checks through `receiver_stop_finish.py` and the `*-LabReceiverStopCapture` functions in `LabCommon.psm1`.

- Runs `sudo -n tcpdump` over ssh on the receiver. Filter: TCP port 6000, 1 to 16 byte payload, FIN/RST. Output: `stages/<run>/receiver-stop/`. Print the command: `python -I scripts/performance/receiver_stop_finish.py --print-command`.
- `Assert-LabReceiverStopCaptureReady`: host `python`, receiver `tcpdump` with passwordless sudo.
- Verdict `receiver_stop_finish`: `confirmed` (STOP then FINISH, same connection), `not-seen` (fails the strict stage pass), `not-checked` (no capture; `windows-startup-capacity.ps1`). In `completion.json`, `stage-summary.json`, the ledger, both reports.
- Offline: `python -I scripts/performance/receiver_stop_finish.py target/performance/<id>/stages/<run>`. Test: `test_receiver_stop_finish.py`.

#### Script reference

| PowerShell (host) | Does | Flags |
| --- | --- | --- |
| `LabCommon.psm1` | Shared module | `Invoke-LabGuest`, `Receive-LabFile`, `Initialize-LabDnsZones` |
| `Get-DnsWorkloadSource.ps1` | Download names, build workload | `-OutputDirectory` |
| `New-DnsWorkload.ps1` | Name lists, zone, provenance | `source-names.txt`, `source-commit.json` |
| `windows-inventory.ps1` | Read-only inventory | none |
| `windows-smoke-queries.ps1` | Resolve names, check TTLs | `-ProofName`, `-EvidenceRoot` |
| `windows-final-proof.ps1` | Prove normal state | `-EvidenceRoot` |
| `windows-capture.ps1` | Counters, WPR, clock | `-Action`, `-RunId`, `-EvidenceRoot` |
| `windows-run-client.ps1` | Run, sample `dns-stress` | `-Mode`, `-RunId`, `-Qps`, `-Duration` |
| `windows-startup-capacity.ps1` | Startup at max `--capacity` | `-Capacity`, `-CandidatePath`, `-ExpectedHash`, `-ProofName`, `-EvidenceRoot` |
| `windows-parser-campaign.ps1` | Backup/Deploy/Prepare/Start/Stop/Restore/Fetch | `-Action`, `-CampaignId`, `-ExpectedHash`, `-CandidatePath`, `-UpstreamAddress` |
| `windows-parser-stage.ps1` | One parser stage | `-Mode`, `-CampaignId`, `-RunId`, `-Qps`, `-Duration`, `-ReceiverReadyReceipt`, `-ExpectedHash`, `-MatrixApproved` |
| `ubuntu-performance-data-disk.ps1` | Attach/detach VHDX | `-Action`, `-CampaignId` |
| `export-cpu-precise.ps1`, `export-pooled-evidence.ps1` | `xperf`/`wpaexporter` | `-TracePath`, window, output dir |

`windows-startup-capacity.ps1` appends `--capacity` to the service `PathName`, samples SCM until `RUNNING`, and writes `<EvidenceRoot>/<ProofName>.json`. No receiver or campaign ID needed.

Guest shell (root): `setup-ubuntu-workload.sh` installs the dnsmasq fixture, nft counters, collector telemetry, monitor; `restore-ubuntu-workload.sh` reverses it.

#### Python

Guest (root): `prepare-ubuntu-data-disk.py`, `unmount-ubuntu-data-disk.py`, `remove-verified-guest-archives.py`, `archive-ubuntu-run.py`, `ubuntu-storage-budget.py`, `tune-upstream-buffer.py`, `monitor-ubuntu-workload.py`, `receiver-evidence.py`, `summarize-ubuntu-workload.py`, `ubuntu-parser-ramp-control.py`, `ubuntu-parser-ramp-upstream.py`. Copy with `lab.json`, `lab_config.py`, `guest_common.py`, `parser_ramp_receipts.py` to one root-owned directory.

Host (each has `--help`):

- Per run: `analyze-host.py`, `analyze-windows.py`, `aggregate-cpu-precise.py`, `measure-ubuntu-clock.py`.
- Pooled: `summarize-pooled-broad.py`, `-native.py`, `-stacks.py`, `-stage.py`, `-trace-gates.py`, `check-pooled-summary-gates.py`, `audit-pooled-management.py`, `build-pooled-report.py`, `export-pooled-graph-data.py`, `plot-pooled-wpr.py`.
- Parser ramp: `build-parser-ramp-report.py`, `reconcile-parser-ramp.py`, `verify-parser-ramp-archive.py`.
- Receiver archive: `count-receiver-archive.py`, `verify-receiver-archive.py`.
- `pooled_intake_run.py` runs the Rust callback benchmark. It, `plot-pooled-wpr.py`, and `export-pooled-graph-data.py` hash their own source into provenance.
- Shared: `metrics_ledger.py`, `perf_common.py`, `parser_ramp_receipts.py`, `lab_config.py`, `guest_common.py`.
- `plot-tier-runs.py`: one PNG per run and QPS tier, from folders with `run.json` and `timeseries.csv`.

```powershell
python -m venv target/plot-venv
target/plot-venv/Scripts/pip install -r scripts/performance/requirements.txt
target/plot-venv/Scripts/python scripts/performance/plot-tier-runs.py --runs <runs-root> --out docs/history/assets/performance [--tier 10000]
```

Loss percent = lost events over events offered (received plus `etw_lost`). `"tier_summary": false` in `run.json` excludes a run. Test (same venv): `python -I scripts/performance/test_plot_tier_runs.py`.

#### Tests

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

#### File moves and renames

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

### Common Gotchas

- Ubuntu controllers default `--campaign-id` to their dated campaigns and embed its filesystem UUIDs. Pass the campaign.
- `-ExpectedHash` is mandatory. `windows-service-campaign.ps1` Fetch keeps a differing host file.
- `.gitattributes` keeps `*.sh` as LF. Run Python destructive scripts without `--apply` first.
- The recorded campaigns used binaries with a JSONL metrics writer that the current service lacks. Their commands, ledgers, and overhead apply to those binaries only. The service-metrics harnesses reject new campaigns before any VM, load, or SCM change. Before another campaign, design compatible telemetry, and use a fresh backup, unique run IDs, verified guest identities, and fixture readiness.
