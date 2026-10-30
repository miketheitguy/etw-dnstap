# DNS workload provenance

This page helps maintainers reproduce the pinned 5,000-name lab workload and
understand its source, fixture values, and receiver measurement boundaries.
The generated files are inputs to the host load client and DNS fixture setup.

## Generate the workload

From the repository root, use PowerShell with HTTPS access to GitHub's raw
content host. Run the source-download and generation helper:

```powershell
powershell.exe -NoProfile -File `
  scripts/performance/Get-DnsWorkloadSource.ps1
```

The helper downloads the pinned
SecLists source, verifies its SHA-256 hash, and generates names and a Windows DNS
zone file under `target/performance/workload`.

Confirm that `auth-names.txt` and `rec-names.txt` each contain 5,000 names and
that the generated `provenance.json` records the source SHA-256 and fixture
values. The directory also contains `relative-names.txt` and
`auth.stress.test.dns`. Generation does not install the Windows DNS zone or
Ubuntu upstream. Use the [campaign plan](../../docs/pooled-vm-performance-plan.md)
for the recorded fixture and readiness requirements, then the
[DNS stress client guide](../../tools/dns-stress/README.md) to run load.

## Source and fixture values

The source contains common subdomain names, rather than a ranked list of full
public domains. Selection takes the first 5,000 valid unique names from the
20,000-entry source after excluding the `wpad` and `isatap` labels. This preserves
the Windows DNS global query block list. Every selected name passes strict ASCII
DNS-label validation.
The workload appends each name to `auth.stress.test` and `rec.stress.test`, so
load stays within reserved test zones and the controlled lab upstream.

See [the provenance receipt](workload-provenance.json) for the commit, checksum,
name count, answer address, and TTLs. See [the SecLists license](LICENSE-SecLists)
for its MIT license and attribution. The pinned upstream source is
[SecLists commit 1e3a742](https://github.com/danielmiessler/SecLists/blob/1e3a7423c117ea878679a29c760564fe442021c0/Discovery/DNS/subdomains-top1million-20000.txt).

The authoritative Windows zone contains A records for `192.0.2.53` with TTL 300.
The recursive test uses a conditional forwarder to the Ubuntu guest, whose
dnsmasq authoritative fixture returns the same address with TTL 0. A repeated
query must produce an upstream query before treating a run as uncached recursion.

TTL zero and the rotating list do not prove one upstream lookup per client
query throughout a loaded run. Preserve actual upstream query/reply counts and
use them in the [record ledger](../../docs/measurement-terms.md#dns-work-and-capture-records).

## Receiver evidence boundaries

For measured runs, the collector's existing JSON stdout pipeline writes to a
durable file instead of the interactive console and journal. Receiver evidence
uses byte-offset boundaries, per-operation JSON counts, and collector telemetry.
The interactive console and journal configuration is restored after testing.
Collector telemetry refreshes once per second. Keep the FrameStreams session
open for at least three seconds after client drain before taking the end marker,
because a stopped per-connection worker does not synchronously flush its last
telemetry increment.
