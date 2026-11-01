# Checks the hardening metadata of a shipped Windows x64 image with dumpbin.
#
# Usage:
#   pwsh -NoProfile -File scripts/ci/verify-pe.ps1 -Path target/x86_64-pc-windows-msvc/release/etw-dnstap.exe
#
# Fails (exit 1) when a required flag is missing: x64 machine, large-address
# awareness, high-entropy ASLR, dynamic base, NX, CFG (image flag, instrumented
# code, function table), the EH continuation table, the CET compatibility
# marker, a security cookie, base relocations and the load configuration. It
# also fails when the image carries a CodeView (PDB) debug entry, imports an
# AddressSanitizer runtime, or imports the dynamic VC/UCRT runtime (the release
# profile links the CRT statically).
#
# dumpbin comes from -DumpBin, then PATH, then the newest Visual Studio C++
# toolset that vswhere reports. Parsing uses dumpbin's hexadecimal values, not
# its English labels where a number is available. Absolute user paths embedded
# in strings are reported as warnings only.
#
# Works on Windows PowerShell 5.1 and PowerShell 7.
param(
    [Parameter(Mandatory = $true)]
    [string]$Path,

    [string]$DumpBin
)

$ErrorActionPreference = 'Stop'

function Find-DumpBin {
    if ($DumpBin) { return $DumpBin }
    $onPath = Get-Command -Name 'dumpbin.exe' -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) {
        throw 'dumpbin.exe is not on PATH and vswhere.exe is not installed.'
    }
    $found = @(& $vswhere -latest -products '*' `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
            -find 'VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe')
    if ($found.Count -eq 0) {
        throw 'No Visual Studio C++ toolset with dumpbin.exe was found.'
    }
    return ($found | Sort-Object | Select-Object -Last 1)
}

function Invoke-DumpBin([string]$Tool, [string]$Switch, [string]$Image) {
    # dumpbin writes some warnings to stderr; keep going and check its exit code.
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $lines = & $Tool $Switch $Image 2>&1 | ForEach-Object { "$_" }
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
    if ($code -ne 0) { throw "dumpbin $Switch exited with $code" }
    return [string[]]$lines
}

# Returns the first capture group of the first matching line, or $null.
function Find-Value([string[]]$Lines, [string]$Pattern) {
    foreach ($line in $Lines) {
        $m = [regex]::Match($line, $Pattern)
        if ($m.Success) { return $m.Groups[1].Value }
    }
    return $null
}

function ConvertFrom-Hex([string]$Text) {
    if ($null -eq $Text) { return $null }
    return [Convert]::ToUInt64($Text, 16)
}

$results = New-Object System.Collections.Generic.List[object]

function Add-Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

function Add-BitCheck([string]$Name, $Value, [uint64]$Mask, [string]$Source) {
    if ($null -eq $Value) {
        Add-Check $Name $false "$Source not found in dumpbin output"
        return
    }
    $set = (([uint64]$Value) -band $Mask) -eq $Mask
    Add-Check $Name $set ("{0} = 0x{1:X}" -f $Source, [uint64]$Value)
}

$image = (Resolve-Path -LiteralPath $Path).ProviderPath
$tool = Find-DumpBin
Write-Output "dumpbin: $tool"
Write-Output "image:   $image"

$headers = Invoke-DumpBin $tool '/headers' $image
$loadConfig = Invoke-DumpBin $tool '/loadconfig' $image
$dependents = Invoke-DumpBin $tool '/dependents' $image
$bytes = [System.IO.File]::ReadAllBytes($image)

# FILE HEADER values. The first "characteristics" line is the file header.
$machine = ConvertFrom-Hex (Find-Value $headers '^\s*([0-9A-Fa-f]+) machine')
$fileChar = ConvertFrom-Hex (Find-Value $headers '^\s*([0-9A-Fa-f]+) characteristics\s*$')
Add-Check 'x64 machine (0x8664)' ($machine -eq 0x8664) ("machine = 0x{0:X}" -f $machine)
Add-BitCheck 'large-address aware' $fileChar 0x20 'FILE HEADER characteristics'

# Optional header DLL characteristics and the extended CET marker.
$dllChar = ConvertFrom-Hex (Find-Value $headers '^\s*([0-9A-Fa-f]+) DLL characteristics\s*$')
$extChar = ConvertFrom-Hex (Find-Value $headers '^\s*([0-9A-Fa-f]+) extended DLL characteristics\s*$')
Add-BitCheck 'high-entropy ASLR (HIGH_ENTROPY_VA)' $dllChar 0x20 'DLL characteristics'
Add-BitCheck 'dynamic base (ASLR)' $dllChar 0x40 'DLL characteristics'
Add-BitCheck 'NX compatible (DEP)' $dllChar 0x100 'DLL characteristics'
Add-BitCheck 'CFG image flag (GUARD_CF)' $dllChar 0x4000 'DLL characteristics'
Add-BitCheck 'CET compatible marker' $extChar 0x1 'extended DLL characteristics'

$relocSize = ConvertFrom-Hex (Find-Value $headers '^\s*[0-9A-Fa-f]+ \[\s*([0-9A-Fa-f]+)\] RVA \[size\] of Base Relocation Directory')
Add-Check 'base relocations present' ($null -ne $relocSize -and $relocSize -gt 0) ("Base Relocation size = 0x{0:X}" -f $relocSize)

# Load configuration: CFG, EH continuation, stack cookie.
$guardFlags = ConvertFrom-Hex (Find-Value $loadConfig '^\s*([0-9A-Fa-f]+) Guard Flags\s*$')
$cfgCount = ConvertFrom-Hex (Find-Value $loadConfig '^\s*([0-9A-Fa-f]+) Guard CF function count')
$ehCount = ConvertFrom-Hex (Find-Value $loadConfig '^\s*([0-9A-Fa-f]+) Guard EH continuation count')
$cookie = ConvertFrom-Hex (Find-Value $loadConfig '^\s*([0-9A-Fa-f]+) Security Cookie')
Add-Check 'load configuration present' ($null -ne $guardFlags) 'Guard Flags line found'
Add-BitCheck 'CFG instrumented code (IMAGE_GUARD_CF_INSTRUMENTED)' $guardFlags 0x100 'Guard Flags'
Add-BitCheck 'CFG function table present' $guardFlags 0x400 'Guard Flags'
Add-Check 'CFG function count > 0' ($null -ne $cfgCount -and $cfgCount -gt 0) ("Guard CF function count = {0}" -f $cfgCount)
Add-BitCheck 'EH continuation table present' $guardFlags 0x400000 'Guard Flags'
Add-Check 'EH continuation count > 0' ($null -ne $ehCount -and $ehCount -gt 0) ("Guard EH continuation count = {0}" -f $ehCount)
Add-Check 'security cookie present' ($null -ne $cookie -and $cookie -ne 0) ("Security Cookie = 0x{0:X}" -f $cookie)

# Debug directory: a CodeView entry means a PDB path is embedded.
$debugTypes = @()
foreach ($line in $headers) {
    $m = [regex]::Match($line, '^\s*[0-9A-Fa-f]{8}\s+(\S+)\s+[0-9A-Fa-f]+\s+[0-9A-Fa-f]{8}\s+[0-9A-Fa-f]+')
    if ($m.Success) { $debugTypes += $m.Groups[1].Value }
}
Add-Check 'no CodeView (PDB) debug entry' (-not ($debugTypes -contains 'cv')) ("debug entry types: {0}" -f (($debugTypes | Sort-Object -Unique) -join ', '))

# Imports: no ASan runtime, no dynamic VC/UCRT runtime.
$deps = @()
foreach ($line in $dependents) {
    $m = [regex]::Match($line, '^\s+([\w.-]+\.dll)\s*$')
    if ($m.Success) { $deps += $m.Groups[1].Value }
}
$asan = @($deps | Where-Object { $_ -match '^clang_rt\.' })
Add-Check 'no AddressSanitizer runtime import' ($asan.Count -eq 0) ("asan imports: {0}" -f ($asan -join ', '))
$dynCrt = @($deps | Where-Object { $_ -match '^(vcruntime\d+(_\d+)?|msvcp\d+(_\w+)?|ucrtbase|api-ms-win-crt-[a-z0-9-]+)\.dll$' })
Add-Check 'no dynamic VC/UCRT runtime import (static CRT)' ($dynCrt.Count -eq 0) ("CRT imports: {0}" -f ($dynCrt -join ', '))

# Warning only: absolute user paths in strings (panic locations and similar).
$text = [System.Text.Encoding]::ASCII.GetString($bytes)
$userPaths = [regex]::Matches($text, '[A-Za-z]:\\Users\\').Count

$failed = @($results | Where-Object { -not $_.Ok })
foreach ($r in $results) {
    $tag = if ($r.Ok) { 'PASS' } else { 'FAIL' }
    Write-Output ("[{0}] {1}: {2}" -f $tag, $r.Name, $r.Detail)
}
if ($userPaths -gt 0) {
    Write-Output ("[WARN] {0} absolute C:\Users\ path string(s) embedded in the image" -f $userPaths)
}
Write-Output ("PE verification: {0} checks, {1} failed." -f $results.Count, $failed.Count)
if ($failed.Count -gt 0) { exit 1 }
exit 0
