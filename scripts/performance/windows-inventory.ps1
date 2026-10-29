$ErrorActionPreference = 'Stop'
$vm = Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac') { throw 'Wrong VM identity' }
$credential = Import-Clixml -LiteralPath (Join-Path $PSScriptRoot '../../target/lab-windows/private/administrator.clixml')
Invoke-Command -VMId $vm.Id -Credential $credential -ScriptBlock {
    Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'" | Select-Object Name,State,ProcessId,PathName,StartName
    Get-Command wpr.exe,xperf.exe -ErrorAction SilentlyContinue | Select-Object Source
    Get-Counter -ListSet Process,Thread,DNS | Select-Object CounterSetName,Paths
    Get-NetIPAddress -AddressFamily IPv4 | Select-Object IPAddress,InterfaceAlias
} | ConvertTo-Json -Depth 5
