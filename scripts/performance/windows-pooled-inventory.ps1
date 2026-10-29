$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$out = Join-Path $root 'target/performance/pooled-vm-20261029-0942/windows-campaign/inventory.json'
if (Test-Path -LiteralPath $out) { throw 'Inventory exists' }
$vm = Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac' -or $vm.State -ne 'Running' -or $vm.ProcessorCount -ne 2 -or $vm.MemoryAssigned -ne 4294967296) { throw 'Unexpected VM identity/resources/state' }
$cred = Import-Clixml -LiteralPath (Join-Path $root 'target/lab-windows/private/administrator.clixml')
$guest = Invoke-Command -VMId $vm.Id -Credential $cred -ScriptBlock {
    $svc = Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"
    $volume = Get-Volume -DriveLetter C
    if ($volume.SizeRemaining -lt 8GB) { throw 'Guest disk guard: less than 8 GiB free' }
    [ordered]@{
        utc=[DateTime]::UtcNow.ToString('o')
        service=$svc | Select-Object Name,State,ProcessId,PathName,StartName
        ip=@(Get-NetIPAddress -AddressFamily IPv4 | Select-Object IPAddress,InterfaceAlias)
        volume=$volume | Select-Object Size,SizeRemaining
        binary_sha256=(Get-FileHash 'C:/Program Files/EtwDnstap/etw-dnstap.exe').Hash
        executable_sddl=(Get-Acl 'C:/Program Files/EtwDnstap/etw-dnstap.exe').Sddl
        wpr_path=(Get-Command wpr.exe).Source
        wpr_status=(& wpr.exe -status 2>&1 | Out-String)
        wpr_profiles=(& wpr.exe -profiles 2>&1 | Out-String)
        sidtype=(& sc.exe qsidtype EtwDnstap | Out-String)
        privileges=(& sc.exe qprivs EtwDnstap | Out-String)
        scm_sddl=(& sc.exe sdshow EtwDnstap | Out-String)
        authoritative_records=@(Get-DnsServerResourceRecord -ZoneName auth.stress.test -RRType A).Count
        rec_zone=@(Get-DnsServerZone | Where-Object ZoneName -eq 'rec.stress.test').Count
        logman=(& logman.exe query | Out-String)
    }
}
$result = [ordered]@{vm=$vm | Select-Object Name,Id,State,ProcessorCount,MemoryAssigned;host_volumes=@(Get-PSDrive C,D | Select-Object Name,Free,Used);guest=$guest}
$result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $out -Encoding UTF8
Get-Content -LiteralPath $out -Raw
