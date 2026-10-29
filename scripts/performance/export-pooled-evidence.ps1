[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Inspect','Export','Context')][string]$Action,
    [Parameter(Mandatory)][string]$TracePath,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [double]$ViewStartSeconds = 0,
    [double]$ViewEndSeconds = 0,
    [ValidatePattern('^[a-zA-Z0-9_.-]+ \([0-9]+\)$')][string[]]$TargetProcess = @(),
    [ValidateSet('Scheduling','Sampled','Disk','HardFault')][string[]]$Tables = @('Scheduling','Sampled','Disk','HardFault'),
    [switch]$Symbols,
    [switch]$FileEvents
)
$ErrorActionPreference = 'Stop'
# Run only between load stages. Never append to or overwrite prior evidence.
$trace = (Resolve-Path -LiteralPath $TracePath).Path
if (Test-Path -LiteralPath $OutputDirectory) { throw "Preserve existing evidence: $OutputDirectory" }
$output = (New-Item -ItemType Directory -Path $OutputDirectory).FullName
$toolRoot = 'C:/Program Files (x86)/Windows Kits/10/Windows Performance Toolkit'
$xperf = Join-Path $toolRoot 'xperf.exe'
$exporter = Join-Path $toolRoot 'wpaexporter.exe'
$profile = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot 'pooled-evidence.wpaProfile')).Path

function Invoke-RecordedTool {
    param([string]$Executable, [string[]]$ToolArguments, [string]$Name)
    $started = [DateTime]::UtcNow.ToString('o')
    @{ executable=$Executable; arguments=$ToolArguments; started_utc=$started } |
        ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $output "$Name-command.json") -Encoding utf8
    & $Executable @ToolArguments *> (Join-Path $output "$Name.log")
    $toolExit = $LASTEXITCODE
    @{ exit_code=$toolExit; started_utc=$started; ended_utc=[DateTime]::UtcNow.ToString('o') } |
        ConvertTo-Json | Set-Content -LiteralPath (Join-Path $output "$Name-result.json") -Encoding utf8
    if ($toolExit -ne 0) { throw "$Name failed ($toolExit); preserve and inspect its log" }
}

$receipt = @{
    action=$Action; trace=$trace; trace_bytes=(Get-Item -LiteralPath $trace).Length
    trace_sha256=(Get-FileHash -LiteralPath $trace -Algorithm SHA256).Hash
    start_seconds=$ViewStartSeconds; end_seconds=$ViewEndSeconds
    target_process=$TargetProcess; tables=$Tables; symbols=[bool]$Symbols; file_events=[bool]$FileEvents
    profile_sha256=(Get-FileHash -LiteralPath $profile -Algorithm SHA256).Hash
    xperf_version=(Get-Item -LiteralPath $xperf).VersionInfo.FileVersion
    exporter_version=(Get-Item -LiteralPath $exporter).VersionInfo.FileVersion
    symbol_path=$env:_NT_SYMBOL_PATH; symcache_path=$env:_NT_SYMCACHE_PATH
}
$receipt | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $output 'receipt.json') -Encoding utf8
if ($Action -eq 'Inspect') {
    Invoke-RecordedTool $xperf @('-i',$trace,'-a','tracestats','-timespan','actual') 'trace-timespan'
    Invoke-RecordedTool $xperf @('-i',$trace,'-a','tracestats','-detail','stack') 'trace-events-stacks'
    Invoke-RecordedTool $xperf @('-i',$trace,'-a','cswitch','-util') 'cswitch-full-coverage'
    Invoke-RecordedTool $xperf @('-i',$trace,'-a','process','-thread') 'process-thread-lifetimes'
} else {
    if (-not [double]::IsFinite($ViewStartSeconds) -or -not [double]::IsFinite($ViewEndSeconds) -or
        $ViewStartSeconds -lt 0 -or $ViewEndSeconds -le $ViewStartSeconds) {
        throw 'Export requires a finite 0 <= ViewStartSeconds < ViewEndSeconds'
    }
    $culture = [Globalization.CultureInfo]::InvariantCulture
    $start = $ViewStartSeconds.ToString('0.#########',$culture) + 's'
    $end = $ViewEndSeconds.ToString('0.#########',$culture) + 's'
    $startUs = [long][Math]::Round($ViewStartSeconds * 1000000)
    $endUs = [long][Math]::Round($ViewEndSeconds * 1000000)
    if ($Action -eq 'Context') {
        Invoke-RecordedTool $xperf @('-i',$trace,'-a','cswitch','-util','-range',"$startUs","$endUs") 'cpu-per-vcpu-inclusive'
        Invoke-RecordedTool $xperf @('-i',$trace,'-a','cswitch','-util','-exc_dpcisr','-range',"$startUs","$endUs") 'cpu-per-vcpu-exclusive'
        Invoke-RecordedTool $xperf @('-i',$trace,'-a','dpcisr','-summary','-range',"$startUs","$endUs") 'dpc-isr-summary'
        # Summary tables suffice here. Do not request fine-grained DPC/ISR
        # intervals: this installed tool interprets interval 1 much more finely
        # than the public reference suggests and can generate enormous output.
        Invoke-RecordedTool $xperf @('-i',$trace,'-a','profile','-detail','-range',"$startUs","$endUs") 'sampled-process-modules'
        Write-Output $output
        return
    }
    if ($TargetProcess.Count -eq 0) { throw 'Export requires exact target process names with PIDs to bound raw stack output' }
    [xml]$view = Get-Content -LiteralPath $profile -Raw
    $tableNames = @{
        'c58f5fea-0319-4046-932d-e695ebe20b47'='Scheduling'
        'b855361e-7be0-4bc8-a754-3e8507715ca5'='Sampled'
        '12c5387c-58ca-45d4-a5c6-450475c30e06'='Disk'
        '7c60aeff-a47f-4ba8-942c-d2b154087ab9'='HardFault'
    }
    foreach ($graph in @($view.SelectNodes('//*[local-name()="Graph"]'))) {
        if ($tableNames[$graph.Guid] -notin $Tables) {
            $null = $graph.ParentNode.RemoveChild($graph)
            continue
        }
        $processColumn = if ($graph.Guid -eq 'c58f5fea-0319-4046-932d-e695ebe20b47') { 'New Process' } else { 'Process' }
        $predicates = @($TargetProcess | ForEach-Object { '[' + $processColumn + ']:="' + $_ + '"' })
        $graph.Preset.SetAttribute('InitialFilterQuery',($predicates -join ' OR '))
        $graph.Preset.SetAttribute('InitialFilterShouldKeep','true')
    }
    $runtimeProfile = Join-Path $output 'view.wpaProfile'
    $view.Save($runtimeProfile)
    $exportArguments = @('-i',$trace,'-range',$start,$end,'-profile',$runtimeProfile,
        '-nodefault','-addsearchdir',(Join-Path $toolRoot 'CustomDataSources/Event_Tracing_for_Windows'),
        '-outputfolder',$output,'-prefix','evidence_')
    if ($Symbols) { $exportArguments += '-symbols' }
    Invoke-RecordedTool $exporter $exportArguments 'wpa-export'
    Invoke-RecordedTool $xperf @('-i',$trace,'-a','cswitch','-process','-thread','-exc_dpcisr','-range',"$startUs","$endUs") 'cpu-precise-exclusive'
    Invoke-RecordedTool $xperf @('-i',$trace,'-a','diskio','-detail','-range',"$startUs","$endUs") 'disk-io-detail'
    Invoke-RecordedTool $xperf @('-i',$trace,'-a','hardfault','-range',"$startUs","$endUs") 'hard-faults'
    if ($FileEvents) {
        # Official classic FileIo event-class GUID. Retains all processes in the
        # selected window so file-object/name and OperationEnd joins remain possible.
        Invoke-RecordedTool $xperf @('-i',$trace,'-a','dumper','-provider',
            '{90cbdc39-4a3e-11d1-84f4-0000f80464e3}','-stacktimeshifting',
            '-range',"$startUs","$endUs") 'file-io-events'
    }
}
Write-Output $output
