#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Parser-ramp Windows VM controller: Inventory, Backup, Deploy, Prepare, Start, Stop, Status, Restore, Fetch.

.DESCRIPTION
Protects a baseline of the service and DNS configuration, deploys a hash-verified candidate
binary, prepares the temporary recursive forwarder (ownership recorded in owned-zones.txt),
starts/stops the service and restores the baseline. Backup refuses to reuse an existing
guest campaign directory or host evidence directory. Files are copied back with SHA-256
verification.

.PARAMETER Action
Lifecycle step.
.PARAMETER CampaignId
Campaign directory name; guest <data_dir>/<CampaignId>, host target/performance/<CampaignId>/windows.
.PARAMETER RunId
Run label for receipt names.
.PARAMETER ExpectedHash
SHA-256 of the candidate service binary.
.PARAMETER CandidatePath
Host path of the candidate binary (Deploy).
.PARAMETER UpstreamAddress
IPv4 address of the lab upstream (Prepare); defaults to nothing, it must be explicit.

.OUTPUTS
Timestamped JSON receipts and fetched evidence under target/performance/<CampaignId>/windows.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Inventory','Backup','Deploy','Prepare','Start','Stop','Status','Restore','Fetch')][string]$Action,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$CampaignId,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ExpectedHash,
    [ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId = 'campaign',
    [string]$CandidatePath,
    [string]$UpstreamAddress
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'LabCommon.psm1') -Force
$config = Get-LabConfig
$out = Resolve-LabPath "$($config.paths.evidence_root)/$CampaignId/windows"
if ($Action -eq 'Backup' -and (Test-Path -LiteralPath $out) -and @(Get-ChildItem -LiteralPath $out -Force).Count) { throw "Campaign evidence already exists; refusing to reuse CampaignId '$CampaignId': $out" }
New-Item -ItemType Directory -Path $out -Force | Out-Null
$vm = Get-LabVm -RequireRunning -CheckResources -RequireFixedMemory
if ($Action -notin 'Stop','Restore','Fetch','Status' -and (Get-PSDrive C).Free -lt 20GB) { throw 'Host free-space guard' }
$guestBase = "$($config.windows_vm.data_dir)/$CampaignId"
$session = Get-LabSession -Vm $vm
try {
    if ($Action -eq 'Prepare') {
        $ip = $null
        if (-not [Net.IPAddress]::TryParse($UpstreamAddress, [ref]$ip) -or $ip.AddressFamily -ne 'InterNetwork') { throw 'Explicit upstream IPv4 required' }
        Invoke-LabGuest -Session $session -ArgumentList $guestBase -ScriptBlock { param($base) if (-not (Test-Path "$base/service-original.json")) { throw 'Missing protected baseline' } }
        Initialize-LabDnsZones -Session $session -BaseDirectory $guestBase -UpstreamAddress $UpstreamAddress
    }
    if ($Action -eq 'Deploy') {
        if (-not $CandidatePath -or (Get-FileHash -LiteralPath $CandidatePath).Hash -ne $ExpectedHash) { throw 'Verified candidate required' }
        Copy-Item -LiteralPath $CandidatePath -Destination "$guestBase/candidate.exe" -ToSession $session
    }
    $guestArguments = @($Action,$RunId,$ExpectedHash,$CampaignId,$guestBase,$config.windows_vm.install_exe,$config.windows_vm.service_name,$config.dns.auth_zone,$config.dns.rec_zone)
    $result=Invoke-LabGuest -Session $session -ArgumentList $guestArguments -ScriptBlock {
        param($action,$run,$expected,$campaignId,$base,$installed,$serviceName,$authZone,$recZone)
        $svc=Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
        if ($svc.StartName -ne 'LocalSystem' -or $svc.PathName -match '--metrics|--output|--foreground') { throw 'Unexpected SCM identity/arguments' }
        if ($action -notin 'Stop','Restore','Fetch','Status' -and (Get-Volume C).SizeRemaining -lt 8GB) { throw 'Guest disk guard' }
        if ($action -eq 'Backup') {
            if (Test-Path -LiteralPath $base) { throw 'Preserve existing campaign backup' }
            if ($svc.State -ne 'Running') { throw 'Expected baseline Running' }
            if (@(Get-DnsServerZone | Where-Object ZoneName -eq $recZone).Count) { throw 'Recursive zone already exists' }
            New-Item -ItemType Directory -Path $base | Out-Null
            & icacls.exe $base /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
            if ($LASTEXITCODE) { throw 'Protect backup failed' }
            Copy-Item -LiteralPath $installed -Destination "$base/baseline.exe"
            $svc | Select-Object Name,PathName,StartName,StartMode,State | ConvertTo-Json | Set-Content "$base/service-original.json" -Encoding UTF8
            Get-Acl -LiteralPath $installed | Export-Clixml "$base/executable-original-acl.xml"
            foreach ($setting in 'Recursion','Cache','Forwarder') { & "Get-DnsServer$setting" | Export-Clixml "$base/dns-$setting.xml" }
            & reg.exe export "HKLM\SYSTEM\CurrentControlSet\Services\$serviceName" "$base/service-original.reg" /y | Out-Null
            if ($LASTEXITCODE) { throw 'Export service configuration failed' }
            [ordered]@{baseline_sha256=(Get-FileHash "$base/baseline.exe").Hash;executable_sddl=(Get-Acl $installed).Sddl;directory_sddl=(Get-Acl (Split-Path $installed)).Sddl;scm_sddl=(& sc.exe sdshow $serviceName | Out-String);sidtype=(& sc.exe qsidtype $serviceName | Out-String);privileges=(& sc.exe qprivs $serviceName | Out-String)} | ConvertTo-Json | Set-Content "$base/policy-original.json" -Encoding UTF8
        }
        if ($action -in 'Deploy','Prepare','Restore') { if (-not (Test-Path "$base/service-original.json")) { throw 'Missing protected baseline' } }
        if ($action -eq 'Deploy') {
            if ((Get-FileHash "$base/candidate.exe").Hash -ne $expected) { throw 'Guest candidate hash mismatch' }
            Stop-Service $serviceName
            $acl=Get-Acl $installed
            Copy-Item -LiteralPath "$base/candidate.exe" -Destination $installed -Force
            if ((Get-FileHash $installed).Hash -ne $expected -or (Get-Acl $installed).Sddl -ne $acl.Sddl) { throw 'Deployment hash/ACL mismatch' }
        }
        if ($action -eq 'Start') {
            if ($svc.State -ne 'Stopped' -or (Get-FileHash $installed).Hash -ne $expected) { throw 'Start state/hash mismatch' }
            Start-Service $serviceName
        }
        if ($action -eq 'Stop') { Stop-Service $serviceName; if ((Get-Service $serviceName).Status -ne 'Stopped') { throw 'SCM stop failed' } }
        if ($action -eq 'Restore') {
            Stop-Service $serviceName -ErrorAction SilentlyContinue
            $original=Get-Content "$base/service-original.json" -Raw | ConvertFrom-Json
            if ($svc.PathName -ne $original.PathName) {
                $change=Invoke-CimMethod -InputObject $svc -MethodName Change -Arguments @{PathName=$original.PathName}
                if ($change.ReturnValue) { throw 'Restore SCM arguments failed' }
            }
            $null = Remove-LabOwnedZone -Base $base -Zone $recZone -SkipIfNotOwned
            $policy=Get-Content "$base/policy-original.json" -Raw | ConvertFrom-Json
            if ((Get-Acl $installed).Sddl -ne $policy.executable_sddl -or (& sc.exe sdshow $serviceName | Out-String) -ne $policy.scm_sddl -or (& sc.exe qsidtype $serviceName | Out-String) -ne $policy.sidtype -or (& sc.exe qprivs $serviceName | Out-String) -ne $policy.privileges) { throw 'Service policy changed' }
            if ((Get-FileHash $installed).Hash -ne $expected) { throw 'Retained candidate mismatch' }
            foreach ($item in @(
                @{name='Recursion';properties=@('AdditionalTimeout','Enable','RetryInterval','SecureResponse','Timeout')},
                @{name='Cache';properties=@('LockingPercent','EnablePollutionProtection','IgnorePolicies','MaxKBSize','MaxNegativeTtl','MaxTtl','StoreEmptyAuthenticationResponse')},
                @{name='Forwarder';properties=@('IPAddress','ReorderingEnable','Timeout','UseRootHint')}
            )) {
                $before=Import-Clixml "$base/dns-$($item.name).xml"
                $after=& "Get-DnsServer$($item.name)"
                foreach ($property in $item.properties) {
                    $left=@($before.$property | ForEach-Object ToString) -join ','
                    $right=@($after.$property | ForEach-Object ToString) -join ','
                    if ($left -ne $right) { throw "DNS baseline changed: $($item.name).$property" }
                }
            }
            if ($original.State -eq 'Running') { Start-Service $serviceName }
        }
        $inventory=$action -in 'Inventory','Backup','Prepare','Restore'
        [ordered]@{
            utc=[DateTime]::UtcNow.ToString('o');action=$action;run_id=$run;campaign_id=$campaignId
            service=Get-CimInstance Win32_Service -Filter "Name='$serviceName'" | Select-Object Name,State,ProcessId,PathName,StartName,ExitCode
            binary_sha256=(Get-FileHash $installed).Hash;free_bytes=(Get-Volume C).SizeRemaining
            available_memory_kib=(Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory
            ip=@(Get-NetIPAddress -AddressFamily IPv4 | Select-Object IPAddress,InterfaceAlias)
            authoritative_records=$(if ($inventory) {@(Get-DnsServerResourceRecord -ZoneName $authZone -RRType A).Count})
            recursive_zone_count=$(if ($inventory) {@(Get-DnsServerZone | Where-Object ZoneName -eq $recZone).Count})
            scm_sddl=(& sc.exe sdshow $serviceName | Out-String);sidtype=(& sc.exe qsidtype $serviceName | Out-String);privileges=(& sc.exe qprivs $serviceName | Out-String)
        }
    }
    $receipt=[ordered]@{vm=$vm | Select-Object Name,Id,State,ProcessorCount,MemoryAssigned,DynamicMemoryEnabled;guest=$result}
    $path=Join-Path $out "$RunId-$Action-$([DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')).json"
    Write-LabJson -Path $path -InputObject $receipt -Depth 8
    if ($Action -in 'Backup','Fetch','Restore') {
        $null = Receive-LabDirectory -Session $session -GuestDirectory $guestBase -Destination $out -ExcludeExtension '.exe' -Overwrite
    }
    $receipt | ConvertTo-Json -Depth 8
} finally { Remove-PSSession $session }
