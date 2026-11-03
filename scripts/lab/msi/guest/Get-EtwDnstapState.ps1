#requires -Version 5.1
<#
.SYNOPSIS
Saves the EtwDnstap state of a guest as one JSON document (lab step L0).

.DESCRIPTION
Reads, and changes nothing: the sc.exe query output, the raw ImagePath, every value of
the Application event source key (name, registry type, data), the folder ACLs, and the
content of ProgramData\EtwDnstap. The host driver stores the result as the pre-test
state. Test-EtwDnstapInstalled compares the event source values with it.

The script prints the JSON to the output stream.
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$serviceName = 'EtwDnstap'
$installFolder = Join-Path $env:ProgramFiles 'EtwDnstap'
$dataFolder = Join-Path $env:ProgramData 'EtwDnstap'

function Get-RegistryValueSnapshot([string]$SubKeyPath) {
    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($SubKeyPath)
    if ($null -eq $key) { return $null }
    try {
        $values = foreach ($name in $key.GetValueNames()) {
            $kind = $key.GetValueKind($name)
            $raw = $key.GetValue($name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $data = switch ($kind) {
                'Binary' { ($raw | ForEach-Object { $_.ToString('x2') }) -join '' }
                'MultiString' { $raw -join "`n" }
                default { [string]$raw }
            }
            [pscustomobject]@{ name = $name; kind = [string]$kind; data = $data }
        }
        return [pscustomobject]@{ path = "HKLM\$SubKeyPath"; values = @($values); subKeys = @($key.GetSubKeyNames()) }
    }
    finally { $key.Dispose() }
}

function Get-FolderSnapshot([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return [pscustomobject]@{ path = $Path; exists = $false } }
    $acl = Get-Acl -LiteralPath $Path
    [pscustomobject]@{
        path   = $Path
        exists = $true
        sddl   = $acl.Sddl
        icacls = @(icacls.exe $Path)
    }
}

function Get-ContentSnapshot([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return @() }
    @(Get-ChildItem -LiteralPath $Path -Force | ForEach-Object {
            $files = @(if ($_.PSIsContainer) { Get-ChildItem -LiteralPath $_.FullName -Force -Recurse -File -ErrorAction SilentlyContinue } else { $_ })
            [pscustomobject]@{
                name       = $_.Name
                kind       = if ($_.PSIsContainer) { 'folder' } else { 'file' }
                attributes = [string]$_.Attributes
                fileCount  = $files.Count
                bytes      = [int64]$(if ($files.Count -gt 0) { ($files | Measure-Object -Property Length -Sum).Sum } else { 0 })
            }
        })
}

$imagePath = $null
$serviceKey = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey("SYSTEM\CurrentControlSet\Services\$serviceName")
if ($null -ne $serviceKey) {
    $imagePath = [string]$serviceKey.GetValue('ImagePath', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $serviceKey.Dispose()
}

$state = [pscustomobject]@{
    computer       = $env:COMPUTERNAME
    osVersion      = [string][Environment]::OSVersion.Version
    capturedAt     = (Get-Date).ToString('o')
    scQc           = @(sc.exe qc $serviceName)
    scQsidtype     = @(sc.exe qsidtype $serviceName)
    scQprivs       = @(sc.exe qprivs $serviceName)
    scQfailure     = @(sc.exe qfailure $serviceName)
    scQfailureflag = @(sc.exe qfailureflag $serviceName)
    scSdshow       = @(sc.exe sdshow $serviceName)
    imagePath      = $imagePath
    eventSource    = Get-RegistryValueSnapshot "SYSTEM\CurrentControlSet\Services\EventLog\Application\$serviceName"
    folders        = @(
        Get-FolderSnapshot $installFolder
        Get-FolderSnapshot $dataFolder
        Get-FolderSnapshot (Join-Path $dataFolder 'spool')
        Get-FolderSnapshot (Join-Path $dataFolder 'backup')
    )
    installFolder  = @(Get-ContentSnapshot $installFolder)
    dataFolder     = @(Get-ContentSnapshot $dataFolder)
}
$state | ConvertTo-Json -Depth 8
