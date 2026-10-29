[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Prepare','Deploy','Start','Stop','Fetch','Restore','Status','UpdateNames')][string]$Action,
    [string]$RunId = 'setup'
)
$ErrorActionPreference = 'Stop'
if ($RunId -notmatch '^[A-Za-z0-9_-]{1,64}$') { throw 'Invalid run ID' }
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$out = Join-Path $root 'target/performance/windows'
New-Item -ItemType Directory -Path $out -Force | Out-Null
$vm = Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac') { throw 'Wrong VM identity' }
$credential = Import-Clixml -LiteralPath (Join-Path $root 'target/lab-windows/private/administrator.clixml')
$session = New-PSSession -VMId $vm.Id -Credential $credential
try {
    if ($Action -eq 'Prepare') {
        Copy-Item -LiteralPath (Join-Path $root 'target/performance/workload/auth.stress.test.dns') -Destination 'C:/Windows/System32/dns/auth.stress.test.dns' -ToSession $session
    }
    if ($Action -eq 'Deploy') {
        Copy-Item -LiteralPath (Join-Path $root 'target/x86_64-pc-windows-msvc/release/etw-dnstap.exe') -Destination 'C:/Windows/Temp/etw-dnstap-perf.exe' -ToSession $session
    }
    $result = Invoke-Command -Session $session -ArgumentList $Action,$RunId -ScriptBlock {
        param($action,$run)
        $ErrorActionPreference = 'Stop'
        $base = 'C:/ProgramData/EtwDnstap/performance'
        $service = Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"
        if ($action -eq 'Prepare') {
            if (Test-Path -LiteralPath $base) { throw 'Performance directory already exists; inspect prior run first' }
            New-Item -ItemType Directory -Path $base | Out-Null
            & icacls.exe $base /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
            if ($LASTEXITCODE -ne 0) { throw 'Protect performance directory failed' }
            $service | Select-Object Name,PathName,StartName,StartMode,State | ConvertTo-Json | Set-Content "$base/service-original.json"
            Copy-Item 'C:/Program Files/EtwDnstap/etw-dnstap.exe' "$base/service-original.exe"
            Get-FileHash "$base/service-original.exe" | ConvertTo-Json | Set-Content "$base/service-original-hash.json"
            & sc.exe qsidtype EtwDnstap | Set-Content "$base/sidtype.txt"
            & sc.exe qprivs EtwDnstap | Set-Content "$base/privileges.txt"
            Get-DnsServerRecursion | Export-Clixml "$base/recursion-original.xml"
            Get-DnsServerForwarder | Export-Clixml "$base/forwarders-original.xml"
            if (Get-DnsServerZone -Name auth.stress.test -ErrorAction SilentlyContinue) { throw 'Authoritative zone already exists' }
            if (Get-DnsServerZone -Name rec.stress.test -ErrorAction SilentlyContinue) { throw 'Recursive zone already exists' }
            Add-DnsServerPrimaryZone -Name auth.stress.test -ZoneFile auth.stress.test.dns -LoadExisting -DynamicUpdate None
            Add-DnsServerConditionalForwarderZone -Name rec.stress.test -MasterServers 172.27.215.60 -UseRecursion:$false
            'auth.stress.test','rec.stress.test' | Set-Content "$base/owned-zones.txt"
        }
        if ($action -eq 'Deploy') {
            if (-not (Test-Path "$base/service-original.exe")) { throw 'Missing baseline backup' }
            Stop-Service EtwDnstap
            Copy-Item 'C:/Windows/Temp/etw-dnstap-perf.exe' 'C:/Program Files/EtwDnstap/etw-dnstap.exe' -Force
            Get-FileHash 'C:/Program Files/EtwDnstap/etw-dnstap.exe' | ConvertTo-Json | Set-Content "$base/instrumented-hash.json"
        }
        if ($action -eq 'Start') {
            if ((Get-Service EtwDnstap).Status -ne 'Stopped') { throw 'Service must be stopped before starting run' }
            $path = '"C:\Program Files\EtwDnstap\etw-dnstap.exe" --receiver 172.27.215.60:6000 --session-name EtwDnstap-Lab --metrics "C:\ProgramData\EtwDnstap\performance\' + $run + '"'
            $changed = Invoke-CimMethod -InputObject $service -MethodName Change -Arguments @{PathName=$path}
            if ($changed.ReturnValue -ne 0) { throw "Set measured service arguments failed: $($changed.ReturnValue)" }
            Start-Service EtwDnstap
        }
        if ($action -eq 'Stop') { Stop-Service EtwDnstap }
        if ($action -eq 'UpdateNames') {
            if (-not (Test-Path "$base/owned-zones.txt")) { throw 'Missing owned zone marker' }
            Remove-DnsServerResourceRecord -ZoneName auth.stress.test -Name wpad -RRType A -Force
            Add-DnsServerResourceRecordA -ZoneName auth.stress.test -Name bills -IPv4Address 192.0.2.53 -TimeToLive ([TimeSpan]::FromSeconds(300))
        }
        if ($action -eq 'Status') {
            $latest = Get-ChildItem -LiteralPath $base -Filter "$run.*.jsonl" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
            if ($latest) { Get-Content -LiteralPath $latest.FullName -Tail 1 }
            Get-DnsServerGlobalQueryBlockList | Select-Object Enable,List | ConvertTo-Json -Depth 4
            Get-WinEvent -FilterHashtable @{LogName='Application';ProviderName='EtwDnstap';StartTime=(Get-Date).AddMinutes(-15)} -ErrorAction SilentlyContinue | Select-Object TimeCreated,Id,Message | ConvertTo-Json -Depth 4
        }
        if ($action -eq 'Restore') {
            Stop-Service EtwDnstap -ErrorAction SilentlyContinue
            $original = Get-Content "$base/service-original.json" -Raw | ConvertFrom-Json
            $changed = Invoke-CimMethod -InputObject $service -MethodName Change -Arguments @{PathName=$original.PathName}
            if ($changed.ReturnValue -ne 0) { throw "Restore original service command failed: $($changed.ReturnValue)" }
            if ((Get-Content "$base/owned-zones.txt") -contains 'rec.stress.test') { Remove-DnsServerZone -Name rec.stress.test -Force }
            $before = Import-Clixml "$base/recursion-original.xml"
            $after = Get-DnsServerRecursion
            foreach ($property in 'AdditionalTimeout','Enable','RetryInterval','SecureResponse','Timeout') {
                if ($before.$property -ne $after.$property) { throw "DNS recursion setting changed: $property" }
            }
            $after | Select-Object AdditionalTimeout,Enable,RetryInterval,SecureResponse,Timeout | ConvertTo-Json | Set-Content "$base/recursion-final.json"
            Get-DnsServerForwarder | Export-Clixml "$base/forwarders-final.xml"
            Get-DnsServerGlobalQueryBlockList | Select-Object Enable,List | ConvertTo-Json | Set-Content "$base/blocklist-final.json"
            & sc.exe qsidtype EtwDnstap | Set-Content "$base/sidtype-final.txt"
            & sc.exe qprivs EtwDnstap | Set-Content "$base/privileges-final.txt"
            & wpr.exe -status 2>&1 | Set-Content "$base/wpr-final.txt"
            & logman.exe query | Set-Content "$base/logman-final.txt"
            $aCount = @(Get-DnsServerResourceRecord -ZoneName auth.stress.test -RRType A).Count
            if ($aCount -ne 5000) { throw "Expected 5000 retained authoritative A records, got $aCount" }
            @{AuthoritativeARecords=$aCount;RetainedBinary=(Get-FileHash 'C:/Program Files/EtwDnstap/etw-dnstap.exe').Hash;RestoredArguments=$original.PathName} | ConvertTo-Json | Set-Content "$base/restoration-final.json"
            if ($original.State -eq 'Running') { Start-Service EtwDnstap }
        }
        Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'" | Select-Object Name,State,ProcessId,PathName,StartName,ExitCode,ServiceSpecificExitCode
        Get-Date -Format o
        Get-FileHash 'C:/Program Files/EtwDnstap/etw-dnstap.exe'
    }
    $result | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $out "$RunId-$Action.json")
    if ($Action -in 'Fetch','Restore') {
        $files = Invoke-Command -Session $session -ScriptBlock { Get-ChildItem -LiteralPath 'C:/ProgramData/EtwDnstap/performance' -File | Where-Object Extension -ne '.exe' | Select-Object Name,Length,@{n='Sha256';e={(Get-FileHash -LiteralPath $_.FullName).Hash}} }
        foreach ($file in $files) {
            $encoded = Invoke-Command -Session $session -ArgumentList $file.Name -ScriptBlock { param($name) [Convert]::ToBase64String([IO.File]::ReadAllBytes("C:/ProgramData/EtwDnstap/performance/$name")) }
            $destination = Join-Path $out $file.Name
            [IO.File]::WriteAllBytes($destination,[Convert]::FromBase64String($encoded))
            if ((Get-FileHash -LiteralPath $destination).Hash -ne $file.Sha256) { throw "Copy hash mismatch: $($file.Name)" }
        }
    }
    $result | Format-List
} finally { Remove-PSSession $session }
