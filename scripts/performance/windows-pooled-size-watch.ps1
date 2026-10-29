[CmdletBinding()]
param([Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId,[Parameter(Mandatory)][long]$BaselineFreeBytes,[ValidateRange(1,600)][int]$Seconds=360)
$ErrorActionPreference='Stop'
$root=(Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$vm=Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac') { throw 'Wrong VM' }
$cred=Import-Clixml (Join-Path $root 'target/lab-windows/private/administrator.clixml')
$path=Join-Path $root "target/performance/pooled-vm-20261029-0942/stages/$RunId/conservative-size-watch.jsonl"
if (Test-Path $path) { throw 'Preserve watcher evidence' }
$deadline=[DateTime]::UtcNow.AddSeconds($Seconds)
while ([DateTime]::UtcNow -lt $deadline) {
    $guard=Invoke-Command -VMId $vm.Id -Credential $cred -ArgumentList $RunId,$BaselineFreeBytes -ScriptBlock {
        param($run,$baseline)
        $svc=Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"
        $free=(Get-Volume C).SizeRemaining
        $delta=$baseline-$free
        $tripped=$delta -ge 4GB -or $free -lt 8GB
        if ($tripped -and $svc.State -eq 'Running') { Stop-Service EtwDnstap }
        [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');free_bytes=$free;free_decrease_bytes=$delta;guard_tripped=$tripped;service_state=$svc.State}
    }
    $guard | ConvertTo-Json -Compress | Add-Content -LiteralPath $path
    if ($guard.guard_tripped -or $guard.service_state -ne 'Running') { break }
    Start-Sleep -Seconds 5
}
