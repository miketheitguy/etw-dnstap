#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Final check that the Windows VM service, zones and answers are back to the normal state.

.DESCRIPTION
Requires the EtwDnstap service running as LocalSystem without --metrics, the authoritative
zone intact, the temporary recursive forwarder gone and the authoritative answer correct.
Writes final-query-proof.json under the evidence root.

.PARAMETER EvidenceRoot
Repository-relative evidence directory (default target/performance/windows).

.OUTPUTS
<EvidenceRoot>/final-query-proof.json
#>
[CmdletBinding()]
param([string]$EvidenceRoot = 'target/performance/windows')
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'LabCommon.psm1') -Force
$config = Get-LabConfig
$evidence = Resolve-LabPath $EvidenceRoot
New-Item -ItemType Directory -Path $evidence -Force | Out-Null
$vm = Get-LabVm
$session = Get-LabSession -Vm $vm
try {
    $result = Invoke-LabGuest -Session $session -ArgumentList $config.windows_vm.service_name, $config.windows_vm.install_exe, $config.dns.auth_zone, $config.dns.rec_zone, ([int]$config.dns.auth_record_count), $config.dns.answer_name, $config.dns.answer_ip, ([int]$config.dns.answer_ttl) -ScriptBlock {
        param($serviceName,$installed,$authZone,$recZone,$authCount,$answerName,$answerIp,$answerTtl)
        $service = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
        if ($service.State -ne 'Running' -or $service.PathName -match '--metrics' -or $service.StartName -ne 'LocalSystem') { throw 'Unexpected final service state' }
        $aCount = @(Get-DnsServerResourceRecord -ZoneName $authZone -RRType A).Count
        if ($aCount -ne $authCount) { throw 'Authoritative zone count changed' }
        if (@(Get-DnsServerZone | Where-Object ZoneName -eq $recZone).Count -ne 0) { throw 'Temporary recursive zone still exists' }
        $proof = Test-LabAnswerLocal -Name $answerName -Address $answerIp -Ttl $answerTtl
        if (-not $proof.passed) { throw 'Final authoritative answer failed' }
        [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');service=$service | Select-Object Name,State,ProcessId,PathName,StartName,ExitCode;AuthoritativeARecords=$aCount;RecursiveForwarderAbsent=$true;Answer=$proof.answers;Binary=(Get-FileHash $installed).Hash}
    }
} finally { Remove-PSSession $session }
Write-LabJson -Path (Join-Path $evidence 'final-query-proof.json') -InputObject $result -Depth 5 -Force
$result | Format-List
