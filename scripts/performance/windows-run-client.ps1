[CmdletBinding()]
param([Parameter(Mandatory)][ValidateSet('authoritative','recursive')][string]$Mode,[Parameter(Mandatory)][string]$RunId,[ValidateRange(1,64)][int]$Threads=2,[ValidateRange(1,3600)][int]$Duration=300,[long]$Qps=0,[string]$EvidenceRoot='target/performance/client')
$ErrorActionPreference = 'Stop'
if ($RunId -notmatch '^[A-Za-z0-9_-]{1,64}$') { throw 'Invalid run ID' }
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$exe = Join-Path $root 'tools/dns-stress/target/x86_64-pc-windows-msvc/release/dns-stress.exe'
$names = Join-Path $root ('target/performance/workload/' + $(if ($Mode -eq 'authoritative') {'auth-names.txt'} else {'rec-names.txt'}))
$evidence = [IO.Path]::GetFullPath([IO.Path]::Combine($root,$EvidenceRoot))
if (-not $evidence.StartsWith($root + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw 'Evidence root must stay within the workspace' }
$directory = Join-Path $evidence $RunId
if (Test-Path -LiteralPath $directory) { throw 'Client run directory exists' }
New-Item -ItemType Directory -Path $directory | Out-Null
$arguments = @('--server','172.27.215.163:53','--names',$names,'--mode',$Mode,'--threads',"$Threads",'--duration',"$Duration",'--inflight','256','--timeout-ms','2000','--output-dir',$directory)
if ($Qps -gt 0) { $arguments += @('--qps',"$Qps") }
$manifest = [ordered]@{command=$exe;arguments=$arguments;exe_sha256=(Get-FileHash -LiteralPath $exe).Hash;names_sha256=(Get-FileHash -LiteralPath $names).Hash;host_start_utc=[DateTime]::UtcNow.ToString('o');logical_processors=[Environment]::ProcessorCount}
$manifest | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $directory 'invocation.json')
$process = Start-Process -FilePath $exe -ArgumentList $arguments -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $directory 'stdout.txt') -RedirectStandardError (Join-Path $directory 'stderr.txt')
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
$samples | Export-Csv (Join-Path $directory 'host-process.csv') -NoTypeInformation
@{exit_code=$process.ExitCode;host_end_utc=[DateTime]::UtcNow.ToString('o');pid=$process.Id} | ConvertTo-Json | Set-Content (Join-Path $directory 'exit.json')
Get-Content (Join-Path $directory 'stdout.txt')
if ($process.ExitCode -ne 0) { throw "Client exit code: $($process.ExitCode)" }
