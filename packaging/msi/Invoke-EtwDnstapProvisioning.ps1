#requires -Version 5.1
<#
.SYNOPSIS
Provisioning steps that the EtwDnstap MSI runs as SYSTEM (plan W7).

.DESCRIPTION
Install: checks RECEIVER as docs/guide/install.md step 1 does, creates or checks
ProgramData\EtwDnstap and its spool folder with the rules of install.md (the
New-ProtectedFolder, Test-ProtectedFolder and Test-SpoolFolder functions are
ported without loosening), adds the service SID to Performance Log Users,
runs Grant-EtwDnstapEtwAccess.ps1 from the install folder, and sets the service
recovery policy with sc.exe failure (install.md step 5).

Uninstall: stops the leftover ETW session, removes the service SID from
Performance Log Users, runs the grant script with -Remove, and deletes the spool
and backup folders that the product created. Reparse points are never followed or
deleted, and content that the product did not create is kept and listed. The group
membership is read and removed by the service SID, not by the account name, so it
works after the service was deleted by hand. If that step fails, the script prints
a WARNING line and continues with the grant removal and the folder removal.

Install also creates the non-exportable RSA-3072 machine key 'EtwDnstap Spool Key' and the
Parameters\SpoolKey registry subkey of the service, each with a protected DACL that is read
back and checked. The ACL of Parameters is never changed. Uninstall deletes the key and the
SpoolKey subkey. An existing key and an existing WrappedKey value are kept.

Each change prints one line, which the MSI log keeps. Apart from that warning, the
script exits 1 on the first failure and writes no secrets. It is run by WixQuietExec
from the install folder, by full path, as SYSTEM, with no profile.

.PARAMETER Action
Install, Uninstall, or PreserveForUpgrade. PreserveForUpgrade runs when a major upgrade removes
the old product. It copies the WrappedKey value out of the service key, because deleting the
service deletes that key. The next Install puts it back.

.PARAMETER SessionName
The --session-name of the service. 1 to 128 characters: ASCII letters, digits, '_', '.', '-'.
The first character must not be '-'.

.PARAMETER Receiver
IP:port of the collector, as IPv4 or [IPv6]:port. Required for Install.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Install', 'Uninstall', 'PreserveForUpgrade')]
    [string]$Action,

    # Case-sensitive and anchored with \z: the default ValidatePattern ignores case,
    # which lets U+212A and U+0130 match [A-Za-z], and its $ accepts a trailing newline.
    # No leading '-': the MSI passes the name on command lines (this script's
    # powershell.exe -File line and the service's ImagePath), where a value that starts
    # with '-' is read as a parameter or an option name. The service CLI has the same rule.
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}\z', Options = 'None')]
    [string]$SessionName,

    [string]$Receiver
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ServiceName = 'EtwDnstap'
# The service SID of EtwDnstap: S-1-5-80 plus the SHA-1 of the upper-cased name, as
# Get-ServiceSidFromName in Grant-EtwDnstapEtwAccess.ps1 derives it. Product.wxs uses the
# same value in the install folder ACL. Test-SpoolFolder checks it against the system's
# own lookup on every install, so a wrong value fails the install.
$ServiceSidText = 'S-1-5-80-3892396922-4053150511-3308241269-1610546069-1753937115'
$PerformanceLogUsersSid = 'S-1-5-32-559'
$trustedSids = 'S-1-5-18', 'S-1-5-32-544'
$DataRoot = Join-Path $env:ProgramData 'EtwDnstap'
$SpoolDirectory = Join-Path $DataRoot 'spool'
$BackupDirectory = Join-Path $DataRoot 'backup'
$GrantScript = Join-Path $PSScriptRoot 'Grant-EtwDnstapEtwAccess.ps1'

# Writes straight to stdout. Write-Output would join the pipeline, and a function that
# returns a value (Remove-OwnedTree) would return the message with the value.
function Write-Step([string]$Text) {
    [Console]::Out.WriteLine('[EtwDnstap] ' + $Text)
}

# ---- install.md step 1: the receiver check (mirrors check_receiver) ----------
# install.md uses [System.Net.IPEndPoint]::TryParse, which .NET Framework (Windows
# PowerShell 5.1) does not have. This is the same check on the same inputs: split the
# endpoint, then parse the address with IPAddress.TryParse, which takes IP literals only.
function Test-Receiver([string]$Text) {
    $addressText = $null
    $portText = $null
    if ($Text -match '^\[([^\]]+)\]:(\d{1,5})$') {
        $addressText = $Matches[1]
        $portText = $Matches[2]
    }
    elseif ($Text -match '^([0-9.]+):(\d{1,5})$') {
        $addressText = $Matches[1]
        $portText = $Matches[2]
    }
    $address = $null
    if ($null -eq $addressText -or -not [System.Net.IPAddress]::TryParse($addressText, [ref]$address)) {
        throw "Receiver must be a numeric IP address and port, for example 192.0.2.10:6000. Got '$Text'."
    }
    $port = [int]$portText
    if ($port -eq 0 -or $port -gt 65535) {
        throw 'Receiver must have a unicast IP address and a nonzero port.'
    }
    $firstOctet = if ($address.AddressFamily -eq 'InterNetwork') { $address.GetAddressBytes()[0] } else { 0 }
    $isMulticast = ($address.AddressFamily -eq 'InterNetworkV6' -and $address.IsIPv6Multicast) -or
        ($firstOctet -ge 224 -and $firstOctet -le 239)
    if ($address.Equals([System.Net.IPAddress]::Any) -or
        $address.Equals([System.Net.IPAddress]::IPv6Any) -or
        $isMulticast) {
        throw 'Receiver must have a unicast IP address and a nonzero port.'
    }
    if ($address.AddressFamily -eq 'InterNetworkV6') { return "[$($address)]:$port" }
    return "$($address):$port"
}

# ---- install.md snapshot functions, ported unchanged ------------------------
function Test-ProtectedFolder {
    param([string]$Path)
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "'$Path' is a reparse point. Inspect it. Do not use it."
    }
    $acl = Get-Acl -LiteralPath $Path
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($trustedSids -notcontains $owner) {
        throw "'$Path' is owned by $owner. Inspect it. Do not use it."
    }
    $extra = @($acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]) |
            Where-Object { $trustedSids -notcontains $_.IdentityReference.Value })
    if ($extra.Count -gt 0) {
        throw "'$Path' has an ACE for $($extra[0].IdentityReference). Inspect it. Do not use it."
    }
}

function New-ProtectedFolder {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param([string]$Path)
    if (-not $PSCmdlet.ShouldProcess($Path, 'Create protected folder')) { return }
    New-Item -ItemType Directory -Path $Path -ErrorAction Stop | Out-Null
    icacls.exe $Path /setowner '*S-1-5-32-544' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "icacls /setowner failed on '$Path'." }
    icacls.exe $Path /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "icacls failed on '$Path'." }
    if (@(Get-ChildItem -LiteralPath $Path -Force).Count -ne 0) { throw "'$Path' is not empty. Inspect it." }
    Test-ProtectedFolder $Path
}

function Test-SpoolFolder {
    param([string]$Path)
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "'$Path' is a reparse point. Inspect it. Do not use it."
    }
    Test-ProtectedFolder (Split-Path -Parent $Path)
    $acl = Get-Acl -LiteralPath $Path
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($trustedSids -notcontains $owner) {
        throw "'$Path' is owned by $owner. Inspect it. Do not use it."
    }
    $serviceSid = (New-Object Security.Principal.NTAccount "NT SERVICE\$ServiceName").
        Translate([Security.Principal.SecurityIdentifier]).Value
    if ($serviceSid -ne $ServiceSidText) {
        throw "NT SERVICE\$ServiceName resolves to $serviceSid, not the expected service SID. Inspect it."
    }
    $rules = @($acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
    $extra = @($rules | Where-Object {
            $trustedSids -notcontains $_.IdentityReference.Value -and
            $_.IdentityReference.Value -ne $serviceSid
        })
    if ($extra.Count -gt 0) {
        throw "'$Path' has an ACE for $($extra[0].IdentityReference). Inspect it. Do not use it."
    }
    # The service gets two entries: the folder itself (write, read and execute, no
    # delete), and Modify on the contents only.
    $service = @($rules | Where-Object { $_.IdentityReference.Value -eq $serviceSid })
    $own = @($service | Where-Object {
            $_.InheritanceFlags -eq [Security.AccessControl.InheritanceFlags]::None
        })
    $contents = @($service | Where-Object {
            $_.InheritanceFlags -ne [Security.AccessControl.InheritanceFlags]::None
        })
    $modify = [Security.AccessControl.FileSystemRights]::Modify -bor
        [Security.AccessControl.FileSystemRights]::Synchronize
    $container = [Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
        [Security.AccessControl.InheritanceFlags]::ObjectInherit
    $inheritOnly = [Security.AccessControl.PropagationFlags]::InheritOnly
    $ownRights = if ($own.Count -eq 1) { $own[0].FileSystemRights } else { 0 }
    if ($service.Count -ne 2 -or $own.Count -ne 1 -or $contents.Count -ne 1 -or
        ($ownRights -band [Security.AccessControl.FileSystemRights]::Delete) -ne 0 -or
        ($ownRights -band [Security.AccessControl.FileSystemRights]::WriteData) -eq 0 -or
        $contents[0].InheritanceFlags -ne $container -or
        $contents[0].PropagationFlags -ne $inheritOnly -or
        $contents[0].FileSystemRights -ne $modify) {
        throw "'$Path' does not give the service the expected rights. Inspect it."
    }
}

# ---- Spool key and SpoolKey registry subkey (spool encryption plan, task I) -----------
# The functions take the key name, the service SID, and the registry hive as parameters
# with the real values as defaults, so that Test-EtwDnstapProvisioningKeys.ps1 can run
# them against a user key, a throwaway HKCU path, and the current user's SID.

# NCryptSetProperty / NCryptGetProperty flag for the "Security Descr" property.
$DaclSecurityInformation = [System.Security.Cryptography.CngPropertyOptions]4

# The right that the service SID gets on the spool key, as an SDDL right string. Task K
# confirms whether read is enough for NCryptDecrypt. Change it here only.
$SpoolKeyServiceRight = 'GR'
$SpoolKeyName = 'EtwDnstap Spool Key'
$SpoolKeyBits = 3072
# Parameters is created if it is missing and its ACL is never changed. The SpoolKey
# subkey under it holds the WrappedKey value and has its own protected DACL.
$ParametersSubKey = 'SYSTEM\CurrentControlSet\Services\EtwDnstap\Parameters'
$SpoolKeySubKey = $ParametersSubKey + '\SpoolKey'
$WrappedKeyValueName = 'WrappedKey'
# Holds a copy of the WrappedKey value while a major upgrade removes the old product.
# Deleting the service (DeleteServices) deletes its whole registry key, Parameters
# included. This key is outside HKLM\SOFTWARE\EtwDnstap, which the old product removes.
$UpgradeBackupSubKey = 'SOFTWARE\EtwDnstapUpgrade'

function Get-Hklm {
    return [Microsoft.Win32.RegistryKey]::OpenBaseKey(
        [Microsoft.Win32.RegistryHive]::LocalMachine, [Microsoft.Win32.RegistryView]::Registry64)
}

function Get-SpoolKeySddl([string]$ServiceSid) {
    return "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;$SpoolKeyServiceRight;;;$ServiceSid)"
}

# The service queries and sets values (0x20003: query value, set value, read control)
# on this subkey only. It cannot change the DACL or create subkeys.
function Get-SpoolKeySubKeySddl([string]$ServiceSid) {
    return "D:P(A;;KA;;;SY)(A;;KA;;;BA)(A;;0x20003;;;$ServiceSid)"
}

# Throws unless the DACL is protected and holds exactly the ACEs of the expected SDDL.
function Test-DaclMatches {
    param(
        [Parameter(Mandatory = $true)][byte[]]$ActualDescriptor,
        [Parameter(Mandatory = $true)][string]$ExpectedSddl,
        [Parameter(Mandatory = $true)][string]$What
    )
    $actual = New-Object System.Security.AccessControl.RawSecurityDescriptor ($ActualDescriptor, 0)
    $expected = New-Object System.Security.AccessControl.RawSecurityDescriptor $ExpectedSddl
    $protected = [System.Security.AccessControl.ControlFlags]::DiscretionaryAclProtected
    if (($actual.ControlFlags -band $protected) -eq 0) {
        throw "The DACL of $What is not protected (it inherits ACEs). Inspect it."
    }
    # The Microsoft Software KSP stores a key DACL with the generic rights mapped to
    # the key file's specific rights and keeps the generic bits as well (GA reads back as
    # 0xD01F01FF, GR as 0x80120089). Both sides are reduced to the specific rights:
    # the generic bits of the expected side are expanded with the file mapping, and the
    # top byte is dropped on both sides. Registry masks have no generic bits and compare as is.
    $format = {
        param($acl)
        @($acl | ForEach-Object {
                $mask = [int64]$_.AccessMask -band 0xFFFFFFFFL
                if ($mask -band 0x80000000L) { $mask = $mask -bor 0x120089 }
                if ($mask -band 0x40000000L) { $mask = $mask -bor 0x120116 }
                if ($mask -band 0x20000000L) { $mask = $mask -bor 0x1200A0 }
                if ($mask -band 0x10000000L) { $mask = $mask -bor 0x1F01FF }
                '{0} {1} mask=0x{2:X} flags={3}' -f $_.AceQualifier, $_.SecurityIdentifier.Value, ($mask -band 0xFFFFFF), $_.AceFlags
            } | Sort-Object)
    }
    $have = & $format $actual.DiscretionaryAcl
    $want = & $format $expected.DiscretionaryAcl
    if (($have -join '; ') -ne ($want -join '; ')) {
        throw ("The DACL of $What is not as expected. Expected: " + ($want -join '; ') +
            '. Found: ' + ($have -join '; ') + '.')
    }
}

function Initialize-SpoolKey {
    [CmdletBinding()]
    param(
        [string]$Name = $SpoolKeyName,
        [string]$ServiceSid = $ServiceSidText,
        [bool]$MachineKey = $true
    )
    $provider = [System.Security.Cryptography.CngProvider]::MicrosoftSoftwareKeyStorageProvider
    $openOptions = if ($MachineKey) { [System.Security.Cryptography.CngKeyOpenOptions]::MachineKey } else { [System.Security.Cryptography.CngKeyOpenOptions]::None }
    $key = $null
    try {
        if ([System.Security.Cryptography.CngKey]::Exists($Name, $provider, $openOptions)) {
            $key = [System.Security.Cryptography.CngKey]::Open($Name, $provider, $openOptions)
            # A key that is not as the plan says is never replaced: the wrapped spool key
            # in the registry depends on it. An administrator decides.
            if ($key.Algorithm -ne [System.Security.Cryptography.CngAlgorithm]::Rsa -or $key.KeySize -ne $SpoolKeyBits) {
                throw "The existing key '$Name' is not an RSA-$SpoolKeyBits key. Inspect it. Do not use it."
            }
            if ($key.ExportPolicy -ne [System.Security.Cryptography.CngExportPolicies]::None) {
                throw "The existing key '$Name' can be exported. Inspect it. Do not use it."
            }
            Write-Step "Accepted existing key '$Name' (RSA-$SpoolKeyBits, not exportable)."
        }
        else {
            $creation = New-Object System.Security.Cryptography.CngKeyCreationParameters
            $creation.Provider = $provider
            $creation.ExportPolicy = [System.Security.Cryptography.CngExportPolicies]::None
            $creation.KeyCreationOptions = if ($MachineKey) { [System.Security.Cryptography.CngKeyCreationOptions]::MachineKey } else { [System.Security.Cryptography.CngKeyCreationOptions]::None }
            $length = [System.Security.Cryptography.CngProperty]::new(
                'Length', [BitConverter]::GetBytes([int]$SpoolKeyBits), [System.Security.Cryptography.CngPropertyOptions]::None)
            $creation.Parameters.Add($length)
            $key = [System.Security.Cryptography.CngKey]::Create([System.Security.Cryptography.CngAlgorithm]::Rsa, $Name, $creation)
            Write-Step "Created key '$Name' (RSA-$SpoolKeyBits, not exportable)."
        }

        # Reapplied on every install and repair, so that a changed DACL is put back.
        $sddl = Get-SpoolKeySddl $ServiceSid
        $descriptor = New-Object System.Security.AccessControl.RawSecurityDescriptor $sddl
        $bytes = New-Object byte[] $descriptor.BinaryLength
        $descriptor.GetBinaryForm($bytes, 0)
        $property = [System.Security.Cryptography.CngProperty]::new('Security Descr', $bytes, $DaclSecurityInformation)
        $key.SetProperty($property)

        $readBack = $key.GetProperty('Security Descr', $DaclSecurityInformation).GetValue()
        Test-DaclMatches -ActualDescriptor $readBack -ExpectedSddl $sddl -What "key '$Name'"
        Write-Step "Set and checked the DACL of key '$Name': SYSTEM and Administrators all, service SID $ServiceSid $SpoolKeyServiceRight."
    }
    finally {
        if ($null -ne $key) { $key.Dispose() }
    }
}

function Remove-SpoolKey {
    [CmdletBinding()]
    param(
        [string]$Name = $SpoolKeyName,
        [bool]$MachineKey = $true
    )
    $provider = [System.Security.Cryptography.CngProvider]::MicrosoftSoftwareKeyStorageProvider
    $openOptions = if ($MachineKey) { [System.Security.Cryptography.CngKeyOpenOptions]::MachineKey } else { [System.Security.Cryptography.CngKeyOpenOptions]::None }
    if (-not [System.Security.Cryptography.CngKey]::Exists($Name, $provider, $openOptions)) {
        Write-Step "No key '$Name'."
        return
    }
    $key = [System.Security.Cryptography.CngKey]::Open($Name, $provider, $openOptions)
    $key.Delete()   # Delete also releases the handle.
    Write-Step "Deleted key '$Name'."
}

# Opens a key with only the rights asked for, so that a key whose DACL gives less than
# KEY_READ (the SpoolKey subkey in the test) can still be checked. Returns $null if missing.
function Open-RegistryKey {
    param($Hive, [string]$Path, [System.Security.AccessControl.RegistryRights]$Rights, [bool]$Writable = $false)
    $check = if ($Writable) { [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree } else { [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadSubTree }
    return $Hive.OpenSubKey($Path, $check, $Rights)
}
$RegistryExists = [System.Security.AccessControl.RegistryRights]::ReadPermissions
$RegistryReadValue = [System.Security.AccessControl.RegistryRights]::QueryValues
$RegistryWriteValue = [System.Security.AccessControl.RegistryRights]::QueryValues -bor [System.Security.AccessControl.RegistryRights]::SetValue

# Sets a protected DACL on a registry key that exists, then reads it back.
function Set-RegistryKeyDacl {
    param($Hive, [string]$SubKey, [string]$Sddl, [string]$What)
    $rights = [System.Security.AccessControl.RegistryRights]::ChangePermissions -bor
        [System.Security.AccessControl.RegistryRights]::ReadPermissions
    $key = $Hive.OpenSubKey($SubKey, [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree, $rights)
    if ($null -eq $key) { throw "Could not open $What to set its DACL." }
    try {
        $security = New-Object System.Security.AccessControl.RegistrySecurity
        $security.SetSecurityDescriptorSddlForm($Sddl, [System.Security.AccessControl.AccessControlSections]::Access)
        $key.SetAccessControl($security)
        $readBack = $key.GetAccessControl([System.Security.AccessControl.AccessControlSections]::Access).GetSecurityDescriptorBinaryForm()
    }
    finally {
        $key.Dispose()
    }
    Test-DaclMatches -ActualDescriptor $readBack -ExpectedSddl $Sddl -What $What
}

# Creates Parameters only if it is missing, with the default inherited ACL, and never
# changes its ACL. Creates the SpoolKey subkey and sets its protected DACL on every run.
# The WrappedKey value is never touched.
function Initialize-SpoolKeySubKey {
    [CmdletBinding()]
    param(
        $Hive = (Get-Hklm),
        [string]$ParametersPath = $ParametersSubKey,
        [string]$SubKeyPath = $SpoolKeySubKey,
        [string]$ServiceSid = $ServiceSidText
    )
    $parameters = Open-RegistryKey $Hive $ParametersPath $RegistryExists
    if ($null -eq $parameters) {
        $Hive.CreateSubKey($ParametersPath).Dispose()
        Write-Step "Created registry key $ParametersPath (default ACL, not changed)."
    }
    else {
        $parameters.Dispose()
        Write-Step "Accepted existing registry key $ParametersPath (its ACL is not changed)."
    }
    $subKey = Open-RegistryKey $Hive $SubKeyPath $RegistryExists
    if ($null -eq $subKey) {
        $Hive.CreateSubKey($SubKeyPath).Dispose()
        Write-Step "Created registry key $SubKeyPath."
    }
    else {
        $subKey.Dispose()
        Write-Step "Accepted existing registry key $SubKeyPath (an existing $WrappedKeyValueName value is kept)."
    }
    Set-RegistryKeyDacl -Hive $Hive -SubKey $SubKeyPath -Sddl (Get-SpoolKeySubKeySddl $ServiceSid) -What "registry key $SubKeyPath"
    Write-Step "Set and checked the DACL of $SubKeyPath : SYSTEM and Administrators full control, service SID $ServiceSid query value, set value, read control."
}

# Upgrade, old product, before DeleteServices: copy WrappedKey out of the service key.
# A failure is a warning: the service then makes a new spool key at its next start.
function Save-WrappedKeyForUpgrade {
    [CmdletBinding()]
    param(
        $Hive = (Get-Hklm),
        [string]$SubKeyPath = $SpoolKeySubKey,
        [string]$BackupPath = $UpgradeBackupSubKey,
        [string]$BackupSddl = 'D:P(A;;KA;;;SY)(A;;KA;;;BA)'
    )
    try {
        $key = Open-RegistryKey $Hive $SubKeyPath $RegistryReadValue
        $value = $null
        if ($null -ne $key) {
            try { $value = $key.GetValue($WrappedKeyValueName, $null) } finally { $key.Dispose() }
        }
        if ($value -isnot [byte[]]) {
            Write-Step "No $WrappedKeyValueName value in $SubKeyPath to keep for the upgrade."
            return
        }
        $backup = $Hive.CreateSubKey($BackupPath)
        try { $backup.SetValue($WrappedKeyValueName, $value, [Microsoft.Win32.RegistryValueKind]::Binary) } finally { $backup.Dispose() }
        Set-RegistryKeyDacl -Hive $Hive -SubKey $BackupPath -Sddl $BackupSddl -What "registry key $BackupPath"
        Write-Step "Kept the $WrappedKeyValueName value in $BackupPath for the upgrade ($($value.Length) bytes)."
    }
    catch {
        Write-Step ("WARNING: could not keep the $WrappedKeyValueName value for the upgrade. " +
            'The service makes a new spool key at its next start and deletes the spool segments it cannot read. ' +
            'Error: ' + $_.Exception.Message)
    }
}

# Install, new product: put the kept WrappedKey back if the service key lost it, then
# delete the copy. Needs the SpoolKey subkey to exist.
function Restore-WrappedKeyAfterUpgrade {
    [CmdletBinding()]
    param(
        $Hive = (Get-Hklm),
        [string]$SubKeyPath = $SpoolKeySubKey,
        [string]$BackupPath = $UpgradeBackupSubKey
    )
    $backup = Open-RegistryKey $Hive $BackupPath $RegistryReadValue
    if ($null -eq $backup) { return }
    try { $saved = $backup.GetValue($WrappedKeyValueName, $null) } finally { $backup.Dispose() }
    if ($saved -is [byte[]]) {
        $key = Open-RegistryKey $Hive $SubKeyPath $RegistryWriteValue $true
        try {
            if ($null -eq $key.GetValue($WrappedKeyValueName, $null)) {
                $key.SetValue($WrappedKeyValueName, $saved, [Microsoft.Win32.RegistryValueKind]::Binary)
                Write-Step "Restored the $WrappedKeyValueName value from $BackupPath ($($saved.Length) bytes)."
            }
            else {
                Write-Step "The $WrappedKeyValueName value is still in $SubKeyPath. The copy is not needed."
            }
        }
        finally { $key.Dispose() }
    }
    $Hive.DeleteSubKeyTree($BackupPath, $false)
    Write-Step "Deleted registry key $BackupPath."
}

# Deletes the SpoolKey subkey and the upgrade copy. Parameters and its other values stay.
function Remove-SpoolKeySubKey {
    [CmdletBinding()]
    param(
        $Hive = (Get-Hklm),
        [string]$SubKeyPath = $SpoolKeySubKey,
        [string]$BackupPath = $UpgradeBackupSubKey
    )
    foreach ($path in $SubKeyPath, $BackupPath) {
        $key = Open-RegistryKey $Hive $path $RegistryExists
        if ($null -eq $key) {
            Write-Step "No registry key $path."
            continue
        }
        $key.Dispose()
        $Hive.DeleteSubKeyTree($path, $false)
        Write-Step "Deleted registry key $path."
    }
}

# ---- W7 install steps ------------------------------------------------------------
function Initialize-DataFolders {
    if (Test-Path -LiteralPath $DataRoot) {
        Test-ProtectedFolder $DataRoot
        Write-Step "Accepted existing folder $DataRoot (protected)."
    }
    else {
        New-ProtectedFolder $DataRoot
        Write-Step "Created $DataRoot (protected)."
    }

    if (Test-Path -LiteralPath $SpoolDirectory) {
        Test-SpoolFolder $SpoolDirectory
        Write-Step "Accepted existing spool folder $SpoolDirectory (as expected)."
    }
    else {
        New-ProtectedFolder $SpoolDirectory
        icacls.exe $SpoolDirectory /grant "NT SERVICE\${ServiceName}:(RX,W)" | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'icacls grant on the spool folder failed (folder itself).' }
        icacls.exe $SpoolDirectory /grant "NT SERVICE\${ServiceName}:(OI)(CI)(IO)(M)" | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'icacls grant on the spool folder failed (contents).' }
        Test-SpoolFolder $SpoolDirectory
        Write-Step "Created spool folder $SpoolDirectory with the service's rights."
    }
}

function Get-PerformanceLogUsersName {
    # net.exe, Get-LocalGroupMember and the NetLocalGroup functions need the group name
    # without the BUILTIN\ part.
    $account = (New-Object Security.Principal.SecurityIdentifier $PerformanceLogUsersSid).
        Translate([Security.Principal.NTAccount]).Value
    return $account.Substring($account.IndexOf([char]92) + 1)
}

# Local group membership by SID, through NetLocalGroupGetMembers and
# NetLocalGroupDelMembers at level 0 (LOCALGROUP_MEMBERS_INFO_0, a SID only). Neither
# looks up an account name, so a member SID that no longer resolves (the service was
# deleted) is read and removed like any other. The alternatives fail on such a member:
# Translate() on NT SERVICE\EtwDnstap throws IdentityNotMappedException,
# Get-LocalGroupMember in Windows PowerShell 5.1 throws on any orphaned member SID in
# the group, and net.exe takes account names only. The ADSI WinNT provider binds a
# WinNT://S-1-... path as a domain name, and IsMember() with such a path returned
# False for a real member when tested.
#
# Add-Type compiles this with csc.exe, which application control, antivirus, or
# constrained language mode can block. The uninstall path uses only this, because the
# service may be gone. The install path uses it only when the name-based check fails
# (Test-ServiceInGroupForInstall). A failed compile is kept and thrown again, so the
# script never compiles it twice.
$script:LocalGroupApiError = $null
function Initialize-LocalGroupApi {
    if ('EtwDnstap.LocalGroup' -as [type]) { return }
    if ($null -ne $script:LocalGroupApiError) { throw $script:LocalGroupApiError }
    try {
        Add-LocalGroupType
    }
    catch {
        $script:LocalGroupApiError = 'Add-Type could not compile the NetLocalGroup functions: ' +
            $_.Exception.Message
        throw $script:LocalGroupApiError
    }
}

function Add-LocalGroupType {
    Add-Type -Namespace EtwDnstap -Name LocalGroup -MemberDefinition @'
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
private struct LOCALGROUP_MEMBERS_INFO_0 { public System.IntPtr Sid; }

[System.Runtime.InteropServices.DllImport("netapi32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
private static extern uint NetLocalGroupGetMembers(string serverName, string localGroupName, uint level,
    out System.IntPtr buffer, uint preferredMaximumLength, out uint entriesRead, out uint totalEntries,
    ref System.IntPtr resumeHandle);

[System.Runtime.InteropServices.DllImport("netapi32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
private static extern uint NetLocalGroupDelMembers(string serverName, string groupName, uint level,
    LOCALGROUP_MEMBERS_INFO_0[] buffer, uint totalEntries);

[System.Runtime.InteropServices.DllImport("netapi32.dll")]
private static extern uint NetApiBufferFree(System.IntPtr buffer);

private const uint ErrorMoreData = 234;
private const uint ErrorMemberNotInAlias = 1377;

// The SID text of every member of the local group. Throws Win32Exception on failure.
public static string[] GetMemberSids(string groupName) {
    var sids = new System.Collections.Generic.List<string>();
    var resume = System.IntPtr.Zero;
    int size = System.Runtime.InteropServices.Marshal.SizeOf(typeof(LOCALGROUP_MEMBERS_INFO_0));
    uint status;
    do {
        System.IntPtr buffer;
        uint read;
        uint total;
        status = NetLocalGroupGetMembers(null, groupName, 0, out buffer, 0xFFFFFFFF, out read, out total, ref resume);
        try {
            if (status != 0 && status != ErrorMoreData) {
                throw new System.ComponentModel.Win32Exception((int)status);
            }
            for (uint i = 0; i < read; i++) {
                var entry = (LOCALGROUP_MEMBERS_INFO_0)System.Runtime.InteropServices.Marshal.PtrToStructure(
                    new System.IntPtr(buffer.ToInt64() + (long)i * size), typeof(LOCALGROUP_MEMBERS_INFO_0));
                sids.Add(new System.Security.Principal.SecurityIdentifier(entry.Sid).Value);
            }
        }
        finally {
            if (buffer != System.IntPtr.Zero) { NetApiBufferFree(buffer); }
        }
    } while (status == ErrorMoreData);
    return sids.ToArray();
}

// Removes one member by SID. Returns false if it was not a member. Throws Win32Exception
// on any other failure.
public static bool RemoveMemberSid(string groupName, string sidText) {
    var sid = new System.Security.Principal.SecurityIdentifier(sidText);
    var bytes = new byte[sid.BinaryLength];
    sid.GetBinaryForm(bytes, 0);
    var handle = System.Runtime.InteropServices.GCHandle.Alloc(bytes, System.Runtime.InteropServices.GCHandleType.Pinned);
    try {
        var entries = new LOCALGROUP_MEMBERS_INFO_0[1];
        entries[0].Sid = handle.AddrOfPinnedObject();
        uint status = NetLocalGroupDelMembers(null, groupName, 0, entries, 1);
        if (status == ErrorMemberNotInAlias) { return false; }
        if (status != 0) { throw new System.ComponentModel.Win32Exception((int)status); }
        return true;
    }
    finally {
        handle.Free();
    }
}
'@
}

# By the service SID, through netapi32. Works when the service no longer exists.
function Test-ServiceInGroup([string]$GroupName) {
    Initialize-LocalGroupApi
    $members = @([EtwDnstap.LocalGroup]::GetMemberSids($GroupName))
    return ($members -contains $ServiceSidText)
}

# Install only. The service exists here (InstallServices ran, and Test-SpoolFolder
# checked that NT SERVICE\EtwDnstap resolves to the expected SID), so the name-based
# check works and needs no compiler. If it throws (for example on an orphaned member
# SID in the group), the SID-based check runs once. If both fail, the install fails.
function Test-ServiceInGroupForInstall([string]$GroupName) {
    $nameError = $null
    try {
        $serviceSid = (New-Object Security.Principal.NTAccount "NT SERVICE\$ServiceName").
            Translate([Security.Principal.SecurityIdentifier]).Value
        $members = @(Get-LocalGroupMember -Group $GroupName -ErrorAction Stop | ForEach-Object { $_.SID.Value })
        return ($members -contains $serviceSid)
    }
    catch {
        $nameError = $_.Exception.Message
    }
    Write-Step "The name-based membership check on $GroupName failed ($nameError). Reading the members by SID instead."
    try {
        return (Test-ServiceInGroup $GroupName)
    }
    catch {
        throw "Could not read the members of $GroupName. By name: $nameError By SID: $($_.Exception.Message)"
    }
}

# The grant script has ConfirmImpact High, so it asks before each change unless
# -Confirm:$false is given, and a non-interactive host cannot answer. Named parameters
# go through a hashtable: an array splat passes '-Remove' as a positional string.
function Invoke-Grant([hashtable]$Parameters) {
    if (-not (Test-Path -LiteralPath $GrantScript)) { throw "Grant script not found at $GrantScript." }
    & $GrantScript @Parameters -Confirm:$false
}

function Invoke-Install {
    $endpoint = Test-Receiver $Receiver
    Write-Step "Receiver $endpoint accepted."

    Initialize-DataFolders

    # The key and the subkey first: the service that starts after the restart needs both.
    Initialize-SpoolKey
    Initialize-SpoolKeySubKey
    Restore-WrappedKeyAfterUpgrade

    $group = Get-PerformanceLogUsersName
    if (Test-ServiceInGroupForInstall $group) {
        Write-Step "NT SERVICE\$ServiceName is already a member of $group."
    }
    else {
        net.exe localgroup "$group" "NT SERVICE\$ServiceName" /add | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "net localgroup /add failed (exit code $LASTEXITCODE)." }
        Write-Step "Added NT SERVICE\$ServiceName to $group."
    }

    Invoke-Grant @{ SessionName = $SessionName }
    Write-Step "Ran the ETW grant for session '$SessionName'. Restart the computer before the service starts."

    # install.md step 5. Reset after one day, restart after 5 s, 5 s, then 60 s. The
    # non-crash failure flag is set by the ServiceConfig row in Product.wxs.
    & sc.exe failure $ServiceName reset= 86400 actions= restart/5000/restart/5000/restart/60000 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "sc.exe failure failed (exit code $LASTEXITCODE)." }
    Write-Step 'Set the recovery policy: restart after 5 s, 5 s, 60 s; reset after 86400 s.'
}

# ---- W7 uninstall steps ----------------------------------------------------------
function Stop-LeftoverSession {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param()
    if (-not $PSCmdlet.ShouldProcess($SessionName, 'Stop leftover ETW session')) { return }
    $names = @(logman.exe query -ets 2>$null | ForEach-Object { ($_.Trim() -split '\s+')[0] })
    if ($names -contains $SessionName) {
        logman.exe stop $SessionName -ets | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "logman stop $SessionName -ets failed (exit code $LASTEXITCODE)." }
        Write-Step "Stopped the leftover ETW session '$SessionName'."
    }
    else {
        Write-Step "No ETW session '$SessionName' is running."
    }
}

# Deletes the files and folders under Path, never following a reparse point.
# Returns $true when Path is gone. A reparse point or an unexpected item is kept
# and listed; the remaining content stays.
function Remove-OwnedTree {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param([string]$Path)
    if (-not $PSCmdlet.ShouldProcess($Path, 'Remove owned content')) { return $false }
    if (-not (Test-Path -LiteralPath $Path)) { return $true }
    $item = Get-Item -LiteralPath $Path -Force
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        Write-Step "KEPT: '$Path' is a reparse point. Inspect it. It was not followed or deleted."
        return $false
    }
    if (-not $item.PSIsContainer) {
        Remove-Item -LiteralPath $Path -Force -Confirm:$false
        Write-Step "Removed file $Path."
        return $true
    }
    $complete = $true
    foreach ($child in @(Get-ChildItem -LiteralPath $Path -Force)) {
        if (-not (Remove-OwnedTree -Path $child.FullName)) { $complete = $false }
    }
    if ($complete) {
        Remove-Item -LiteralPath $Path -Force -Confirm:$false
        Write-Step "Removed folder $Path."
    }
    else {
        Write-Step "KEPT: '$Path' still holds content that was kept."
    }
    return $complete
}

# Uninstall only. A failure here is a warning: the grant removal and the folder removal
# must still run, and a membership that is left behind grants nothing once the service
# and its ETW grant are gone.
function Remove-ServiceFromGroup {
    try {
        $group = Get-PerformanceLogUsersName
        if (-not (Test-ServiceInGroup $group)) {
            Write-Step "Service SID $ServiceSidText is not in $group."
        }
        elseif ([EtwDnstap.LocalGroup]::RemoveMemberSid($group, $ServiceSidText)) {
            Write-Step "Removed service SID $ServiceSidText (NT SERVICE\$ServiceName) from $group."
        }
        else {
            Write-Step "Service SID $ServiceSidText is not in $group."
        }
    }
    catch {
        Write-Step ("WARNING: could not read or remove the membership of service SID $ServiceSidText " +
            "in Performance Log Users ($PerformanceLogUsersSid). The uninstall continues. " +
            'Remove the SID from the group by hand. Error: ' + $_.Exception.Message)
    }
}

function Invoke-Uninstall {
    Stop-LeftoverSession

    Remove-ServiceFromGroup

    Invoke-Grant @{ Remove = $true }
    Write-Step 'Removed the ETW grants of the service SID.'

    Remove-SpoolKeySubKey
    Remove-SpoolKey

    if (Test-Path -LiteralPath $DataRoot) {
        $dataItem = Get-Item -LiteralPath $DataRoot -Force
        if ($dataItem.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            Write-Step "KEPT: '$DataRoot' is a reparse point. Inspect it."
        }
        else {
            [void](Remove-OwnedTree -Path $SpoolDirectory)
            [void](Remove-OwnedTree -Path $BackupDirectory)
            $left = @(Get-ChildItem -LiteralPath $DataRoot -Force)
            if ($left.Count -eq 0) {
                Remove-Item -LiteralPath $DataRoot -Force -Confirm:$false
                Write-Step "Removed empty folder $DataRoot."
            }
            else {
                foreach ($item in $left) { Write-Step "KEPT (not created by EtwDnstap): $($item.FullName)" }
            }
        }
    }
    else {
        Write-Step "No folder $DataRoot."
    }
}

try {
    switch ($Action) {
        'Install' { Invoke-Install }
        'Uninstall' { Invoke-Uninstall }
        'PreserveForUpgrade' { Save-WrappedKeyForUpgrade }
    }
    exit 0
}
catch {
    Write-Output ('[EtwDnstap] FAILED: ' + $_.Exception.Message)
    exit 1
}
