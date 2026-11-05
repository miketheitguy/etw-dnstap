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

$functionNames = 'Write-Step', 'Get-Hklm', 'Get-SpoolKeySddl', 'Get-SpoolKeySubKeySddl', 'Test-DaclMatches',
    'Initialize-SpoolKey', 'Remove-SpoolKey', 'Open-RegistryKey', 'Set-RegistryKeyDacl', 'Initialize-SpoolKeySubKey',
    'Save-WrappedKeyForUpgrade', 'Restore-WrappedKeyAfterUpgrade', 'Remove-SpoolKeySubKey'
$variableNames = 'DaclSecurityInformation', 'SpoolKeyServiceRight', 'SpoolKeyName', 'SpoolKeyBits',
    'ParametersSubKey', 'SpoolKeySubKey', 'WrappedKeyValueName', 'UpgradeBackupSubKey', 'ServiceSidText', 'RegistryExists', 'RegistryReadValue', 'RegistryWriteValue'

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
$parameters = "$root\Parameters"
$subKey = "$parameters\SpoolKey"
$backup = "$root\UpgradeBackup"
# Test only: the test user can read the copy. SYSTEM and Administrators only in the real install.
$backupSddl = "D:P(A;;KA;;;SY)(A;;KA;;;BA)(A;;KA;;;$userSid)"
$provider = [System.Security.Cryptography.CngProvider]::MicrosoftSoftwareKeyStorageProvider
$none = [System.Security.Cryptography.CngKeyOpenOptions]::None

function Get-DaclSddl([string]$Path) {
    $key = Open-RegistryKey $hive $Path $RegistryExists
    try { return $key.GetAccessControl([System.Security.AccessControl.AccessControlSections]::Access).GetSecurityDescriptorSddlForm('Access') }
    finally { $key.Dispose() }
}

try {
    [Console]::Out.WriteLine('Spool key')
    Initialize-SpoolKey -Name $keyName -ServiceSid $userSid -MachineKey $false | Out-Null
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
    Initialize-SpoolKey -Name $keyName -ServiceSid $userSid -MachineKey $false | Out-Null
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

    [Console]::Out.WriteLine('SpoolKey registry subkey')
    # Parameters exists first, with another value and its default ACL. It must not change.
    $hive.CreateSubKey($parameters).Dispose()
    $p = $hive.OpenSubKey($parameters, $true)
    $p.SetValue('Other', 'keep', [Microsoft.Win32.RegistryValueKind]::String)
    $p.Dispose()
    $parametersAclBefore = Get-DaclSddl $parameters

    Initialize-SpoolKeySubKey -Hive $hive -ParametersPath $parameters -SubKeyPath $subKey -ServiceSid $userSid | Out-Null
    Assert-That ((Get-DaclSddl $parameters) -eq $parametersAclBefore) 'the ACL of Parameters is unchanged'
    $subSddl = Get-DaclSddl $subKey
    Assert-That ($subSddl.StartsWith('D:P')) 'the subkey DACL is protected'
    $subDescriptor = New-Object System.Security.AccessControl.RawSecurityDescriptor $subSddl
    $serviceAces = @($subDescriptor.DiscretionaryAcl | Where-Object { $_.SecurityIdentifier.Value -eq $userSid })
    Assert-That (($serviceAces.Count -eq 1) -and ($serviceAces[0].AccessMask -eq 0x20003)) 'the service SID has exactly 0x20003 on the subkey'
    Assert-That ($subDescriptor.DiscretionaryAcl.Count -eq 3) 'the subkey DACL has three ACEs (SYSTEM, Administrators, service SID)'

    # A WrappedKey value survives a second run.
    $s = (Open-RegistryKey $hive $subKey $RegistryWriteValue $true)
    $s.SetValue($WrappedKeyValueName, [byte[]](1, 2, 3, 4), [Microsoft.Win32.RegistryValueKind]::Binary)
    $s.Dispose()
    Initialize-SpoolKeySubKey -Hive $hive -ParametersPath $parameters -SubKeyPath $subKey -ServiceSid $userSid | Out-Null
    $s = Open-RegistryKey $hive $subKey $RegistryReadValue
    $value = $s.GetValue($WrappedKeyValueName)
    $s.Dispose()
    Assert-That (($value -join ',') -eq '1,2,3,4') 'a second run keeps WrappedKey'

    # Upgrade: save, lose the subkey (DeleteServices), run install again, value is back.
    Save-WrappedKeyForUpgrade -Hive $hive -SubKeyPath $subKey -BackupPath $backup -BackupSddl $backupSddl | Out-Null
    Assert-That ($null -ne (Open-RegistryKey $hive $backup $RegistryExists)) 'the upgrade copy exists'
    # Test only: the test user holds just 0x20003 on the subkey, so give it full control
    # before the delete. SYSTEM and Administrators hold it in the real install.
    Set-RegistryKeyDacl -Hive $hive -SubKey $subKey -Sddl "D:P(A;;KA;;;$userSid)" -What 'test subkey'
    $hive.DeleteSubKeyTree($subKey)
    Initialize-SpoolKeySubKey -Hive $hive -ParametersPath $parameters -SubKeyPath $subKey -ServiceSid $userSid | Out-Null
    Restore-WrappedKeyAfterUpgrade -Hive $hive -SubKeyPath $subKey -BackupPath $backup | Out-Null
    $s = Open-RegistryKey $hive $subKey $RegistryReadValue
    $value = $s.GetValue($WrappedKeyValueName)
    $s.Dispose()
    Assert-That (($value -join ',') -eq '1,2,3,4') 'the upgrade copy restores WrappedKey'
    Assert-That ($null -eq (Open-RegistryKey $hive $backup $RegistryExists)) 'the upgrade copy is deleted after the restore'

    # Restore does not replace a WrappedKey that is already there.
    Save-WrappedKeyForUpgrade -Hive $hive -SubKeyPath $subKey -BackupPath $backup -BackupSddl $backupSddl | Out-Null
    $s = (Open-RegistryKey $hive $subKey $RegistryWriteValue $true)
    $s.SetValue($WrappedKeyValueName, [byte[]](9, 9), [Microsoft.Win32.RegistryValueKind]::Binary)
    $s.Dispose()
    Restore-WrappedKeyAfterUpgrade -Hive $hive -SubKeyPath $subKey -BackupPath $backup | Out-Null
    $s = Open-RegistryKey $hive $subKey $RegistryReadValue
    $value = $s.GetValue($WrappedKeyValueName)
    $s.Dispose()
    Assert-That (($value -join ',') -eq '9,9') 'a restore keeps an existing WrappedKey'

    # Uninstall: the subkey goes, Parameters and its other values stay. A second run is quiet.
    Set-RegistryKeyDacl -Hive $hive -SubKey $subKey -Sddl "D:P(A;;KA;;;$userSid)" -What 'test subkey'
    Remove-SpoolKeySubKey -Hive $hive -SubKeyPath $subKey -BackupPath $backup | Out-Null
    Assert-That ($null -eq (Open-RegistryKey $hive $subKey $RegistryExists)) 'the SpoolKey subkey is deleted'
    $p = $hive.OpenSubKey($parameters)
    Assert-That ($null -ne $p -and $p.GetValue('Other') -eq 'keep') 'Parameters and its other value stay'
    $p.Dispose()
    Remove-SpoolKeySubKey -Hive $hive -SubKeyPath $subKey -BackupPath $backup | Out-Null
    Assert-That $true 'removing a missing subkey is not an error'

    # Parameters missing: it is created.
    $hive.DeleteSubKeyTree($root)
    Initialize-SpoolKeySubKey -Hive $hive -ParametersPath $parameters -SubKeyPath $subKey -ServiceSid $userSid | Out-Null
    Assert-That ($null -ne (Open-RegistryKey $hive $subKey $RegistryExists)) 'a missing Parameters key is created with the subkey'
}
finally {
    try { Remove-SpoolKey -Name $keyName -MachineKey $false | Out-Null } catch { }
    try {
        if ($null -ne (Open-RegistryKey $hive $subKey $RegistryExists)) {
            Set-RegistryKeyDacl -Hive $hive -SubKey $subKey -Sddl "D:P(A;;KA;;;$userSid)" -What 'test subkey'
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
