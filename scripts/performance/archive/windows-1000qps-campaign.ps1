#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Historical 1000 QPS service-metrics campaign controller (cleanup and evidence actions only).

.DESCRIPTION
Inventory/Prepare/Restore/Proof/Fetch for a dated campaign whose run ID used to be baked
into this script. Prepare is disabled because the TCP-only service no longer supports JSONL
metrics. Restore removes the temporary recursive forwarder only when it is recorded in
owned-zones.txt (the legacy owned-rec-forwarder.txt marker is also honoured).

.PARAMETER Action
Inventory, Prepare (disabled), Restore, Proof or Fetch.
.PARAMETER CampaignId
Campaign directory name. Guest: <data_dir>/<CampaignId>; host: target/performance/windows/<CampaignId>.
Prepare refuses to reuse an existing host directory.
.PARAMETER ExpectedHash
SHA-256 of the service binary that must be installed.

.OUTPUTS
<Action>.json receipts plus hash-verified copies of the guest evidence files on Fetch/Restore/Proof.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Inventory','Prepare','Restore','Proof','Fetch')][string]$Action,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$CampaignId,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ExpectedHash
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '../LabCommon.psm1') -Force
if ($Action -eq 'Prepare') { throw 'Historical service-metrics harness: new campaigns are disabled because the TCP-only service no longer supports JSONL metrics. Existing evidence and cleanup actions remain available.' }
$config = Get-LabConfig
$out = Resolve-LabPath "$($config.paths.evidence_root)/windows/$CampaignId"
if ($Action -eq 'Prepare' -and (Test-Path -LiteralPath $out)) { throw "Campaign evidence already exists; refusing to reuse CampaignId '$CampaignId': $out" }
New-Item -ItemType Directory -Path $out -Force | Out-Null
$vm = Get-LabVm -RequireRunning -CheckResources
$session = Get-LabSession -Vm $vm
$guestBase = "$($config.windows_vm.data_dir)/$CampaignId"
try {
    if ($Action -eq 'Prepare') {
        Initialize-LabDnsZones -Session $session -BaseDirectory $guestBase -UpstreamAddress $config.network.receiver_ip
    }
    $guestArguments = @($Action, $guestBase, $ExpectedHash, $config.windows_vm.install_exe, $config.windows_vm.service_name,
        $config.dns.auth_zone, $config.dns.rec_zone, [int]$config.dns.auth_record_count, $config.dns.answer_name, $config.dns.answer_ip, [int]$config.dns.answer_ttl)
    $result = Invoke-LabGuest -Session $session -ArgumentList $guestArguments -ScriptBlock {
        param($action,$base,$expected,$installed,$serviceName,$authZone,$recZone,$authCount,$answerName,$answerIp,$answerTtl)
        $service = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
        $hash = (Get-FileHash $installed).Hash
        if ($hash -ne $expected) { throw 'Unexpected service binary' }
        $aCount = @(Get-DnsServerResourceRecord -ZoneName $authZone -RRType A).Count
        if ($aCount -ne $authCount) { throw "Expected retained $authCount-record authoritative zone" }
        if ($action -eq 'Restore') {
            if (-not (Test-Path "$base/service-original.json")) { throw 'Missing current campaign backup' }
            Stop-Service $serviceName -ErrorAction SilentlyContinue
            $original = Get-Content "$base/service-original.json" -Raw | ConvertFrom-Json
            $changed = Invoke-CimMethod -InputObject $service -MethodName Change -Arguments @{PathName=$original.PathName}
            if ($changed.ReturnValue -ne 0) { throw "Restore command line failed: $($changed.ReturnValue)" }
            $null = Remove-LabOwnedZone -Base $base -Zone $recZone
            $before = Import-Clixml "$base/recursion-original.xml"
            $after = Get-DnsServerRecursion
            foreach ($property in 'AdditionalTimeout','Enable','RetryInterval','SecureResponse','Timeout') {
                if ($before.$property -ne $after.$property) { throw "Recursion baseline changed: $property" }
            }
            $after | Select-Object AdditionalTimeout,Enable,RetryInterval,SecureResponse,Timeout | ConvertTo-Json | Set-Content "$base/recursion-final.json" -Encoding UTF8
            Get-DnsServerForwarder | Export-Clixml "$base/forwarders-final.xml"
            & sc.exe qsidtype $serviceName | Set-Content "$base/sidtype-final.txt" -Encoding UTF8
            & sc.exe qprivs $serviceName | Set-Content "$base/privileges-final.txt" -Encoding UTF8
            & wpr.exe -status 2>&1 | Set-Content "$base/wpr-final.txt" -Encoding UTF8
            & logman.exe query | Set-Content "$base/logman-final.txt" -Encoding UTF8
            if ($original.State -eq 'Running') { Start-Service $serviceName }
        }
        if ($action -eq 'Proof') {
            if ($service.State -ne 'Running' -or $service.PathName -match '--metrics') { throw 'Service not restored' }
            if (@(Get-DnsServerZone | Where-Object ZoneName -eq $recZone).Count) { throw 'Temporary recursive forwarder remains' }
            $proof = Test-LabAnswerLocal -Name $answerName -Address $answerIp -Ttl $answerTtl
            if (-not $proof.passed) { throw 'Final authoritative answer failed' }
            $proof.answers | ConvertTo-Json | Set-Content "$base/final-query-proof.json" -Encoding UTF8
        }
        [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');action=$action;service=Get-CimInstance Win32_Service -Filter "Name='$serviceName'" | Select-Object Name,State,ProcessId,PathName,StartName,ExitCode;binary_sha256=$hash;authoritative_a_records=$aCount;ipv4=@(Get-NetIPAddress -AddressFamily IPv4 | Select-Object -ExpandProperty IPAddress)}
    }
    Write-LabJson -Path (Join-Path $out "$Action.json") -InputObject $result -Depth 5 -Force
    if ($Action -in 'Fetch','Restore','Proof') {
        $null = Receive-LabDirectory -Session $session -GuestDirectory $guestBase -Destination $out -ExcludeExtension '.exe' -Overwrite
    }
    $result | Format-List
} finally { Remove-PSSession $session }
