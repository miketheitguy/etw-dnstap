#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Historical pooled-VM stage orchestrator (WPR + counters + DNS client + service start/stop).

.DESCRIPTION
Disabled: the TCP-only service no longer supports the JSONL metrics this harness measured.
Kept for reference and so the evidence it produced stays interpretable. It drives
windows-pooled-wpr.ps1 and windows-service-campaign.ps1 (in this directory) and
windows-capture.ps1 / windows-run-client.ps1 (one level up). The dated campaign directory
that used to be baked in is now the mandatory -CampaignId and the service hash the
mandatory -ExpectedHash.

.PARAMETER Mode
authoritative or recursive.
.PARAMETER CampaignId
Campaign directory under target/performance.
.PARAMETER RunId
Stage run ID (the stage directory must not exist).
.PARAMETER ExpectedHash
SHA-256 of the installed service binary.
.PARAMETER ReceiverReadyReceipt
Receiver READY receipt from the Ubuntu side.
.PARAMETER MatrixApproved
Operator approval of the run matrix.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('authoritative','recursive')][string]$Mode,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$CampaignId,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ExpectedHash,
    [Parameter(Mandatory)][ValidateRange(1000,128000)][int]$Qps,
    [Parameter(Mandatory)][ValidateSet(60,300)][int]$Duration,
    [Parameter(Mandatory)][string]$ReceiverReadyReceipt,
    [Parameter(Mandatory)][switch]$MatrixApproved,
    [ValidateRange(4,12)][int]$TraceBudgetGiB=4,
    [ValidateRange(0,12884901888)][long]$ForecastTraceBytes=0,
    [switch]$CompressTraceTransfer
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
throw 'Historical service-metrics harness: new campaigns are disabled because the TCP-only service no longer supports JSONL metrics. Existing evidence and cleanup actions remain available.'
Import-Module (Join-Path $PSScriptRoot '../LabCommon.psm1') -Force
$config=Get-LabConfig
$root=Get-LabRoot
$parentScripts=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$campaign="$($config.paths.evidence_root)/$CampaignId"
$out=Resolve-LabPath "$campaign/stages/$RunId"
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
$vm=Get-LabVm -RequireRunning -CheckResources
$session=Get-LabSession -Vm $vm
$client=$null
$traceStarted=$false
$serviceStarted=$false
$countersStarted=$false
$guards=[Collections.Generic.List[object]]::new()
$failure=$null
$serviceStopped=$false
function Run-Helper {
    # Runs a sibling script in a child Windows PowerShell; -Parent selects scripts/performance instead of archive.
    param([string]$Script,[string[]]$ToolArgs,[switch]$Parent)
    $directory = if ($Parent) { $parentScripts } else { $PSScriptRoot }
    Invoke-LabNative -FilePath 'powershell.exe' -ArgumentList (@('-NoProfile','-ExecutionPolicy','Bypass','-File',(Join-Path $directory $Script)) + $ToolArgs)
}
function Run-Campaign {
    param([string[]]$ToolArgs)
    Run-Helper 'windows-service-campaign.ps1' (@('-Variant','Pooled','-CampaignId',$CampaignId,'-ExpectedHash',$ExpectedHash) + $ToolArgs)
}
try {
    $pre=Invoke-LabGuest -Session $session -ArgumentList $ExpectedHash,$config.windows_vm.install_exe,$config.windows_vm.service_name -ScriptBlock {
        param($expectedHash,$installed,$serviceName)
        $svc=Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
        if ($svc.State -ne 'Stopped') { throw 'Expected stopped service at stage boundary' }
        if ((Get-FileHash $installed).Hash -ne $expectedHash) { throw 'Candidate hash mismatch' }
        if ((Get-Volume C).SizeRemaining -lt 8GB) { throw 'Guest disk guard' }
        $os=Get-CimInstance Win32_OperatingSystem
        if ($os.FreePhysicalMemory -lt 1MB) { throw 'Guest available-memory guard' }
        @{utc=[DateTime]::UtcNow.ToString('o');free_bytes=(Get-Volume C).SizeRemaining;available_memory_kib=$os.FreePhysicalMemory}
    }
    Write-LabJson -Path "$out/preflight.json" -InputObject $pre
    $hostFree=(Get-PSDrive C).Free
    if ($TraceBudgetGiB -gt 4 -and ($pre.free_bytes -lt (2*$ForecastTraceBytes+10GB) -or $hostFree -lt (2*$ForecastTraceBytes+20GB))) { throw 'Expanded trace budget lacks approved merge/temp/free-space safety headroom' }
    [ordered]@{utc=[DateTime]::UtcNow.ToString('o');run_id=$RunId;mode=$Mode;requested_qps=$Qps;seconds=$Duration;trace_budget_bytes=$TraceBudgetGiB*1GB;forecast_trace_bytes=$ForecastTraceBytes;compress_trace_transfer=[bool]$CompressTraceTransfer;host_free_bytes=$hostFree;guest_free_bytes=$pre.free_bytes;expanded_cap_confirmation_only=($TraceBudgetGiB-gt4);required_guest_headroom_bytes=2*$ForecastTraceBytes+10GB;required_host_headroom_bytes=2*$ForecastTraceBytes+20GB} | ConvertTo-Json | Set-Content "$out/budget-receipt.json" -Encoding UTF8
    Run-Helper 'windows-pooled-wpr.ps1' @('-Action','Start','-CampaignId',$CampaignId,'-RunId',$RunId)
    $traceStarted=$true
    $serviceStarted=$true
    Run-Campaign @('-Action','Start','-RunId',$RunId)
    Run-Helper 'windows-capture.ps1' @('-Action','Start','-RunId',$RunId,'-EvidenceRoot',"$campaign/capture") -Parent
    $countersStarted=$true
    $clientArgs=@('-NoProfile','-ExecutionPolicy','Bypass','-File',(Join-Path $parentScripts 'windows-run-client.ps1'),'-Mode',$Mode,'-RunId',$RunId,'-Duration',"$Duration",'-Qps',"$Qps",'-EvidenceRoot',"$campaign/client")
    $client=Start-Process powershell.exe -ArgumentList (Join-LabArguments -Arguments $clientArgs) -WindowStyle Hidden -PassThru -RedirectStandardOutput "$out/client-wrapper-stdout.txt" -RedirectStandardError "$out/client-wrapper-stderr.txt"
    $clientHandle=$client.Handle
    $deadline=[DateTime]::UtcNow.AddSeconds($Duration+45)
    while (-not $client.HasExited) {
        if ([DateTime]::UtcNow -gt $deadline) { throw 'Client bounded completion deadline exceeded' }
        if ((Get-PSDrive C).Free -lt 20GB) { throw 'Host disk guard during load' }
        $guard=Invoke-LabGuest -Session $session -ArgumentList $RunId,"$($config.windows_vm.data_dir)/$CampaignId",$config.windows_vm.service_name -ScriptBlock {
            param($run,$campaignDir,$serviceName)
            $dir="$campaignDir/$run-wpr/temp"
            $bytes=(Get-ChildItem -LiteralPath $dir -File -Recurse -Force | Measure-Object Length -Sum).Sum
            $os=Get-CimInstance Win32_OperatingSystem
            $svc=Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
            $nativeMetrics=$null
            $metricFile=Get-ChildItem -LiteralPath $campaignDir -Filter "$run.*.jsonl" -File | Select-Object -First 1
            if ($metricFile) {
                foreach ($line in @(Get-Content -LiteralPath $metricFile.FullName -Tail 2)) {
                    try { $nativeMetrics=$line | ConvertFrom-Json -ErrorAction Stop } catch { }
                }
            }
            [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');trace_temp_bytes=[long]$bytes;free_bytes=(Get-Volume C).SizeRemaining;available_memory_kib=$os.FreePhysicalMemory;service_state=$svc.State;service_pid=$svc.ProcessId;native_metrics=$nativeMetrics}
        }
        $guards.Add($guard)
        $guard | ConvertTo-Json -Compress | Add-Content "$out/guards.jsonl" -Encoding UTF8
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
        try { Run-Campaign @('-Action','Stop','-RunId',$RunId); $serviceStopped=$true } catch { $failure="$failure; SCM stop: $_" }
    }
    try { Run-Campaign @('-Action','Fetch','-RunId',$RunId) } catch { $failure="$failure; metrics fetch: $_" }
    if ($traceStarted) {
        try {
            $stopArgs=@('-Action','Stop','-CampaignId',$CampaignId,'-RunId',$RunId)
            if ($CompressTraceTransfer) { $stopArgs+='-CompressTransfer' }
            Run-Helper 'windows-pooled-wpr.ps1' $stopArgs
        } catch { $failure="$failure; WPR stop: $_" }
    }
    if ($countersStarted) {
        try { Run-Helper 'windows-capture.ps1' @('-Action','Clock','-RunId',$RunId,'-EvidenceRoot',"$campaign/capture") -Parent; Run-Helper 'windows-capture.ps1' @('-Action','Stop','-RunId',$RunId,'-EvidenceRoot',"$campaign/capture") -Parent } catch { $failure="$failure; counters stop: $_" }
    }
    Remove-PSSession $session
    [ordered]@{utc=[DateTime]::UtcNow.ToString('o');run_id=$RunId;mode=$Mode;qps=$Qps;duration=$Duration;trace_budget_bytes=$TraceBudgetGiB*1GB;forecast_trace_bytes=$ForecastTraceBytes;compress_trace_transfer=[bool]$CompressTraceTransfer;failure=$failure;guards=$guards.Count;service_stopped=$serviceStopped;trace_started=$traceStarted;counters_started=$countersStarted} | ConvertTo-Json -Depth 4 | Set-Content "$out/completion.json" -Encoding UTF8
}
if ($failure) { throw $failure }
Write-Output "WINDOWS_STAGE_COMPLETE $RunId; receiver final stable marker and all-category ledger gate still required"
