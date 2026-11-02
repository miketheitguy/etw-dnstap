# Checks that the MSVC linker's LNK2047 rejections match the approved allowlist.
#
# Usage:
#   cargo build --locked --release 2>&1 | tee release-build.log
#   pwsh -NoProfile -File scripts/ci/verify-link-exceptions.ps1 -LogPath release-build.log
#
# The release image is linked with /GUARD:EHCONT and /FORCE:GUARDEHCONT
# (.cargo/config.toml). The first flag rejects any object that carries
# exception-handling metadata but no EH-continuation table (LNK2047). The
# second flag links anyway, and rustc forwards the linker's text as one
# "warning: linker stdout:" block. This script reads a captured build log,
# collects every LNK2047 line, strips the rustc metadata hash from the
# archive name (libring-<hash>.rlib(x.o) becomes libring.rlib(x.o)), and
# compares the set with scripts/ci/link-exceptions.txt.
#
# Fails (exit 1) when:
# - the log has no cargo "Finished" line for the release profile, or does not
#   show the etw-dnstap crate being compiled, so an empty, truncated, or fully
#   cached log fails instead of passing. (Cargo replays cached diagnostics
#   for a fresh crate, so a warm local target prints the LNK2047 lines
#   without relinking. Run `cargo clean --release -p etw-dnstap` first to get
#   a log this check accepts.)
# - the linker reports an object that is not in the allowlist (the exception
#   widened without review);
# - an allowlisted object is not reported (the exception is stale, and the
#   entry, or /FORCE:GUARDEHCONT itself, can be removed);
# - the linker reports LNK2046 (local unwind without /guard:ehcont), the
#   other EHCONT rejection, which the allowlist does not cover.
#
# LNK4291 (SEH in COMDATs, conservative metadata generated) is counted and
# printed but does not fail the check: the linker still emits correct
# metadata for those objects.
#
# ANSI color sequences (CARGO_TERM_COLOR=always) are stripped before parsing.
#
# Works on Windows PowerShell 5.1 and PowerShell 7.
param(
    [Parameter(Mandatory = $true)]
    [string]$LogPath,

    [string]$AllowlistPath
)

$ErrorActionPreference = 'Stop'

if (-not $AllowlistPath) {
    $AllowlistPath = Join-Path $PSScriptRoot 'link-exceptions.txt'
}

$results = New-Object System.Collections.Generic.List[object]

function Add-Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

# Reads the allowlist: one <archive>(<object>) entry per line, '#' comments
# and blank lines ignored. Returns a sorted, de-duplicated string array.
function Read-Allowlist([string]$File) {
    $entries = @()
    foreach ($raw in [System.IO.File]::ReadAllLines($File)) {
        $line = $raw.Trim()
        if ($line.Length -eq 0 -or $line.StartsWith('#')) { continue }
        if ($line -notmatch '^[^\s()]+\([^\s()]+\)$') {
            throw "Malformed allowlist entry in ${File}: '$line'. Expected <archive>(<object>)."
        }
        $entries += $line
    }
    return @($entries | Sort-Object -Unique)
}

# Normalizes the module reference the linker prints before "LNK2047": drops
# any directory, then drops the rustc metadata hash from the archive name.
# An unrecognized shape is returned unchanged so that it shows up as an
# unexpected entry rather than disappearing.
function ConvertTo-ExceptionKey([string]$Module) {
    $m = [regex]::Match($Module, '^(?<archive>[^()]+)\((?<object>[^()]+)\)$')
    if (-not $m.Success) { return $Module }
    $archive = $m.Groups['archive'].Value -replace '^.*[\\/]', ''
    $object = $m.Groups['object'].Value -replace '^.*[\\/]', ''
    # lib<crate>-<16 hex>.rlib -> lib<crate>.rlib (also .lib).
    $archive = [regex]::Replace($archive, '-[0-9a-f]{16}(\.(rlib|lib))$', '$1')
    return "$archive($object)"
}

$log = (Resolve-Path -LiteralPath $LogPath).ProviderPath
$allowlistFile = (Resolve-Path -LiteralPath $AllowlistPath).ProviderPath
Write-Output "log:       $log"
Write-Output "allowlist: $allowlistFile"

$text = [System.IO.File]::ReadAllText($log)
# Strip terminal escape sequences before matching: CSI (ESC [ ... final
# byte) for colors, and OSC (ESC ] ... BEL or ESC \) for the hyperlinks
# cargo puts around the profile name in its "Finished" line.
$text = [regex]::Replace($text, "\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)", '')
$text = [regex]::Replace($text, "\x1b\[[0-9;?]*[ -/]*[@-~]", '')
$lines = @($text -split "`r?`n")

# Positive checks: the log is a complete cargo release build that compiled,
# and therefore linked, the service crate in this run. A warm target
# directory can skip the link, and the linker output with it.
$finished = @($lines | Where-Object { $_ -match '^\s*Finished\s+`release`\s+profile' })
Add-Check 'log parsed (cargo Finished line for the release profile)' ($finished.Count -gt 0) ("{0} line(s) read" -f $lines.Count)
$compiled = @($lines | Where-Object { $_ -match '^\s*Compiling\s+etw-dnstap\s+v' })
Add-Check 'log parsed (etw-dnstap compiled in this run)' ($compiled.Count -gt 0) ("{0} Compiling line(s) for etw-dnstap" -f $compiled.Count)

# Every LNK2047 line, whatever rustc printed in front of it. The linker
# prints: <module> : error LNK2047: module contains C++ EH or complex EH
# metadata but was not compiled with /guard:ehcont
$observed = @{}
$lnk2047Lines = 0
foreach ($line in $lines) {
    $m = [regex]::Match($line, '(?<module>\S+)\s*:\s*(?:warning|error)\s+LNK2047\b')
    if (-not $m.Success) { continue }
    $lnk2047Lines++
    $key = ConvertTo-ExceptionKey $m.Groups['module'].Value
    if ($observed.ContainsKey($key)) { $observed[$key]++ } else { $observed[$key] = 1 }
}
$observedKeys = @($observed.Keys | Sort-Object)
foreach ($key in $observedKeys) {
    Write-Output ("LNK2047:   {0} (x{1})" -f $key, $observed[$key])
}

$allowed = Read-Allowlist $allowlistFile
$unexpected = @($observedKeys | Where-Object { $allowed -notcontains $_ })
$missing = @($allowed | Where-Object { $observedKeys -notcontains $_ })

$unexpectedText = if ($unexpected.Count -eq 0) { 'none' } else { ($unexpected -join ', ') + '. Review, then add it to scripts/ci/link-exceptions.txt (procedure in its header).' }
$missingText = if ($missing.Count -eq 0) { 'none' } else { ($missing -join ', ') + '. Delete the stale line from scripts/ci/link-exceptions.txt (procedure in its header).' }
Add-Check 'no LNK2047 object outside the allowlist' ($unexpected.Count -eq 0) ("unexpected: {0}" -f $unexpectedText)
Add-Check 'every allowlisted object was reported (allowlist not stale)' ($missing.Count -eq 0) ("missing: {0}" -f $missingText)
Add-Check 'LNK2047 set equals the allowlist' ($unexpected.Count -eq 0 -and $missing.Count -eq 0) ("{0} LNK2047 line(s), {1} distinct object(s), {2} allowlisted" -f $lnk2047Lines, $observedKeys.Count, $allowed.Count)

# LNK2046 is the other /guard:ehcont rejection. Not allowlisted, so any
# occurrence fails.
$lnk2046 = @($lines | Where-Object { $_ -match '\bLNK2046\b' })
Add-Check 'no LNK2046 (local unwind) diagnostics' ($lnk2046.Count -eq 0) ("{0} line(s)" -f $lnk2046.Count)

# Informational only.
$lnk4291 = @($lines | Where-Object { $_ -match '\bLNK4291\b' })
Write-Output ("info:      {0} LNK4291 line(s) (conservative metadata, not an exception)" -f $lnk4291.Count)

$failed = @($results | Where-Object { -not $_.Ok })
foreach ($r in $results) {
    $tag = if ($r.Ok) { 'PASS' } else { 'FAIL' }
    Write-Output ("[{0}] {1}: {2}" -f $tag, $r.Name, $r.Detail)
}
Write-Output ("Link exception verification: {0} checks, {1} failed." -f $results.Count, $failed.Count)
if ($failed.Count -gt 0) { exit 1 }
exit 0
