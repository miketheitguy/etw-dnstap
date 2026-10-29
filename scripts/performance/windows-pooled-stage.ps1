[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('authoritative','recursive')][string]$Mode,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId,
    [Parameter(Mandatory)][ValidateRange(1000,128000)][int]$Qps,
    [Parameter(Mandatory)][ValidateSet(60,300)][int]$Duration,
    [Parameter(Mandatory)][string]$ReceiverReadyReceipt,
    [Parameter(Mandatory)][switch]$MatrixApproved,
    [ValidateRange(4,12)][int]$TraceBudgetGiB=4,
    [ValidateRange(0,12884901888)][long]$ForecastTraceBytes=0,
    [switch]$CompressTraceTransfer
)
$ErrorActionPreference='Stop'
$root=(Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$campaign='target/performance/pooled-vm-20261029-0942'
$out=Join-Path $root "$campaign/stages/$RunId"
if (-not $MatrixApproved -or -not (Test-Path -LiteralPath $ReceiverReadyReceipt)) { throw 'Missing matrix/receiver gate' }
$ready=Get-Content -LiteralPath $ReceiverReadyReceipt -Raw | ConvertFrom-Json
if ($ready.run -ne $RunId -or $ready.configured_global_qps -ne $Qps -or $ready.duration_seconds -ne $Duration -or $ready.stable_publication -ne $true) { throw 'Receiver READY identity/configuration/stability mismatch' }
if ($ready.forecast_raw_bytes -le 0 -or $ready.forecast_raw_bytes -gt $ready.raw_budget_bytes) { throw 'Receiver forecast exceeds raw budget' }
$separateVolumes=$ready.storage_protocol -eq 'separate_raw_data_disk_and_root_archives'
if ($separateVolumes) {
    $approvedRawCaps=@(12GB,48GB)
    if ($Mode -eq 'recursive' -and $Duration -eq 60 -and $Qps -in @(56000,64000)) { $approvedRawCaps+=24GB }
    if ($ready.archive_double_footprint_fits -ne $false -or $ready.separate_volume_reserve_fits -ne $true -or $ready.raw_budget_bytes -notin $approvedRawCaps -or $ready.min_free_bytes -lt 3GB -or $ready.safety_bytes -lt 1GB) { throw 'Separate-volume protocol lacks its explicit approved limits' }
    foreach ($volume in $ready.raw_volume,$ready.archive_volume) {
        foreach ($field in 'mountpoint','filesystem_uuid','device_path','major_minor') {
            if ([string]::IsNullOrWhiteSpace([string]$volume.$field)) { throw "Missing verified volume identity: $field" }
        }
    }
    if ($ready.raw_volume.filesystem_uuid -eq $ready.archive_volume.filesystem_uuid -or $ready.raw_volume.device_path -eq $ready.archive_volume.device_path -or $ready.raw_volume.major_minor -eq $ready.archive_volume.major_minor) { throw 'Raw and archive volume identities must be distinct' }
    if ([string]::IsNullOrWhiteSpace([string]$ready.raw_volume.device_serial) -and [string]::IsNullOrWhiteSpace([string]$ready.raw_volume.device_wwn)) { throw 'Missing new data-disk serial/WWN identity' }
    $requiredRaw=$ready.forecast_raw_bytes+$ready.min_free_bytes+$ready.safety_bytes
    $requiredArchive=$ready.archive_compression_reserve_bytes+$ready.min_free_bytes+$ready.safety_bytes
    if ($ready.archive_compression_reserve_bytes -lt [Math]::Ceiling($ready.forecast_raw_bytes/4) -or $ready.raw_volume.required_bytes -ne $requiredRaw -or $ready.archive_volume.required_bytes -ne $requiredArchive -or $ready.raw_volume.available_bytes -lt $requiredRaw -or $ready.archive_volume.available_bytes -lt $requiredArchive) { throw 'Separate raw/archive live free-space reserve mismatch' }
    if ($ready.storage_receipt -ne 'ubuntu/temporary-data-disk.json' -or $ready.collector_output_diff -ne 'ubuntu/collector-data-path.diff') { throw 'Unexpected approved storage/configuration receipt paths' }
    foreach ($relative in $ready.storage_receipt,$ready.collector_output_diff) {
        if (-not (Test-Path -LiteralPath (Join-Path $root "$campaign/$relative"))) { throw 'Missing preserved storage/configuration evidence' }
    }
} elseif ($ready.archive_double_footprint_fits -ne $true -or $ready.available_bytes -lt ($ready.forecast_raw_bytes*2+$ready.min_free_bytes)) {
    throw 'Receiver forecast exceeds legacy raw/archive/free-space budget'
}
if ($ready.marker -ne "$RunId-start.json" -or [DateTime]::Parse($ready.ready_utc).ToUniversalTime() -lt [DateTime]::UtcNow.AddMinutes(-10)) { throw 'Receiver READY marker is stale or mismatched' }
if ($TraceBudgetGiB -gt 4 -and ($Duration -ne 300 -or $ForecastTraceBytes -le 0 -or $ForecastTraceBytes -gt $TraceBudgetGiB*1GB)) { throw 'Expanded trace budget requires a bounded 300-second confirmation forecast within its cap' }
if (Test-Path -LiteralPath $out) { throw 'Stage exists; preserve evidence' }
New-Item -ItemType Directory -Path $out | Out-Null
Copy-Item -LiteralPath $ReceiverReadyReceipt -Destination "$out/receiver-ready-receipt.json"
if ($separateVolumes) {
    Copy-Item -LiteralPath (Join-Path $root "$campaign/$($ready.storage_receipt)") -Destination "$out/temporary-data-disk.json"
    Copy-Item -LiteralPath (Join-Path $root "$campaign/$($ready.collector_output_diff)") -Destination "$out/collector-data-path.diff"
}
if ((Get-PSDrive C).Free -lt 20GB) { throw 'Host disk guard' }
$vm=Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac' -or $vm.ProcessorCount -ne 2 -or $vm.MemoryAssigned -ne 4294967296 -or $vm.State -ne 'Running') { throw 'VM identity/resources/state changed' }
$credential=Import-Clixml (Join-Path $root 'target/lab-windows/private/administrator.clixml')
$session=New-PSSession -VMId $vm.Id -Credential $credential
$client=$null
$traceStarted=$false
$serviceStarted=$false
$countersStarted=$false
$guards=[Collections.Generic.List[object]]::new()
$failure=$null
$serviceStopped=$false
function Run-Helper {
    param([string]$Script,[string[]]$ToolArgs)
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot $Script) @ToolArgs
    if ($LASTEXITCODE -ne 0) { throw "$Script failed ($LASTEXITCODE)" }
}
try {
    $pre=Invoke-Command -Session $session -ScriptBlock {
        $svc=Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"
        if ($svc.State -ne 'Stopped') { throw 'Expected stopped service at stage boundary' }
        if ((Get-FileHash 'C:/Program Files/EtwDnstap/etw-dnstap.exe').Hash -ne '98FDAB43AA24AE1BE31A7AABCCBDD67A2F64C87DC1C50C428B638B3C091F5D7C') { throw 'Candidate hash mismatch' }
        if ((Get-Volume C).SizeRemaining -lt 8GB) { throw 'Guest disk guard' }
        $os=Get-CimInstance Win32_OperatingSystem
        if ($os.FreePhysicalMemory -lt 1MB) { throw 'Guest available-memory guard' }
        @{utc=[DateTime]::UtcNow.ToString('o');free_bytes=(Get-Volume C).SizeRemaining;available_memory_kib=$os.FreePhysicalMemory}
    }
    $pre | ConvertTo-Json | Set-Content "$out/preflight.json"
    $hostFree=(Get-PSDrive C).Free
    if ($TraceBudgetGiB -gt 4 -and ($pre.free_bytes -lt (2*$ForecastTraceBytes+10GB) -or $hostFree -lt (2*$ForecastTraceBytes+20GB))) { throw 'Expanded trace budget lacks approved merge/temp/free-space safety headroom' }
    [ordered]@{utc=[DateTime]::UtcNow.ToString('o');run_id=$RunId;mode=$Mode;requested_qps=$Qps;seconds=$Duration;trace_budget_bytes=$TraceBudgetGiB*1GB;forecast_trace_bytes=$ForecastTraceBytes;compress_trace_transfer=[bool]$CompressTraceTransfer;host_free_bytes=$hostFree;guest_free_bytes=$pre.free_bytes;expanded_cap_confirmation_only=($TraceBudgetGiB-gt4);required_guest_headroom_bytes=2*$ForecastTraceBytes+10GB;required_host_headroom_bytes=2*$ForecastTraceBytes+20GB} | ConvertTo-Json | Set-Content "$out/budget-receipt.json"
    Run-Helper 'windows-pooled-wpr.ps1' @('-Action','Start','-RunId',$RunId)
    $traceStarted=$true
    $serviceStarted=$true
    Run-Helper 'windows-pooled-campaign.ps1' @('-Action','Start','-RunId',$RunId)
    Run-Helper 'windows-capture.ps1' @('-Action','Start','-RunId',$RunId,'-EvidenceRoot',"$campaign/capture")
    $countersStarted=$true
    $clientArgs=@('-NoProfile','-ExecutionPolicy','Bypass','-File',(Join-Path $PSScriptRoot 'windows-run-client.ps1'),'-Mode',$Mode,'-RunId',$RunId,'-Duration',"$Duration",'-Qps',"$Qps",'-EvidenceRoot',"$campaign/client")
    $client=Start-Process powershell.exe -ArgumentList $clientArgs -WindowStyle Hidden -PassThru -RedirectStandardOutput "$out/client-wrapper-stdout.txt" -RedirectStandardError "$out/client-wrapper-stderr.txt"
    $clientHandle=$client.Handle
    $deadline=[DateTime]::UtcNow.AddSeconds($Duration+45)
    while (-not $client.HasExited) {
        if ([DateTime]::UtcNow -gt $deadline) { throw 'Client bounded completion deadline exceeded' }
        if ((Get-PSDrive C).Free -lt 20GB) { throw 'Host disk guard during load' }
        $guard=Invoke-Command -Session $session -ArgumentList $RunId -ScriptBlock {
            param($run)
            $dir="C:/ProgramData/EtwDnstap/performance/pooled-vm-20261029-0942/$run-wpr/temp"
            $bytes=(Get-ChildItem -LiteralPath $dir -File -Recurse -Force | Measure-Object Length -Sum).Sum
            $os=Get-CimInstance Win32_OperatingSystem
            $svc=Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"
            $nativeMetrics=$null
            $metricFile=Get-ChildItem -LiteralPath 'C:/ProgramData/EtwDnstap/performance/pooled-vm-20261029-0942' -Filter "$run.*.jsonl" -File | Select-Object -First 1
            if ($metricFile) {
                foreach ($line in @(Get-Content -LiteralPath $metricFile.FullName -Tail 2)) {
                    try { $nativeMetrics=$line | ConvertFrom-Json -ErrorAction Stop } catch { }
                }
            }
            [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');trace_temp_bytes=[long]$bytes;free_bytes=(Get-Volume C).SizeRemaining;available_memory_kib=$os.FreePhysicalMemory;service_state=$svc.State;service_pid=$svc.ProcessId;native_metrics=$nativeMetrics}
        }
        $guards.Add($guard)
        $guard | ConvertTo-Json -Compress | Add-Content "$out/guards.jsonl"
        if ($guard.trace_temp_bytes -ge $TraceBudgetGiB*1GB -or ($pre.free_bytes-$guard.free_bytes) -ge $TraceBudgetGiB*1GB -or $guard.free_bytes -lt 8GB -or $guard.available_memory_kib -lt 1MB -or $guard.service_state -ne 'Running') { throw 'Guest trace/disk/memory/service guard activated' }
        Start-Sleep -Seconds 5
        $client.Refresh()
    }
    $client.WaitForExit()
    $clientExit=Get-Content -LiteralPath (Join-Path $root "$campaign/client/$RunId/exit.json") -Raw | ConvertFrom-Json
    if ($client.ExitCode -ne 0 -or $clientExit.exit_code -ne 0) { throw "Client wrapper exit $($client.ExitCode); DNS client exit $($clientExit.exit_code)" }
    Start-Sleep -Seconds 3
} catch {
    $failure=$_.ToString()
    if ($client -and -not $client.HasExited) {
        # Stop only the owned helper and its owned DNS client after a resource guard.
        $children=Get-CimInstance Win32_Process -Filter "ParentProcessId=$($client.Id)" | Where-Object Name -eq 'dns-stress.exe'
        foreach ($child in $children) { Stop-Process -Id $child.ProcessId -ErrorAction SilentlyContinue }
        Stop-Process -Id $client.Id -ErrorAction SilentlyContinue
    }
} finally {
    if ($serviceStarted) {
        try { Run-Helper 'windows-pooled-campaign.ps1' @('-Action','Stop','-RunId',$RunId); $serviceStopped=$true } catch { $failure="$failure; SCM stop: $_" }
    }
    try { Run-Helper 'windows-pooled-campaign.ps1' @('-Action','Fetch','-RunId',$RunId) } catch { $failure="$failure; metrics fetch: $_" }
    if ($traceStarted) {
        try {
            $stopArgs=@('-Action','Stop','-RunId',$RunId)
            if ($CompressTraceTransfer) { $stopArgs+='-CompressTransfer' }
            Run-Helper 'windows-pooled-wpr.ps1' $stopArgs
        } catch { $failure="$failure; WPR stop: $_" }
    }
    if ($countersStarted) {
        try { Run-Helper 'windows-capture.ps1' @('-Action','Clock','-RunId',$RunId,'-EvidenceRoot',"$campaign/capture"); Run-Helper 'windows-capture.ps1' @('-Action','Stop','-RunId',$RunId,'-EvidenceRoot',"$campaign/capture") } catch { $failure="$failure; counters stop: $_" }
    }
    Remove-PSSession $session
    [ordered]@{utc=[DateTime]::UtcNow.ToString('o');run_id=$RunId;mode=$Mode;qps=$Qps;duration=$Duration;trace_budget_bytes=$TraceBudgetGiB*1GB;forecast_trace_bytes=$ForecastTraceBytes;compress_trace_transfer=[bool]$CompressTraceTransfer;failure=$failure;guards=$guards.Count;service_stopped=$serviceStopped;trace_started=$traceStarted;counters_started=$countersStarted} | ConvertTo-Json -Depth 4 | Set-Content "$out/completion.json"
}
if ($failure) { throw $failure }
Write-Output "WINDOWS_STAGE_COMPLETE $RunId; receiver final stable marker and all-category ledger gate still required"

