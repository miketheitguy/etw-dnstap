[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
$root=(Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$campaign=Join-Path $root 'target/performance/pooled-vm-20261029-0942'
$out=Join-Path $campaign 'windows-campaign/final-policy-proof.json'
if (Test-Path -LiteralPath $out) { throw 'Preserve existing final proof' }
$baseline=Get-Content (Join-Path $campaign 'windows-campaign/inventory.json') -Raw | ConvertFrom-Json
$vm=Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac' -or $vm.State -ne 'Running' -or $vm.ProcessorCount -ne 2 -or $vm.MemoryAssigned -ne 4GB) { throw 'VM identity/resources/state changed' }
$credential=Import-Clixml (Join-Path $root 'target/lab-windows/private/administrator.clixml')
$proof=Invoke-Command -VMId $vm.Id -Credential $credential -ScriptBlock {
    $base='C:/ProgramData/EtwDnstap/performance/pooled-vm-20261029-0942'
    $original=Get-Content "$base/service-original.json" -Raw | ConvertFrom-Json
    $service=Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"
    if ($service.State -ne 'Running' -or $service.StartName -ne 'LocalSystem' -or $service.PathName -ne $original.PathName -or $service.PathName -match '--metrics|--capacity') { throw 'Normal running SCM configuration was not restored' }
    $hash=(Get-FileHash 'C:/Program Files/EtwDnstap/etw-dnstap.exe').Hash
    if ($hash -ne '98FDAB43AA24AE1BE31A7AABCCBDD67A2F64C87DC1C50C428B638B3C091F5D7C') { throw 'Unexpected retained binary' }
    if (@(Get-DnsServerZone | Where-Object ZoneName -eq 'rec.stress.test').Count) { throw 'Owned conditional forwarder remains' }
    $settings=@{}
    foreach ($item in @(
        @{name='recursion';before=Import-Clixml "$base/recursion-original.xml";after=Get-DnsServerRecursion;properties=@('AdditionalTimeout','Enable','RetryInterval','SecureResponse','Timeout')},
        @{name='cache';before=Import-Clixml "$base/cache-original.xml";after=Get-DnsServerCache;properties=@('LockingPercent','EnablePollutionProtection','IgnorePolicies','MaxKBSize','MaxNegativeTtl','MaxTtl','StoreEmptyAuthenticationResponse')},
        @{name='forwarder';before=Import-Clixml "$base/forwarders-original.xml";after=Get-DnsServerForwarder;properties=@('IPAddress','ReorderingEnable','Timeout','UseRootHint')}
    )) {
        $values=@{}
        foreach ($property in $item.properties) {
            $before=@($item.before.$property | ForEach-Object ToString) -join ','
            $after=@($item.after.$property | ForEach-Object ToString) -join ','
            if ($before -ne $after) { throw "DNS setting changed: $($item.name).$property" }
            $values[$property]=@{before=$before;after=$after;equal=$true}
        }
        $settings[$item.name]=$values
    }
    $wpr=(& wpr.exe -status 2>&1 | Out-String)
    if ($wpr -notmatch 'WPR is not recording') { throw 'Unexpected active WPR recording' }
    $logman=(& logman.exe query | Out-String)
    if ($logman -match 'pooled-|Running') { throw 'A task-owned or active counter collector remains' }
    [ordered]@{utc=[DateTime]::UtcNow.ToString('o');service=$service | Select-Object Name,State,ProcessId,PathName,StartName,ExitCode;binary_sha256=$hash;executable_sddl=(Get-Acl 'C:/Program Files/EtwDnstap/etw-dnstap.exe').Sddl;scm_sddl=(& sc.exe sdshow EtwDnstap | Out-String);sidtype=(& sc.exe qsidtype EtwDnstap | Out-String);privileges=(& sc.exe qprivs EtwDnstap | Out-String);dns_settings=$settings;authoritative_a_records=@(Get-DnsServerResourceRecord -ZoneName auth.stress.test -RRType A).Count;rec_zone_absent=$true;wpr_status=$wpr;logman=$logman;guest_free_bytes=(Get-Volume C).SizeRemaining;ip=@(Get-NetIPAddress -AddressFamily IPv4 | Select-Object IPAddress,InterfaceAlias)}
}
foreach ($property in 'executable_sddl','scm_sddl','sidtype','privileges') {
    if ([string]$proof.$property -ne [string]$baseline.guest.$property) { throw "Service security baseline changed: $property" }
}
if ($proof.authoritative_a_records -ne 5000) { throw 'Authoritative fixture count changed' }
[ordered]@{vm=$vm | Select-Object Name,Id,State,ProcessorCount,MemoryAssigned;security_matches_initial_inventory=$true;guest=$proof} | ConvertTo-Json -Depth 9 | Set-Content -LiteralPath $out -Encoding utf8
Get-Content -LiteralPath $out -Raw
