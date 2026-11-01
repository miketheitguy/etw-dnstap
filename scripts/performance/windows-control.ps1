#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Early service-metrics control harness for the Windows VM (Stop, Fetch, Restore, Status remain usable).

.DESCRIPTION
Prepare, Deploy, Start and UpdateNames are disabled because the TCP-only service no longer
supports JSONL metrics. Restore returns the service, DNS recursion/forwarders and zones to
the saved baseline and removes the temporary recursive forwarder only when it is recorded in
owned-zones.txt. Fetch/Restore copy the guest evidence back with SHA-256 verification.

.PARAMETER Action
Prepare, Deploy, Start, Stop, Fetch, Restore, Status or UpdateNames (the first, second, third and last are disabled).
.PARAMETER RunId
Run label used in receipt and metrics names (default 'setup').

.OUTPUTS
target/performance/windows/<RunId>-<Action>.json and fetched guest evidence files.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Prepare','Deploy','Start','Stop','Fetch','Restore','Status','UpdateNames')][string]$Action,
    [ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId = 'setup'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'LabCommon.psm1') -Force
if ($Action -in 'Prepare','Deploy','Start','UpdateNames') { throw 'Historical service-metrics harness: new campaigns are disabled because the TCP-only service no longer supports JSONL metrics. Existing evidence and cleanup actions remain available.' }
$config = Get-LabConfig
$root = Get-LabRoot
$out = Resolve-LabPath "$($config.paths.evidence_root)/windows"
New-Item -ItemType Directory -Path $out -Force | Out-Null
$vm = Get-LabVm
$session = Get-LabSession -Vm $vm
$guestBase = $config.windows_vm.data_dir
try {
    if ($Action -eq 'Prepare') {
        Copy-Item -LiteralPath (Resolve-LabPath "$($config.paths.workload_dir)/$($config.dns.auth_zone).dns") -Destination "C:/Windows/System32/dns/$($config.dns.auth_zone).dns" -ToSession $session
    }
    if ($Action -eq 'Deploy') {
        Copy-Item -LiteralPath (Join-Path $root 'target/x86_64-pc-windows-msvc/release/etw-dnstap.exe') -Destination 'C:/Windows/Temp/etw-dnstap-perf.exe' -ToSession $session
    }
    if ($Action -eq 'Prepare') {
        Invoke-LabGuest -Session $session -ArgumentList $guestBase, $config.windows_vm.install_exe, $config.windows_vm.service_name -ScriptBlock {
            param($base,$installed,$serviceName)
            if (Test-Path -LiteralPath $base) { throw 'Performance directory already exists; inspect prior run first' }
            New-Item -ItemType Directory -Path $base | Out-Null
            & icacls.exe $base /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
            if ($LASTEXITCODE -ne 0) { throw 'Protect performance directory failed' }
            Get-CimInstance Win32_Service -Filter "Name='$serviceName'" | Select-Object Name,PathName,StartName,StartMode,State | ConvertTo-Json | Set-Content "$base/service-original.json" -Encoding UTF8
            Copy-Item $installed "$base/service-original.exe"
            Get-FileHash "$base/service-original.exe" | ConvertTo-Json | Set-Content "$base/service-original-hash.json" -Encoding UTF8
            & sc.exe qsidtype $serviceName | Set-Content "$base/sidtype.txt" -Encoding UTF8
            & sc.exe qprivs $serviceName | Set-Content "$base/privileges.txt" -Encoding UTF8
            Get-DnsServerRecursion | Export-Clixml "$base/recursion-original.xml"
            Get-DnsServerForwarder | Export-Clixml "$base/forwarders-original.xml"
        }
        Initialize-LabDnsZones -Session $session -BaseDirectory $guestBase -UpstreamAddress $config.network.receiver_ip -AuthoritativeZoneFile "$($config.dns.auth_zone).dns"
    }
    $startCommandLine = Get-LabServiceCommandLine -ExtraArguments @('--metrics', (($guestBase -replace '/', '\') + '\' + $RunId))
    $guestArguments = @($Action, $RunId, $guestBase, $config.windows_vm.install_exe, $config.windows_vm.service_name, $config.dns.auth_zone, $config.dns.rec_zone,
        [int]$config.dns.auth_record_count, $startCommandLine)
    $result = Invoke-LabGuest -Session $session -ArgumentList $guestArguments -ScriptBlock {
        param($action,$run,$base,$installed,$serviceName,$authZone,$recZone,$authCount,$startCommandLine)
        $service = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
        if ($action -eq 'Deploy') {
            if (-not (Test-Path "$base/service-original.exe")) { throw 'Missing baseline backup' }
            Stop-Service $serviceName
            Copy-Item 'C:/Windows/Temp/etw-dnstap-perf.exe' $installed -Force
            Get-FileHash $installed | ConvertTo-Json | Set-Content "$base/instrumented-hash.json" -Encoding UTF8
        }
        if ($action -eq 'Start') {
            if ((Get-Service $serviceName).Status -ne 'Stopped') { throw 'Service must be stopped before starting run' }
            $changed = Invoke-CimMethod -InputObject $service -MethodName Change -Arguments @{PathName=$startCommandLine}
            if ($changed.ReturnValue -ne 0) { throw "Set measured service arguments failed: $($changed.ReturnValue)" }
            Start-Service $serviceName
        }
        if ($action -eq 'Stop') { Stop-Service $serviceName }
        if ($action -eq 'UpdateNames') {
            if (@(Get-LabOwnedZones -Base $base) -notcontains $authZone) { throw 'Missing owned zone marker' }
            Remove-DnsServerResourceRecord -ZoneName $authZone -Name wpad -RRType A -Force
            Add-DnsServerResourceRecordA -ZoneName $authZone -Name bills -IPv4Address 192.0.2.53 -TimeToLive ([TimeSpan]::FromSeconds(300))
        }
        if ($action -eq 'Status') {
            $latest = Get-ChildItem -LiteralPath $base -Filter "$run.*.jsonl" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
            if ($latest) { Get-Content -LiteralPath $latest.FullName -Tail 1 }
            Get-DnsServerGlobalQueryBlockList | Select-Object Enable,List | ConvertTo-Json -Depth 4
            Get-WinEvent -FilterHashtable @{LogName='Application';ProviderName=$serviceName;StartTime=(Get-Date).AddMinutes(-15)} -ErrorAction SilentlyContinue | Select-Object TimeCreated,Id,Message | ConvertTo-Json -Depth 4
        }
        if ($action -eq 'Restore') {
            Stop-Service $serviceName -ErrorAction SilentlyContinue
            $original = Get-Content "$base/service-original.json" -Raw | ConvertFrom-Json
            $changed = Invoke-CimMethod -InputObject $service -MethodName Change -Arguments @{PathName=$original.PathName}
            if ($changed.ReturnValue -ne 0) { throw "Restore original service command failed: $($changed.ReturnValue)" }
            $null = Remove-LabOwnedZone -Base $base -Zone $recZone -SkipIfNotOwned
            $before = Import-Clixml "$base/recursion-original.xml"
            $after = Get-DnsServerRecursion
            foreach ($property in 'AdditionalTimeout','Enable','RetryInterval','SecureResponse','Timeout') {
                if ($before.$property -ne $after.$property) { throw "DNS recursion setting changed: $property" }
            }
            $after | Select-Object AdditionalTimeout,Enable,RetryInterval,SecureResponse,Timeout | ConvertTo-Json | Set-Content "$base/recursion-final.json" -Encoding UTF8
            Get-DnsServerForwarder | Export-Clixml "$base/forwarders-final.xml"
            Get-DnsServerGlobalQueryBlockList | Select-Object Enable,List | ConvertTo-Json | Set-Content "$base/blocklist-final.json" -Encoding UTF8
            & sc.exe qsidtype $serviceName | Set-Content "$base/sidtype-final.txt" -Encoding UTF8
            & sc.exe qprivs $serviceName | Set-Content "$base/privileges-final.txt" -Encoding UTF8
            & wpr.exe -status 2>&1 | Set-Content "$base/wpr-final.txt" -Encoding UTF8
            & logman.exe query | Set-Content "$base/logman-final.txt" -Encoding UTF8
            $aCount = @(Get-DnsServerResourceRecord -ZoneName $authZone -RRType A).Count
            if ($aCount -ne $authCount) { throw "Expected $authCount retained authoritative A records, got $aCount" }
            @{AuthoritativeARecords=$aCount;RetainedBinary=(Get-FileHash $installed).Hash;RestoredArguments=$original.PathName} | ConvertTo-Json | Set-Content "$base/restoration-final.json" -Encoding UTF8
            if ($original.State -eq 'Running') { Start-Service $serviceName }
        }
        Get-CimInstance Win32_Service -Filter "Name='$serviceName'" | Select-Object Name,State,ProcessId,PathName,StartName,ExitCode,ServiceSpecificExitCode
        Get-Date -Format o
        Get-FileHash $installed
    }
    Write-LabJson -Path (Join-Path $out "$RunId-$Action.json") -InputObject $result -Depth 5 -Force
    if ($Action -in 'Fetch','Restore') {
        $null = Receive-LabDirectory -Session $session -GuestDirectory $guestBase -Destination $out -ExcludeExtension '.exe' -Overwrite
    }
    $result | Format-List
} finally { Remove-PSSession $session }
