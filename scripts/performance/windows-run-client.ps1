#requires -Version 5.1
<#
.SYNOPSIS
Runs the dns-stress client on the host against the lab Windows DNS server and records process samples.

.DESCRIPTION
Starts tools/dns-stress (built for x86_64-pc-windows-msvc) with the generated name list,
samples the client process once per second and writes invocation.json, host-process.csv and
exit.json under <EvidenceRoot>/<RunId>. Refuses to reuse an existing run directory. The
client exit code is propagated as a terminating error when non-zero.

.PARAMETER Mode
authoritative or recursive (selects auth-names.txt or rec-names.txt).
.PARAMETER RunId
Run label (1-64 letters, digits, '_' or '-').
.PARAMETER Threads
Client threads (default 2).
.PARAMETER Duration
Load duration in seconds (default 300).
.PARAMETER Qps
Global query rate; 0 (default) means unthrottled.
.PARAMETER EvidenceRoot
Repository-relative evidence root (default target/performance/client); must stay inside the repository.

.OUTPUTS
<EvidenceRoot>/<RunId>/{invocation.json,stdout.txt,stderr.txt,host-process.csv,exit.json} and the client's own output.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('authoritative','recursive')][string]$Mode,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId,
    [ValidateRange(1,64)][int]$Threads = 2,
    [ValidateRange(1,3600)][int]$Duration = 300,
    [long]$Qps = 0,
    [string]$EvidenceRoot = 'target/performance/client'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'LabCommon.psm1') -Force
$config = Get-LabConfig
$exe = Resolve-LabPath $config.paths.dns_stress_exe
$names = Resolve-LabPath ($config.paths.workload_dir + '/' + $(if ($Mode -eq 'authoritative') { 'auth-names.txt' } else { 'rec-names.txt' }))
foreach ($required in $exe, $names) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) { throw "Missing input: $required (build tools/dns-stress and run Get-DnsWorkloadSource.ps1 first)" }
}
$directory = Join-Path (Resolve-LabPath $EvidenceRoot) $RunId
if (Test-Path -LiteralPath $directory) { throw 'Client run directory exists' }
New-Item -ItemType Directory -Path $directory -Force | Out-Null
$arguments = @('--server', "$($config.network.dns_ip):53", '--names', $names, '--mode', $Mode, '--threads', "$Threads", '--duration', "$Duration", '--inflight', '256', '--timeout-ms', '2000', '--output-dir', $directory)
if ($Qps -gt 0) { $arguments += @('--qps', "$Qps") }
$manifest = [ordered]@{command=$exe;arguments=$arguments;exe_sha256=(Get-FileHash -LiteralPath $exe).Hash;names_sha256=(Get-FileHash -LiteralPath $names).Hash;host_start_utc=[DateTime]::UtcNow.ToString('o');logical_processors=[Environment]::ProcessorCount}
Write-LabJson -Path (Join-Path $directory 'invocation.json') -InputObject $manifest -Depth 4
$process = Start-Process -FilePath $exe -ArgumentList (Join-LabArguments -Arguments $arguments) -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $directory 'stdout.txt') -RedirectStandardError (Join-Path $directory 'stderr.txt')
# Reading .Handle keeps the exit code available in Windows PowerShell 5.1.
$processHandle = $process.Handle
$samples = [Collections.Generic.List[object]]::new()
while (-not $process.HasExited) {
    $process.Refresh()
    try {
        $samples.Add([pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');pid=$process.Id;cpu_seconds=$process.TotalProcessorTime.TotalSeconds;private_bytes=$process.PrivateMemorySize64;working_set=$process.WorkingSet64;threads=$process.Threads.Count;handles=$process.HandleCount;host_cpu_percent=(Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'").PercentProcessorTime})
    } catch { if (-not $process.HasExited) { throw } }
    Start-Sleep -Milliseconds 1000
}
$process.WaitForExit()
$samples | Export-Csv (Join-Path $directory 'host-process.csv') -NoTypeInformation -Encoding UTF8
Write-LabJson -Path (Join-Path $directory 'exit.json') -InputObject @{exit_code=$process.ExitCode;host_end_utc=[DateTime]::UtcNow.ToString('o');pid=$process.Id}
Get-Content -LiteralPath (Join-Path $directory 'stdout.txt')
if ($process.ExitCode -ne 0) { throw "Client exit code: $($process.ExitCode)" }
