[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Inspect','Export')][string]$Action,
    [Parameter(Mandatory)][ValidatePattern('^[a-zA-Z0-9_-]+$')][string]$RunId,
    [Parameter(Mandatory)][string]$TracePath,
    [double]$ViewStartSeconds = 0,
    [double]$ViewEndSeconds = 0,
    [string]$OutputRoot = (Join-Path $PSScriptRoot '../../target/performance/review/1000qps')
)
$ErrorActionPreference = 'Stop'
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
    @{ executable=$Executable; arguments=$ToolArguments; trace=$trace; started_utc=$started } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $command -Encoding utf8
    & $Executable @ToolArguments *> $log
    $toolExit = $LASTEXITCODE
    @{ exit_code=$toolExit; started_utc=$started; ended_utc=[DateTime]::UtcNow.ToString('o') } | ConvertTo-Json | Set-Content -LiteralPath $result -Encoding utf8
    if ($toolExit -ne 0) { throw "$Name failed ($toolExit); inspect $log" }
}

if ($Action -eq 'Inspect') {
    Invoke-RecordedTool $xperf @('-i', $trace, '-a', 'tracestats', '-timespan') 'trace-headers'
    Invoke-RecordedTool $xperf @('-i', $trace, '-a', 'cswitch', '-util') 'cswitch-coverage'
} else {
    if (-not [double]::IsFinite($ViewStartSeconds) -or -not [double]::IsFinite($ViewEndSeconds) -or $ViewStartSeconds -lt 0 -or $ViewEndSeconds -le $ViewStartSeconds) {
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
