[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Inventory','Backup','Prepare','Deploy','Start','Stop','Status','Restore','Proof','Fetch')][string]$Action,
    [ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId='campaign',
    [ValidateSet(8192)][int]$Capacity=8192,
    [ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ExpectedHash='98FDAB43AA24AE1BE31A7AABCCBDD67A2F64C87DC1C50C428B638B3C091F5D7C',
    [string]$CandidatePath,
    [switch]$ReviewGateApproved,
    [switch]$UseBinaryDefault
)
$ErrorActionPreference = 'Stop'
if ($Action -in 'Backup','Prepare','Deploy','Start') { throw 'Historical service-metrics harness: new campaigns are disabled because the TCP-only service no longer supports JSONL metrics. Existing evidence and cleanup actions remain available.' }
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$out = Join-Path $root 'target/performance/pooled-vm-20261029-0942/windows-campaign'
New-Item -ItemType Directory -Path $out -Force | Out-Null
$vm = Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac' -or $vm.State -ne 'Running') { throw 'Unexpected VM identity/state' }
if ($vm.ProcessorCount -ne 2 -or $vm.MemoryAssigned -ne 4294967296) { throw 'VM resources changed' }; if ($Action -in 'Backup','Prepare','Deploy','Start' -and (Get-PSDrive C).Free -lt 20GB) { throw 'Host disk guard' }
$credential = Import-Clixml -LiteralPath (Join-Path $root 'target/lab-windows/private/administrator.clixml')
$session = New-PSSession -VMId $vm.Id -Credential $credential
try {
    if ($Action -eq 'Deploy') {
        if (-not $ReviewGateApproved -or -not $CandidatePath) { throw 'Candidate deployment requires reviewed gate and explicit path' }
        if ((Get-FileHash -LiteralPath $CandidatePath).Hash -ne $ExpectedHash) { throw 'Host candidate hash mismatch' }
        Copy-Item -LiteralPath $CandidatePath -Destination 'C:/ProgramData/EtwDnstap/performance/pooled-vm-20261029-0942/candidate-staged.exe' -ToSession $session
    }
    $result = Invoke-Command -Session $session -ArgumentList $Action,$RunId,$Capacity,$ExpectedHash,[bool]$UseBinaryDefault -ScriptBlock {
        param($action,$run,$capacity,$expected,$useDefault)
        $ErrorActionPreference = 'Stop'
        $base = 'C:/ProgramData/EtwDnstap/performance/pooled-vm-20261029-0942'; if ($action -in 'Backup','Prepare','Deploy','Start' -and (Get-Volume -DriveLetter C).SizeRemaining -lt 8GB) { throw 'Guest disk guard' }
        $installed = 'C:/Program Files/EtwDnstap/etw-dnstap.exe'
        $service = Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"
        $hash = (Get-FileHash -LiteralPath $installed).Hash
        if ($action -ne 'Deploy' -and $hash -ne $expected) { throw 'Installed service hash mismatch' }
        if ($service.StartName -ne 'LocalSystem') { throw 'Service identity changed' }
        # DNS management inventory is restricted to pre-load/proof actions.
        # Status/Stop/Fetch/Restore must neither perturb DNS nor block cleanup.
        $aCount = $null
        if ($action -in 'Inventory','Backup','Prepare','Deploy','Start','Proof') {
            $aCount = @(Get-DnsServerResourceRecord -ZoneName auth.stress.test -RRType A).Count
            if ($aCount -ne 5000) { throw 'Expected retained 5000-record authoritative zone' }
        }
        if ($action -eq 'Backup') {
            if (Test-Path -LiteralPath $base) { throw 'Campaign backup exists; preserve it' }
            if ($service.State -ne 'Running' -or $service.PathName -match '--metrics') { throw 'Unexpected baseline service state' }
            if (@(Get-DnsServerZone | Where-Object ZoneName -eq 'rec.stress.test').Count) { throw 'Temporary recursive zone unexpectedly present' }
            New-Item -ItemType Directory -Path $base | Out-Null
            & icacls.exe $base /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
            if ($LASTEXITCODE -ne 0) { throw 'Protect campaign backup failed' }
            $service | Select-Object Name,PathName,StartName,StartMode,State | ConvertTo-Json | Set-Content "$base/service-original.json"
            Copy-Item -LiteralPath $installed -Destination "$base/service-original.exe"
            if ((Get-FileHash "$base/service-original.exe").Hash -ne $expected) { throw 'Guest baseline backup mismatch' }
            $hash | Set-Content "$base/baseline-sha256.txt"
            (Get-Acl -LiteralPath $installed).Sddl | Set-Content "$base/executable-original-sddl.txt"
            Get-DnsServerRecursion | Export-Clixml "$base/recursion-original.xml"
            Get-DnsServerForwarder | Export-Clixml "$base/forwarders-original.xml"
            Get-DnsServerCache | Export-Clixml "$base/cache-original.xml"
            & sc.exe qsidtype EtwDnstap | Set-Content "$base/sidtype-original.txt"
            & sc.exe qprivs EtwDnstap | Set-Content "$base/privileges-original.txt"
        }
        if ($action -eq 'Prepare') {
            if (-not (Test-Path "$base/service-original.json")) { throw 'Missing current campaign backup' }
            if (@(Get-DnsServerZone | Where-Object ZoneName -eq 'rec.stress.test').Count) { throw 'Recursive zone already exists; refuse overwrite' }
            Add-DnsServerConditionalForwarderZone -Name rec.stress.test -MasterServers 172.27.215.60 -UseRecursion:$false
            'rec.stress.test' | Set-Content "$base/owned-rec-forwarder.txt"
        }
        if ($action -eq 'Deploy') {
            if (-not (Test-Path "$base/service-original.exe")) { throw 'Missing protected guest baseline' }
            if ((Get-FileHash "$base/candidate-staged.exe").Hash -ne $expected) { throw 'Staged candidate hash mismatch' }
            Stop-Service EtwDnstap -ErrorAction SilentlyContinue
            $beforeAcl = (Get-Acl -LiteralPath $installed).Sddl
            Copy-Item -LiteralPath "$base/candidate-staged.exe" -Destination $installed -Force
            $hash = (Get-FileHash -LiteralPath $installed).Hash
            if ($hash -ne $expected -or (Get-Acl -LiteralPath $installed).Sddl -ne $beforeAcl) { throw 'Deployment hash or executable ACL mismatch' }
            @{utc=[DateTime]::UtcNow.ToString('o');sha256=$hash;executable_sddl_unchanged=$true} | ConvertTo-Json | Set-Content "$base/$run-deployment.json"
        }
        if ($action -eq 'Start') {
            if ($service.State -ne 'Stopped') { throw 'Service must be stopped before a run' }
            if (@(Get-ChildItem -LiteralPath $base -Filter "$run.*.jsonl").Count) { throw 'Run metrics already exist; preserve prior run' }
            $path = '"C:\Program Files\EtwDnstap\etw-dnstap.exe" --receiver 172.27.215.60:6000 --session-name EtwDnstap-Lab --metrics "C:\ProgramData\EtwDnstap\performance\pooled-vm-20261029-0942\' + $run + '"'
            $changed = Invoke-CimMethod -InputObject $service -MethodName Change -Arguments @{PathName=$path}
            if ($changed.ReturnValue -ne 0) { throw 'Set measured service arguments failed' }
            Start-Service EtwDnstap
        }
        if ($action -eq 'Stop') { Stop-Service EtwDnstap; $svcAfter=Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"; if ($svcAfter.State -ne 'Stopped') { throw 'SCM stop did not finish' } }
        $lastMetrics = $null
        if ($action -eq 'Status') {
            $file = Get-ChildItem -LiteralPath $base -Filter "$run.*.jsonl" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
            if ($file) {
                $complete = @(Get-Content -LiteralPath $file.FullName -Tail 2 | ForEach-Object { try { $_ | ConvertFrom-Json -ErrorAction Stop } catch { } })
                $lastMetrics = $complete | Select-Object -Last 1
            }
        }
        if ($action -eq 'Restore') {
            Stop-Service EtwDnstap -ErrorAction SilentlyContinue
            $original = Get-Content "$base/service-original.json" -Raw | ConvertFrom-Json
            if ($original.PathName -match '--metrics|--capacity') { throw 'Unexpected original argument shape; inspect before restoration' }
            $normalPath = $original.PathName
            $changed = Invoke-CimMethod -InputObject $service -MethodName Change -Arguments @{PathName=$normalPath}
            if ($changed.ReturnValue -ne 0) { throw 'Restore normal arguments failed' }
            if ((Get-Content "$base/owned-rec-forwarder.txt") -ne 'rec.stress.test') { throw 'Missing forwarder ownership marker' }
            Remove-DnsServerZone -Name rec.stress.test -Force
            $before = Import-Clixml "$base/recursion-original.xml"
            $after = Get-DnsServerRecursion
            foreach ($property in 'AdditionalTimeout','Enable','RetryInterval','SecureResponse','Timeout') {
                if ($before.$property -ne $after.$property) { throw "Recursion baseline changed: $property" }
            }
            Get-DnsServerCache | Export-Clixml "$base/cache-final.xml"
            Get-DnsServerForwarder | Export-Clixml "$base/forwarders-final.xml"
            & sc.exe qsidtype EtwDnstap | Set-Content "$base/sidtype-final.txt"
            & sc.exe qprivs EtwDnstap | Set-Content "$base/privileges-final.txt"
            & wpr.exe -status 2>&1 | Set-Content "$base/wpr-final.txt"
            & logman.exe query | Set-Content "$base/logman-final.txt"
            @{utc=[DateTime]::UtcNow.ToString('o');sha256=$hash;capacity=$capacity;uses_binary_default=$true;normal_arguments=$normalPath;authoritative_a_records=$aCount} | ConvertTo-Json | Set-Content "$base/restoration-final.json"
            if ($original.State -eq 'Running') { Start-Service EtwDnstap }
        }
        if ($action -eq 'Proof') {
            if ($service.State -ne 'Running' -or $service.PathName -match '--metrics') { throw 'Service not restored' }
            if (@(Get-DnsServerZone | Where-Object ZoneName -eq 'rec.stress.test').Count) { throw 'Temporary recursive forwarder remains' }
            $answer = Resolve-DnsName -Name www.auth.stress.test -Server 127.0.0.1 -Type A -DnsOnly
            if (@($answer | Where-Object { $_.IPAddress -eq '192.0.2.53' -and $_.TTL -eq 300 }).Count -ne 1) { throw 'Final authoritative answer failed' }
            $answer | Select-Object Name,Type,TTL,IPAddress | ConvertTo-Json | Set-Content "$base/final-query-proof.json"
        }
        [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');action=$action;run_id=$run;requested_capacity=$capacity;binary_sha256=$hash;authoritative_a_records=$aCount;last_metrics=$lastMetrics;service=Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'" | Select-Object Name,State,ProcessId,PathName,StartName,ExitCode}
    }
    $suffix = if ($Action -eq 'Status') { '-' + [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() } else { '' }
    $result | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $out "$RunId-$Action$suffix.json")
    if ($Action -in 'Fetch','Restore','Proof') {
        $files = Invoke-Command -Session $session -ScriptBlock { Get-ChildItem -LiteralPath 'C:/ProgramData/EtwDnstap/performance/pooled-vm-20261029-0942' -File | Where-Object Extension -ne '.exe' | Select-Object Name,@{n='Sha256';e={(Get-FileHash -LiteralPath $_.FullName).Hash}} }
        foreach ($file in $files) {
            $destination = Join-Path $out $file.Name
            if (Test-Path -LiteralPath $destination) {
                if ((Get-FileHash -LiteralPath $destination).Hash -eq $file.Sha256) { continue }
                throw "Preserve existing mismatched campaign evidence: $($file.Name)"
            }
            $encoded = Invoke-Command -Session $session -ArgumentList $file.Name -ScriptBlock { param($name) [Convert]::ToBase64String([IO.File]::ReadAllBytes("C:/ProgramData/EtwDnstap/performance/pooled-vm-20261029-0942/$name")) }
            [IO.File]::WriteAllBytes($destination,[Convert]::FromBase64String($encoded))
            if ((Get-FileHash -LiteralPath $destination).Hash -ne $file.Sha256) { throw 'Campaign copy hash mismatch' }
        }
    }
    $result | Format-List
} finally { Remove-PSSession $session }


