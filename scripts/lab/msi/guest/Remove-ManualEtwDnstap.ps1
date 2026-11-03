#requires -Version 5.1
<#
.SYNOPSIS
Removes a hand-provisioned EtwDnstap (docs/guide/uninstall.md steps 1 to 6 and 8).

.DESCRIPTION
Lab step L1. Stops the service, removes the service SID from Performance Log Users and
runs the grant script with -Remove (both while the service SID still resolves), deletes
the service, stops a leftover ETW session, unregisters the performance counters,
removes the Application event source, and deletes the install folder.

It leaves ProgramData\EtwDnstap and everything in it in place (the lab's performance
folder is evidence). Reboot the guest afterwards: ETW applies the descriptors at boot.

Each step prints one line. The script exits 1 on the first failure.

.PARAMETER SessionName
The --session-name of the hand-provisioned service.

.PARAMETER GrantScript
Path of Grant-EtwDnstapEtwAccess.ps1. Default: the copy in the install folder.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SessionName,

    [string]$GrantScript
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$serviceName = 'EtwDnstap'
$installFolder = Join-Path $env:ProgramFiles 'EtwDnstap'
$manifest = Join-Path $installFolder 'etw-dnstap.man'

function Write-Step([string]$Text) { Write-Output ('[Remove-Manual] ' + $Text) }

try {
    $service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
    if ($null -ne $service) {
        if ($service.Status -ne 'Stopped') {
            sc.exe stop $serviceName | Out-Null
            $deadline = (Get-Date).AddSeconds(60)
            while ((Get-Service -Name $serviceName).Status -ne 'Stopped' -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }
            if ((Get-Service -Name $serviceName).Status -ne 'Stopped') { throw 'The service did not stop within 60 seconds.' }
            Write-Step 'Stopped the service.'
        }
        else { Write-Step 'The service is already stopped.' }

        # uninstall.md does the group removal (step 3) after the service delete (step 2).
        # The name NT SERVICE\EtwDnstap resolves only while the service exists, so this
        # script removes the membership and the grants first, as the MSI does.
        $group = (New-Object Security.Principal.SecurityIdentifier 'S-1-5-32-559').Translate([Security.Principal.NTAccount]).Value.Split([char]92)[-1]
        $members = @(Get-LocalGroupMember -Group $group | ForEach-Object { $_.SID.Value })
        $serviceSid = (New-Object Security.Principal.NTAccount "NT SERVICE\$serviceName").Translate([Security.Principal.SecurityIdentifier]).Value
        if ($members -contains $serviceSid) {
            net.exe localgroup "$group" "NT SERVICE\$serviceName" /delete | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "net localgroup /delete failed (exit code $LASTEXITCODE)." }
            Write-Step "Removed NT SERVICE\$serviceName from $group."
        }
        else { Write-Step "NT SERVICE\$serviceName is not in $group." }

        if (-not $GrantScript) { $GrantScript = Join-Path $installFolder 'Grant-EtwDnstapEtwAccess.ps1' }
        if (-not (Test-Path -LiteralPath $GrantScript)) { throw "Grant script not found: $GrantScript" }
        $grantOutput = @(& $GrantScript -Remove -Confirm:$false)
        $grantOutput | ForEach-Object { Write-Step ('grant: ' + $_) }
        Write-Step 'Ran the grant script with -Remove.'

        sc.exe delete $serviceName | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "sc.exe delete failed (exit code $LASTEXITCODE)." }
        Write-Step 'Deleted the service.'
    }
    else { Write-Step 'No service EtwDnstap.' }

    $names = @(logman.exe query -ets 2>$null | ForEach-Object { ($_.Trim() -split '\s+')[0] })
    if ($names -contains $SessionName) {
        logman.exe stop $SessionName -ets | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "logman stop failed (exit code $LASTEXITCODE)." }
        Write-Step "Stopped the ETW session '$SessionName'."
    }
    else { Write-Step "No ETW session '$SessionName'." }

    if (Test-Path -LiteralPath $manifest) {
        unlodctr.exe /m:"$manifest" | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "unlodctr failed (exit code $LASTEXITCODE)." }
        Write-Step 'Unregistered the performance counters.'
    }
    else { Write-Step 'No manifest to unregister.' }

    if ([System.Diagnostics.EventLog]::SourceExists($serviceName)) {
        Remove-EventLog -Source $serviceName
        Write-Step 'Removed the event source.'
    }
    else { Write-Step 'No event source.' }

    if (Test-Path -LiteralPath $installFolder) {
        Remove-Item -LiteralPath $installFolder -Recurse -Force
        Write-Step 'Removed the install folder.'
    }
    else { Write-Step 'No install folder.' }

    Write-Step 'Done. ProgramData\EtwDnstap is untouched. Reboot the guest: ETW applies descriptor changes at boot.'
    exit 0
}
catch {
    Write-Output ('[Remove-Manual] FAILED: ' + $_.Exception.Message)
    exit 1
}
