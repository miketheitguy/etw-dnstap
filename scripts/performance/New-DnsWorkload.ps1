[CmdletBinding()]
param(
    [string]$SourceDirectory = 'target/performance/workload',
    [string]$OutputDirectory = 'target/performance/workload',
    [int]$TargetCount = 5000,
    [string[]]$ExcludeLabels = @('wpad', 'isatap')
)
$ErrorActionPreference = 'Stop'
$source = Join-Path $SourceDirectory 'source-names.txt'
$commit = (Get-Content -LiteralPath (Join-Path $SourceDirectory 'source-commit.json') -Raw | ConvertFrom-Json).sha
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
    AuthoritativeZone = 'auth.stress.test'
    RecursiveZone = 'rec.stress.test'
    AnswerIPv4 = '192.0.2.53'
    AuthoritativeTTLSeconds = 300
    RecursiveUpstreamTTLSeconds = 0
    Safety = 'Queries use reserved .test zones and a controlled private upstream; no public DNS load.'
}
$provenance | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $OutputDirectory 'provenance.json') -Encoding utf8
$provenance | ConvertTo-Json
