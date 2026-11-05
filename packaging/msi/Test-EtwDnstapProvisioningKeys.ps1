#requires -Version 5.1
<#
.SYNOPSIS
Tests the spool key and registry functions of Invoke-EtwDnstapProvisioning.ps1 without installing anything.

.DESCRIPTION
Loads the functions from the provisioning script by parsing it (the script is not run). Then
runs them against a throwaway per-user CNG key, a throwaway HKCU path, and the current
user's SID in place of the service SID. It creates nothing on the machine scope and deletes
everything it creates. Run it with Windows PowerShell 5.1:

    powershell.exe -NoProfile -File packaging\msi\Test-EtwDnstapProvisioningKeys.ps1

Exit code 0 means every check passed.
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$scriptPath = Join-Path $PSScriptRoot 'Invoke-EtwDnstapProvisioning.ps1'
$ast = [System.Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$null, [ref]$null)

$functionNames = 'Write-Step', 'Get-Hklm', 'Get-SpoolKeySddl', 'Get-ProtectKeySddl', 'Test-DaclMatches',
    'Initialize-SpoolKey', 'Remove-SpoolKey', 'Open-RegistryKey', 'Set-RegistryKeyDacl', 'Initialize-ProtectKey',
    'Remove-ObsoleteSpoolKeySubKey', 'Remove-ProtectKey'
$variableNames = 'DaclSecurityInformation', 'SpoolKeyServiceRight', 'SpoolKeyName', 'SpoolKeyBits',
    'ProtectParentSubKey', 'ProtectSubKey', 'WrappedKeyValueName', 'ObsoleteSpoolKeySubKey', 'ServiceSidText', 'RegistryExists', 'RegistryReadValue', 'RegistryWriteValue'

foreach ($name in $functionNames) {
    $found = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name }, $true))
    if ($found.Count -ne 1) { throw "Function $name not found exactly once in the provisioning script." }
    . ([scriptblock]::Create($found[0].Extent.Text))
}
foreach ($name in $variableNames) {
    $found = @($ast.FindAll({ param($n)
                $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and
                $n.Left -is [System.Management.Automation.Language.VariableExpressionAst] -and
                $n.Left.VariablePath.UserPath -eq $name }, $false))
    if ($found.Count -ne 1) { throw "Variable $name not assigned exactly once at the top of the provisioning script." }
    Invoke-Expression $found[0].Extent.Text
}

# The original text of a function, to put it back after a test replaced it.
function Get-FunctionText([string]$Name) {
    $found = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $Name }, $true))
    return $found[0].Extent.Text
}

$failures = New-Object System.Collections.Generic.List[string]
function Assert-That([bool]$Condition, [string]$Message) {
    if ($Condition) { [Console]::Out.WriteLine('  ok: ' + $Message) }
    else { [Console]::Out.WriteLine('  FAIL: ' + $Message); $failures.Add($Message) }
}
function Assert-Throws([scriptblock]$Block, [string]$Pattern, [string]$Message) {
    $text = $null
    try { & $Block } catch { $text = $_.Exception.Message }
    Assert-That (($null -ne $text) -and ($text -match $Pattern)) $Message
}

# The test user stands in for the service SID. With the real right (read), the test user
# could not change or delete its own key, because only SYSTEM and Administrators hold all
# rights. So the test gives its user all rights; the real right is a constant in the script.
$SpoolKeyServiceRight = 'GA'

$userSid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$suffix = [guid]::NewGuid().ToString('N')
$keyName = "EtwDnstap Spool Key Test $suffix"
$hive = [Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::CurrentUser, [Microsoft.Win32.RegistryView]::Registry64)
$root = "Software\EtwDnstapTest$suffix"
$parent = "$root\ETW-DNSTapService"
$protect = "$parent\Protect"
$parametersPath = "$root\Parameters"
$obsolete = "$parametersPath\SpoolKey"
$provider = [System.Security.Cryptography.CngProvider]::MicrosoftSoftwareKeyStorageProvider
$none = [System.Security.Cryptography.CngKeyOpenOptions]::None

function Get-DaclSddl([string]$Path) {
    $key = Open-RegistryKey $hive $Path $RegistryExists
    try { return $key.GetAccessControl([System.Security.AccessControl.AccessControlSections]::Access).GetSecurityDescriptorSddlForm('Access') }
    finally { $key.Dispose() }
}

try {
    [Console]::Out.WriteLine('Spool key')
    $created = Initialize-SpoolKey -Name $keyName -ServiceSid $userSid -MachineKey $false
    Assert-That (($created -is [bool]) -and $created) 'F4: Initialize-SpoolKey returns true when it created the key'
    Assert-That ([System.Security.Cryptography.CngKey]::Exists($keyName, $provider, $none)) 'key exists after the first run'
    $key = [System.Security.Cryptography.CngKey]::Open($keyName, $provider, $none)
    Assert-That ($key.KeySize -eq 3072) 'key is RSA-3072'
    Assert-That ($key.ExportPolicy -eq [System.Security.Cryptography.CngExportPolicies]::None) 'key is not exportable'
    Assert-That ($key.Provider -eq $provider) 'key is in the Microsoft Software KSP'
    $before = $key.GetProperty('Security Descr', $DaclSecurityInformation).GetValue()
    $key.Dispose()

    # Break the DACL, then run again: the DACL is reapplied and the key is the same key.
    $key = [System.Security.Cryptography.CngKey]::Open($keyName, $provider, $none)
    $broken = New-Object System.Security.AccessControl.RawSecurityDescriptor ("D:P(A;;GA;;;$userSid)")
    $brokenBytes = New-Object byte[] $broken.BinaryLength
    $broken.GetBinaryForm($brokenBytes, 0)
    $key.SetProperty([System.Security.Cryptography.CngProperty]::new('Security Descr', $brokenBytes, $DaclSecurityInformation))
    $key.Dispose()
    $created = Initialize-SpoolKey -Name $keyName -ServiceSid $userSid -MachineKey $false
    Assert-That (($created -is [bool]) -and -not $created) 'F4: Initialize-SpoolKey returns false when it accepted an existing key'
    $key = [System.Security.Cryptography.CngKey]::Open($keyName, $provider, $none)
    $after = $key.GetProperty('Security Descr', $DaclSecurityInformation).GetValue()
    $key.Dispose()
    Assert-That (($before -join ',') -eq ($after -join ',')) 'a second run puts the DACL back'

    # The DACL check throws when the DACL differs.
    Assert-Throws { Test-DaclMatches -ActualDescriptor $brokenBytes -ExpectedSddl (Get-SpoolKeySddl $userSid) -What 'test' } 'not as expected' 'a wrong DACL fails the check'
    $unprotected = New-Object System.Security.AccessControl.RawSecurityDescriptor ('D:(A;;GA;;;SY)')
    $unprotectedBytes = New-Object byte[] $unprotected.BinaryLength
    $unprotected.GetBinaryForm($unprotectedBytes, 0)
    Assert-Throws { Test-DaclMatches -ActualDescriptor $unprotectedBytes -ExpectedSddl 'D:P(A;;GA;;;SY)' -What 'test' } 'not protected' 'an inheriting DACL fails the check'

    Remove-SpoolKey -Name $keyName -MachineKey $false | Out-Null
    Assert-That (-not [System.Security.Cryptography.CngKey]::Exists($keyName, $provider, $none)) 'key is gone after remove'
    Remove-SpoolKey -Name $keyName -MachineKey $false | Out-Null
    Assert-That $true 'removing a missing key is not an error'

    # F6: a CNG failure while the key is looked up or deleted is a warning, not an exception.
    # A name of 5000 characters makes the provider throw.
    $threw = $false
    try { Remove-SpoolKey -Name ('a' * 5000) -MachineKey $false | Out-Null } catch { $threw = $true }
    Assert-That (-not $threw) 'F6: a failed key delete does not throw'

    [Console]::Out.WriteLine('Protect registry key')
    # Test only: the test user holds just 0x20003 on Protect, so it needs full control
    # before a delete. SYSTEM and Administrators hold it in the real install.
    $fullFor = "D:P(A;;KA;;;$userSid)"

    Initialize-ProtectKey -Hive $hive -ParentPath $parent -ProtectPath $protect -ServiceSid $userSid | Out-Null
    Assert-That ($null -ne (Open-RegistryKey $hive $parent $RegistryExists)) 'a missing parent key is created'
    $protectSddl = Get-DaclSddl $protect
    Assert-That ($protectSddl.StartsWith('D:P')) 'the Protect DACL is protected'
    $descriptor = New-Object System.Security.AccessControl.RawSecurityDescriptor $protectSddl
    $serviceAces = @($descriptor.DiscretionaryAcl | Where-Object { $_.SecurityIdentifier.Value -eq $userSid })
    Assert-That (($serviceAces.Count -eq 1) -and ($serviceAces[0].AccessMask -eq 0x20003)) 'the service SID has exactly 0x20003 on Protect'
    Assert-That ($descriptor.DiscretionaryAcl.Count -eq 3) 'the Protect DACL has three ACEs (SYSTEM, Administrators, service SID)'
    Assert-That ($null -eq (Open-RegistryKey $hive $parametersPath $RegistryExists)) 'nothing is created under Parameters'

    # A second run keeps the parent ACL and puts a broken Protect DACL back.
    $parentAclBefore = Get-DaclSddl $parent
    Set-RegistryKeyDacl -Hive $hive -SubKey $protect -Sddl $fullFor -What 'test Protect'
    Initialize-ProtectKey -Hive $hive -ParentPath $parent -ProtectPath $protect -ServiceSid $userSid | Out-Null
    Assert-That ((Get-DaclSddl $parent) -eq $parentAclBefore) 'the ACL of the parent key is unchanged'
    Assert-That ((Get-DaclSddl $protect) -eq $protectSddl) 'a second run puts the Protect DACL back'

    # An existing WrappedKey is never written or deleted by a normal install.
    $s = Open-RegistryKey $hive $protect $RegistryWriteValue $true
    $s.SetValue($WrappedKeyValueName, [byte[]](1, 2, 3, 4), [Microsoft.Win32.RegistryValueKind]::Binary)
    $s.Dispose()
    Initialize-ProtectKey -Hive $hive -ParentPath $parent -ProtectPath $protect -ServiceSid $userSid -KeyWasCreated $false | Out-Null
    $s = Open-RegistryKey $hive $protect $RegistryReadValue
    $value = $s.GetValue($WrappedKeyValueName)
    $s.Dispose()
    Assert-That (($value -join ',') -eq '1,2,3,4') 'a normal install keeps WrappedKey'

    # F4: the install made a new CNG key. The stale value cannot be unwrapped and is deleted.
    Initialize-ProtectKey -Hive $hive -ParentPath $parent -ProtectPath $protect -ServiceSid $userSid -KeyWasCreated $true | Out-Null
    $s = Open-RegistryKey $hive $protect $RegistryReadValue
    Assert-That ($null -eq $s.GetValue($WrappedKeyValueName, $null)) 'F4: a new CNG key deletes the stale WrappedKey'
    $s.Dispose()
    Initialize-ProtectKey -Hive $hive -ParentPath $parent -ProtectPath $protect -ServiceSid $userSid -KeyWasCreated $true | Out-Null
    Assert-That $true 'F4: a new CNG key with no WrappedKey is not an error'

    # The Protect DACL is part of the CreateSubKey call. The DACL step after it is replaced
    # by one that throws; the key must still have the protected DACL from its creation.
    Set-RegistryKeyDacl -Hive $hive -SubKey $protect -Sddl $fullFor -What 'test Protect'
    $hive.DeleteSubKeyTree($protect)
    $realSetDacl = Get-FunctionText 'Set-RegistryKeyDacl'
    function Set-RegistryKeyDacl { throw 'mocked DACL failure' }
    $threw = $false
    try { Initialize-ProtectKey -Hive $hive -ParentPath $parent -ProtectPath $protect -ServiceSid $userSid | Out-Null } catch { $threw = $true }
    finally { . ([scriptblock]::Create($realSetDacl)) }
    Assert-That $threw 'a failed DACL step fails the install step'
    Assert-That ($null -ne (Open-RegistryKey $hive $protect $RegistryExists)) 'F5: the Protect key was created'
    # SetAccessControl only adds the AI flag; the ACEs are the same.
    Assert-That (((Get-DaclSddl $protect) -replace '^D:PAI', 'D:P') -eq ($protectSddl -replace '^D:PAI', 'D:P')) 'F5: Protect has its protected DACL from its creation (no inherited Users ACE)'

    [Console]::Out.WriteLine('Obsolete Parameters\SpoolKey subkey')
    $hive.CreateSubKey($parametersPath).Dispose()
    $s = $hive.OpenSubKey($parametersPath, $true)
    $s.SetValue('Other', 'keep', [Microsoft.Win32.RegistryValueKind]::String)
    $s.Dispose()
    $parametersAclBefore = Get-DaclSddl $parametersPath
    $hive.CreateSubKey($obsolete).Dispose()
    Remove-ObsoleteSpoolKeySubKey -Hive $hive -SubKeyPath $obsolete | Out-Null
    Assert-That ($null -eq (Open-RegistryKey $hive $obsolete $RegistryExists)) 'the obsolete SpoolKey subkey is deleted'
    $s = $hive.OpenSubKey($parametersPath)
    Assert-That ($null -ne $s -and $s.GetValue('Other') -eq 'keep') 'Parameters and its other value stay'
    $s.Dispose()
    Assert-That ((Get-DaclSddl $parametersPath) -eq $parametersAclBefore) 'the ACL of Parameters is unchanged'
    Remove-ObsoleteSpoolKeySubKey -Hive $hive -SubKeyPath $obsolete | Out-Null
    Assert-That $true 'removing a missing obsolete subkey is not an error'
    # A subkey that cannot be deleted is a warning (the old subkey gave the service 0x20003 only).
    $hive.CreateSubKey($obsolete).Dispose()
    Set-RegistryKeyDacl -Hive $hive -SubKey $obsolete -Sddl "D:P(A;;0x20003;;;$userSid)" -What 'test obsolete'
    $threw = $false
    try { Remove-ObsoleteSpoolKeySubKey -Hive $hive -SubKeyPath $obsolete | Out-Null } catch { $threw = $true }
    Assert-That (-not $threw) 'F6: a failed delete of the obsolete subkey is a warning'
    Assert-That ($null -ne (Open-RegistryKey $hive $obsolete $RegistryExists)) 'F6: the obsolete subkey is still there after the failed delete'
    Set-RegistryKeyDacl -Hive $hive -SubKey $obsolete -Sddl $fullFor -What 'test obsolete'

    [Console]::Out.WriteLine('Uninstall')
    # F6: Protect has 0x20003 only for the test user, so its delete fails: a warning, no throw,
    # and the parent stays because it still holds Protect.
    $threw = $false
    try { Remove-ProtectKey -Hive $hive -ParentPath $parent -ProtectPath $protect | Out-Null } catch { $threw = $true }
    Assert-That (-not $threw) 'F6: a failed Protect delete does not throw'
    Assert-That ($null -ne (Open-RegistryKey $hive $protect $RegistryExists)) 'F6: Protect is still there after the failed delete'
    Assert-That ($null -ne (Open-RegistryKey $hive $parent $RegistryExists)) 'the parent stays while Protect is there'

    # A parent with another value is kept.
    Set-RegistryKeyDacl -Hive $hive -SubKey $protect -Sddl $fullFor -What 'test Protect'
    $s = $hive.OpenSubKey($parent, $true)
    $s.SetValue('Other', 'keep', [Microsoft.Win32.RegistryValueKind]::String)
    $s.Dispose()
    Remove-ProtectKey -Hive $hive -ParentPath $parent -ProtectPath $protect | Out-Null
    Assert-That ($null -eq (Open-RegistryKey $hive $protect $RegistryExists)) 'uninstall deletes Protect'
    Assert-That ($null -ne (Open-RegistryKey $hive $parent $RegistryExists)) 'uninstall keeps a parent key that is not empty'

    # A parent with a subkey of another product is kept; an empty one is deleted.
    $s = $hive.OpenSubKey($parent, $true)
    $s.DeleteValue('Other')
    $s.Dispose()
    $hive.CreateSubKey("$parent\Other").Dispose()
    Remove-ProtectKey -Hive $hive -ParentPath $parent -ProtectPath $protect | Out-Null
    Assert-That ($null -ne (Open-RegistryKey $hive $parent $RegistryExists)) 'uninstall keeps a parent key that has another subkey'
    $hive.DeleteSubKey("$parent\Other")
    Remove-ProtectKey -Hive $hive -ParentPath $parent -ProtectPath $protect | Out-Null
    Assert-That ($null -eq (Open-RegistryKey $hive $parent $RegistryExists)) 'uninstall deletes the empty parent key'
    Remove-ProtectKey -Hive $hive -ParentPath $parent -ProtectPath $protect | Out-Null
    Assert-That $true 'a second uninstall run is not an error'

    [Console]::Out.WriteLine('Packaging')
    $script = Get-Content -LiteralPath $scriptPath -Raw
    $wxs = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'CustomActions.wxs') -Raw
    $product = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'Product.wxs') -Raw
    Assert-That ($script -notmatch 'PreserveForUpgrade|EtwDnstapUpgrade|Restore-WrappedKey|Save-WrappedKey|CommitInstall') 'no upgrade copy code is left in the script'
    Assert-That ($wxs -notmatch 'Preserve|CommitInstall|RollbackInstall|Execute="commit"') 'no upgrade copy custom action is left'
    Assert-That ($product -notmatch 'Preserve') 'Product.wxs has no upgrade copy action'
    Assert-That ($product -notmatch 'ETW-DNSTapService') 'WiX does not author the Protect key'
    Assert-That ($script -match "ProtectParentSubKey = 'SOFTWARE\\ETW-DNSTapService'") 'the Protect key is under SOFTWARE\ETW-DNSTapService'
    Assert-That ($product -match '<ServiceDependency Id="DNS"') 'F2: the service depends on DNS'
    Assert-That ($product -match '<ServiceDependency Id="KeyIso"') 'F2: the service depends on KeyIso'
    $install = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Invoke-Install' }, $true))
    Assert-That ($install[0].Extent.Text -notmatch 'SetValue|DeleteSubKey') 'Invoke-Install itself writes no registry value and deletes no key'
}
finally {
    try { Remove-SpoolKey -Name $keyName -MachineKey $false | Out-Null } catch { }
    try {
        foreach ($path in $protect, $obsolete) {
            if ($null -ne (Open-RegistryKey $hive $path $RegistryExists)) {
                Set-RegistryKeyDacl -Hive $hive -SubKey $path -Sddl "D:P(A;;KA;;;$userSid)" -What 'test key'
            }
        }
        $hive.DeleteSubKeyTree($root, $false)
    }
    catch { [Console]::Out.WriteLine('Cleanup problem: ' + $_.Exception.Message) }
    $hive.Dispose()
}

if ($failures.Count -gt 0) {
    [Console]::Out.WriteLine("$($failures.Count) check(s) failed.")
    exit 1
}
[Console]::Out.WriteLine('All checks passed.')
exit 0
