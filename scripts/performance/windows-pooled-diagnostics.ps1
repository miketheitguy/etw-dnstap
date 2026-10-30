[CmdletBinding()]
param([Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId,[Parameter(Mandatory)][string]$SinceUtc,[switch]$RawEventScan)
$ErrorActionPreference='Stop'
$root=(Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$out=Join-Path $root "target/performance/pooled-vm-20261029-0942/stages/$RunId/service-diagnostics.json"
if ($RawEventScan) { $out=Join-Path $root "target/performance/pooled-vm-20261029-0942/stages/$RunId/service-diagnostics-raw-events.json" }
if (Test-Path -LiteralPath $out) { throw 'Preserve existing diagnostics' }
$vm=Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac') { throw 'Wrong VM' }
$credential=Import-Clixml (Join-Path $root 'target/lab-windows/private/administrator.clixml')
$result=Invoke-Command -VMId $vm.Id -Credential $credential -ArgumentList @($SinceUtc,[bool]$RawEventScan) -ScriptBlock {
    param($since,$rawScan)
    $start=[DateTime]::Parse($since).ToUniversalTime()
    if ($rawScan) {
        $queryErrors=@()
        $windowEvents=@(Get-WinEvent -FilterHashtable @{LogName='Application';StartTime=$start} -MaxEvents 500 -ErrorAction SilentlyContinue -ErrorVariable +queryErrors)
        $application=@($windowEvents | Where-Object { $_.ToXml() -match 'EtwDnstap' } | Select-Object TimeCreated,Id,RecordId,ProviderName,Message,@{n='Xml';e={$_.ToXml()}})
        $recentEvents=@(Get-WinEvent -LogName Application -MaxEvents 500 -ErrorAction SilentlyContinue -ErrorVariable +queryErrors)
        $recent=@($recentEvents | Where-Object { $_.ToXml() -match 'EtwDnstap' } | Select-Object -First 10 TimeCreated,Id,RecordId,ProviderName,Message,@{n='Xml';e={$_.ToXml()}})
        $classic=@(Get-EventLog -LogName Application -Newest 1000 -ErrorAction SilentlyContinue -ErrorVariable +queryErrors | Where-Object Source -eq 'EtwDnstap' | Select-Object -First 10 TimeGenerated,EventID,Index,Source,EntryType,Message,ReplacementStrings)
        [ordered]@{utc=[DateTime]::UtcNow.ToString('o');since_utc=$start.ToString('o');window_records_examined=$windowEvents.Count;recent_records_examined=$recentEvents.Count;window_application=$application;recent_application=$recent;classic_application=$classic;log_metadata=@(Get-WinEvent -ListLog Application,System | Select-Object LogName,IsEnabled,RecordCount,LogMode,LastWriteTime);query_errors=@($queryErrors | ForEach-Object ToString)}
        return
    }
    $application=@(Get-WinEvent -FilterHashtable @{LogName='Application';ProviderName='EtwDnstap';StartTime=$start} -ErrorAction SilentlyContinue | Select-Object -First 20 TimeCreated,Id,RecordId,LevelDisplayName,ProviderName,Message,@{n='Xml';e={$_.ToXml()}})
    $scm=@(Get-WinEvent -FilterHashtable @{LogName='System';ProviderName='Service Control Manager';StartTime=$start} -ErrorAction SilentlyContinue | Where-Object Message -match 'EtwDnstap' | Select-Object -First 20 TimeCreated,Id,RecordId,LevelDisplayName,ProviderName,Message,@{n='Xml';e={$_.ToXml()}})
    [ordered]@{utc=[DateTime]::UtcNow.ToString('o');since_utc=$start.ToString('o');service=Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'" | Select-Object Name,State,ProcessId,ExitCode,ServiceSpecificExitCode,PathName;application_events=$application;scm_events=$scm;wpr_status=(& wpr.exe -status | Out-String);free_bytes=(Get-Volume C).SizeRemaining}
}
$result | ConvertTo-Json -Depth 7 | Set-Content -LiteralPath $out -Encoding utf8
Get-Content -LiteralPath $out -Raw
