#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Resolves the lab smoke-test names against the guest DNS server and checks the answers.

.DESCRIPTION
Queries www/bills in the authoritative and recursive lab zones three times each from inside
the guest, requires the recursive answers to carry TTL 0 and the authoritative zone to hold
the expected record count, then writes <ProofName>.json under the evidence root.

.PARAMETER ProofName
Output file name without extension (default smoke-query-proof).
.PARAMETER EvidenceRoot
Repository-relative evidence directory (default target/performance/windows); must stay inside the repository.

.OUTPUTS
<EvidenceRoot>/<ProofName>.json
#>
[CmdletBinding()]
param(
    [ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$ProofName = 'smoke-query-proof',
    [string]$EvidenceRoot = 'target/performance/windows'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'LabCommon.psm1') -Force
$config = Get-LabConfig
$evidence = Resolve-LabPath $EvidenceRoot
New-Item -ItemType Directory -Path $evidence -Force | Out-Null
$vm = Get-LabVm
$session = Get-LabSession -Vm $vm
try {
    $result = Invoke-LabGuest -Session $session -ArgumentList $config.dns.auth_zone, $config.dns.rec_zone, $config.dns.answer_ip, ([int]$config.dns.auth_record_count) -ScriptBlock {
        param($authZone,$recZone,$answerIp,$authCount)
        $rows = @()
        foreach ($zone in $authZone, $recZone) {
            $mode = $zone.Split('.')[0]
            foreach ($label in 'www','bills') {
                foreach ($attempt in 1..3) {
                    $answer = Resolve-DnsName -Name "$label.$zone" -Server 127.0.0.1 -Type A -DnsOnly
                    $rows += $answer | Select-Object Name,Type,TTL,IPAddress
                    if ($mode -eq 'rec' -and @($answer | Where-Object { $_.Type -eq 'A' -and $_.TTL -eq 0 -and $_.IPAddress -eq $answerIp }).Count -ne 1) { throw 'Recursive TTL0 answer proof failed' }
                }
            }
        }
        $aCount = @(Get-DnsServerResourceRecord -ZoneName $authZone -RRType A).Count
        if ($aCount -ne $authCount) { throw "Expected $authCount authoritative A records, found $aCount" }
        $rows
    }
} finally { Remove-PSSession $session }
Write-LabJson -Path (Join-Path $evidence "$ProofName.json") -InputObject $result -Depth 5 -Force
$result | Format-Table Name,Type,TTL,IPAddress
