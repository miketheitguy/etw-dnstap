$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$vm = Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac') { throw 'Wrong VM identity' }
$credential = Import-Clixml -LiteralPath (Join-Path $root 'target/lab-windows/private/administrator.clixml')
$result = Invoke-Command -VMId $vm.Id -Credential $credential -ScriptBlock {
    $ErrorActionPreference = 'Stop'
    $service = Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"
    if ($service.State -ne 'Running' -or $service.PathName -match '--metrics' -or $service.StartName -ne 'LocalSystem') { throw 'Unexpected final service state' }
    $aCount = @(Get-DnsServerResourceRecord -ZoneName auth.stress.test -RRType A).Count
    if ($aCount -ne 5000) { throw 'Authoritative zone count changed' }
    if (@(Get-DnsServerZone | Where-Object ZoneName -eq rec.stress.test).Count -ne 0) { throw 'Temporary recursive zone still exists' }
    $answer = Resolve-DnsName -Name www.auth.stress.test -Server 127.0.0.1 -Type A -DnsOnly
    if (@($answer | Where-Object { $_.IPAddress -eq '192.0.2.53' -and $_.TTL -eq 300 }).Count -ne 1) { throw 'Final authoritative answer failed' }
    [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');service=$service | Select-Object Name,State,ProcessId,PathName,StartName,ExitCode;AuthoritativeARecords=$aCount;RecursiveForwarderAbsent=$true;Answer=$answer | Select-Object Name,Type,TTL,IPAddress;Binary=(Get-FileHash 'C:/Program Files/EtwDnstap/etw-dnstap.exe').Hash}
}
$result | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $root 'target/performance/windows/final-query-proof.json')
$result | Format-List
