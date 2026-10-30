[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('authoritative','recursive')][string]$Mode,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId,
    [Parameter(Mandatory)][ValidateRange(1000,128000)][int]$Qps,
    [Parameter(Mandatory)][ValidateSet(60,300)][int]$Duration,
    [Parameter(Mandatory)][string]$ReceiverReadyReceipt,
    [ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$CampaignId='parser-ramp-20261030',
    [ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ExpectedHash='487FFD782B730B7E1A849626AF68BCB6C8A0EF2057ACB7D8E5D932B7EA49A1AE',
    [ValidateSet('all_supported','client_only')][string]$CapturePolicy='all_supported',
    [Parameter(Mandatory)][switch]$MatrixApproved
)
$ErrorActionPreference='Stop'
$root=(Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$campaign=Join-Path $root "target/performance/$CampaignId"
$out=Join-Path $campaign "stages/$RunId"
if (-not $MatrixApproved -or -not (Test-Path $ReceiverReadyReceipt)) { throw 'Approved matrix and receiver start receipt required' }
$ready=Get-Content $ReceiverReadyReceipt -Raw | ConvertFrom-Json
if ($ready.run -ne $RunId -or $ready.mode -ne $Mode -or $ready.configured_global_qps -ne $Qps -or $ready.duration_seconds -ne $Duration -or $ready.receiver_ready -ne $true -or $ready.start_marker_ready -ne $true -or [DateTime]::Parse($ready.ready_utc).ToUniversalTime() -lt [DateTime]::UtcNow.AddMinutes(-10)) { throw 'Fresh matching receiver READY/start marker required' }
if ($CapturePolicy -eq 'client_only' -and ($ready.campaign_id -ne $CampaignId -or $ready.capture_policy -ne $CapturePolicy)) { throw 'Receiver campaign/capture policy mismatch' }
if (Test-Path $out) {
    $existing=@(Get-ChildItem -LiteralPath $out -Force | Where-Object Name -notin @('receiver','receiver-ready.json','start-marker.json'))
    if ($existing.Count) { throw 'Preserve prior Windows stage evidence' }
}
$vm=Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac' -or $vm.State -ne 'Running' -or $vm.ProcessorCount -ne 2 -or $vm.MemoryAssigned -ne 4GB -or $vm.DynamicMemoryEnabled) { throw 'VM identity/resources/state mismatch' }
if ((Get-PSDrive C).Free -lt 20GB) { throw 'Host disk preflight guard' }
$credential=Import-Clixml (Join-Path $root 'target/lab-windows/private/administrator.clixml')
$session=New-PSSession -VMId $vm.Id -Credential $credential
$client=$null
$started=$false
$failure=$null
New-Item -ItemType Directory -Path $out -Force | Out-Null
if ([IO.Path]::GetFullPath((Resolve-Path $ReceiverReadyReceipt).Path) -ne [IO.Path]::GetFullPath((Join-Path $out 'receiver-ready.json'))) { Copy-Item $ReceiverReadyReceipt "$out/receiver-ready.json" }
try {
    $pre=Invoke-Command -Session $session -ArgumentList $ExpectedHash -ScriptBlock {
        param($expected)
        $svc=Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"
        if ($svc.State -ne 'Stopped' -or $svc.StartName -ne 'LocalSystem' -or $svc.PathName -match '--metrics|--output|--foreground') { throw 'Service preflight mismatch' }
        if ((Get-FileHash 'C:/Program Files/EtwDnstap/etw-dnstap.exe').Hash -ne $expected) { throw 'Candidate mismatch' }
        if ((Get-Volume C).SizeRemaining -lt 8GB -or (Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory -lt 1MB) { throw 'Guest disk/memory preflight guard' }
        $ip=@(Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.IPAddress -ne '127.0.0.1' -and $_.IPAddress -notlike '169.254.*' })
        if ($ip.Count -ne 1) { throw 'Ambiguous guest IPv4' }
        @{ip=$ip[0].IPAddress;service_arguments=$svc.PathName;free_bytes=(Get-Volume C).SizeRemaining}
    }
    $pre.campaign_id=$CampaignId
    $pre.capture_policy=$CapturePolicy
    $pre.expected_binary_sha256=$ExpectedHash
    if ($CapturePolicy -eq 'client_only') { $pre.source_filter=@{provider_event_ids=@(256,257);level=4;keyword_mask='0x8000000000000000'} }
    $pre | ConvertTo-Json | Set-Content "$out/preflight.json"
    $exe=Join-Path $root 'tools/dns-stress/target/x86_64-pc-windows-msvc/release/dns-stress.exe'
    $names=Join-Path $root ('target/performance/workload/'+$(if ($Mode -eq 'authoritative') {'auth-names.txt'} else {'rec-names.txt'}))
    if (@(Get-Content $names | Where-Object { $_ -and $_ -notmatch '^#' }).Count -ne 5000) { throw 'Expected 5000-name rotation' }
    $clientOut=Join-Path $out 'client'
    New-Item -ItemType Directory -Path $clientOut | Out-Null
    $arguments=@('--server',"$($pre.ip):53",'--names',('"'+$names+'"'),'--mode',$Mode,'--threads','2','--duration',"$Duration",'--inflight','256','--timeout-ms','2000','--qps',"$Qps",'--output-dir',('"'+$clientOut+'"'))
    @{command=$exe;arguments=$arguments;exe_sha256=(Get-FileHash $exe).Hash;names_sha256=(Get-FileHash $names).Hash;host_ipv4=@(Get-NetIPAddress -AddressFamily IPv4 | Select-Object IPAddress,InterfaceAlias);host_start_utc=[DateTime]::UtcNow.ToString('o')} | ConvertTo-Json -Depth 5 | Set-Content "$out/invocation.json"
    $started=$true
    Invoke-Command -Session $session -ScriptBlock { Start-Service EtwDnstap; if ((Get-Service EtwDnstap).Status -ne 'Running') { throw 'Service failed to start' } }
    $client=Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput "$clientOut/stdout.txt" -RedirectStandardError "$clientOut/stderr.txt"
    $handle=$client.Handle
    $deadline=[DateTime]::UtcNow.AddSeconds($Duration+30)
    while (-not $client.HasExited) {
        if ([DateTime]::UtcNow -gt $deadline -or (Get-PSDrive C).Free -lt 20GB) { throw 'Client deadline/host disk guard' }
        $guard=Invoke-Command -Session $session -ScriptBlock {
            $svc=Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"
            $proc=Get-Process -Id $svc.ProcessId
            @{utc=[DateTime]::UtcNow.ToString('o');service_state=$svc.State;pid=$svc.ProcessId;free_bytes=(Get-Volume C).SizeRemaining;available_memory_kib=(Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory;cpu_seconds=$proc.TotalProcessorTime.TotalSeconds;private_bytes=$proc.PrivateMemorySize64;working_set=$proc.WorkingSet64;handles=$proc.HandleCount;threads=$proc.Threads.Count}
        }
        $guard | ConvertTo-Json -Compress | Add-Content "$out/guards.jsonl"
        if ($guard.service_state -ne 'Running' -or $guard.free_bytes -lt 8GB -or $guard.available_memory_kib -lt 1MB) { throw 'Guest resource/service guard' }
        @{utc=[DateTime]::UtcNow.ToString('o');cpu_seconds=$client.TotalProcessorTime.TotalSeconds;private_bytes=$client.PrivateMemorySize64;working_set=$client.WorkingSet64;udp_endpoints=@(Get-NetUDPEndpoint -OwningProcess $client.Id -ErrorAction SilentlyContinue | Select-Object LocalAddress,LocalPort)} | ConvertTo-Json -Depth 4 -Compress | Add-Content "$out/client-process.jsonl"
        Start-Sleep -Seconds 5
        $client.Refresh()
    }
    $client.WaitForExit()
    @{exit_code=$client.ExitCode;end_utc=[DateTime]::UtcNow.ToString('o');pid=$client.Id} | ConvertTo-Json | Set-Content "$out/client-exit.json"
    if ($client.ExitCode -ne 0) { throw "Client failed: $($client.ExitCode)" }
    Start-Sleep -Seconds 3
} catch {
    $failure=$_.ToString()
    if ($client -and -not $client.HasExited) { Stop-Process -Id $client.Id -ErrorAction SilentlyContinue }
} finally {
    if ($started) {
        try {
            $stop=Invoke-Command -Session $session -ScriptBlock { Stop-Service EtwDnstap; Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'" | Select-Object Name,State,ExitCode,ServiceSpecificExitCode,ProcessId }
            $stop | ConvertTo-Json | Set-Content "$out/service-stop.json"
            if ($stop.State -ne 'Stopped' -or $stop.ExitCode -ne 0) {
                $events=Invoke-Command -Session $session -ScriptBlock {
                    Get-WinEvent -LogName Application -MaxEvents 2000 | Where-Object ProviderName -eq 'EtwDnstap' | Select-Object -First 8 TimeCreated,Id,LevelDisplayName,ProviderName,Message
                }
                $events | ConvertTo-Json -Depth 4 | Set-Content "$out/service-error-events.json"
                throw 'SCM shutdown not clean'
            }
        } catch { $failure="$failure; SCM stop: $_" }
    }
    Remove-PSSession $session
    @{utc=[DateTime]::UtcNow.ToString('o');campaign_id=$CampaignId;expected_sha256=$ExpectedHash;run=$RunId;mode=$Mode;qps=$Qps;duration_seconds=$Duration;failure=$failure;service_started=$started;collector_end_marker_pending=$true} | ConvertTo-Json | Set-Content "$out/completion.json"
}
if ($failure) { throw $failure }
Write-Output "WINDOWS_CLIENT_COMPLETE_AND_SCM_STOPPED $RunId; receiver drain plateau/end marker/count reconciliation required"
