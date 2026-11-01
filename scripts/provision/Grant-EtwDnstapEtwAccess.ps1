#requires -Version 5.1
<#
.SYNOPSIS
Grants the EtwDnstap service SID TRACELOG_CREATE_REALTIME on its own ETW
session GUID, and nowhere else.

.DESCRIPTION
EtwDnstap only consumes events; it never logs events into ETW. It is the
controller of its own real-time session: it creates the session (StartTrace),
enables the Microsoft-Windows-DNSServer provider in it (EnableTraceEx2),
consumes it (OpenTrace/ProcessTrace), and queries and stops it (ControlTrace).

The service runs as NT AUTHORITY\LocalService with a RESTRICTED service SID type
(sc.exe sidtype EtwDnstap restricted). Windows calls the resulting token
"write-restricted": for any access that Windows classes as a write (a change of
state, not ETW event data), a second access check considers only the
restricting SIDs (the service SID, Everyone, the logon SID and S-1-5-33).
Creating a session is such a change, so the service SID itself needs
TRACELOG_CREATE_REALTIME (0x20). Everything else passes on the defaults.

ETW checks that right against the security descriptor of the session's GUID,
and falls back to the shared default descriptor
{0811c1af-7a07-4a06-82ed-869455cdf713} when that GUID has none. EtwDnstap starts
its session with a stable name-based GUID, so this script grants the right on
that GUID only. Windows scopes its own services' ETW grants the same way
(NT SERVICE\DPS, WdiServiceHost, EventLog, BFE and mpssvc hold rights on
specific GUIDs, never on the default). Editing the default instead would also
leak the grant into every descriptor that other components later copy from it.

The session GUID is SHA-1 over the UTF-16LE text 'EtwDnstap.Session:<name>',
with the RFC 9562 version-5 and variant bits set, read as new Guid(byte[16]).
The name is hashed exactly as given (no case folding); pass the same
--session-name the service uses. For the default name EtwDnstap the GUID is
{05e1db3c-1322-58c7-8235-0463fccba32b}.

The new descriptor of the session GUID is seeded from the default descriptor,
so the principals that the default descriptor lets create sessions (for
example Administrators, LocalService, NetworkService and Performance Log Users)
can also create a session with that GUID. That is harmless: it adds no right
that those principals lack on any other GUID without a descriptor of its own.

Every run also strips ACEs for the service SID from every other descriptor
under HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security. That removes grants
made by earlier versions of this script (on the default GUID and on the DNS
provider GUID) and copies of them that other components made. A non-default
descriptor that, without those ACEs, equals the default descriptor is deleted,
so ETW falls back to the default exactly as before. Nothing else is changed.
A descriptor that cannot be parsed is left unchanged, with a warning. If the
descriptor of a session GUID exists and cannot be parsed, the script stops with
an error and does not grant on it.

Before the first change, the script saves the state it is about to change in
-BackupPath (default $env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>, with an
ACL for SYSTEM and Administrators only):

  wmi-security.reg  reg.exe export of HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security
  changes.json      the original bytes of each value that the run changes or
                    deletes, and the names of the values that the run creates

-WhatIf changes nothing, and that includes the backup. A run that changes
nothing creates no backup. -Restore <path> writes the original bytes back and
deletes the values that the run created.

The backup location is not trusted by default, because a standard user can
create folders in ProgramData. The script creates ProgramData\EtwDnstap,
\backup and the run's folder with owner Administrators, no inherited ACEs, and
full control for SYSTEM and Administrators only. It stops with an error if one
of those levels already exists and is a reparse point, is owned by anyone else,
or lets another principal write to it. It does not take ownership of such a
folder. It never reuses an existing backup folder. -BackupPath must lie below
ProgramData\EtwDnstap\backup. -Restore applies the same checks to changes.json
and to every folder above it, and it validates every entry before it applies
any.

changes.json also holds a SHA-256 of the bytes that the run wrote. -Restore
skips a value that holds other bytes now, with a warning, because something
changed it after the run. -Force overwrites it. A restore prints a summary of
the values restored, deleted and skipped.

ETW applies these descriptors at boot. RESTART THE COMPUTER after running this
script, after -Remove, and after -Restore.

Membership of NT SERVICE\EtwDnstap in Performance Log Users is still required
by the service's startup check. This script does not change group membership:

  net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add

Validated on Windows Server 2025, build 26100, on 2026-11-01. See
docs/history/localservice-restricted-validation.md.

.PARAMETER SessionName
Every --session-name an EtwDnstap instance on this host uses. Default: EtwDnstap.
Grants for session names not listed are removed.

.PARAMETER ServiceName
Service whose service SID receives the grant. Default: EtwDnstap.

.PARAMETER Remove
Remove every ACE for the service SID from every ETW descriptor (uninstall).

.PARAMETER BackupPath
Directory for the registry export and changes.json. It must lie below
$env:ProgramData\EtwDnstap\backup and must not exist. The script creates it, with
an ACL for SYSTEM and Administrators only, immediately before its first change.
Default: $env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>.

.PARAMETER Restore
Path of a backup directory (or of its changes.json) from an earlier run. Writes
the original bytes back, deletes the values that run created, and reminds the
operator to restart the computer. Cannot be combined with the other parameters
except -Force.

.PARAMETER Force
With -Restore: also overwrite or delete a value that holds other bytes than the
run wrote.

.EXAMPLE
.\Grant-EtwDnstapEtwAccess.ps1 -WhatIf

Shows the session GUID and every change, without making any.

.EXAMPLE
.\Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap

Grants the right on the default session's GUID. Restart the computer afterwards.

.EXAMPLE
.\Grant-EtwDnstapEtwAccess.ps1 -Remove

Removes all of the service SID's ETW grants. Restart the computer afterwards.

.EXAMPLE
.\Grant-EtwDnstapEtwAccess.ps1 -Restore "$env:ProgramData\EtwDnstap\backup\20261101120000"

Puts back the descriptors that the run saved. Restart the computer afterwards.
#>
[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High', DefaultParameterSetName = 'Grant')]
param(
    [Parameter(ParameterSetName = 'Grant')]
    [ValidateNotNullOrEmpty()]
    [string[]]$SessionName = @('EtwDnstap'),

    [Parameter(ParameterSetName = 'Grant')]
    [ValidatePattern('^[A-Za-z0-9_.-]+$')]
    [string]$ServiceName = 'EtwDnstap',

    [Parameter(ParameterSetName = 'Grant')]
    [switch]$Remove,

    [Parameter(ParameterSetName = 'Grant')]
    [ValidateNotNullOrEmpty()]
    [string]$BackupPath = (Join-Path $env:ProgramData ('EtwDnstap\backup\' + (Get-Date -Format 'yyyyMMddHHmmssfff'))),

    [Parameter(Mandatory = $true, ParameterSetName = 'Restore')]
    [ValidateNotNullOrEmpty()]
    [string]$Restore,

    [Parameter(ParameterSetName = 'Restore')]
    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$DefaultGuid = '0811c1af-7a07-4a06-82ed-869455cdf713'
$TRACELOG_CREATE_REALTIME = [uint32]0x20
# EVENTSECURITYOPERATION: SetDACL = 0, SetSACL = 1, AddDACL = 2, AddSACL = 3.
$EventSecurityAddDACL = [uint32]2
$WmiSecurityKey = 'HKLM:\SYSTEM\CurrentControlSet\Control\WMI\Security'
$WmiSecurityKeyNative = 'HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security'
$Sections = [Security.AccessControl.AccessControlSections]'Owner, Group, Access'

# Backup state. Nothing is written to disk until Initialize-Backup runs, and
# Initialize-Backup runs only from inside a change that ShouldProcess approved, so
# -WhatIf never creates a backup.
$script:BackupStarted = $false
$script:ChangedValues = New-Object System.Collections.Generic.List[object]
$script:CreatedValues = New-Object System.Collections.Generic.List[object]

if (-not ('EtwDnstap.EtwSecurity' -as [type])) {
    Add-Type -Namespace EtwDnstap -Name EtwSecurity -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("advapi32.dll")]
public static extern uint EventAccessControl(
    ref System.Guid Guid,
    uint Operation,
    byte[] Sid,
    uint Rights,
    [System.Runtime.InteropServices.MarshalAs(System.Runtime.InteropServices.UnmanagedType.U1)] bool AllowOrDeny);
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
        $hash = $sha1.ComputeHash([Text.Encoding]::Unicode.GetBytes($Name.ToUpperInvariant()))
    }
    finally {
        $sha1.Dispose()
    }
    $parts = for ($i = 0; $i -lt 5; $i++) { [BitConverter]::ToUInt32($hash, 4 * $i) }
    return New-Object Security.Principal.SecurityIdentifier("S-1-5-80-$($parts -join '-')")
}

# Same derivation as src/etw/session_guid.rs.
function Get-SessionGuid {
    param([string]$Name)
    $sha1 = [Security.Cryptography.SHA1]::Create()
    try {
        $hash = $sha1.ComputeHash([Text.Encoding]::Unicode.GetBytes("EtwDnstap.Session:$Name"))
    }
    finally {
        $sha1.Dispose()
    }
    $bytes = [byte[]]$hash[0..15]
    $bytes[7] = ($bytes[7] -band 0x0F) -bor 0x50
    $bytes[8] = ($bytes[8] -band 0x3F) -bor 0x80
    return [Guid]::new($bytes)
}

function Read-Descriptor {
    param([string]$Name)
    $value = (Get-Item -LiteralPath $WmiSecurityKey).GetValue($Name)
    if ($null -eq $value) { return $null }
    try { return New-Object Security.AccessControl.RawSecurityDescriptor([byte[]]$value, 0) }
    catch {
        Write-Warning "Descriptor {$Name} is unparseable; left unchanged. $($_.Exception.Message)"
        return $null
    }
}

$TrustedOwners = @('S-1-5-18', 'S-1-5-32-544')
$BackupRoot = Join-Path $env:ProgramData 'EtwDnstap\backup'
# FileSystemRights bits that let a principal change a folder or a file in it:
# write data, append, write EA, delete child, write attributes, delete, change
# permissions, take ownership, plus the generic all, write and execute-free bits.
$WriteMask = [int64](0x2 -bor 0x4 -bor 0x10 -bor 0x40 -bor 0x100 -bor 0x10000 -bor 0x40000 -bor 0x80000 -bor 0x10000000 -bor 0x40000000)

function Get-FullPathNoSlash {
    param([string]$Path)
    return [IO.Path]::GetFullPath($Path).TrimEnd('\')
}

# Throws unless Path is a real folder (or file) that only SYSTEM and
# Administrators own, that is not a reparse point, and, with -CheckAcl, that no
# other principal can write to, delete or re-permission. The script never takes
# ownership of an object that someone else made. The operator must inspect it.
function Assert-TrustedObject {
    param([string]$Path, [switch]$CheckAcl)
    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "'$Path' is a reparse point (junction or link). Do not use it. Inspect it, and remove it by hand if it is not yours."
    }
    $acl = Get-Acl -LiteralPath $Path
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier])
    if ($TrustedOwners -notcontains $owner.Value) {
        throw "'$Path' is owned by $owner, not by SYSTEM or Administrators. A standard user may have created it. Inspect it before you delete it or run this script again."
    }
    if ($CheckAcl) {
        foreach ($rule in $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
            if ($rule.AccessControlType -ne [Security.AccessControl.AccessControlType]::Allow) { continue }
            if ($TrustedOwners -contains $rule.IdentityReference.Value) { continue }
            if (([int64][uint32]$rule.FileSystemRights -band $WriteMask) -ne 0) {
                throw "'$Path' lets $($rule.IdentityReference) change it (rights $($rule.FileSystemRights)). Inspect the ACL before you use it."
            }
        }
    }
}

# Every level of Path below ProgramData\EtwDnstap\backup, from the top down.
function Get-BackupLevel {
    param([string]$Path)
    $full = Get-FullPathNoSlash $Path
    $root = Get-FullPathNoSlash $BackupRoot
    if (-not $full.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw "'$Path' is not below '$root'. The script uses no other location."
    }
    $levels = New-Object System.Collections.Generic.List[string]
    $levels.Add((Split-Path -Parent (Split-Path -Parent $root)))
    $levels.Add((Split-Path -Parent $root))
    $levels.Add($root)
    $current = $root
    foreach ($part in $full.Substring($root.Length + 1).Split('\')) {
        $current = Join-Path $current $part
        $levels.Add($current)
    }
    return , $levels.ToArray()
}

# Read-only. Runs at the start of a grant or removal, with -WhatIf too. Existing
# levels must be trusted, and the new backup folder must not exist.
function Assert-BackupLocation {
    $levels = Get-BackupLevel -Path $BackupPath
    $programData = Get-FullPathNoSlash $env:ProgramData
    foreach ($level in $levels) {
        if (-not (Test-Path -LiteralPath $level)) { continue }
        if ((Get-FullPathNoSlash $level) -eq $programData) {
            Assert-TrustedObject -Path $level
        }
        else {
            Assert-TrustedObject -Path $level -CheckAcl
        }
    }
    if (Test-Path -LiteralPath $BackupPath) {
        throw "Backup folder '$BackupPath' already exists. The script never reuses one. Choose another -BackupPath, or inspect and remove that folder."
    }
}

# Makes a folder that only SYSTEM and Administrators can use: owner Administrators,
# no inherited ACEs, full control for those two.
function Lock-Folder {
    param([string]$Path)
    & icacls.exe $Path /setowner '*S-1-5-32-544' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "icacls /setowner failed on '$Path' (exit code $LASTEXITCODE)." }
    & icacls.exe $Path /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "icacls failed on '$Path' (exit code $LASTEXITCODE)." }
}

function Get-Sha256Hex {
    param([byte[]]$Bytes)
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash($Bytes))).Replace('-', '') }
    finally { $sha.Dispose() }
}

function Write-ChangeLog {
    $log = [pscustomobject]@{
        Version  = 2
        Written  = (Get-Date).ToString('o')
        Computer = $env:COMPUTERNAME
        Key      = $WmiSecurityKeyNative
        Changed  = $script:ChangedValues.ToArray()
        Created  = $script:CreatedValues.ToArray()
    }
    $json = ConvertTo-Json -InputObject $log -Depth 4
    Set-Content -LiteralPath (Join-Path $BackupPath 'changes.json') -Value $json -Encoding UTF8
}

# Creates the backup folders and exports the whole WMI\Security key. Runs once,
# before the first change. Every level below ProgramData is created with an
# owner and ACL of SYSTEM and Administrators only. An existing level must already
# be trusted. The backup folder itself must not exist. It throws if the backup
# cannot be made, so that no change happens without one.
function Initialize-Backup {
    if ($script:BackupStarted) { return }
    $levels = Get-BackupLevel -Path $BackupPath
    $programData = Get-FullPathNoSlash $env:ProgramData
    foreach ($level in $levels) {
        if ((Get-FullPathNoSlash $level) -eq $programData) {
            Assert-TrustedObject -Path $level
            continue
        }
        if (Test-Path -LiteralPath $level) {
            if ($level -eq $BackupPath) { throw "Backup folder '$BackupPath' already exists." }
            Assert-TrustedObject -Path $level -CheckAcl
            continue
        }
        [void](New-Item -ItemType Directory -Path $level)
        Lock-Folder -Path $level
        if (@(Get-ChildItem -LiteralPath $level -Force).Count -ne 0) {
            throw "'$level' was not empty right after it was created. Inspect it."
        }
        Assert-TrustedObject -Path $level -CheckAcl
    }
    $regFile = Join-Path $BackupPath 'wmi-security.reg'
    & reg.exe export $WmiSecurityKeyNative $regFile /y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "reg.exe export failed (exit code $LASTEXITCODE)." }
    $script:BackupStarted = $true
    Write-ChangeLog
    Write-Output "Backup written to '$BackupPath'."
}

# Records the value's state before the first change to it. A value that exists
# is saved byte for byte. A value that does not exist is recorded as created by
# this run. Only the first call for a name has an effect. Complete-Backup adds
# the hash of the bytes the run wrote.
function Save-Original {
    param([string]$Name)
    Initialize-Backup
    foreach ($entry in $script:CreatedValues) { if ($entry.Name -eq $Name) { return } }
    foreach ($entry in $script:ChangedValues) { if ($entry.Name -eq $Name) { return } }
    $value = (Get-Item -LiteralPath $WmiSecurityKey).GetValue($Name)
    if ($null -eq $value) {
        $script:CreatedValues.Add([pscustomobject]@{ Name = $Name; WrittenSha256 = '' })
    }
    else {
        $script:ChangedValues.Add([pscustomobject]@{
                Name          = $Name
                OriginalBytes = [Convert]::ToBase64String([byte[]]$value)
                WrittenSha256 = ''
            })
    }
    Write-ChangeLog
}

# Runs when the script ends, even after an error. Records the SHA-256 of what
# each touched value holds now (empty text for a value that is gone), so that
# -Restore can tell whether anything changed the value after this run.
function Complete-Backup {
    if (-not $script:BackupStarted) { return }
    foreach ($entry in @($script:ChangedValues) + @($script:CreatedValues)) {
        $value = (Get-Item -LiteralPath $WmiSecurityKey).GetValue($entry.Name)
        $entry.WrittenSha256 = if ($null -eq $value) { '' } else { Get-Sha256Hex -Bytes ([byte[]]$value) }
    }
    Write-ChangeLog
}

# Reads and checks changes.json, and checks the folders above it. Returns the
# validated entries. Nothing is changed here.
function Read-ValidatedChangeLog {
    param([string]$Path)
    $full = Get-FullPathNoSlash $Path
    $file = if (Test-Path -LiteralPath $full -PathType Container) { Join-Path $full 'changes.json' } else { $full }
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "No changes.json at '$file'." }
    $folder = Split-Path -Parent $file
    foreach ($level in (Get-BackupLevel -Path $folder)) {
        if ((Get-FullPathNoSlash $level) -eq (Get-FullPathNoSlash $env:ProgramData)) {
            Assert-TrustedObject -Path $level
        }
        else {
            Assert-TrustedObject -Path $level -CheckAcl
        }
    }
    Assert-TrustedObject -Path $file -CheckAcl

    $log = Get-Content -LiteralPath $file -Raw | ConvertFrom-Json
    if ($log.Version -ne 2 -or $log.Key -ne $WmiSecurityKeyNative) {
        throw "'$file' is not a changes.json that this script wrote."
    }
    if ($log.Computer -ne $env:COMPUTERNAME) {
        throw "'$file' was written on $($log.Computer), not on $env:COMPUTERNAME."
    }
    $seen = @{}
    $changed = New-Object System.Collections.Generic.List[object]
    $created = New-Object System.Collections.Generic.List[object]
    foreach ($kind in 'Changed', 'Created') {
        foreach ($entry in @($log.$kind)) {
            if ($null -eq $entry) { continue }
            $parsed = [Guid]::Empty
            if (-not [Guid]::TryParse([string]$entry.Name, [ref]$parsed) -or $parsed.ToString() -ne [string]$entry.Name) {
                throw "Entry name '$($entry.Name)' is not a GUID in the form 00000000-0000-0000-0000-000000000000 (lower case)."
            }
            if ($seen.ContainsKey($entry.Name)) { throw "Entry {$($entry.Name)} appears twice." }
            $seen[$entry.Name] = $true
            $hash = [string]$entry.WrittenSha256
            if ($hash -ne '' -and $hash -notmatch '^[0-9A-F]{64}$') { throw "Entry {$($entry.Name)} has an invalid hash." }
            if ($kind -eq 'Changed') {
                try {
                    $bytes = [Convert]::FromBase64String([string]$entry.OriginalBytes)
                    [void](New-Object Security.AccessControl.RawSecurityDescriptor($bytes, 0))
                }
                catch { throw "Entry {$($entry.Name)} does not hold a valid security descriptor. $($_.Exception.Message)" }
                $changed.Add([pscustomobject]@{ Name = [string]$entry.Name; Bytes = $bytes; WrittenSha256 = $hash })
            }
            else {
                $created.Add([pscustomobject]@{ Name = [string]$entry.Name; WrittenSha256 = $hash })
            }
        }
    }
    return [pscustomobject]@{ Changed = $changed.ToArray(); Created = $created.ToArray() }
}

# Writes the saved bytes back and deletes the values that the run created. A
# value that something changed after the run is skipped unless -Force is given.
# Everything is validated before anything is applied.
function Restore-Backup {
    [CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
    param([string]$Path, [switch]$Force)
    $plan = Read-ValidatedChangeLog -Path $Path
    $restored = @(); $deleted = @(); $skipped = @()
    function Get-CurrentHash {
        param([string]$Name)
        $value = (Get-Item -LiteralPath $WmiSecurityKey).GetValue($Name)
        if ($null -eq $value) { return '' }
        return (Get-Sha256Hex -Bytes ([byte[]]$value))
    }
    $actions = New-Object System.Collections.Generic.List[object]
    foreach ($entry in $plan.Changed) {
        $now = Get-CurrentHash -Name $entry.Name
        if ($now -ne $entry.WrittenSha256 -and -not $Force) {
            Write-Warning "Skipped {$($entry.Name)}: its value changed after the run that saved it. Use -Force to overwrite it."
            $skipped += $entry.Name
            continue
        }
        $actions.Add([pscustomobject]@{ Kind = 'Restore'; Entry = $entry })
    }
    foreach ($entry in $plan.Created) {
        $now = Get-CurrentHash -Name $entry.Name
        if ($now -eq '') { Write-Output "{$($entry.Name)} is already absent."; continue }
        if ($now -ne $entry.WrittenSha256 -and -not $Force) {
            Write-Warning "Skipped {$($entry.Name)}: its value changed after the run that created it. Use -Force to delete it."
            $skipped += $entry.Name
            continue
        }
        $actions.Add([pscustomobject]@{ Kind = 'Delete'; Entry = $entry })
    }
    foreach ($action in $actions) {
        $name = $action.Entry.Name
        if ($action.Kind -eq 'Restore') {
            if ($PSCmdlet.ShouldProcess("{$name}", 'Write the original descriptor bytes back')) {
                Set-ItemProperty -LiteralPath $WmiSecurityKey -Name $name -Value $action.Entry.Bytes -Type Binary
                $restored += $name
            }
        }
        elseif ($PSCmdlet.ShouldProcess("{$name}", 'Delete the descriptor that the run created')) {
            Remove-ItemProperty -LiteralPath $WmiSecurityKey -Name $name
            $deleted += $name
        }
    }
    Write-Output "Restore summary: $($restored.Count) restored, $($deleted.Count) deleted, $($skipped.Count) skipped."
    foreach ($n in $restored) { Write-Output "  restored {$n}" }
    foreach ($n in $deleted) { Write-Output "  deleted  {$n}" }
    foreach ($n in $skipped) { Write-Output "  skipped  {$n}" }
    Write-Output 'Restart the computer: ETW applies these descriptors at boot.'
}

# Strips every ACE for Sid from the descriptor's DACL; returns how many.
function Remove-SidAce {
    param($Descriptor, [Security.Principal.SecurityIdentifier]$Sid)
    $acl = $Descriptor.DiscretionaryAcl
    if ($null -eq $acl) { return 0 }
    $removed = 0
    for ($i = $acl.Count - 1; $i -ge 0; $i--) {
        $ace = $acl[$i]
        if ($ace -is [Security.AccessControl.KnownAce] -and $ace.SecurityIdentifier -eq $Sid) {
            $acl.RemoveAce($i)
            $removed++
        }
    }
    return $removed
}

# Strips the service SID from descriptors: every one except those named in
# Keep, or, when Only is given, just those. Writes one line per change.
function Clear-ServiceSidAce {
    [CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
    param([Security.Principal.SecurityIdentifier]$Sid, [string[]]$Keep = @(), [string[]]$Only = @())
    $default = Read-Descriptor -Name $DefaultGuid
    if ($null -eq $default) { throw "Default ETW descriptor {$DefaultGuid} is missing or unparseable." }
    [void](Remove-SidAce -Descriptor $default -Sid $Sid)
    $defaultForm = $default.GetSddlForm($Sections)
    $names = if ($Only.Count -gt 0) { $Only } else { (Get-Item -LiteralPath $WmiSecurityKey).GetValueNames() }
    foreach ($name in $names) {
        if ($Keep -contains $name) { continue }
        $descriptor = Read-Descriptor -Name $name
        if ($null -eq $descriptor) { continue }
        $removed = Remove-SidAce -Descriptor $descriptor -Sid $Sid
        if ($removed -eq 0) { continue }
        if ($name -ne $DefaultGuid -and $descriptor.GetSddlForm($Sections) -eq $defaultForm) {
            if ($PSCmdlet.ShouldProcess("{$name}", "Delete the descriptor; without $removed ACE(s) for $Sid it equals the default")) {
                Save-Original -Name $name
                Remove-ItemProperty -LiteralPath $WmiSecurityKey -Name $name
                Write-Output "Deleted {$name}: it matched the default once $removed ACE(s) for $Sid were removed."
            }
            continue
        }
        if ($PSCmdlet.ShouldProcess("{$name}", "Remove $removed ACE(s) for $Sid")) {
            Save-Original -Name $name
            $binary = New-Object byte[] $descriptor.BinaryLength
            $descriptor.GetBinaryForm($binary, 0)
            Set-ItemProperty -LiteralPath $WmiSecurityKey -Name $name -Value $binary -Type Binary
            Write-Output "Removed $removed ACE(s) for $Sid from {$name}."
        }
    }
}

function Grant-SessionRight {
    [CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
    param([Guid]$Guid, [Security.Principal.SecurityIdentifier]$Sid, [string]$Name)
    $key = $Guid.ToString()
    $descriptor = Read-Descriptor -Name $key
    if ($null -eq $descriptor -and $null -ne (Get-Item -LiteralPath $WmiSecurityKey).GetValue($key)) {
        throw "The descriptor {$key} exists but cannot be parsed. The script does not change it. Inspect it, and delete it by hand if it is damaged."
    }
    if ($null -ne $descriptor) {
        $aces = @($descriptor.DiscretionaryAcl | Where-Object {
                $_ -is [Security.AccessControl.KnownAce] -and $_.SecurityIdentifier -eq $Sid })
        if ($aces.Count -eq 1 -and
            $aces[0].AceType -eq [Security.AccessControl.AceType]::AccessAllowed -and
            [uint32]$aces[0].AccessMask -eq $TRACELOG_CREATE_REALTIME) {
            Write-Output "Already granted 0x20 on session '$Name' {$Guid}."
            return
        }
        # Any other ACE for the service SID here is replaced by exactly 0x20.
        Clear-ServiceSidAce -Sid $Sid -Only @($key)
    }
    if ($PSCmdlet.ShouldProcess("session '$Name' {$Guid}", "Grant TRACELOG_CREATE_REALTIME (0x20) to $Sid")) {
        Save-Original -Name $key
        $sidBytes = New-Object byte[] $Sid.BinaryLength
        $Sid.GetBinaryForm($sidBytes, 0)
        # With no descriptor of its own, ETW seeds the GUID's new descriptor
        # from the default and adds this ACE. The principals of the default
        # descriptor can therefore also create a session with this GUID. That
        # is harmless; see README.md.
        $status = [EtwDnstap.EtwSecurity]::EventAccessControl(
            [ref]$Guid, $EventSecurityAddDACL, $sidBytes, $TRACELOG_CREATE_REALTIME, $true)
        if ($status -ne 0) {
            throw "EventAccessControl(AddDACL, {$Guid}) failed with Win32 error $status."
        }
        Write-Output "Granted 0x20 on session '$Name' {$Guid} to $Sid."
    }
}

function Show-Descriptor {
    param([string]$Name, [string]$Label)
    $descriptor = Read-Descriptor -Name $Name
    if ($null -eq $descriptor) {
        Write-Output "$Label {$Name}: no descriptor of its own (the default applies)"
    }
    else {
        Write-Output "$Label {$Name} SDDL:"
        Write-Output ('  ' + $descriptor.GetSddlForm([Security.AccessControl.AccessControlSections]::Access))
    }
}

if (-not (Test-Administrator)) {
    throw 'Run this script from an elevated PowerShell session.'
}

if ($PSCmdlet.ParameterSetName -eq 'Restore') {
    Restore-Backup -Path $Restore -Force:$Force
    return
}

# Fails now, before any change, if the backup location is not trustworthy.
Assert-BackupLocation

try {
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

    $sessions = @(foreach ($name in $SessionName) {
            if ($name.Length -eq 0 -or $name.Contains([char]0)) { throw "Invalid session name '$name'." }
            [pscustomobject]@{ Name = $name; Guid = Get-SessionGuid -Name $name }
        })

    if ($Remove) {
        Clear-ServiceSidAce -Sid $sid
    }
    else {
        foreach ($session in $sessions) {
            Write-Output "Session '$($session.Name)' GUID: {$($session.Guid)}"
        }
        $keep = @($sessions | ForEach-Object { $_.Guid.ToString() })
        Clear-ServiceSidAce -Sid $sid -Keep $keep
        foreach ($session in $sessions) {
            Grant-SessionRight -Guid $session.Guid -Sid $sid -Name $session.Name
        }
        foreach ($session in $sessions) {
            Show-Descriptor -Name $session.Guid.ToString() -Label "Session '$($session.Name)'"
        }
    }
    Show-Descriptor -Name $DefaultGuid -Label 'Default'
    if ($script:BackupStarted) {
        Write-Output "To undo this run: .\Grant-EtwDnstapEtwAccess.ps1 -Restore '$BackupPath'"
    }
    Write-Output 'Restart the computer before starting the service: ETW applies these descriptors at boot.'
}
finally {
    Complete-Backup
}
