#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Proves the Windows VM returned to its pre-campaign service, DNS and tracing state.

.DESCRIPTION
Compares the guest against the saved baseline (service-original.json and the recursion,
cache and forwarder XML files) and against the initial inventory, then writes
windows-campaign/final-policy-proof.json (no overwrite). Historical pooled-VM helper: the
dated campaign directory that used to be baked in is now the mandatory -CampaignId and the
retained binary hash is the mandatory -ExpectedHash.

.PARAMETER CampaignId
Campaign directory under target/performance.
.PARAMETER ExpectedHash
SHA-256 of the binary that must remain installed.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$CampaignId,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ExpectedHash
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '../LabCommon.psm1') -Force
$config = Get-LabConfig
$campaign = Resolve-LabPath "$($config.paths.evidence_root)/$CampaignId"
$out = Join-Path $campaign 'windows-campaign/final-policy-proof.json'
if (Test-Path -LiteralPath $out) { throw 'Preserve existing final proof' }
$baseline = Get-Content (Join-Path $campaign 'windows-campaign/inventory.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$vm = Get-LabVm -RequireRunning -CheckResources
$session = Get-LabSession -Vm $vm
try {
    $proof = Invoke-LabGuest -Session $session -ArgumentList "$($config.windows_vm.data_dir)/$CampaignId", $ExpectedHash, $config.windows_vm.service_name, $config.windows_vm.install_exe, $config.dns.auth_zone, $config.dns.rec_zone -ScriptBlock {
        param($base,$expectedHash,$serviceName,$installed,$authZone,$recZone)
        $original = Get-Content "$base/service-original.json" -Raw | ConvertFrom-Json
        $service = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
        if ($service.State -ne 'Running' -or $service.StartName -ne 'LocalSystem' -or $service.PathName -ne $original.PathName -or $service.PathName -match '--metrics|--capacity') { throw 'Normal running SCM configuration was not restored' }
        $hash = (Get-FileHash $installed).Hash
        if ($hash -ne $expectedHash) { throw 'Unexpected retained binary' }
        if (@(Get-DnsServerZone | Where-Object ZoneName -eq $recZone).Count) { throw 'Owned conditional forwarder remains' }
        $settings = @{}
        foreach ($item in @(
            @{name='recursion';before=Import-Clixml "$base/recursion-original.xml";after=Get-DnsServerRecursion;properties=@('AdditionalTimeout','Enable','RetryInterval','SecureResponse','Timeout')},
            @{name='cache';before=Import-Clixml "$base/cache-original.xml";after=Get-DnsServerCache;properties=@('LockingPercent','EnablePollutionProtection','IgnorePolicies','MaxKBSize','MaxNegativeTtl','MaxTtl','StoreEmptyAuthenticationResponse')},
            @{name='forwarder';before=Import-Clixml "$base/forwarders-original.xml";after=Get-DnsServerForwarder;properties=@('IPAddress','ReorderingEnable','Timeout','UseRootHint')}
        )) {
            $values = @{}
            foreach ($property in $item.properties) {
                $before = @($item.before.$property | ForEach-Object ToString) -join ','
                $after = @($item.after.$property | ForEach-Object ToString) -join ','
                if ($before -ne $after) { throw "DNS setting changed: $($item.name).$property" }
                $values[$property] = @{before=$before;after=$after;equal=$true}
            }
            $settings[$item.name] = $values
        }
        $wpr = (& wpr.exe -status 2>&1 | Out-String)
        if ($wpr -notmatch 'WPR is not recording') { throw 'Unexpected active WPR recording' }
        $logman = (& logman.exe query | Out-String)
        if ($logman -match 'pooled-|Running') { throw 'A task-owned or active counter collector remains' }
        [ordered]@{utc=[DateTime]::UtcNow.ToString('o');service=$service | Select-Object Name,State,ProcessId,PathName,StartName,ExitCode;binary_sha256=$hash;executable_sddl=(Get-Acl $installed).Sddl;scm_sddl=(& sc.exe sdshow $serviceName | Out-String);sidtype=(& sc.exe qsidtype $serviceName | Out-String);privileges=(& sc.exe qprivs $serviceName | Out-String);dns_settings=$settings;authoritative_a_records=@(Get-DnsServerResourceRecord -ZoneName $authZone -RRType A).Count;rec_zone_absent=$true;wpr_status=$wpr;logman=$logman;guest_free_bytes=(Get-Volume C).SizeRemaining;ip=@(Get-NetIPAddress -AddressFamily IPv4 | Select-Object IPAddress,InterfaceAlias)}
    }
} finally { Remove-PSSession $session }
foreach ($property in 'executable_sddl','scm_sddl','sidtype','privileges') {
    if ([string]$proof.$property -ne [string]$baseline.guest.$property) { throw "Service security baseline changed: $property" }
}
if ($proof.authoritative_a_records -ne [int]$config.dns.auth_record_count) { throw 'Authoritative fixture count changed' }
Write-LabJson -Path $out -InputObject ([ordered]@{vm=$vm | Select-Object Name,Id,State,ProcessorCount,MemoryAssigned;security_matches_initial_inventory=$true;guest=$proof}) -Depth 9
Get-Content -LiteralPath $out -Raw
