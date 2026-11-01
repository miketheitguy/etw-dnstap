#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Starts/stops the guest performance-counter collector, WPR trace and clock probes for a run.

.DESCRIPTION
Start validates and creates a logman counter collector for the EtwDnstap and DNS processes;
Trace starts a bounded WPR GeneralProfile recording (never cancelling an existing one);
Stop ends the collector and exports CSV; Fetch copies the run's capture directory;
Clock records host/guest clock offset bounds. Files are hash-verified on transfer.

.PARAMETER Action
Start, Trace, Stop, Fetch or Clock.
.PARAMETER RunId
Run label (1-64 letters, digits, '_' or '-').
.PARAMETER EvidenceRoot
Repository-relative evidence root (default target/performance/windows); must stay inside the repository.

.OUTPUTS
<EvidenceRoot>/<RunId>/ with clock-bounds*.json and the fetched capture files.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Start','Trace','Stop','Fetch','Clock')][string]$Action,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId,
    [string]$EvidenceRoot = 'target/performance/windows'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'LabCommon.psm1') -Force
$config = Get-LabConfig
$evidence = Resolve-LabPath $EvidenceRoot
$out = Join-Path $evidence $RunId
New-Item -ItemType Directory -Path $out -Force | Out-Null
$vm = Get-LabVm
$session = Get-LabSession -Vm $vm
try {
    if ($Action -in 'Start','Clock') {
        $clocks = @()
        foreach ($attempt in 1..3) {
            $before = [DateTime]::UtcNow
            $guest = Invoke-LabGuest -Session $session -ScriptBlock { [DateTime]::UtcNow.ToString('o') }
            $after = [DateTime]::UtcNow
            $clocks += [pscustomobject]@{host_before=$before.ToString('o');guest_utc=$guest;host_after=$after.ToString('o');offset_lower_ms=([DateTime]::Parse($guest).ToUniversalTime()-$after).TotalMilliseconds;offset_upper_ms=([DateTime]::Parse($guest).ToUniversalTime()-$before).TotalMilliseconds}
        }
        Write-LabJson -Path (Join-Path $out $(if ($Action -eq 'Clock') {'clock-bounds-end.json'} else {'clock-bounds.json'})) -InputObject $clocks -Force
    }
    if ($Action -eq 'Clock') { return }
    Invoke-LabGuest -Session $session -ArgumentList $Action,$RunId,$config.windows_vm.data_dir,$config.windows_vm.service_name,$config.windows_vm.install_exe -ScriptBlock {
        param($action,$run,$dataDir,$serviceName,$installed)
        $directory = "$dataDir/$run-capture"
        $collector = "EtwDnstapPerf-$run"
        if ($action -eq 'Start') {
            if (Test-Path -LiteralPath $directory) { throw 'Capture directory exists' }
            New-Item -ItemType Directory -Path $directory | Out-Null
            $svc = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
            if ($svc.State -ne 'Running') { throw 'Service is not running' }
            $dns = Get-Process -Name dns
            $ids = @($svc.ProcessId,$dns.Id)
            $instances = @(Get-Counter '\Process(*)\ID Process' | ForEach-Object CounterSamples | Where-Object { $ids -contains [int]$_.CookedValue } | Select-Object -ExpandProperty InstanceName)
            if ($instances.Count -ne 2) { throw 'Could not resolve exact service and DNS process instances' }
            $processCounters = @('ID Process','% Processor Time','% User Time','% Privileged Time','Private Bytes','Working Set','Working Set - Private','Virtual Bytes','Page Faults/sec','Thread Count','Handle Count','IO Read Bytes/sec','IO Write Bytes/sec','IO Other Bytes/sec','IO Read Operations/sec','IO Write Operations/sec','IO Other Operations/sec')
            $threadCounters = @('ID Process','ID Thread','% Processor Time','% User Time','% Privileged Time','Context Switches/sec','Thread State','Thread Wait Reason')
            $counters = @()
            foreach ($instance in $instances) {
                foreach ($counter in $processCounters) { $counters += "\Process($instance)\$counter" }
                foreach ($counter in $threadCounters) { $counters += "\Thread($instance/*)\$counter" }
            }
            $counters += @('\Processor(_Total)\% Processor Time','\Processor(_Total)\% User Time','\Processor(_Total)\% Privileged Time','\System\Processor Queue Length','\System\Context Switches/sec','\Memory\Available MBytes','\Memory\Committed Bytes','\Memory\Pages/sec','\PhysicalDisk(_Total)\Avg. Disk sec/Read','\PhysicalDisk(_Total)\Avg. Disk sec/Write','\PhysicalDisk(_Total)\Current Disk Queue Length','\Network Interface(*)\Bytes Total/sec','\Network Interface(*)\Packets Received Errors','\Network Interface(*)\Packets Outbound Errors','\UDPv4\Datagrams Received Errors','\UDPv4\Datagrams Received/sec','\UDPv4\Datagrams Sent/sec','\TCPv4\Segments Retransmitted/sec')
            $counters += @((Get-Counter -ListSet DNS).Paths)
            $counters | Set-Content "$directory/counters.txt" -Encoding ASCII
            $verification = Get-Counter -Counter $counters -ErrorAction SilentlyContinue -ErrorVariable invalid
            @($invalid | ForEach-Object { $_.ToString() }) | Set-Content "$directory/counter-validation-errors.txt" -Encoding UTF8
            $threadParents = @{}
            foreach ($sample in $verification.CounterSamples) {
                if ($sample.Path -match '\\thread\(([^)]+)\)\\id process$' -and $ids -contains [int]$sample.CookedValue) { $threadParents[$Matches[1]] = $true }
            }
            # Include only paths verified on this OS. Keep the exclusions as evidence.
            $validPaths = @($verification.CounterSamples | Where-Object {
                $_.Status -eq 0 -and ($_.Path -notmatch '\\thread\(([^)]+)\)' -or $threadParents.ContainsKey($Matches[1]))
            } | Select-Object -ExpandProperty Path -Unique)
            if ($validPaths.Count -lt 40) { throw 'Too few valid counters' }
            $validPaths | Set-Content "$directory/valid-counters.txt" -Encoding ASCII
            $metadata = [ordered]@{run=$run;start_utc=[DateTime]::UtcNow.ToString('o');service_pid=$svc.ProcessId;dns_pid=$dns.Id;instances=$instances;logical_processors=[Environment]::ProcessorCount;service_command=$svc.PathName;service_sha256=(Get-FileHash $installed).Hash;valid_counter_paths=$validPaths.Count}
            $metadata | ConvertTo-Json -Depth 5 | Set-Content "$directory/metadata.json" -Encoding UTF8
            & logman.exe create counter $collector -cf "$directory/valid-counters.txt" -si 00:00:01 -f bin -max 256 -rf 00:12:00 -o "$directory/counters.blg" | Set-Content "$directory/logman-create.txt" -Encoding UTF8
            if ($LASTEXITCODE -ne 0) { throw 'Create logman collector failed' }
            & logman.exe start $collector | Set-Content "$directory/logman-start.txt" -Encoding UTF8
            if ($LASTEXITCODE -ne 0) { throw 'Start logman collector failed' }
        }
        if ($action -eq 'Trace') {
            & wpr.exe -status 2>&1 | Set-Content "$directory/wpr-status-before.txt" -Encoding UTF8
            # WPR refuses to start when another recording is active; never cancel it.
            & wpr.exe -start GeneralProfile 2>&1 | Set-Content "$directory/wpr-start.txt" -Encoding UTF8
            if ($LASTEXITCODE -ne 0) { throw 'WPR did not start; existing recording is preserved' }
            @"
Start-Sleep -Seconds 45
[DateTime]::UtcNow.ToString('o') | Set-Content '$directory/wpr-stop-begin-utc.txt' -Encoding UTF8
& wpr.exe -stop '$directory/peak.etl' 'EtwDnstap $run bounded 45 second peak' 2>&1 | Set-Content '$directory/wpr-stop.txt' -Encoding UTF8
`$LASTEXITCODE | Set-Content '$directory/wpr-stop-exit.txt' -Encoding UTF8
[DateTime]::UtcNow.ToString('o') | Set-Content '$directory/wpr-stop-end-utc.txt' -Encoding UTF8
"@ | Set-Content "$directory/stop-trace.ps1" -Encoding UTF8
            Start-Process powershell.exe -WindowStyle Hidden -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File',"$directory/stop-trace.ps1") | Out-Null
            [DateTime]::UtcNow.ToString('o') | Set-Content "$directory/wpr-start-utc.txt" -Encoding UTF8
        }
        if ($action -eq 'Stop') {
            & logman.exe stop $collector | Set-Content "$directory/logman-stop.txt" -Encoding UTF8
            if ($LASTEXITCODE -ne 0) { throw 'Stop collector failed' }
            & logman.exe delete $collector | Set-Content "$directory/logman-delete.txt" -Encoding UTF8
            $blg = @(Get-ChildItem -LiteralPath $directory -Filter '*.blg')
            if ($blg.Count -ne 1) { throw 'Expected exactly one BLG capture' }
            & relog.exe $blg[0].FullName -f csv -o "$directory/counters.csv" | Set-Content "$directory/relog.txt" -Encoding UTF8
            if ($LASTEXITCODE -ne 0) { throw 'Export counters failed' }
            [DateTime]::UtcNow.ToString('o') | Set-Content "$directory/stop-utc.txt" -Encoding UTF8
            Get-CimInstance Win32_Service -Filter "Name='$serviceName'" | Select-Object Name,State,ProcessId,PathName | ConvertTo-Json | Set-Content "$directory/service-end.json" -Encoding UTF8
            Get-Process -Name etw-dnstap,dns -ErrorAction SilentlyContinue | Select-Object Name,Id,CPU,PrivateMemorySize64,WorkingSet64,HandleCount,@{n='ThreadIds';e={@($_.Threads | Select-Object -ExpandProperty Id)}} | ConvertTo-Json -Depth 4 | Set-Content "$directory/processes-end.json" -Encoding UTF8
        }
        Get-ChildItem -LiteralPath $directory | Select-Object Name,Length
    } | Format-Table -AutoSize
    if ($Action -in 'Stop','Fetch') {
        $guestDirectory = "$($config.windows_vm.data_dir)/$RunId-capture"
        $null = Receive-LabDirectory -Session $session -GuestDirectory $guestDirectory -Destination $out -ExcludeExtension @() -Overwrite
    }
} finally { Remove-PSSession $session }
