#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Records the baseline Windows VM, service, WPR and DNS inventory before a campaign.

.DESCRIPTION
Writes windows-campaign/inventory.json under the campaign directory; refuses to overwrite
an existing inventory. Historical pooled-VM helper: the dated campaign directory that used
to be baked in is now the mandatory -CampaignId.

.PARAMETER CampaignId
Campaign directory under target/performance.
#>
[CmdletBinding()]
param([Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$CampaignId)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '../LabCommon.psm1') -Force
$config = Get-LabConfig
$out = Resolve-LabPath "$($config.paths.evidence_root)/$CampaignId/windows-campaign/inventory.json"
if (Test-Path -LiteralPath $out) { throw 'Inventory exists' }
New-Item -ItemType Directory -Force -Path (Split-Path $out) | Out-Null
$vm = Get-LabVm -RequireRunning -CheckResources
$session = Get-LabSession -Vm $vm
try {
    $guest = Invoke-LabGuest -Session $session -ArgumentList $config.windows_vm.service_name, $config.windows_vm.install_exe, $config.dns.auth_zone, $config.dns.rec_zone -ScriptBlock {
        param($serviceName,$installed,$authZone,$recZone)
        $svc = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
        $volume = Get-Volume -DriveLetter C
        if ($volume.SizeRemaining -lt 8GB) { throw 'Guest disk guard: less than 8 GiB free' }
        [ordered]@{
            utc=[DateTime]::UtcNow.ToString('o')
            service=$svc | Select-Object Name,State,ProcessId,PathName,StartName
            ip=@(Get-NetIPAddress -AddressFamily IPv4 | Select-Object IPAddress,InterfaceAlias)
            volume=$volume | Select-Object Size,SizeRemaining
            binary_sha256=(Get-FileHash $installed).Hash
            executable_sddl=(Get-Acl $installed).Sddl
            wpr_path=(Get-Command wpr.exe).Source
            wpr_status=(& wpr.exe -status 2>&1 | Out-String)
            wpr_profiles=(& wpr.exe -profiles 2>&1 | Out-String)
            sidtype=(& sc.exe qsidtype $serviceName | Out-String)
            privileges=(& sc.exe qprivs $serviceName | Out-String)
            scm_sddl=(& sc.exe sdshow $serviceName | Out-String)
            authoritative_records=@(Get-DnsServerResourceRecord -ZoneName $authZone -RRType A).Count
            rec_zone=@(Get-DnsServerZone | Where-Object ZoneName -eq $recZone).Count
            logman=(& logman.exe query | Out-String)
        }
    }
} finally { Remove-PSSession $session }
$result = [ordered]@{vm=$vm | Select-Object Name,Id,State,ProcessorCount,MemoryAssigned;host_volumes=@(Get-PSDrive C,D | Select-Object Name,Free,Used);guest=$guest}
Write-LabJson -Path $out -InputObject $result -Depth 8
Get-Content -LiteralPath $out -Raw
