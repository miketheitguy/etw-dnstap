[CmdletBinding()]
param([Parameter(Mandatory)][ValidateSet('Start','Trace','Stop','Fetch','Clock')][string]$Action,[Parameter(Mandatory)][string]$RunId,[string]$EvidenceRoot='target/performance/windows')
$ErrorActionPreference = 'Stop'
if ($RunId -notmatch '^[A-Za-z0-9_-]{1,64}$') { throw 'Invalid run ID' }
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$evidence = [IO.Path]::GetFullPath([IO.Path]::Combine($root,$EvidenceRoot))
if (-not $evidence.StartsWith($root + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw 'Evidence root must stay within the workspace' }
$out = Join-Path $evidence $RunId
New-Item -ItemType Directory -Path $out -Force | Out-Null
$vm = Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac') { throw 'Wrong VM identity' }
$credential = Import-Clixml -LiteralPath (Join-Path $root 'target/lab-windows/private/administrator.clixml')
$session = New-PSSession -VMId $vm.Id -Credential $credential
try {
    if ($Action -in 'Start','Clock') {
        $clocks = @()
        foreach ($attempt in 1..3) {
            $before = [DateTime]::UtcNow
            $guest = Invoke-Command -Session $session -ScriptBlock { [DateTime]::UtcNow.ToString('o') }
            $after = [DateTime]::UtcNow
            $clocks += [pscustomobject]@{host_before=$before.ToString('o');guest_utc=$guest;host_after=$after.ToString('o');offset_lower_ms=([DateTime]::Parse($guest).ToUniversalTime()-$after).TotalMilliseconds;offset_upper_ms=([DateTime]::Parse($guest).ToUniversalTime()-$before).TotalMilliseconds}
        }
        $clocks | ConvertTo-Json | Set-Content (Join-Path $out $(if ($Action -eq 'Clock') {'clock-bounds-end.json'} else {'clock-bounds.json'}))
    }
    if ($Action -eq 'Clock') { return }
    Invoke-Command -Session $session -ArgumentList $Action,$RunId -ScriptBlock {
        param($action,$run)
        $ErrorActionPreference = 'Stop'
        $directory = "C:/ProgramData/EtwDnstap/performance/$run-capture"
        $collector = "EtwDnstapPerf-$run"
        if ($action -eq 'Start') {
            if (Test-Path -LiteralPath $directory) { throw 'Capture directory exists' }
            New-Item -ItemType Directory -Path $directory | Out-Null
            $svc = Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"
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
            $counters | Set-Content "$directory/counters.txt"
            $verification = Get-Counter -Counter $counters -ErrorAction SilentlyContinue -ErrorVariable invalid
            @($invalid | ForEach-Object { $_.ToString() }) | Set-Content "$directory/counter-validation-errors.txt"
            $threadParents = @{}
            foreach ($sample in $verification.CounterSamples) {
                if ($sample.Path -match '\\thread\(([^)]+)\)\\id process$' -and $ids -contains [int]$sample.CookedValue) { $threadParents[$Matches[1]] = $true }
            }
            # Include only paths verified on this OS. Keep the exclusions as evidence.
            $validPaths = @($verification.CounterSamples | Where-Object {
                $_.Status -eq 0 -and ($_.Path -notmatch '\\thread\(([^)]+)\)' -or $threadParents.ContainsKey($Matches[1]))
            } | Select-Object -ExpandProperty Path -Unique)
            if ($validPaths.Count -lt 40) { throw 'Too few valid counters' }
            $validPaths | Set-Content "$directory/valid-counters.txt"
            $metadata = [ordered]@{run=$run;start_utc=[DateTime]::UtcNow.ToString('o');service_pid=$svc.ProcessId;dns_pid=$dns.Id;instances=$instances;logical_processors=[Environment]::ProcessorCount;service_command=$svc.PathName;service_sha256=(Get-FileHash 'C:/Program Files/EtwDnstap/etw-dnstap.exe').Hash;valid_counter_paths=$validPaths.Count}
            $metadata | ConvertTo-Json -Depth 5 | Set-Content "$directory/metadata.json"
            & logman.exe create counter $collector -cf "$directory/valid-counters.txt" -si 00:00:01 -f bin -max 256 -rf 00:12:00 -o "$directory/counters.blg" | Set-Content "$directory/logman-create.txt"
            if ($LASTEXITCODE -ne 0) { throw 'Create logman collector failed' }
            & logman.exe start $collector | Set-Content "$directory/logman-start.txt"
            if ($LASTEXITCODE -ne 0) { throw 'Start logman collector failed' }
        }
        if ($action -eq 'Trace') {
            & wpr.exe -status 2>&1 | Set-Content "$directory/wpr-status-before.txt"
            # WPR refuses to start when another recording is active; never cancel it.
            & wpr.exe -start GeneralProfile 2>&1 | Set-Content "$directory/wpr-start.txt"
            if ($LASTEXITCODE -ne 0) { throw 'WPR did not start; existing recording is preserved' }
            @"
Start-Sleep -Seconds 45
[DateTime]::UtcNow.ToString('o') | Set-Content '$directory/wpr-stop-begin-utc.txt'
& wpr.exe -stop '$directory/peak.etl' 'EtwDnstap $run bounded 45 second peak' 2>&1 | Set-Content '$directory/wpr-stop.txt'
`$LASTEXITCODE | Set-Content '$directory/wpr-stop-exit.txt'
[DateTime]::UtcNow.ToString('o') | Set-Content '$directory/wpr-stop-end-utc.txt'
"@ | Set-Content "$directory/stop-trace.ps1"
            Start-Process powershell.exe -WindowStyle Hidden -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File',"$directory/stop-trace.ps1") | Out-Null
            [DateTime]::UtcNow.ToString('o') | Set-Content "$directory/wpr-start-utc.txt"
        }
        if ($action -eq 'Stop') {
            & logman.exe stop $collector | Set-Content "$directory/logman-stop.txt"
            if ($LASTEXITCODE -ne 0) { throw 'Stop collector failed' }
            & logman.exe delete $collector | Set-Content "$directory/logman-delete.txt"
            $blg = @(Get-ChildItem -LiteralPath $directory -Filter '*.blg')
            if ($blg.Count -ne 1) { throw 'Expected exactly one BLG capture' }
            & relog.exe $blg[0].FullName -f csv -o "$directory/counters.csv" | Set-Content "$directory/relog.txt"
            if ($LASTEXITCODE -ne 0) { throw 'Export counters failed' }
            [DateTime]::UtcNow.ToString('o') | Set-Content "$directory/stop-utc.txt"
            Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'" | Select-Object Name,State,ProcessId,PathName | ConvertTo-Json | Set-Content "$directory/service-end.json"
            Get-Process -Name etw-dnstap,dns -ErrorAction SilentlyContinue | Select-Object Name,Id,CPU,PrivateMemorySize64,WorkingSet64,HandleCount,@{n='ThreadIds';e={@($_.Threads | Select-Object -ExpandProperty Id)}} | ConvertTo-Json -Depth 4 | Set-Content "$directory/processes-end.json"
        }
        Get-ChildItem -LiteralPath $directory | Select-Object Name,Length
    } | Format-Table -AutoSize
    if ($Action -in 'Stop','Fetch') {
        $files = Invoke-Command -Session $session -ArgumentList $RunId -ScriptBlock { param($run) Get-ChildItem -LiteralPath "C:/ProgramData/EtwDnstap/performance/$run-capture" -File | Select-Object Name,Length,@{n='Sha256';e={(Get-FileHash -LiteralPath $_.FullName).Hash}} }
        foreach ($file in $files) {
            $source = "C:/ProgramData/EtwDnstap/performance/$RunId-capture/$($file.Name)"
            $destination = Join-Path $out $file.Name
            if ((Test-Path -LiteralPath $destination) -and (Get-FileHash -LiteralPath $destination).Hash -eq $file.Sha256) { continue }
            if ($file.Length -eq 0) { [IO.File]::WriteAllBytes($destination,[byte[]]@()); continue }
            if ($file.Length -lt 2097152) {
                $encoded = Invoke-Command -Session $session -ArgumentList $source -ScriptBlock { param($path) [Convert]::ToBase64String([IO.File]::ReadAllBytes($path)) }
                [IO.File]::WriteAllBytes($destination,[Convert]::FromBase64String($encoded))
            } else {
                try { Copy-Item -LiteralPath $source -Destination $destination -FromSession $session -Force }
                catch { if (-not (Test-Path -LiteralPath $destination) -or (Get-FileHash -LiteralPath $destination).Hash -ne $file.Sha256) { throw } }
            }
            if ((Get-FileHash -LiteralPath $destination).Hash -ne $file.Sha256) { throw "Copy hash mismatch: $($file.Name)" }
        }
    }
} finally { Remove-PSSession $session }
