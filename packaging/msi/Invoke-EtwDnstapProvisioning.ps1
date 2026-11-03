#requires -Version 5.1
<#
.SYNOPSIS
Provisioning steps that the EtwDnstap MSI runs as SYSTEM (plan W7).

.DESCRIPTION
Install: checks RECEIVER as docs/guide/install.md step 1 does, creates or checks
ProgramData\EtwDnstap and its spool folder with the rules of install.md (the
New-ProtectedFolder, Test-ProtectedFolder and Test-SpoolFolder functions are
ported without loosening), adds the service SID to Performance Log Users, and
runs Grant-EtwDnstapEtwAccess.ps1 from the install folder.

Uninstall: stops the leftover ETW session, removes the service SID from
Performance Log Users, runs the grant script with -Remove, and deletes the spool
and backup folders that the product created. Reparse points are never followed or
deleted, and content that the product did not create is kept and listed.

Each change prints one line, which the MSI log keeps. The script exits 1 on the
first failure and writes no secrets. It is run by WixQuietExec from the install
folder, by full path, as SYSTEM, with no profile.

.PARAMETER Action
Install or Uninstall.

.PARAMETER SessionName
The --session-name of the service. Letters, digits, '_', '.', '-'.

.PARAMETER Receiver
IP:port of the collector. Required for Install.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Install', 'Uninstall')]
    [string]$Action,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[A-Za-z0-9_.-]{1,128}$')]
    [string]$SessionName,

    [string]$Receiver
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ServiceName = 'EtwDnstap'
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
    # Get-LocalGroupMember and net.exe need the group name without the BUILTIN\ domain part.
    $account = (New-Object Security.Principal.SecurityIdentifier $PerformanceLogUsersSid).
        Translate([Security.Principal.NTAccount]).Value
    return $account.Substring($account.IndexOf([char]92) + 1)
}

function Test-ServiceInGroup([string]$GroupName) {
    $serviceSid = (New-Object Security.Principal.NTAccount "NT SERVICE\$ServiceName").
        Translate([Security.Principal.SecurityIdentifier]).Value
    $members = @(Get-LocalGroupMember -Group $GroupName | ForEach-Object { $_.SID.Value })
    return ($members -contains $serviceSid)
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

    $group = Get-PerformanceLogUsersName
    if (Test-ServiceInGroup $group) {
        Write-Step "NT SERVICE\$ServiceName is already a member of $group."
    }
    else {
        net.exe localgroup "$group" "NT SERVICE\$ServiceName" /add | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "net localgroup /add failed (exit code $LASTEXITCODE)." }
        Write-Step "Added NT SERVICE\$ServiceName to $group."
    }

    Invoke-Grant @{ SessionName = $SessionName }
    Write-Step "Ran the ETW grant for session '$SessionName'. Restart the computer before the service starts."
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

function Invoke-Uninstall {
    Stop-LeftoverSession

    $group = Get-PerformanceLogUsersName
    if (Test-ServiceInGroup $group) {
        net.exe localgroup "$group" "NT SERVICE\$ServiceName" /delete | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "net localgroup /delete failed (exit code $LASTEXITCODE)." }
        Write-Step "Removed NT SERVICE\$ServiceName from $group."
    }
    else {
        Write-Step "NT SERVICE\$ServiceName is not in $group."
    }

    Invoke-Grant @{ Remove = $true }
    Write-Step 'Removed the ETW grants of the service SID.'

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
    if ($Action -eq 'Install') { Invoke-Install } else { Invoke-Uninstall }
    exit 0
}
catch {
    Write-Output ('[EtwDnstap] FAILED: ' + $_.Exception.Message)
    exit 1
}
