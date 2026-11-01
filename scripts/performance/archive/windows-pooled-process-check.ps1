#requires -Version 5.1
<#
.SYNOPSIS
Checks that a retained Process handle preserves exact child exit codes (no VM access).

.DESCRIPTION
Runs two throwaway powershell.exe children (exit 0 and 7) and writes
build/process-handle-check.json under the campaign directory; refuses to overwrite.
Historical pooled-VM helper: the dated campaign directory that used to be baked in is now
the mandatory -CampaignId.

.PARAMETER CampaignId
Campaign directory under target/performance.
#>
[CmdletBinding()]
param([Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$CampaignId)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '../LabCommon.psm1') -Force
$config = Get-LabConfig
$out = Resolve-LabPath "$($config.paths.evidence_root)/$CampaignId/build/process-handle-check.json"
if (Test-Path -LiteralPath $out) { throw 'Preserve existing process-handle check' }
New-Item -ItemType Directory -Force -Path (Split-Path $out) | Out-Null
$results = @()
foreach ($expected in 0,7) {
    $process = Start-Process powershell.exe -ArgumentList @('-NoProfile','-Command',"exit $expected") -WindowStyle Hidden -PassThru
    $retainedHandle = $process.Handle
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not $process.HasExited) {
        if ([DateTime]::UtcNow -gt $deadline) { Stop-Process -Id $process.Id; throw 'Nonload child completion deadline' }
        Start-Sleep -Milliseconds 50
        $process.Refresh()
    }
    $process.WaitForExit()
    $actual = $process.ExitCode
    $results += [ordered]@{expected=$expected;actual=$actual;retained_handle_nonzero=($retainedHandle -ne [IntPtr]::Zero);pass=($null -ne $actual -and $actual -eq $expected)}
    if ($null -eq $actual -or $actual -ne $expected) { throw 'Retained process handle did not preserve exact exit code' }
}
Write-LabJson -Path $out -InputObject ([ordered]@{utc=[DateTime]::UtcNow.ToString('o');powershell_version=$PSVersionTable.PSVersion.ToString();nonload=$true;checks=$results})
Get-Content -LiteralPath $out -Raw
