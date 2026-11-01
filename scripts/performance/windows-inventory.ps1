#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Prints a read-only inventory of the Windows lab VM (service, tracing tools, counters, IPv4).

.DESCRIPTION
Verifies the VM identity, then lists the EtwDnstap service, wpr.exe/xperf.exe locations, the
Process/Thread/DNS performance counter sets and the guest IPv4 addresses as JSON on stdout.
Changes nothing.

.OUTPUTS
JSON on the pipeline.
#>
[CmdletBinding()] param()
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'LabCommon.psm1') -Force
$config = Get-LabConfig
$vm = Get-LabVm
$session = Get-LabSession -Vm $vm
try {
    Invoke-LabGuest -Session $session -ArgumentList $config.windows_vm.service_name -ScriptBlock {
        param($serviceName)
        Get-CimInstance Win32_Service -Filter "Name='$serviceName'" | Select-Object Name,State,ProcessId,PathName,StartName
        Get-Command wpr.exe,xperf.exe -ErrorAction SilentlyContinue | Select-Object Source
        Get-Counter -ListSet Process,Thread,DNS | Select-Object CounterSetName,Paths
        Get-NetIPAddress -AddressFamily IPv4 | Select-Object IPAddress,InterfaceAlias
    } | ConvertTo-Json -Depth 5
} finally { Remove-PSSession $session }
