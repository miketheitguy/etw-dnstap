[CmdletBinding()]
param([Parameter(Mandatory)][ValidateSet('Inventory','Prepare','Restore','Proof','Fetch')][string]$Action)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$out = Join-Path $root 'target/performance/windows/campaign-1000qps-20261028'
New-Item -ItemType Directory -Path $out -Force | Out-Null
$vm = Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac' -or $vm.State -ne 'Running') { throw 'Unexpected VM identity/state' }
if ($vm.ProcessorCount -ne 2 -or $vm.MemoryAssigned -ne 4294967296) { throw 'VM resources changed' }
$credential = Import-Clixml -LiteralPath (Join-Path $root 'target/lab-windows/private/administrator.clixml')
$session = New-PSSession -VMId $vm.Id -Credential $credential
try {
    $result = Invoke-Command -Session $session -ArgumentList $Action -ScriptBlock {
        param($action)
        $ErrorActionPreference = 'Stop'
        $base = 'C:/ProgramData/EtwDnstap/performance/campaign-1000qps-20261028'
        $service = Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"
        $hash = (Get-FileHash 'C:/Program Files/EtwDnstap/etw-dnstap.exe').Hash
        if ($hash -ne '0FCDD17E08A3DDD42F4F132F7F03F80A5513FEBC66E54F90A5EBBC8135BE2C1D') { throw 'Unexpected service binary' }
        $aCount = @(Get-DnsServerResourceRecord -ZoneName auth.stress.test -RRType A).Count
        if ($aCount -ne 5000) { throw 'Expected retained 5000-record authoritative zone' }
        if ($action -eq 'Prepare') {
            if (Test-Path -LiteralPath $base) { throw 'Campaign backup already exists; preserve and inspect it' }
            if (@(Get-DnsServerZone | Where-Object ZoneName -eq 'rec.stress.test').Count) { throw 'Recursive forwarder already exists; no ownership established' }
            if ($service.State -ne 'Running' -or $service.StartName -ne 'LocalSystem' -or $service.PathName -match '--metrics') { throw 'Unexpected pre-campaign service baseline' }
            New-Item -ItemType Directory -Path $base | Out-Null
            & icacls.exe $base /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
            if ($LASTEXITCODE -ne 0) { throw 'Protect campaign backup failed' }
            $service | Select-Object Name,PathName,StartName,StartMode,State | ConvertTo-Json | Set-Content "$base/service-original.json"
            Copy-Item 'C:/Program Files/EtwDnstap/etw-dnstap.exe' "$base/service-original.exe"
            $hash | Set-Content "$base/service-original-sha256.txt"
            Get-DnsServerRecursion | Export-Clixml "$base/recursion-original.xml"
            Get-DnsServerForwarder | Export-Clixml "$base/forwarders-original.xml"
            & sc.exe qsidtype EtwDnstap | Set-Content "$base/sidtype-original.txt"
            & sc.exe qprivs EtwDnstap | Set-Content "$base/privileges-original.txt"
            Add-DnsServerConditionalForwarderZone -Name rec.stress.test -MasterServers 172.27.215.60 -UseRecursion:$false
            'rec.stress.test' | Set-Content "$base/owned-rec-forwarder.txt"
        }
        if ($action -eq 'Restore') {
            if (-not (Test-Path "$base/service-original.json")) { throw 'Missing current campaign backup' }
            Stop-Service EtwDnstap -ErrorAction SilentlyContinue
            $original = Get-Content "$base/service-original.json" -Raw | ConvertFrom-Json
            $changed = Invoke-CimMethod -InputObject $service -MethodName Change -Arguments @{PathName=$original.PathName}
            if ($changed.ReturnValue -ne 0) { throw "Restore command line failed: $($changed.ReturnValue)" }
            if ((Get-Content "$base/owned-rec-forwarder.txt") -ne 'rec.stress.test') { throw 'Unexpected forwarder ownership marker' }
            Remove-DnsServerZone -Name rec.stress.test -Force
            $before = Import-Clixml "$base/recursion-original.xml"
            $after = Get-DnsServerRecursion
            foreach ($property in 'AdditionalTimeout','Enable','RetryInterval','SecureResponse','Timeout') {
                if ($before.$property -ne $after.$property) { throw "Recursion baseline changed: $property" }
            }
            $after | Select-Object AdditionalTimeout,Enable,RetryInterval,SecureResponse,Timeout | ConvertTo-Json | Set-Content "$base/recursion-final.json"
            Get-DnsServerForwarder | Export-Clixml "$base/forwarders-final.xml"
            & sc.exe qsidtype EtwDnstap | Set-Content "$base/sidtype-final.txt"
            & sc.exe qprivs EtwDnstap | Set-Content "$base/privileges-final.txt"
            & wpr.exe -status 2>&1 | Set-Content "$base/wpr-final.txt"
            & logman.exe query | Set-Content "$base/logman-final.txt"
            if ($original.State -eq 'Running') { Start-Service EtwDnstap }
        }
        if ($action -eq 'Proof') {
            if ($service.State -ne 'Running' -or $service.PathName -match '--metrics') { throw 'Service not restored' }
            if (@(Get-DnsServerZone | Where-Object ZoneName -eq 'rec.stress.test').Count) { throw 'Temporary recursive forwarder remains' }
            $answer = Resolve-DnsName -Name www.auth.stress.test -Server 127.0.0.1 -Type A -DnsOnly
            if (@($answer | Where-Object { $_.IPAddress -eq '192.0.2.53' -and $_.TTL -eq 300 }).Count -ne 1) { throw 'Final authoritative answer failed' }
            $answer | Select-Object Name,Type,TTL,IPAddress | ConvertTo-Json | Set-Content "$base/final-query-proof.json"
        }
        [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');action=$action;service=Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'" | Select-Object Name,State,ProcessId,PathName,StartName,ExitCode;binary_sha256=$hash;authoritative_a_records=$aCount;ipv4=@(Get-NetIPAddress -AddressFamily IPv4 | Select-Object -ExpandProperty IPAddress)}
    }
    $result | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $out "$Action.json")
    if ($Action -in 'Fetch','Restore','Proof') {
        $files = Invoke-Command -Session $session -ScriptBlock { Get-ChildItem -LiteralPath 'C:/ProgramData/EtwDnstap/performance/campaign-1000qps-20261028' -File | Where-Object Extension -ne '.exe' | Select-Object Name,@{n='Sha256';e={(Get-FileHash -LiteralPath $_.FullName).Hash}} }
        foreach ($file in $files) {
            $encoded = Invoke-Command -Session $session -ArgumentList $file.Name -ScriptBlock { param($name) [Convert]::ToBase64String([IO.File]::ReadAllBytes("C:/ProgramData/EtwDnstap/performance/campaign-1000qps-20261028/$name")) }
            $destination = Join-Path $out $file.Name
            [IO.File]::WriteAllBytes($destination,[Convert]::FromBase64String($encoded))
            if ((Get-FileHash -LiteralPath $destination).Hash -ne $file.Sha256) { throw 'Campaign copy hash mismatch' }
        }
    }
    $result | Format-List
} finally { Remove-PSSession $session }
