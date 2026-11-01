#requires -Version 5.1
<#
.SYNOPSIS
Grants the EtwDnstap service SID the ETW rights its write-restricted token needs.

.DESCRIPTION
EtwDnstap runs as NT AUTHORITY\LocalService with a RESTRICTED service SID type
(sc.exe sidtype EtwDnstap restricted). The SCM gives such a service a
write-restricted token. For write-type access checks the kernel makes a second
pass against the token's restricting SIDs (the service SID, Everyone, the logon
SID and S-1-5-33). A grant to Performance Log Users alone therefore may not
satisfy ETW's write-type rights, so this script grants the rights directly to
the service SID (NT SERVICE\EtwDnstap, S-1-5-80-...).

Two WMI/ETW security GUIDs are changed, using advapi32!EventAccessControl:

  1. Provider {eb79061a-a566-4698-9119-3ed2807060e7}
     (Microsoft-Windows-DNSServer).
       TRACELOG_GUID_ENABLE (0x80)  lets EnableTraceEx2 enable the provider in
       the service's own real-time session.

  2. Default WMI security GUID {0811c1af-7a07-4a06-82ed-869455cdf713}
     (the security descriptor that governs ordinary trace sessions).
       WMIGUID_QUERY            (0x001)  query the session (ControlTrace query).
       WMIGUID_SET              (0x002)  update, flush and stop the session.
       TRACELOG_CREATE_REALTIME (0x020)  StartTrace for a real-time session.
       TRACELOG_ACCESS_REALTIME (0x400)  OpenTrace/ProcessTrace on the session.
     Override the mask with -SessionRights.

Membership of NT SERVICE\EtwDnstap in Performance Log Users is still required
(the service refuses to start without that group enabled in its token). This
script does not change group membership:

  net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add

-Remove reverts the grants for uninstall. EventAccessControl can only add or
replace, so removal reads the current descriptor with EventAccessQuery, strips
the ACEs for the service SID, and writes the descriptor back to
HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security\<guid>, the value that
EventAccessControl itself maintains. It never calls EventAccessRemove, which
would delete the whole descriptor, including the system defaults.

The service SID is derived from the service name (SHA-1 of the upper-cased
name), so -Remove also works after 'sc.exe delete'.

NOT VALIDATED on a live DNS Server. The rights masks and the effect of a
write-restricted token on ETW access checks are assumptions. Use a disposable
host first.

.PARAMETER ServiceName
Service whose service SID receives the grants. Default: EtwDnstap.

.PARAMETER SessionRights
Access mask granted on the default WMI security GUID. Default: 0x423
(WMIGUID_QUERY | WMIGUID_SET | TRACELOG_CREATE_REALTIME | TRACELOG_ACCESS_REALTIME).

.PARAMETER Remove
Remove the ACEs this script adds instead of adding them.

.EXAMPLE
.\Grant-EtwDnstapEtwAccess.ps1 -WhatIf

Shows what would be granted, without changing anything.

.EXAMPLE
.\Grant-EtwDnstapEtwAccess.ps1

Grants the rights and prints the resulting SDDL for both GUIDs.

.EXAMPLE
.\Grant-EtwDnstapEtwAccess.ps1 -Remove

Removes the grants (run during uninstall).
#>
[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [ValidatePattern('^[A-Za-z0-9_.-]+$')]
    [string]$ServiceName = 'EtwDnstap',

    [uint32]$SessionRights = 0x423,

    [switch]$Remove
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ProviderGuid = [Guid]'eb79061a-a566-4698-9119-3ed2807060e7'
$SessionGuid = [Guid]'0811c1af-7a07-4a06-82ed-869455cdf713'
$TRACELOG_GUID_ENABLE = [uint32]0x80
# EVENTSECURITYOPERATION: SetDACL = 0, SetSACL = 1, AddDACL = 2, AddSACL = 3.
$EventSecurityAddDACL = [uint32]2
$WmiSecurityKey = 'HKLM:\SYSTEM\CurrentControlSet\Control\WMI\Security'
$ERROR_MORE_DATA = 234
$ERROR_FILE_NOT_FOUND = 2

if (-not ('EtwDnstap.EtwSecurity' -as [type])) {
    Add-Type -Namespace EtwDnstap -Name EtwSecurity -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("advapi32.dll")]
public static extern uint EventAccessControl(
    ref System.Guid Guid,
    uint Operation,
    byte[] Sid,
    uint Rights,
    [System.Runtime.InteropServices.MarshalAs(System.Runtime.InteropServices.UnmanagedType.U1)] bool AllowOrDeny);

[System.Runtime.InteropServices.DllImport("advapi32.dll")]
public static extern uint EventAccessQuery(
    ref System.Guid Guid,
    byte[] Buffer,
    ref uint BufferSize);
'@
}

function Test-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

# The service SID is S-1-5-80 plus the five little-endian DWORDs of the SHA-1
# hash of the upper-cased service name in UTF-16LE.
function Get-ServiceSidFromName {
    param([string]$Name)
    $sha1 = [Security.Cryptography.SHA1]::Create()
    try {
        $bytes = [Text.Encoding]::Unicode.GetBytes($Name.ToUpperInvariant())
        $hash = $sha1.ComputeHash($bytes)
    }
    finally {
        $sha1.Dispose()
    }
    $parts = for ($i = 0; $i -lt 5; $i++) { [BitConverter]::ToUInt32($hash, 4 * $i) }
    return New-Object Security.Principal.SecurityIdentifier("S-1-5-80-$($parts -join '-')")
}

# Returns the DACL descriptor for a GUID, or $null when no custom descriptor
# exists for it.
function Get-EtwSecurityDescriptor {
    param([Guid]$Guid)
    $size = [uint32]0
    $status = [EtwDnstap.EtwSecurity]::EventAccessQuery([ref]$Guid, $null, [ref]$size)
    if ($status -eq $ERROR_FILE_NOT_FOUND) { return $null }
    if ($status -ne $ERROR_MORE_DATA -and $status -ne 0) {
        throw "EventAccessQuery($Guid) failed with Win32 error $status."
    }
    $buffer = New-Object byte[] $size
    $status = [EtwDnstap.EtwSecurity]::EventAccessQuery([ref]$Guid, $buffer, [ref]$size)
    if ($status -ne 0) {
        throw "EventAccessQuery($Guid) failed with Win32 error $status."
    }
    return New-Object Security.AccessControl.RawSecurityDescriptor($buffer, 0)
}

function Get-AllowedMask {
    param($Descriptor, [Security.Principal.SecurityIdentifier]$Sid)
    $mask = [uint32]0
    if ($null -eq $Descriptor -or $null -eq $Descriptor.DiscretionaryAcl) { return $mask }
    foreach ($ace in $Descriptor.DiscretionaryAcl) {
        if ($ace -is [Security.AccessControl.CommonAce] -and
            $ace.AceType -eq [Security.AccessControl.AceType]::AccessAllowed -and
            $ace.SecurityIdentifier -eq $Sid) {
            $mask = $mask -bor [uint32]$ace.AccessMask
        }
    }
    return $mask
}

function Show-EtwSecurity {
    param([Guid]$Guid, [string]$Label)
    $descriptor = Get-EtwSecurityDescriptor -Guid $Guid
    if ($null -eq $descriptor) {
        Write-Output "$Label {$Guid}: no custom descriptor (system default applies)"
    }
    else {
        Write-Output "$Label {$Guid} SDDL:"
        Write-Output ('  ' + $descriptor.GetSddlForm([Security.AccessControl.AccessControlSections]::Access))
    }
}

function Grant-EtwRight {
    param([Guid]$Guid, [Security.Principal.SecurityIdentifier]$Sid, [uint32]$Rights, [string]$Why)
    $current = Get-AllowedMask -Descriptor (Get-EtwSecurityDescriptor -Guid $Guid) -Sid $Sid
    if (($current -band $Rights) -eq $Rights) {
        Write-Output "Already granted 0x$($Rights.ToString('x')) on {$Guid} to $Sid."
        return
    }
    if ($PSCmdlet.ShouldProcess("{$Guid}", "Grant 0x$($Rights.ToString('x')) to $Sid ($Why)")) {
        $sidBytes = New-Object byte[] $Sid.BinaryLength
        $Sid.GetBinaryForm($sidBytes, 0)
        $status = [EtwDnstap.EtwSecurity]::EventAccessControl(
            [ref]$Guid, $EventSecurityAddDACL, $sidBytes, $Rights, $true)
        if ($status -ne 0) {
            throw "EventAccessControl(AddDACL, {$Guid}) failed with Win32 error $status."
        }
        Write-Output "Granted 0x$($Rights.ToString('x')) on {$Guid} to $Sid ($Why)."
    }
}

function Revoke-EtwRight {
    param([Guid]$Guid, [Security.Principal.SecurityIdentifier]$Sid)
    $descriptor = Get-EtwSecurityDescriptor -Guid $Guid
    if ($null -eq $descriptor -or $null -eq $descriptor.DiscretionaryAcl) {
        Write-Output "No custom descriptor for {$Guid}; nothing to remove."
        return
    }
    $acl = $descriptor.DiscretionaryAcl
    $removed = 0
    for ($i = $acl.Count - 1; $i -ge 0; $i--) {
        $ace = $acl[$i]
        if ($ace -is [Security.AccessControl.CommonAce] -and $ace.SecurityIdentifier -eq $Sid) {
            $acl.RemoveAce($i)
            $removed++
        }
    }
    if ($removed -eq 0) {
        Write-Output "No ACE for $Sid on {$Guid}; nothing to remove."
        return
    }
    if ($PSCmdlet.ShouldProcess("{$Guid}", "Remove $removed ACE(s) for $Sid")) {
        $binary = New-Object byte[] $descriptor.BinaryLength
        $descriptor.GetBinaryForm($binary, 0)
        Set-ItemProperty -Path $WmiSecurityKey -Name $Guid.ToString() -Value $binary -Type Binary
        Write-Output "Removed $removed ACE(s) for $Sid on {$Guid}."
    }
}

if (-not (Test-Administrator)) {
    throw 'Run this script from an elevated PowerShell session.'
}

$sid = Get-ServiceSidFromName -Name $ServiceName
Write-Output "Service SID for NT SERVICE\$ServiceName : $sid"
try {
    $translated = (New-Object Security.Principal.NTAccount("NT SERVICE\$ServiceName")).Translate(
        [Security.Principal.SecurityIdentifier])
    if ($translated -ne $sid) {
        throw "Derived SID $sid differs from the system-resolved SID $translated."
    }
}
catch [Security.Principal.IdentityNotMappedException] {
    Write-Verbose "NT SERVICE\$ServiceName does not resolve (service not registered); using the derived SID."
}

if ($Remove) {
    Revoke-EtwRight -Guid $ProviderGuid -Sid $sid
    Revoke-EtwRight -Guid $SessionGuid -Sid $sid
}
else {
    Grant-EtwRight -Guid $ProviderGuid -Sid $sid -Rights $TRACELOG_GUID_ENABLE `
        -Why 'TRACELOG_GUID_ENABLE: enable the Microsoft-Windows-DNSServer provider'
    Grant-EtwRight -Guid $SessionGuid -Sid $sid -Rights $SessionRights `
        -Why 'create, query, control and consume a real-time session'
}

Show-EtwSecurity -Guid $ProviderGuid -Label 'Provider'
Show-EtwSecurity -Guid $SessionGuid -Label 'Default session security'
