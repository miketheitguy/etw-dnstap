#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Watches guest free space during a stage and stops the EtwDnstap service if a guard trips.

.DESCRIPTION
Appends one JSON line every 5 seconds to conservative-size-watch.jsonl; stops the service when free space drops by 4 GiB from the baseline or below 8 GiB.
Historical pooled-VM campaign helper: the dated campaign directory that used to be baked
in is now the mandatory -CampaignId.
.PARAMETER CampaignId
Campaign directory under target/performance.
.PARAMETER RunId
Stage run ID.
.PARAMETER BaselineFreeBytes
Guest free bytes before the run.
.PARAMETER Seconds
Maximum watch duration (default 360).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$CampaignId,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId,
    [Parameter(Mandatory)][long]$BaselineFreeBytes,
    [ValidateRange(1,600)][int]$Seconds = 360
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '../LabCommon.psm1') -Force
$config = Get-LabConfig
$vm = Get-LabVm
$path = Resolve-LabPath "$($config.paths.evidence_root)/$CampaignId/stages/$RunId/conservative-size-watch.jsonl"
if (Test-Path -LiteralPath $path) { throw 'Preserve watcher evidence' }
$session = Get-LabSession -Vm $vm
try {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $guard = Invoke-LabGuest -Session $session -ArgumentList $BaselineFreeBytes, $config.windows_vm.service_name -ScriptBlock {
            param($baseline,$serviceName)
            $svc = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
            $free = (Get-Volume C).SizeRemaining
            $delta = $baseline - $free
            $tripped = $delta -ge 4GB -or $free -lt 8GB
            if ($tripped -and $svc.State -eq 'Running') { Stop-Service $serviceName }
            [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');free_bytes=$free;free_decrease_bytes=$delta;guard_tripped=$tripped;service_state=$svc.State}
        }
        Write-LabText -Path $path -Text (($guard | ConvertTo-Json -Compress) + [Environment]::NewLine) -Append
        if ($guard.guard_tripped -or $guard.service_state -ne 'Running') { break }
        Start-Sleep -Seconds 5
    }
} finally { Remove-PSSession $session }
