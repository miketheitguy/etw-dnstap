#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Runs one isolated parser-ramp stage: start the service, drive the DNS client, stop the service.

.DESCRIPTION
Requires a fresh matching receiver READY receipt and operator approval (-MatrixApproved).
Records preflight, invocation, per-5-second guard samples, client exit and SCM shutdown
receipts under target/performance/<CampaignId>/stages/<RunId> and refuses to reuse a stage
directory that already holds Windows evidence. The service is always stopped in the finally
block and any failure is re-thrown after the receipts are written.

.PARAMETER Mode
authoritative or recursive workload.
.PARAMETER CampaignId
Campaign directory under target/performance.
.PARAMETER RunId
Stage run ID.
.PARAMETER Qps
Global query rate (1000-128000).
.PARAMETER Duration
Load duration in seconds (60 or 300).
.PARAMETER ReceiverReadyReceipt
READY receipt written by the Ubuntu receiver for this run.
.PARAMETER ExpectedHash
SHA-256 of the candidate service binary that must be installed.
.PARAMETER CapturePolicy
all_supported (default) or client_only.
.PARAMETER MatrixApproved
Operator approval of the run matrix.

.OUTPUTS
preflight.json, invocation.json, guards.jsonl, client/, service-stop.json, completion.json.
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
    [ValidateSet('all_supported','client_only')][string]$CapturePolicy='all_supported',
    [Parameter(Mandatory)][switch]$MatrixApproved
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
Import-Module (Join-Path $PSScriptRoot 'LabCommon.psm1') -Force
$config=Get-LabConfig
$root=Get-LabRoot
$campaign=Resolve-LabPath "$($config.paths.evidence_root)/$CampaignId"
$out=Join-Path $campaign "stages/$RunId"
if (-not $MatrixApproved -or -not (Test-Path $ReceiverReadyReceipt)) { throw 'Approved matrix and receiver start receipt required' }
$ready=Get-Content -LiteralPath $ReceiverReadyReceipt -Raw -Encoding UTF8 | ConvertFrom-Json
if ($ready.run -ne $RunId -or $ready.mode -ne $Mode -or $ready.configured_global_qps -ne $Qps -or $ready.duration_seconds -ne $Duration -or $ready.receiver_ready -ne $true -or $ready.start_marker_ready -ne $true -or [DateTime]::Parse($ready.ready_utc).ToUniversalTime() -lt [DateTime]::UtcNow.AddMinutes(-10)) { throw 'Fresh matching receiver READY/start marker required' }
if ($CapturePolicy -eq 'client_only') {
    $readyCampaign = $ready.PSObject.Properties['campaign_id']
    $readyPolicy = $ready.PSObject.Properties['capture_policy']
    if (-not $readyCampaign -or -not $readyPolicy -or $readyCampaign.Value -ne $CampaignId -or $readyPolicy.Value -ne $CapturePolicy) { throw 'Receiver campaign/capture policy mismatch' }
}
if (Test-Path $out) {
    $existing=@(Get-ChildItem -LiteralPath $out -Force | Where-Object Name -notin @('receiver','receiver-ready.json','start-marker.json'))
    if ($existing.Count) { throw 'Preserve prior Windows stage evidence' }
}
$vm=Get-LabVm -RequireRunning -CheckResources -RequireFixedMemory
if ((Get-PSDrive C).Free -lt 20GB) { throw 'Host disk preflight guard' }
$session=Get-LabSession -Vm $vm
$client=$null
$started=$false
$failure=$null
New-Item -ItemType Directory -Path $out -Force | Out-Null
if ([IO.Path]::GetFullPath((Resolve-Path -LiteralPath $ReceiverReadyReceipt).Path) -ne [IO.Path]::GetFullPath((Join-Path $out 'receiver-ready.json'))) { Copy-Item -LiteralPath $ReceiverReadyReceipt -Destination "$out/receiver-ready.json" }
try {
    $pre=Invoke-LabGuest -Session $session -ArgumentList $ExpectedHash,$config.windows_vm.install_exe,$config.windows_vm.service_name -ScriptBlock {
        param($expected,$installed,$serviceName)
        $svc=Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
        if ($svc.State -ne 'Stopped' -or $svc.StartName -ne 'LocalSystem' -or $svc.PathName -match '--metrics|--output|--foreground') { throw 'Service preflight mismatch' }
        if ((Get-FileHash $installed).Hash -ne $expected) { throw 'Candidate mismatch' }
        if ((Get-Volume C).SizeRemaining -lt 8GB -or (Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory -lt 1MB) { throw 'Guest disk/memory preflight guard' }
        $ip=@(Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.IPAddress -ne '127.0.0.1' -and $_.IPAddress -notlike '169.254.*' })
        if ($ip.Count -ne 1) { throw 'Ambiguous guest IPv4' }
        @{ip=$ip[0].IPAddress;service_arguments=$svc.PathName;free_bytes=(Get-Volume C).SizeRemaining}
    }
    $pre.campaign_id=$CampaignId
    $pre.capture_policy=$CapturePolicy
    $pre.expected_binary_sha256=$ExpectedHash
    if ($CapturePolicy -eq 'client_only') { $pre.source_filter=@{provider_event_ids=@(256,257);level=4;keyword_mask='0x8000000000000000'} }
    Write-LabJson -Path "$out/preflight.json" -InputObject $pre
    $exe=Resolve-LabPath $config.paths.dns_stress_exe
    $names=Resolve-LabPath ($config.paths.workload_dir+'/'+$(if ($Mode -eq 'authoritative') {'auth-names.txt'} else {'rec-names.txt'}))
    if (@(Get-Content $names | Where-Object { $_ -and $_ -notmatch '^#' }).Count -ne 5000) { throw 'Expected 5000-name rotation' }
    $clientOut=Join-Path $out 'client'
    New-Item -ItemType Directory -Path $clientOut | Out-Null
    $arguments=@('--server',"$($pre.ip):53",'--names',$names,'--mode',$Mode,'--threads','2','--duration',"$Duration",'--inflight','256','--timeout-ms','2000','--qps',"$Qps",'--output-dir',$clientOut)
    @{command=$exe;arguments=$arguments;exe_sha256=(Get-FileHash $exe).Hash;names_sha256=(Get-FileHash $names).Hash;host_ipv4=@(Get-NetIPAddress -AddressFamily IPv4 | Select-Object IPAddress,InterfaceAlias);host_start_utc=[DateTime]::UtcNow.ToString('o')} | ConvertTo-Json -Depth 5 | Set-Content "$out/invocation.json" -Encoding UTF8
    $started=$true
    Invoke-LabGuest -Session $session -ArgumentList $config.windows_vm.service_name -ScriptBlock { param($serviceName) Start-Service $serviceName; if ((Get-Service $serviceName).Status -ne 'Running') { throw 'Service failed to start' } }
    $client=Start-Process -FilePath $exe -ArgumentList (Join-LabArguments -Arguments $arguments) -WindowStyle Hidden -PassThru -RedirectStandardOutput "$clientOut/stdout.txt" -RedirectStandardError "$clientOut/stderr.txt"
    # Reading .Handle keeps the exit code available in Windows PowerShell 5.1.
    $handle=$client.Handle
    $deadline=[DateTime]::UtcNow.AddSeconds($Duration+30)
    while (-not $client.HasExited) {
        if ([DateTime]::UtcNow -gt $deadline -or (Get-PSDrive C).Free -lt 20GB) { throw 'Client deadline/host disk guard' }
        $guard=Invoke-LabGuest -Session $session -ArgumentList $config.windows_vm.service_name -ScriptBlock {
            param($serviceName)
            $svc=Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
            $proc=Get-Process -Id $svc.ProcessId
            @{utc=[DateTime]::UtcNow.ToString('o');service_state=$svc.State;pid=$svc.ProcessId;free_bytes=(Get-Volume C).SizeRemaining;available_memory_kib=(Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory;cpu_seconds=$proc.TotalProcessorTime.TotalSeconds;private_bytes=$proc.PrivateMemorySize64;working_set=$proc.WorkingSet64;handles=$proc.HandleCount;threads=$proc.Threads.Count}
        }
        Write-LabText -Path "$out/guards.jsonl" -Text (($guard | ConvertTo-Json -Compress) + [Environment]::NewLine) -Append
        if ($guard.service_state -ne 'Running' -or $guard.free_bytes -lt 8GB -or $guard.available_memory_kib -lt 1MB) { throw 'Guest resource/service guard' }
        @{utc=[DateTime]::UtcNow.ToString('o');cpu_seconds=$client.TotalProcessorTime.TotalSeconds;private_bytes=$client.PrivateMemorySize64;working_set=$client.WorkingSet64;udp_endpoints=@(Get-NetUDPEndpoint -OwningProcess $client.Id -ErrorAction SilentlyContinue | Select-Object LocalAddress,LocalPort)} | ConvertTo-Json -Depth 4 -Compress | Add-Content "$out/client-process.jsonl" -Encoding UTF8
        Start-Sleep -Seconds 5
        $client.Refresh()
    }
    $client.WaitForExit()
    @{exit_code=$client.ExitCode;end_utc=[DateTime]::UtcNow.ToString('o');pid=$client.Id} | ConvertTo-Json | Set-Content "$out/client-exit.json" -Encoding UTF8
    if ($client.ExitCode -ne 0) { throw "Client failed: $($client.ExitCode)" }
    Start-Sleep -Seconds 3
} catch {
    $failure=$_.ToString()
    if ($client -and -not $client.HasExited) { Stop-Process -Id $client.Id -ErrorAction SilentlyContinue }
} finally {
    if ($started) {
        try {
            $stop=Invoke-LabGuest -Session $session -ArgumentList $config.windows_vm.service_name -ScriptBlock { param($serviceName) Stop-Service $serviceName; Get-CimInstance Win32_Service -Filter "Name='$serviceName'" | Select-Object Name,State,ExitCode,ServiceSpecificExitCode,ProcessId }
            $stop | ConvertTo-Json | Set-Content "$out/service-stop.json" -Encoding UTF8
            if ($stop.State -ne 'Stopped' -or $stop.ExitCode -ne 0) {
                $events=Invoke-LabGuest -Session $session -ArgumentList $config.windows_vm.service_name -ScriptBlock {
                    param($serviceName)
                    Get-WinEvent -LogName Application -MaxEvents 2000 | Where-Object ProviderName -eq $serviceName | Select-Object -First 8 TimeCreated,Id,LevelDisplayName,ProviderName,Message
                }
                $events | ConvertTo-Json -Depth 4 | Set-Content "$out/service-error-events.json" -Encoding UTF8
                throw 'SCM shutdown not clean'
            }
        } catch { $failure="$failure; SCM stop: $_" }
    }
    Remove-PSSession $session
    @{utc=[DateTime]::UtcNow.ToString('o');campaign_id=$CampaignId;expected_sha256=$ExpectedHash;run=$RunId;mode=$Mode;qps=$Qps;duration_seconds=$Duration;failure=$failure;service_started=$started;collector_end_marker_pending=$true} | ConvertTo-Json | Set-Content "$out/completion.json" -Encoding UTF8
}
if ($failure) { throw $failure }
Write-Output "WINDOWS_CLIENT_COMPLETE_AND_SCM_STOPPED $RunId; receiver drain plateau/end marker/count reconciliation required"
