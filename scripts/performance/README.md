# Performance lab scripts

These scripts drive the etw-dnstap lab. The lab has three parts:

- A Windows Server VM, Hyper-V guest `DNSTap-WS2025`. It is the forwarder under
  test and sends DNSTAP to the receiver.
- A receiver on an Ubuntu VM, `DNSTap-Ubuntu2604`.
- A DNS client on the Hyper-V host that generates load.

The scripts create evidence under `target/performance/`.

**These scripts change real VMs.** Some format a disk, remove DNS zones, replace the installed
service binary or detach virtual disks. Read the [destructive steps](#destructive-steps) first.
Nothing here is meant to be run on a machine that is not the dedicated lab host.

## Prerequisites

- A Windows host with the **Hyper-V** role and PowerShell module, run from an elevated
  **Windows PowerShell 5.1** session. All `.ps1` files declare `#requires -Version 5.1`. The Hyper-V
  module loads natively only in Windows PowerShell. PowerShell 7 reaches it through the implicit
  compatibility session. That session returns deserialized VM objects, which break
  `Remove-VMHardDiskDrive` and the identity checks. The scripts also parse under PowerShell 7.
- The two VMs, with the names and GUIDs in [`lab.json`](lab.json). Every script verifies the VM GUID
  before touching anything. The Windows guest has the DNS Server role, the `EtwDnstap` service,
  and WPR/logman. The Ubuntu guest has `dnscollector`, `dnsmasq`, `nft`, `tcpdump` and `python3`.
- Windows Performance Toolkit (`xperf.exe`, `wpaexporter.exe`) on the host for the `export-*` scripts.
- `tools/dns-stress` built for `x86_64-pc-windows-msvc` (the client the host runs).
- Python 3.11+ on the host (`pip install -r requirements.txt`). Guest-side Python uses the Ubuntu
  system `python3` and the standard library only. The archived `archive/ubuntu-service-stop-config.py`
  also needs PyYAML on the guest (`apt install python3-yaml`).

### Guest administrator credential (`administrator.clixml`)

PowerShell Direct needs a credential. Create it once, as the same Windows user that runs the scripts
(the file is DPAPI-protected and not portable between users or machines):

```powershell
$path = 'target/lab-windows/private/administrator.clixml'
New-Item -ItemType Directory -Force (Split-Path $path) | Out-Null
Get-Credential Administrator | Export-Clixml -LiteralPath $path
```

`target/` is git-ignored. If the file is missing, `Get-LabSession` stops with these same instructions.

### SSH to the Ubuntu VM

`measure-ubuntu-clock.py` and the Ubuntu controllers use `ssh -F target/lab-ubuntu/ssh_config dnstap-ubuntu`
(both values come from `lab.json`). Create `target/lab-ubuntu/ssh_config` with a `Host dnstap-ubuntu`
entry (`HostName`, `User dnstaplab`, `IdentityFile`, `IdentitiesOnly yes`, `UserKnownHostsFile` pointing
at a lab-only known_hosts file). **Check that it does not contain `StrictHostKeyChecking no`**.
Use `StrictHostKeyChecking yes` (or `accept-new` for the first connection only) and pin the host key.

### Lab constants

All shared constants live in [`lab.json`](lab.json): VM names/GUIDs, receiver `172.27.215.60:6000`,
DNS server `172.27.215.163`, install paths, `EtwDnstap-Lab` session name, zones, SSH alias. It is read by
`LabCommon.psm1` (`Get-LabConfig`), by [`lab_config.py`](lab_config.py) and, as plain data, by the shell
scripts. Change a value there, not in the scripts. Per-campaign values (campaign IDs, binary hashes, disk
identity) are parameters, never baked in.

## Run order

1. **Workload (host, once):** `Get-DnsWorkloadSource.ps1` (downloads the pinned SecLists names, and is the only
   networked script) then, if you only need to regenerate, `New-DnsWorkload.ps1`.
2. **Inspect:** run `windows-inventory.ps1`, and optionally `windows-smoke-queries.ps1`.
3. **Ubuntu fixture (guest, root):** copy `lab.json`, `lab_config.py`, `monitor-ubuntu-workload.py`,
   `tune-upstream-buffer.py` and `rec-names.txt` next to the setup script, then
   `sudo ./setup-ubuntu-workload.sh`. It first installs everything root will run into the root-owned
   `/usr/local/lib/etw-dnstap-lab` (dirs 0755, files 0644/0755). Root never executes files from the home
   directory. Undo with `sudo ./restore-ubuntu-workload.sh`.
4. **Optional separate data disk:** `ubuntu-performance-data-disk.ps1 -Action Attach -CampaignId <id> -WhatIf`
   (host), then `prepare-ubuntu-data-disk.py` (guest, dry run first), later `unmount-ubuntu-data-disk.py`
   and `ubuntu-performance-data-disk.ps1 -Action Detach`.
5. **Parser-ramp campaign:** `windows-parser-campaign.ps1` (`Inventory`, `Backup`, `Deploy`, `Prepare`),
   Ubuntu `ubuntu-parser-ramp-upstream.py` / `ubuntu-parser-ramp-control.py`, one
   `windows-parser-stage.ps1` per stage, then `windows-parser-campaign.ps1 -Action Restore`/`Fetch`,
   `windows-final-proof.ps1`, and the verify/reconcile/report scripts below.
6. **Per-run helpers:** `windows-run-client.ps1` (load), `windows-capture.ps1` (counters/WPR/clock).
7. **Analysis (host, offline):** `analyze-host.py`, `analyze-windows.py`, `summarize-pooled-broad.py`, `summarize-pooled-native.py`,
   `summarize-pooled-stacks.py`, `summarize-pooled-stage.py`, `summarize-pooled-trace-gates.py`, `export-cpu-precise.ps1`,
   `export-pooled-evidence.ps1`, `export-pooled-graph-data.py`, `build-parser-ramp-report.py`, `build-pooled-report.py`,
   `plot-pooled-wpr.py`, `aggregate-cpu-precise.py`. The full list is under [Python](#python).

Historical service-metrics harnesses (JSONL metrics were removed from the service) are in
[`archive/`](archive/): `windows-1000qps-campaign.ps1`, `windows-service-campaign.ps1`, `windows-pooled-stage.ps1`,
`windows-pooled-wpr.ps1` and `windows-control.ps1`. Their `Backup/Prepare/Deploy/Start` actions are disabled. The
cleanup/evidence actions still work. Also archived (Python): the service-stop tools `ubuntu-service-stop-config.py` and
`ubuntu-service-stop-pcap.py`, `count-parser-ramp-diagnostic.py`, and the JSONL-metrics analyzers `analyze-ring-fix-cpu.py`,
`analyze-metrics-phases.py`, `analyze-send-window.py` and `summarize-receiver-results.py` (tests: `archive/test_archived_analyzers.py`).

## Destructive steps

| Script | What it changes | Guard |
| --- | --- | --- |
| `prepare-ubuntu-data-disk.py` | `mkfs.ext4` on a disk, mounts it, rewrites the collector drop-in | Dry run by default. Use `--apply` to act. Explicit `--device`/`--mount`. Checks size, serial/WWN, label, not mounted, not the root disk, no partitions/filesystem, host receipt GUID/VHDX/slot. Aborts if `wipefs` sees a signature unless `--wipe-existing-signatures` |
| `unmount-ubuntu-data-disk.py` | `umount` of the data filesystem | Dry run by default. Use `--apply` to act. Re-verifies UUID, serial/WWN, label, major:minor and open descriptors |
| `remove-verified-guest-archives.py` | Deletes guest archive duplicates | Dry run by default. Use `--apply` to act. Requires host hash/count proofs and a campaign-scoped directory |
| `ubuntu-performance-data-disk.ps1` | Creates/attaches a VHDX, or detaches it (VHDX is kept) | `SupportsShouldProcess` (`-WhatIf`/`-Confirm`, `ConfirmImpact High`). GUID, OS-disk and free-space checks |
| zone removal (`Remove-LabOwnedZone`) | `Remove-DnsServerZone -Force` | Only for zones listed in the campaign's `owned-zones.txt` (legacy `owned-rec-forwarder.txt`/`owned-forwarder.txt` still honoured). Anything else is refused |
| `windows-*campaign*.ps1` Deploy/Restore | Replaces the installed service binary / SCM command line | Hash and ACL checks. Backup refuses to reuse a campaign ID |
| `setup-ubuntu-workload.sh` | Edits the collector config, installs units and an nft table | Root only, refuses if a backup/drop-in/log already exists |

Always run the Python destructive scripts without `--apply` first and read the plan.

## Campaign IDs

Campaign and run IDs are never hard coded. `-CampaignId` (1-64 letters, digits, `_`, `-`) is mandatory
for the campaign scripts. `Backup` and `Prepare` refuse to start when the host evidence directory or the
guest campaign directory already exists. An earlier run is therefore never overwritten.

Fetches verify every file's SHA-256 after `Copy-Item -FromSession`.
`windows-service-campaign.ps1` and the archived WPR helper keep an identical file and refuse to
replace a different one. The control, capture, and parser fetches refresh files that legitimately
change. Those files are still hash-verified.

## Script reference

### PowerShell (host)

| Script | Purpose | Inputs | Outputs |
| --- | --- | --- | --- |
| `LabCommon.psm1` | Shared module: config, VM/session, `Invoke-LabGuest`, hash-verified `Receive-LabFile`, `Initialize-LabDnsZones`, argument helpers (`Join-LabArguments`, `Get-LabServiceCommandLine`) | `lab.json` | n/a |
| `Get-DnsWorkloadSource.ps1` | Download pinned SecLists names and run `New-DnsWorkload.ps1` | `-OutputDirectory` | `target/performance/workload/*` |
| `New-DnsWorkload.ps1` | Build `auth-names.txt`, `rec-names.txt`, zone file, provenance | `source-names.txt`, `source-commit.json` | workload files (UTF-8, no BOM) |
| `windows-inventory.ps1` | Read-only guest inventory | none | JSON on stdout |
| `windows-smoke-queries.ps1` | Resolve smoke names, check TTLs and record count | `-ProofName`, `-EvidenceRoot` | `<ProofName>.json` |
| `windows-final-proof.ps1` | Prove service/zones/answers are back to normal | `-EvidenceRoot` | `final-query-proof.json` |
| `windows-capture.ps1` | Counter collector, bounded WPR, clock bounds | `-Action`, `-RunId`, `-EvidenceRoot` | `<EvidenceRoot>/<RunId>/*` |
| `windows-run-client.ps1` | Run `dns-stress` and sample it | `-Mode`, `-RunId`, `-Qps`, `-Duration` | `invocation.json`, `host-process.csv`, `exit.json` |
| `windows-parser-campaign.ps1` | Parser-ramp Backup/Deploy/Prepare/Start/Stop/Restore/Fetch | `-Action`, `-CampaignId`, `-ExpectedHash`, `-CandidatePath`, `-UpstreamAddress` | receipts + fetched files under `target/performance/<id>/windows` |
| `windows-parser-stage.ps1` | One isolated parser stage with guards | `-Mode`, `-CampaignId`, `-RunId`, `-Qps`, `-Duration`, `-ReceiverReadyReceipt`, `-ExpectedHash`, `-MatrixApproved` | `target/performance/<id>/stages/<run>` |
| `ubuntu-performance-data-disk.ps1` | Attach/detach the temporary VHDX | `-Action`, `-CampaignId` | `temporary-data-disk-host.json` / `-detached.json` |
| `export-cpu-precise.ps1`, `export-pooled-evidence.ps1` | Run `xperf`/`wpaexporter` on a trace, recording every call | `-TracePath`, window, output dir | exported tables + receipts (never overwritten) |

### Shell (guest, root)

| Script | Purpose |
| --- | --- |
| `setup-ubuntu-workload.sh` | Install the upstream dnsmasq fixture, nft counters, collector telemetry and monitor (root-owned copies under `/usr/local/lib/etw-dnstap-lab`) |
| `restore-ubuntu-workload.sh` | Remove the fixture and restore the saved collector config |

### Python

Guest-side (Ubuntu, root): `prepare-ubuntu-data-disk.py`, `unmount-ubuntu-data-disk.py`,
`remove-verified-guest-archives.py`, `archive-ubuntu-run.py`, `ubuntu-storage-budget.py`,
`tune-upstream-buffer.py`, `monitor-ubuntu-workload.py`, `receiver-evidence.py`,
`summarize-ubuntu-workload.py`, `ubuntu-parser-ramp-control.py`, `ubuntu-parser-ramp-upstream.py`. Copy them together with `lab.json`,
`lab_config.py`, `guest_common.py`, `parser_ramp_receipts.py` to one directory on the guest (convert
line endings if your checkout uses CRLF, but `.gitattributes` keeps `*.sh` as LF). When running as root, run
them from a root-owned directory, not from the home directory.

Host-side analysis (each has argparse `--help` and a module docstring with its purpose):

- Per-run analysis: `analyze-host.py`, `analyze-windows.py`, `aggregate-cpu-precise.py`, `measure-ubuntu-clock.py`.
- Pooled-run summaries: `summarize-pooled-broad.py`, `summarize-pooled-native.py`, `summarize-pooled-stacks.py`,
  `summarize-pooled-stage.py`, `summarize-pooled-trace-gates.py`, `check-pooled-summary-gates.py`,
  `audit-pooled-management.py`, `build-pooled-report.py`, `export-pooled-graph-data.py`, `plot-pooled-wpr.py`.
- Parser-ramp campaign: `build-parser-ramp-report.py`, `reconcile-parser-ramp.py`, `verify-parser-ramp-archive.py`.
- Receiver archive: `count-receiver-archive.py`, `verify-receiver-archive.py`.
- Pooled intake benchmark: `pooled_intake_run.py` runs the private Rust callback benchmark. It, `plot-pooled-wpr.py` and
  `export-pooled-graph-data.py` hash their own source into provenance records, so editing them changes the recorded hashes.
- Shared code: `metrics_ledger.py` (service ledger validation), `perf_common.py` (JSON/hash/process-name/distribution
  helpers formerly copied between summarizers), `parser_ramp_receipts.py`, `lab_config.py`, `guest_common.py`.

## Tests

Static checks (no VM access):

```powershell
# PowerShell parse check
Get-ChildItem scripts/performance -Recurse -Include *.ps1,*.psm1 | ForEach-Object {
    $e = $null; [void][Management.Automation.Language.Parser]::ParseFile($_.FullName, [ref]$null, [ref]$e); $e }
```

```bash
bash -n scripts/performance/setup-ubuntu-workload.sh scripts/performance/restore-ubuntu-workload.sh
python -I -m py_compile scripts/performance/*.py scripts/performance/archive/*.py
python -I scripts/performance/test_metrics_ledger.py
python -I scripts/performance/test_pooled_intake_run.py
python -I scripts/performance/archive/test_archived_analyzers.py   # archived analyzers; also run by CI
```

The unit tests need no lab. Do not execute the lab scripts to "test" them.

## File moves and renames

| Old | New |
| --- | --- |
| `metrics-ledger-test.py` | `test_metrics_ledger.py` |
| `pooled-intake-run-test.py` | `test_pooled_intake_run.py` |
| `pooled-intake-run.py` | `pooled_intake_run.py` |
| `windows-1000qps-campaign.ps1` | `archive/windows-1000qps-campaign.ps1` |
| `windows-pooled-campaign.ps1` | `archive/windows-service-campaign.ps1 -Variant Pooled` |
| `windows-ring-fix-campaign.ps1` | `archive/windows-service-campaign.ps1 -Variant RingFix` |
| `windows-pooled-stage.ps1`, `windows-pooled-wpr.ps1` | same names under `archive/` |
| `windows-pooled-diagnostics.ps1`, `-final-proof`, `-inventory`, `-process-check`, `-size-watch` | moved to `archive/` in `6e62b2e`. Deleted in `63dd477` (see git history) |
| `windows-control.ps1` | `archive/windows-control.ps1` |
| `ubuntu-service-stop-config.py`, `ubuntu-service-stop-pcap.py` | same names under `archive/` |
| `count-parser-ramp-diagnostic.py` | `archive/count-parser-ramp-diagnostic.py` |
| `analyze-ring-fix-cpu.py`, `analyze-metrics-phases.py`, `analyze-send-window.py`, `summarize-receiver-results.py` | same names under `archive/` (their tests moved from `test_metrics_ledger.py` to `archive/test_archived_analyzers.py`) |

Added: `lab.json`, `LabCommon.psm1`, `lab_config.py`, `perf_common.py`, `guest_common.py`, `requirements.txt`, this README.

## Known limits

- The Ubuntu controllers (`ubuntu-parser-ramp-*.py` and the archived `ubuntu-service-stop-*.py`) still default `--campaign-id`
  to their dated campaigns and embed that campaign's filesystem UUIDs. Pass the campaign explicitly.
- Behaviour changes from the originals: `-ExpectedHash` is now mandatory where a hash used to be a
  default. The service command line quotes the metrics path only when needed. `windows-service-campaign.ps1` Fetch
  no longer overwrites a differing host file.
