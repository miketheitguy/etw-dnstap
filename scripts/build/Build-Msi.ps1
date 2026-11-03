#requires -Version 7.0
<#
.SYNOPSIS
Builds the EtwDnstap MSI from the repository root (plan steps B1 to B7).

.DESCRIPTION
1. Reads the version from `cargo metadata` and checks it against SemVer 2.0.0 and
   the MSI limits. A pre-release or build-metadata part is refused.
2. Runs `cargo clean --release -p etw-dnstap`, then `cargo build --locked --release`, and
   saves the log to target\msi\release-build.log.
   -SkipCargoBuild reuses the existing executable and its log.
3. Runs scripts\ci\verify-link-exceptions.ps1 and scripts\ci\verify-pe.ps1.
4. Checks the executable's version resource (FileVersion, ProductVersion).
5. Builds packaging\msi\EtwDnstap.wixproj. Output: target\msi\etw-dnstap-X.Y.Z-x64.msi.
6. Reads the MSI Property table through Windows Installer and compares it with the
   version, UpgradeCode, ProductName, and Manufacturer.
7. Writes the SHA-256 file next to the MSI and prints the version, path, and hash.

Every path is repository-relative. Writes go only under target\. Any failure stops
the script with a non-zero exit code.

.PARAMETER SkipCargoBuild
Reuse target\x86_64-pc-windows-msvc\release\etw-dnstap.exe and target\msi\release-build.log.

.EXAMPLE
pwsh -NoProfile -File scripts\build\Build-Msi.ps1
#>
[CmdletBinding()]
param(
    [switch]$SkipCargoBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# The MSI's Property table is read with Windows Installer COM. Windows only.
if (-not $IsWindows) { throw 'Build-Msi.ps1 runs on Windows only.' }

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Push-Location -LiteralPath $repoRoot
try {
    $targetDir = Join-Path 'target' 'msi'
    $exePath = Join-Path 'target' (Join-Path 'x86_64-pc-windows-msvc' (Join-Path 'release' 'etw-dnstap.exe'))
    $buildLog = Join-Path $targetDir 'release-build.log'
    $wixProject = Join-Path 'packaging' (Join-Path 'msi' 'EtwDnstap.wixproj')
    $wixObj = 'target/msi/obj/'
    $wixBin = 'target/msi/bin/'

    function Write-Step([string]$Text) { Write-Output "[B] $Text" }

    # ---- B1: version -------------------------------------------------------------
    Write-Step 'B1 read and check the version'
    $metadata = (& cargo metadata --no-deps --format-version 1 --locked) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed (exit code $LASTEXITCODE)." }
    $package = @($metadata.packages | Where-Object { $_.name -eq 'etw-dnstap' })
    if ($package.Count -ne 1) { throw 'cargo metadata did not return exactly one etw-dnstap package.' }
    $cargoVersion = [string]$package[0].version

    # SemVer 2.0.0, section 2, the official grammar.
    $semver = '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)' +
        '(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?' +
        '(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$'
    $match = [regex]::Match($cargoVersion, $semver)
    if (-not $match.Success) { throw "Version '$cargoVersion' is not a SemVer 2.0.0 version." }
    if ($match.Groups[4].Success) {
        throw "Version '$cargoVersion' has a pre-release part. An MSI cannot be built from a pre-release (plan decision 6)."
    }
    if ($match.Groups[5].Success) {
        throw "Version '$cargoVersion' has build metadata. An MSI cannot be built from it (plan decision 6)."
    }
    $major = [int]$match.Groups[1].Value
    $minor = [int]$match.Groups[2].Value
    $patch = [int]$match.Groups[3].Value
    if ($major -gt 255 -or $minor -gt 255 -or $patch -gt 65535) {
        throw "Version '$cargoVersion' is outside the MSI limits (major and minor 0 to 255, patch 0 to 65535)."
    }
    $msiVersion = "$major.$minor.$patch"
    Write-Step "version $msiVersion (Cargo.toml)"

    # ---- B2: release build ------------------------------------------------------------
    New-Item -ItemType Directory -Path $targetDir -Force | Out-Null
    if ($SkipCargoBuild) {
        if (-not (Test-Path -LiteralPath $exePath -PathType Leaf)) { throw "-SkipCargoBuild: $exePath does not exist." }
        if (-not (Test-Path -LiteralPath $buildLog -PathType Leaf)) { throw "-SkipCargoBuild: $buildLog does not exist." }
        Write-Step 'B2 skipped (-SkipCargoBuild): reusing the executable and its log'
    }
    else {
        Write-Step 'B2 cargo build --locked --release'
        # verify-link-exceptions.ps1 needs a log that compiled etw-dnstap in this run.
        # A warm target directory replays the linker output without compiling, so clean the crate first.
        & cargo clean --release -p etw-dnstap 2>&1 | ForEach-Object { "$_" } | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "cargo clean -p etw-dnstap failed (exit code $LASTEXITCODE)." }
        & cargo build --locked --release 2>&1 | ForEach-Object { "$_" } | Tee-Object -FilePath $buildLog | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit code $LASTEXITCODE). See $buildLog." }
    }

    # ---- B3: CI checks on the build ----------------------------------------------------
    Write-Step 'B3 link exception and PE checks'
    & pwsh -NoProfile -File (Join-Path 'scripts' (Join-Path 'ci' 'verify-link-exceptions.ps1')) -LogPath $buildLog
    if ($LASTEXITCODE -ne 0) { throw 'verify-link-exceptions.ps1 failed.' }
    & pwsh -NoProfile -File (Join-Path 'scripts' (Join-Path 'ci' 'verify-pe.ps1')) -Path $exePath
    if ($LASTEXITCODE -ne 0) { throw 'verify-pe.ps1 failed.' }

    # ---- B4: version resource of the executable ----------------------------------------
    Write-Step 'B4 executable version resource'
    $info = [Diagnostics.FileVersionInfo]::GetVersionInfo((Resolve-Path -LiteralPath $exePath).ProviderPath)
    $expectedBinary = "$major.$minor.$patch.0"
    if ($info.FileVersionRaw.ToString() -ne $expectedBinary) {
        throw "Executable binary FileVersion is $($info.FileVersionRaw), expected $expectedBinary."
    }
    if ($info.FileVersion -ne $msiVersion) {
        throw "Executable FileVersion string is '$($info.FileVersion)', expected '$msiVersion'."
    }
    if ($info.ProductVersion -ne $cargoVersion) {
        throw "Executable ProductVersion string is '$($info.ProductVersion)', expected '$cargoVersion'."
    }
    Write-Step "executable FileVersion $msiVersion, ProductVersion $cargoVersion"

    # ---- B5: WiX build ---------------------------------------------------------------
    Write-Step 'B5 dotnet build packaging\msi\EtwDnstap.wixproj'
    $msiName = "etw-dnstap-$msiVersion-x64.msi"
    $msiPath = Join-Path $targetDir $msiName
    if (Test-Path -LiteralPath $msiPath) { Remove-Item -LiteralPath $msiPath -Force }
    $wixExeRel = 'target/x86_64-pc-windows-msvc/release/etw-dnstap.exe'
    & dotnet build ($wixProject -replace '\\', '/') -c Release -nologo `
        "-p:EtwVersion=$msiVersion" `
        "-p:EtwExePath=../../$wixExeRel" `
        "-p:EtwRepoRoot=../../" `
        "-p:BaseIntermediateOutputPath=../../$wixObj" `
        "-p:BaseOutputPath=../../$wixBin" `
        "-p:OutputPath=../../$($targetDir -replace '\\', '/')/"
    if ($LASTEXITCODE -ne 0) { throw "dotnet build of the WiX project failed (exit code $LASTEXITCODE)." }
    if (-not (Test-Path -LiteralPath $msiPath -PathType Leaf)) { throw "The MSI was not produced at $msiPath." }

    # ---- B6: MSI properties --------------------------------------------------------------
    Write-Step 'B6 read the MSI Property table'
    $expected = @{
        ProductVersion = $msiVersion
        UpgradeCode   = '{0F4D1A87-DAD8-4C37-ABD4-FEC4DA400FAB}'
        ProductName   = 'EtwDnstap'
        Manufacturer  = 'miketheitguy'
    }
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $database = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer,
        @((Resolve-Path -LiteralPath $msiPath).ProviderPath, 0))
    $actual = @{}
    try {
        $view = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database,
            @('SELECT `Property`, `Value` FROM `Property`'))
        $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null
        while ($true) {
            $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
            if ($null -eq $record) { break }
            $name = $record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, @(1))
            $value = $record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, @(2))
            $actual[$name] = $value
        }
        $view.GetType().InvokeMember('Close', 'InvokeMethod', $null, $view, $null) | Out-Null
    }
    finally {
        [Runtime.InteropServices.Marshal]::ReleaseComObject($database) | Out-Null
        [Runtime.InteropServices.Marshal]::ReleaseComObject($installer) | Out-Null
    }
    foreach ($key in $expected.Keys) {
        if ($actual[$key] -cne $expected[$key]) {
            throw "MSI property $key is '$($actual[$key])', expected '$($expected[$key])'."
        }
    }
    # The MSI version must match the executable's version too.
    Write-Step "MSI ProductVersion $($actual['ProductVersion']), UpgradeCode $($actual['UpgradeCode'])"

    # ---- B7: hash -------------------------------------------------------------------
    Write-Step 'B7 SHA-256'
    $hash = (Get-FileHash -LiteralPath $msiPath -Algorithm SHA256).Hash
    $hashPath = "$msiPath.sha256"
    "$hash  $msiName" | Set-Content -LiteralPath $hashPath -Encoding ascii
    Write-Output "version: $msiVersion"
    Write-Output "msi:     $msiPath"
    Write-Output "sha256:  $hash"
    Write-Output "hash:    $hashPath"
    exit 0
}
catch {
    Write-Output ("[B] FAILED: " + $_.Exception.Message)
    exit 1
}
finally {
    Pop-Location
}
