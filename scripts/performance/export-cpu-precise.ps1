#requires -Version 5.1
<#
.SYNOPSIS
Inspects or exports precise per-thread CPU data from a Windows Performance Recorder trace.

.DESCRIPTION
Inspect runs xperf trace statistics and a context-switch coverage report. Export runs
wpaexporter with cpu-precise-threads.wpaProfile over an explicit interior time range and an
xperf exclusive-of-DPC/ISR context-switch report over the same window. Every tool call is
recorded (command, log, result) and existing evidence is never overwritten. Requires the
Windows Performance Toolkit (xperf.exe, wpaexporter.exe) from the Windows ADK on the host.

.PARAMETER Action
Inspect or Export.
.PARAMETER RunId
Output sub-directory name.
.PARAMETER TracePath
Path to the .etl trace.
.PARAMETER ViewStartSeconds
Export only: window start in seconds (finite, >= 0).
.PARAMETER ViewEndSeconds
Export only: window end in seconds (finite, > start).
.PARAMETER OutputRoot
Evidence root (default target/performance/review/1000qps under the repository).

.OUTPUTS
<OutputRoot>/<RunId>/ with *-command.json, *.log and *-result.json per tool call, plus the exported tables.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Inspect','Export')][string]$Action,
    [Parameter(Mandatory)][ValidatePattern('^[a-zA-Z0-9_-]+$')][string]$RunId,
    [Parameter(Mandatory)][string]$TracePath,
    [double]$ViewStartSeconds = 0,
    [double]$ViewEndSeconds = 0,
    [string]$OutputRoot = (Join-Path $PSScriptRoot '../../target/performance/review/1000qps')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'LabCommon.psm1') -Force
$trace = (Resolve-Path -LiteralPath $TracePath).Path
$toolsDirectory = 'C:/Program Files (x86)/Windows Kits/10/Windows Performance Toolkit'
$xperf = Join-Path $toolsDirectory 'xperf.exe'
$exporter = Join-Path $toolsDirectory 'wpaexporter.exe'
$profile = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot 'cpu-precise-threads.wpaProfile')).Path
$outputDirectory = Join-Path $OutputRoot $RunId
$null = New-Item -ItemType Directory -Path $outputDirectory -Force

function Invoke-RecordedTool {
    param([string]$Executable, [string[]]$ToolArguments, [string]$Name)
    $log = Join-Path $outputDirectory "$Name.log"
    $command = Join-Path $outputDirectory "$Name-command.json"
    $result = Join-Path $outputDirectory "$Name-result.json"
    foreach ($path in @($log, $command, $result)) {
        if (Test-Path -LiteralPath $path) { throw "Preserve existing evidence: $path" }
    }
    $started = [DateTime]::UtcNow.ToString('o')
    Write-LabJson -Path $command -InputObject @{ executable=$Executable; arguments=$ToolArguments; trace=$trace; started_utc=$started } -Depth 4
    & $Executable @ToolArguments *> $log
    $toolExit = $LASTEXITCODE
    Write-LabJson -Path $result -InputObject @{ exit_code=$toolExit; started_utc=$started; ended_utc=[DateTime]::UtcNow.ToString('o') }
    if ($toolExit -ne 0) { throw "$Name failed ($toolExit); inspect $log" }
}

if ($Action -eq 'Inspect') {
    Invoke-RecordedTool $xperf @('-i', $trace, '-a', 'tracestats', '-timespan') 'trace-headers'
    Invoke-RecordedTool $xperf @('-i', $trace, '-a', 'cswitch', '-util') 'cswitch-coverage'
} else {
    if ([double]::IsNaN($ViewStartSeconds) -or [double]::IsInfinity($ViewStartSeconds) -or [double]::IsNaN($ViewEndSeconds) -or [double]::IsInfinity($ViewEndSeconds) -or $ViewStartSeconds -lt 0 -or $ViewEndSeconds -le $ViewStartSeconds) {
        throw 'Export requires a finite 0 <= ViewStartSeconds < ViewEndSeconds'
    }
    $culture = [Globalization.CultureInfo]::InvariantCulture
    $startSeconds = $ViewStartSeconds.ToString('0.#########', $culture) + 's'
    $endSeconds = $ViewEndSeconds.ToString('0.#########', $culture) + 's'
    $startMicroseconds = [long][Math]::Round($ViewStartSeconds * 1000000)
    $endMicroseconds = [long][Math]::Round($ViewEndSeconds * 1000000)
    $pluginDirectory = Join-Path $toolsDirectory 'CustomDataSources/Event_Tracing_for_Windows'
    $exportArguments = @('-i', $trace, '-range', $startSeconds, $endSeconds, '-profile', $profile, '-nodefault', '-addsearchdir', $pluginDirectory, '-outputfolder', $outputDirectory, '-prefix', 'precise_')
    Invoke-RecordedTool $exporter $exportArguments 'wpa-export'
    # This report explicitly excludes observed DPC/ISR time from scheduled CPU
    # accounting and uses the same interior time range as the WPA export.
    $exclusiveArguments = @('-i', $trace, '-a', 'cswitch', '-process', '-thread', '-exc_dpcisr', '-range', "$startMicroseconds", "$endMicroseconds")
    Invoke-RecordedTool $xperf $exclusiveArguments 'cswitch-exclusive-same-window'
}
Write-Output $outputDirectory
