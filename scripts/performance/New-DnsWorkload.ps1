#requires -Version 5.1
<#
.SYNOPSIS
Generates the authoritative/recursive name lists and the auth zone file from the downloaded source names.

.DESCRIPTION
Reads source-names.txt and source-commit.json (from Get-DnsWorkloadSource.ps1), keeps the first
TargetCount valid unique names (excluding blocked labels), and writes auth-names.txt,
rec-names.txt, relative-names.txt, rejected-names.txt, auth.stress.test.dns and
provenance.json as UTF-8 without BOM. Uses only reserved .test zones; no DNS traffic.

.PARAMETER SourceDirectory
Directory holding source-names.txt and source-commit.json (default: the lab workload directory, target/performance/workload).
.PARAMETER OutputDirectory
Destination directory (default: the lab workload directory, target/performance/workload).
.PARAMETER TargetCount
Number of names to select (default 5000).
.PARAMETER ExcludeLabels
Labels to reject (default wpad, isatap).

.OUTPUTS
The generated workload files plus provenance.json (also written to the pipeline).
#>
[CmdletBinding()]
param(
    [string]$SourceDirectory,
    [string]$OutputDirectory,
    [int]$TargetCount = 5000,
    [string[]]$ExcludeLabels = @('wpad', 'isatap')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'LabCommon.psm1') -Force
$dnsConfig = (Get-LabConfig).dns
$defaultWorkload = Resolve-LabPath (Get-LabConfig).paths.workload_dir
if (-not $SourceDirectory) { $SourceDirectory = $defaultWorkload }
if (-not $OutputDirectory) { $OutputDirectory = $defaultWorkload }
$source = Join-Path $SourceDirectory 'source-names.txt'
$commit = (Get-Content -LiteralPath (Join-Path $SourceDirectory 'source-commit.json') -Raw -Encoding UTF8 | ConvertFrom-Json).sha
if ($commit -notmatch '^[a-f0-9]{40}$') { throw 'Invalid source commit.' }
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$valid = [Collections.Generic.List[string]]::new()
$rejected = [Collections.Generic.List[string]]::new()
$seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
$eligibleCount = 0
foreach ($line in [IO.File]::ReadAllLines((Resolve-Path $source))) {
    $name = $line.Trim().ToLowerInvariant()
    $labels = $name.Split('.')
    $ok = $name.Length -gt 0 -and $name.Length -le 230
    foreach ($label in $labels) {
        if ($label.Length -lt 1 -or $label.Length -gt 63 -or $label -notmatch '^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$' -or $label -in $ExcludeLabels) { $ok = $false }
    }
    if ($ok -and $seen.Add($name)) {
        $eligibleCount++
        if ($valid.Count -lt $TargetCount) { $valid.Add($name) }
    } else { $rejected.Add($line) }
}
if ($valid.Count -ne $TargetCount) { throw 'Insufficient eligible source names.' }
$utf8 = [Text.UTF8Encoding]::new($false)
foreach ($mode in @('auth', 'rec')) {
    [IO.File]::WriteAllLines((Join-Path (Resolve-Path $OutputDirectory) "$mode-names.txt"), [string[]]($valid | ForEach-Object { "$_.$mode.stress.test" }), $utf8)
}
[IO.File]::WriteAllLines((Join-Path (Resolve-Path $OutputDirectory) 'relative-names.txt'), $valid, $utf8)
[IO.File]::WriteAllLines((Join-Path (Resolve-Path $OutputDirectory) 'rejected-names.txt'), $rejected, $utf8)
$zone = [Collections.Generic.List[string]]::new()
$zone.Add('$ORIGIN auth.stress.test.')
$zone.Add('$TTL 300')
$zone.Add('@ IN SOA ns.auth.stress.test. hostmaster.auth.stress.test. ( 2026102801 3600 600 86400 300 )')
$zone.Add('@ IN NS ns.auth.stress.test.')
$zone.Add('ns IN A 192.0.2.53')
foreach ($name in $valid) { if ($name -ne 'ns') { $zone.Add("$name IN A 192.0.2.53") } }
[IO.File]::WriteAllLines((Join-Path (Resolve-Path $OutputDirectory) 'auth.stress.test.dns'), $zone, $utf8)
$provenance = [ordered]@{
    SourceRepository = 'https://github.com/danielmiessler/SecLists'
    SourceCommit = $commit
    SourceUrl = "https://raw.githubusercontent.com/danielmiessler/SecLists/$commit/Discovery/DNS/subdomains-top1million-20000.txt"
    SourceLicense = 'MIT; see LICENSE-SecLists'
    SourceSHA256 = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
    SourceLineCount = [IO.File]::ReadAllLines((Resolve-Path $source)).Count
    ValidUniqueNames = $valid.Count
    EligibleUniqueSourceNames = $eligibleCount
    Selection = "First $TargetCount valid unique names after excluding blocked labels"
    ExcludedLabels = $ExcludeLabels
    RejectedOrDuplicate = $rejected.Count
    AuthoritativeZone = $dnsConfig.auth_zone
    RecursiveZone = $dnsConfig.rec_zone
    AnswerIPv4 = $dnsConfig.answer_ip
    AuthoritativeTTLSeconds = 300
    RecursiveUpstreamTTLSeconds = 0
    Safety = 'Queries use reserved .test zones and a controlled private upstream; no public DNS load.'
}
Write-LabJson -Path (Join-Path $OutputDirectory 'provenance.json') -InputObject $provenance -Force
$provenance | ConvertTo-Json
