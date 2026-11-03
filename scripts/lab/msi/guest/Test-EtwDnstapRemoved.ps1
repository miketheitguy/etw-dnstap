#requires -Version 5.1
<#
.SYNOPSIS
Asserts that EtwDnstap is fully removed from the guest (plan: Test-EtwDnstapRemoved).

.DESCRIPTION
Each assertion prints PASS or FAIL, its name, and the value found. The script exits with
the number of failed assertions (0 means all passed).

.PARAMETER SessionName
The ETW session name that the installation used.

.PARAMETER KeptNames
Names directly under ProgramData\EtwDnstap that may still exist: content that the
product did not create (for example the lab's performance folder). The product
creates `spool` and `backup`, so those two must be gone unless this list names them.
Pass the names as one string, separated by semicolons.

.PARAMETER UpgradeCode
The MSI UpgradeCode. No product with it may be installed.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SessionName,

    [string]$KeptNames = '',

    [string]$UpgradeCode = '{0F4D1A87-DAD8-4C37-ABD4-FEC4DA400FAB}'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$serviceName = 'EtwDnstap'
$serviceSid = 'S-1-5-80-3892396922-4053150511-3308241269-1610546069-1753937115'
$installFolder = Join-Path $env:ProgramFiles 'EtwDnstap'
$dataFolder = Join-Path $env:ProgramData 'EtwDnstap'
$kept = @($KeptNames -split ';' | Where-Object { $_ })
$script:Failures = 0
$script:Passes = 0

function Assert-Check([string]$Name, [bool]$Condition, $Value) {
    $text = if ($null -eq $Value) { '' } else { (@($Value) | ForEach-Object { [string]$_ }) -join ' | ' }
    if ($Condition) { $script:Passes++; Write-Output "PASS $Name : $text" }
    else { $script:Failures++; Write-Output "FAIL $Name : $text" }
}

# sc.exe query: error 1060 means the service does not exist.
$scOutput = @(sc.exe query $serviceName 2>&1) -join ' '
Assert-Check 'service-absent (sc.exe query gives error 1060)' ($scOutput -match '1060') $scOutput

Assert-Check 'install-folder-absent' (-not (Test-Path -LiteralPath $installFolder)) $installFolder

foreach ($product in 'spool', 'backup') {
    $path = Join-Path $dataFolder $product
    $allowed = $kept -contains $product
    Assert-Check "data-$product-absent" ((-not (Test-Path -LiteralPath $path)) -or $allowed) ("exists=$(Test-Path -LiteralPath $path), allowed-as-kept=$allowed")
}
if (Test-Path -LiteralPath $dataFolder) {
    $left = @(Get-ChildItem -LiteralPath $dataFolder -Force | ForEach-Object { $_.Name })
    $unexpected = @($left | Where-Object { $kept -notcontains $_ })
    Assert-Check 'data-folder-holds-only-kept-content' ($unexpected.Count -eq 0) ("present=[$($left -join ', ')] unexpected=[$($unexpected -join ', ')]")
}
else {
    Assert-Check 'data-folder-holds-only-kept-content' $true 'EtwDnstap data folder does not exist'
}

$groupName = (New-Object Security.Principal.SecurityIdentifier 'S-1-5-32-559').Translate([Security.Principal.NTAccount]).Value.Split([char]92)[-1]
$memberSids = @(Get-LocalGroupMember -Group $groupName | ForEach-Object { $_.SID.Value })
Assert-Check 'performance-log-users-has-no-service-sid' ($memberSids -notcontains $serviceSid) ("members=" + $memberSids.Count)

$wmiKey = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Control\WMI\Security')
$withAce = @()
$unparsed = @()
$checked = 0
foreach ($name in $wmiKey.GetValueNames()) {
    $bytes = $wmiKey.GetValue($name)
    if ($bytes -isnot [byte[]]) { continue }
    try { $descriptor = New-Object Security.AccessControl.RawSecurityDescriptor($bytes, 0) } catch { $unparsed += $name; continue }
    $checked++
    if ($null -eq $descriptor.DiscretionaryAcl) { continue }
    foreach ($ace in $descriptor.DiscretionaryAcl) {
        if ($ace -is [Security.AccessControl.KnownAce] -and $ace.SecurityIdentifier.Value -eq $serviceSid) { $withAce += $name; break }
    }
}
$wmiKey.Dispose()
Assert-Check 'etw-no-descriptor-has-service-sid-ace' ($withAce.Count -eq 0) ("descriptors-checked=$checked with-ace=[$($withAce -join ', ')] unparseable=[$($unparsed -join ', ')]")

$sourceKey = "SYSTEM\CurrentControlSet\Services\EventLog\Application\$serviceName"
$sourceKeyObject = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($sourceKey)
Assert-Check 'event-source-key-absent' ($null -eq $sourceKeyObject) "HKLM\$sourceKey"
if ($null -ne $sourceKeyObject) { $sourceKeyObject.Dispose() }

$typeperf = @(typeperf.exe -q $serviceName 2>&1)
$counters = @($typeperf | Where-Object { $_ -match '^\\EtwDnstap' })
Assert-Check 'typeperf-lists-no-counters' ($counters.Count -eq 0) ("counters=" + $counters.Count)

$sessions = @(logman.exe query -ets 2>$null | ForEach-Object { ($_.Trim() -split '\s+')[0] })
Assert-Check 'etw-session-absent' ($sessions -notcontains $SessionName) "session=$SessionName"

$installer = New-Object -ComObject WindowsInstaller.Installer
$related = @($installer.RelatedProducts($UpgradeCode) | ForEach-Object { $_ })
Assert-Check 'no-product-with-upgrade-code (Add/Remove Programs)' ($related.Count -eq 0) ("upgrade-code=$UpgradeCode products=[$($related -join ', ')]")

Assert-Check 'registry-HKLM-SOFTWARE-EtwDnstap-absent' (-not (Test-Path -LiteralPath 'HKLM:\SOFTWARE\EtwDnstap')) 'HKLM\SOFTWARE\EtwDnstap'

Write-Output "SUMMARY pass=$($script:Passes) fail=$($script:Failures)"
exit $script:Failures
