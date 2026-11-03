#requires -Version 5.1
<#
.SYNOPSIS
Asserts that EtwDnstap is installed and provisioned as the MSI should leave it
(plan: Test-EtwDnstapInstalled).

.DESCRIPTION
Each assertion prints PASS or FAIL, its name, and the value found. The script exits with
the number of failed assertions (0 means all passed). It waits for the service to reach
RUNNING until BootWaitSeconds after the last boot. The receiver-side check
(bytes_received rises) needs the receiver, so the host driver does it.

.PARAMETER ExpectedVersion
The version in Cargo.toml.

.PARAMETER Receiver
RECEIVER given to the MSI.

.PARAMETER SessionName
SESSION_NAME given to the MSI.

.PARAMETER ServiceArgs
SERVICE_ARGS given to the MSI (empty for none).

.PARAMETER EventSourceBaseline
Path of the JSON that Get-EtwDnstapState.ps1 wrote before the hand-provisioned install
was removed. The event source key must match its values. Empty skips the comparison.

.PARAMETER BootWaitSeconds
How long after boot the service may take to reach RUNNING. Default 300 (5 minutes).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string]$ExpectedVersion,
    [Parameter(Mandatory = $true)] [string]$Receiver,
    [Parameter(Mandatory = $true)] [string]$SessionName,
    [string]$ServiceArgs = '',
    [string]$EventSourceBaseline = '',
    [int]$BootWaitSeconds = 300,
    [string]$UpgradeCode = '{0F4D1A87-DAD8-4C37-ABD4-FEC4DA400FAB}'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$serviceName = 'EtwDnstap'
$serviceSid = 'S-1-5-80-3892396922-4053150511-3308241269-1610546069-1753937115'
$installFolder = Join-Path $env:ProgramFiles 'EtwDnstap'
$exePath = Join-Path $installFolder 'etw-dnstap.exe'
$dataFolder = Join-Path $env:ProgramData 'EtwDnstap'
$spoolFolder = Join-Path $dataFolder 'spool'
$trustedSids = 'S-1-5-18', 'S-1-5-32-544'
$expectedServiceSd = 'D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWLORC;;;IU)(A;;CCLCSWLORC;;;SU)S:(AU;FA;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;WD)'
$expectedFolderSddl = "O:BAG:SYD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;$serviceSid)"
$script:Failures = 0
$script:Passes = 0

function Assert-Check([string]$Name, [bool]$Condition, $Value) {
    $text = if ($null -eq $Value) { '' } else { (@($Value) | ForEach-Object { [string]$_ }) -join ' | ' }
    if ($Condition) { $script:Passes++; Write-Output "PASS $Name : $text" }
    else { $script:Failures++; Write-Output "FAIL $Name : $text" }
}

# ---- install.md folder checks, ported from the guide ---------------------------------
function Test-ProtectedFolder {
    param([string]$Path)
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "'$Path' is a reparse point." }
    $acl = Get-Acl -LiteralPath $Path
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($trustedSids -notcontains $owner) { throw "'$Path' is owned by $owner." }
    $extra = @($acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]) |
            Where-Object { $trustedSids -notcontains $_.IdentityReference.Value })
    if ($extra.Count -gt 0) { throw "'$Path' has an ACE for $($extra[0].IdentityReference)." }
}

function Test-SpoolFolder {
    param([string]$Path)
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "'$Path' is a reparse point." }
    Test-ProtectedFolder (Split-Path -Parent $Path)
    $acl = Get-Acl -LiteralPath $Path
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($trustedSids -notcontains $owner) { throw "'$Path' is owned by $owner." }
    $rules = @($acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
    $extra = @($rules | Where-Object { $trustedSids -notcontains $_.IdentityReference.Value -and $_.IdentityReference.Value -ne $serviceSid })
    if ($extra.Count -gt 0) { throw "'$Path' has an ACE for $($extra[0].IdentityReference)." }
    $service = @($rules | Where-Object { $_.IdentityReference.Value -eq $serviceSid })
    $own = @($service | Where-Object { $_.InheritanceFlags -eq [Security.AccessControl.InheritanceFlags]::None })
    $contents = @($service | Where-Object { $_.InheritanceFlags -ne [Security.AccessControl.InheritanceFlags]::None })
    $modify = [Security.AccessControl.FileSystemRights]::Modify -bor [Security.AccessControl.FileSystemRights]::Synchronize
    $container = [Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [Security.AccessControl.InheritanceFlags]::ObjectInherit
    $inheritOnly = [Security.AccessControl.PropagationFlags]::InheritOnly
    $ownRights = if ($own.Count -eq 1) { $own[0].FileSystemRights } else { 0 }
    if ($service.Count -ne 2 -or $own.Count -ne 1 -or $contents.Count -ne 1 -or
        ($ownRights -band [Security.AccessControl.FileSystemRights]::Delete) -ne 0 -or
        ($ownRights -band [Security.AccessControl.FileSystemRights]::WriteData) -eq 0 -or
        $contents[0].InheritanceFlags -ne $container -or
        $contents[0].PropagationFlags -ne $inheritOnly -or
        $contents[0].FileSystemRights -ne $modify) {
        throw "'$Path' does not give the service the expected rights."
    }
}

# Same derivation as Grant-EtwDnstapEtwAccess.ps1 (Get-SessionGuid).
function Get-SessionGuid([string]$Name) {
    $sha1 = [Security.Cryptography.SHA1]::Create()
    try { $hash = $sha1.ComputeHash([Text.Encoding]::Unicode.GetBytes("EtwDnstap.Session:$Name")) } finally { $sha1.Dispose() }
    $bytes = [byte[]]$hash[0..15]
    $bytes[7] = ($bytes[7] -band 0x0F) -bor 0x50
    $bytes[8] = ($bytes[8] -band 0x3F) -bor 0x80
    return [Guid]::new($bytes)
}

function Get-ScText([string[]]$Arguments) { return ((& sc.exe @Arguments 2>&1) -join "`n") }

# ---- the service registration ------------------------------------------------------------
$qc = Get-ScText @('qc', $serviceName)
$serviceKey = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey("SYSTEM\CurrentControlSet\Services\$serviceName")
$imagePath = if ($null -ne $serviceKey) { [string]$serviceKey.GetValue('ImagePath', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) } else { '' }
$expectedImagePath = '"' + $exePath + '" --receiver "' + $Receiver + '" --session-name "' + $SessionName + '"'
if ($ServiceArgs) { $expectedImagePath += ' ' + $ServiceArgs }
Assert-Check 'qc-BINARY_PATH_NAME' ($imagePath -ceq $expectedImagePath) "found=[$imagePath] expected=[$expectedImagePath]"
Assert-Check 'qc-START_TYPE-AUTO_START-DELAYED' ($qc -match 'START_TYPE\s*:\s*2\s+AUTO_START\s+\(DELAYED\)') (($qc -split "`n" | Where-Object { $_ -match 'START_TYPE' }) -join '')
Assert-Check 'qc-DEPENDENCIES-DNS' ($qc -match 'DEPENDENCIES\s*:\s*DNS\s*$' -or $qc -match 'DEPENDENCIES\s*:\s*DNS\s*\n') (($qc -split "`n" | Where-Object { $_ -match 'DEPENDENCIES' }) -join '')
Assert-Check 'qc-SERVICE_START_NAME-LocalService' ($qc -match 'SERVICE_START_NAME\s*:\s*NT AUTHORITY\\LocalService\s*($|\n)') (($qc -split "`n" | Where-Object { $_ -match 'SERVICE_START_NAME' }) -join '')

$sid = Get-ScText @('qsidtype', $serviceName)
Assert-Check 'qsidtype-RESTRICTED' ($sid -match 'SERVICE_SID_TYPE:\s+RESTRICTED') (($sid -split "`n" | Where-Object { $_ -match 'SID_TYPE' }) -join '')

$privs = Get-ScText @('qprivs', $serviceName)
$privNames = @([regex]::Matches($privs, 'Se[A-Za-z]+Privilege') | ForEach-Object { $_.Value })
Assert-Check 'qprivs-only-SeChangeNotifyPrivilege' ($privNames.Count -eq 1 -and $privNames[0] -ceq 'SeChangeNotifyPrivilege') ($privNames -join ',')

$failure = Get-ScText @('qfailure', $serviceName)
$delays = @([regex]::Matches($failure, 'RESTART -- Delay = (\d+) milliseconds') | ForEach-Object { [int]$_.Groups[1].Value })
Assert-Check 'qfailure-RESET_PERIOD-86400' ($failure -match 'RESET_PERIOD \(in seconds\)\s*:\s*86400') (($failure -split "`n" | Where-Object { $_ -match 'RESET_PERIOD' }) -join '')
Assert-Check 'qfailure-restart-5000-5000-60000' (($delays -join ',') -eq '5000,5000,60000') ($delays -join ',')

$flag = Get-ScText @('qfailureflag', $serviceName)
$flagValue = if ($null -ne $serviceKey) { $serviceKey.GetValue('FailureActionsOnNonCrashFailures') } else { $null }
Assert-Check 'qfailureflag-TRUE (registry FailureActionsOnNonCrashFailures=1)' ($flag -match 'FAILURE_ACTIONS_ON_NONCRASH_FAILURES:\s+TRUE' -and $flagValue -eq 1) ((($flag -split "`n" | Where-Object { $_ -match 'NONCRASH' }) -join '') + " registry=$flagValue")
if ($null -ne $serviceKey) { $serviceKey.Dispose() }

$sd = ((sc.exe sdshow $serviceName) | Where-Object { $_.Trim() }) -join ''
Assert-Check 'sdshow-equals-install.md-step-4-string' ($sd -ceq $expectedServiceSd) $sd

# ---- folders -----------------------------------------------------------------------------
$folderAcl = Get-Acl -LiteralPath $installFolder
$folderSddl = $folderAcl.GetSecurityDescriptorSddlForm('Owner, Group, Access')
Assert-Check 'install-folder-ACL-equals-W4-SDDL' ($folderSddl -ceq $expectedFolderSddl) "found=[$folderSddl] expected=[$expectedFolderSddl]"

foreach ($file in 'etw-dnstap.exe', 'etw-dnstap.man', 'Grant-EtwDnstapEtwAccess.ps1', 'Invoke-EtwDnstapProvisioning.ps1') {
    Assert-Check "install-folder-has-$file" (Test-Path -LiteralPath (Join-Path $installFolder $file)) $file
}

try { Test-ProtectedFolder $dataFolder; Assert-Check 'data-folder-passes-Test-ProtectedFolder' $true $dataFolder }
catch { Assert-Check 'data-folder-passes-Test-ProtectedFolder' $false $_.Exception.Message }
try { Test-SpoolFolder $spoolFolder; Assert-Check 'spool-folder-passes-Test-SpoolFolder' $true $spoolFolder }
catch { Assert-Check 'spool-folder-passes-Test-SpoolFolder' $false $_.Exception.Message }

# ---- group and ETW descriptors -----------------------------------------------------------
$resolvedSid = (New-Object Security.Principal.NTAccount "NT SERVICE\$serviceName").Translate([Security.Principal.SecurityIdentifier]).Value
Assert-Check 'service-SID-resolves-to-expected' ($resolvedSid -eq $serviceSid) $resolvedSid

$groupName = (New-Object Security.Principal.SecurityIdentifier 'S-1-5-32-559').Translate([Security.Principal.NTAccount]).Value.Split([char]92)[-1]
$memberSids = @(Get-LocalGroupMember -Group $groupName | ForEach-Object { $_.SID.Value })
Assert-Check 'performance-log-users-holds-service-SID' ($memberSids -contains $serviceSid) "group=$groupName members=$($memberSids.Count)"

$sessionGuid = (Get-SessionGuid $SessionName).ToString()
$wmiKey = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Control\WMI\Security')
$sessionAce = @()
$otherWithAce = @()
$unparsed = @()
foreach ($name in $wmiKey.GetValueNames()) {
    $bytes = $wmiKey.GetValue($name)
    if ($bytes -isnot [byte[]]) { continue }
    try { $descriptor = New-Object Security.AccessControl.RawSecurityDescriptor($bytes, 0) } catch { $unparsed += $name; continue }
    if ($null -eq $descriptor.DiscretionaryAcl) { continue }
    foreach ($ace in $descriptor.DiscretionaryAcl) {
        if ($ace -isnot [Security.AccessControl.KnownAce] -or $ace.SecurityIdentifier.Value -ne $serviceSid) { continue }
        if ($name -eq $sessionGuid) { $sessionAce += ('{0} 0x{1:x} {2}' -f $ace.AceQualifier, $ace.AccessMask, $name) }
        else { $otherWithAce += $name }
    }
}
$wmiKey.Dispose()
Assert-Check 'etw-session-GUID-has-one-0x20-allow-ACE' ($sessionAce.Count -eq 1 -and $sessionAce[0] -eq "AccessAllowed 0x20 $sessionGuid") "guid=$sessionGuid aces=[$($sessionAce -join '; ')] unparseable=[$($unparsed -join ', ')]"
Assert-Check 'etw-no-other-descriptor-has-service-SID-ACE' ($otherWithAce.Count -eq 0) "others=[$($otherWithAce -join ', ')]"

# ---- event source ------------------------------------------------------------------------
$sourcePath = "SYSTEM\CurrentControlSet\Services\EventLog\Application\$serviceName"
$sourceKey = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($sourcePath)
if ($null -eq $sourceKey) { Assert-Check 'event-source-key-exists' $false "HKLM\$sourcePath" }
else {
    $found = @{}
    foreach ($name in $sourceKey.GetValueNames()) {
        $kind = [string]$sourceKey.GetValueKind($name)
        $raw = $sourceKey.GetValue($name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $data = switch ($kind) { 'Binary' { ($raw | ForEach-Object { $_.ToString('x2') }) -join '' } 'MultiString' { $raw -join "`n" } default { [string]$raw } }
        $found[$name] = [pscustomobject]@{ kind = $kind; data = $data }
    }
    $sourceKey.Dispose()
    Assert-Check 'event-source-key-exists' $true (($found.Keys | Sort-Object | ForEach-Object { "$_($($found[$_].kind))=$($found[$_].data)" }) -join '; ')
    if ($EventSourceBaseline) {
        $baseline = Get-Content -LiteralPath $EventSourceBaseline -Raw -Encoding UTF8 | ConvertFrom-Json
        $expected = @{}
        foreach ($v in @($baseline.eventSource.values)) { $expected[$v.name] = $v }
        foreach ($name in ($expected.Keys + $found.Keys | Sort-Object -Unique)) {
            if (-not $expected.ContainsKey($name)) { Assert-Check "event-source-matches-L0-$name" $false "extra in MSI registration: $($found[$name].kind) $($found[$name].data)" }
            elseif (-not $found.ContainsKey($name)) { Assert-Check "event-source-matches-L0-$name" $false "missing in MSI registration; L0 had: $($expected[$name].kind) $($expected[$name].data)" }
            else {
                $same = ($found[$name].kind -eq $expected[$name].kind) -and ($found[$name].data -ceq $expected[$name].data)
                Assert-Check "event-source-matches-L0-$name" $same "MSI=[$($found[$name].kind) $($found[$name].data)] L0=[$($expected[$name].kind) $($expected[$name].data)]"
            }
        }
    }
}

# ---- counters and version ----------------------------------------------------------------
$counters = @(typeperf.exe -q $serviceName 2>&1 | Where-Object { $_ -match '^\\EtwDnstap' })
Assert-Check 'typeperf-lists-counters' ($counters.Count -gt 0) ("counters=$($counters.Count) first=" + ($counters | Select-Object -First 1))

$fileVersion = (Get-Item -LiteralPath $exePath).VersionInfo
Assert-Check 'exe-FileVersion-equals-Cargo-version' ($fileVersion.FileVersion -ceq $ExpectedVersion) "FileVersion=$($fileVersion.FileVersion) ProductVersion=$($fileVersion.ProductVersion) expected=$ExpectedVersion"

$installer = New-Object -ComObject WindowsInstaller.Installer
$products = @($installer.RelatedProducts($UpgradeCode) | ForEach-Object { $_ })
Assert-Check 'one-product-with-upgrade-code' ($products.Count -eq 1) ($products -join ', ')
if ($products.Count -eq 1) {
    $uninstall = Get-ItemProperty -LiteralPath "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$($products[0])" -ErrorAction SilentlyContinue
    $display = if ($null -ne $uninstall) { [string]$uninstall.DisplayVersion } else { '' }
    $displayName = if ($null -ne $uninstall) { [string]$uninstall.DisplayName } else { '' }
    Assert-Check 'ARP-DisplayVersion-equals-Cargo-version' ($display -ceq $ExpectedVersion) "DisplayVersion=$display product=$($products[0]) DisplayName=$displayName"
}
Assert-Check 'registry-HKLM-SOFTWARE-EtwDnstap-Installer-exists' (Test-Path -LiteralPath 'HKLM:\SOFTWARE\EtwDnstap\Installer') 'HKLM\SOFTWARE\EtwDnstap\Installer'

# ---- the service reaches RUNNING after boot ----------------------------------------------
$boot = (Get-CimInstance Win32_OperatingSystem).LastBootUpTime
$deadline = $boot.AddSeconds($BootWaitSeconds)
$state = ''
while ($true) {
    $state = ((sc.exe query $serviceName) -join ' ')
    if ($state -match 'STATE\s*:\s*4\s+RUNNING') { break }
    if ((Get-Date) -gt $deadline) { break }
    Start-Sleep -Seconds 3
}
$secondsAfterBoot = [int]((Get-Date) - $boot).TotalSeconds
$running = $state -match 'STATE\s*:\s*4\s+RUNNING'
$stateText = $state -replace '\s+', ' '
Assert-Check 'service-RUNNING-within-boot-window' $running ("running=$running seconds-since-boot-at-check=$secondsAfterBoot window=${BootWaitSeconds}s state=" + $stateText.Substring(0, [Math]::Min(120, $stateText.Length)))

$process = @(Get-CimInstance Win32_Process -Filter "Name='etw-dnstap.exe'")
Assert-Check 'process-runs-from-install-folder' ($process.Count -eq 1 -and $process[0].ExecutablePath -ieq $exePath) ($process | ForEach-Object { "pid=$($_.ProcessId) path=$($_.ExecutablePath)" })

if ($running) {
    Start-Sleep -Seconds 2
    $events = @(Get-WinEvent -FilterHashtable @{ LogName = 'Application'; ProviderName = $serviceName; Id = 2; StartTime = $boot } -ErrorAction SilentlyContinue)
    $message = if ($events.Count -gt 0) { $events[0].Message } else { '' }
    $reported = if ($message -match '\bversion[=:]\s*"?([0-9][0-9A-Za-z.+-]*)') { $Matches[1] } else { '' }
    Assert-Check 'event-2-capture-started-version-equals-Cargo-version' ($reported -ceq $ExpectedVersion) ("events-since-boot=$($events.Count) version=$reported message=" + $message.Substring(0, [Math]::Min(300, $message.Length)))
}
else {
    Assert-Check 'event-2-capture-started-version-equals-Cargo-version' $false 'the service is not running'
}

Write-Output "SUMMARY pass=$($script:Passes) fail=$($script:Failures) seconds-since-boot=$secondsAfterBoot"
exit $script:Failures
