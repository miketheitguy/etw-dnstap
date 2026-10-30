[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Inventory','Backup','Deploy','Prepare','Start','Stop','Status','Restore','Fetch')][string]$Action,
    [ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId='campaign',
    [ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$CampaignId='parser-ramp-20261030',
    [ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ExpectedHash='487FFD782B730B7E1A849626AF68BCB6C8A0EF2057ACB7D8E5D932B7EA49A1AE',
    [string]$CandidatePath,
    [string]$UpstreamAddress
)
$ErrorActionPreference='Stop'
$root=(Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$out=Join-Path $root "target/performance/$CampaignId/windows"
New-Item -ItemType Directory -Path $out -Force | Out-Null
$vm=Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac' -or $vm.State -ne 'Running' -or $vm.ProcessorCount -ne 2 -or $vm.MemoryAssigned -ne 4GB -or $vm.DynamicMemoryEnabled) { throw 'VM identity/resources/state mismatch' }
if ($Action -notin 'Stop','Restore','Fetch','Status' -and (Get-PSDrive C).Free -lt 20GB) { throw 'Host free-space guard' }
$credential=Import-Clixml (Join-Path $root 'target/lab-windows/private/administrator.clixml')
$session=New-PSSession -VMId $vm.Id -Credential $credential
try {
    if ($Action -eq 'Deploy') {
        if (-not $CandidatePath -or (Get-FileHash -LiteralPath $CandidatePath).Hash -ne $ExpectedHash) { throw 'Verified candidate required' }
        Copy-Item -LiteralPath $CandidatePath -Destination "C:/ProgramData/EtwDnstap/performance/$CampaignId/candidate.exe" -ToSession $session
    }
    $result=Invoke-Command -Session $session -ArgumentList $Action,$RunId,$ExpectedHash,$UpstreamAddress,$CampaignId -ScriptBlock {
        param($action,$run,$expected,$upstream,$campaignId)
        $ErrorActionPreference='Stop'
        $base="C:/ProgramData/EtwDnstap/performance/$campaignId"
        $installed='C:/Program Files/EtwDnstap/etw-dnstap.exe'
        $svc=Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'"
        if ($svc.StartName -ne 'LocalSystem' -or $svc.PathName -match '--metrics|--output|--foreground') { throw 'Unexpected SCM identity/arguments' }
        if ($action -notin 'Stop','Restore','Fetch','Status' -and (Get-Volume C).SizeRemaining -lt 8GB) { throw 'Guest disk guard' }
        if ($action -eq 'Backup') {
            if (Test-Path -LiteralPath $base) { throw 'Preserve existing campaign backup' }
            if ($svc.State -ne 'Running') { throw 'Expected baseline Running' }
            if (@(Get-DnsServerZone | Where-Object ZoneName -eq 'rec.stress.test').Count) { throw 'Recursive zone already exists' }
            New-Item -ItemType Directory -Path $base | Out-Null
            & icacls.exe $base /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
            if ($LASTEXITCODE) { throw 'Protect backup failed' }
            Copy-Item -LiteralPath $installed -Destination "$base/baseline.exe"
            $svc | Select-Object Name,PathName,StartName,StartMode,State | ConvertTo-Json | Set-Content "$base/service-original.json"
            Get-Acl -LiteralPath $installed | Export-Clixml "$base/executable-original-acl.xml"
            foreach ($setting in 'Recursion','Cache','Forwarder') { & "Get-DnsServer$setting" | Export-Clixml "$base/dns-$setting.xml" }
            & reg.exe export 'HKLM\SYSTEM\CurrentControlSet\Services\EtwDnstap' "$base/service-original.reg" /y | Out-Null
            if ($LASTEXITCODE) { throw 'Export service configuration failed' }
            [ordered]@{baseline_sha256=(Get-FileHash "$base/baseline.exe").Hash;executable_sddl=(Get-Acl $installed).Sddl;directory_sddl=(Get-Acl (Split-Path $installed)).Sddl;scm_sddl=(& sc.exe sdshow EtwDnstap | Out-String);sidtype=(& sc.exe qsidtype EtwDnstap | Out-String);privileges=(& sc.exe qprivs EtwDnstap | Out-String)} | ConvertTo-Json | Set-Content "$base/policy-original.json"
        }
        if ($action -in 'Deploy','Prepare','Restore') { if (-not (Test-Path "$base/service-original.json")) { throw 'Missing protected baseline' } }
        if ($action -eq 'Deploy') {
            if ((Get-FileHash "$base/candidate.exe").Hash -ne $expected) { throw 'Guest candidate hash mismatch' }
            Stop-Service EtwDnstap
            $acl=Get-Acl $installed
            Copy-Item -LiteralPath "$base/candidate.exe" -Destination $installed -Force
            if ((Get-FileHash $installed).Hash -ne $expected -or (Get-Acl $installed).Sddl -ne $acl.Sddl) { throw 'Deployment hash/ACL mismatch' }
        }
        if ($action -eq 'Prepare') {
            $ip=$null
            if (-not [Net.IPAddress]::TryParse($upstream,[ref]$ip) -or $ip.AddressFamily -ne 'InterNetwork') { throw 'Explicit upstream IPv4 required' }
            if (@(Get-DnsServerZone | Where-Object ZoneName -eq 'rec.stress.test').Count) { throw 'Refuse existing recursive zone' }
            Add-DnsServerConditionalForwarderZone -Name rec.stress.test -MasterServers $upstream -UseRecursion:$false
            'rec.stress.test' | Set-Content "$base/owned-forwarder.txt"
        }
        if ($action -eq 'Start') {
            if ($svc.State -ne 'Stopped' -or (Get-FileHash $installed).Hash -ne $expected) { throw 'Start state/hash mismatch' }
            Start-Service EtwDnstap
        }
        if ($action -eq 'Stop') { Stop-Service EtwDnstap; if ((Get-Service EtwDnstap).Status -ne 'Stopped') { throw 'SCM stop failed' } }
        if ($action -eq 'Restore') {
            Stop-Service EtwDnstap -ErrorAction SilentlyContinue
            $original=Get-Content "$base/service-original.json" -Raw | ConvertFrom-Json
            if ($svc.PathName -ne $original.PathName) {
                $change=Invoke-CimMethod -InputObject $svc -MethodName Change -Arguments @{PathName=$original.PathName}
                if ($change.ReturnValue) { throw 'Restore SCM arguments failed' }
            }
            if (Test-Path "$base/owned-forwarder.txt") {
                if ((Get-Content "$base/owned-forwarder.txt") -ne 'rec.stress.test') { throw 'Ownership marker mismatch' }
                Remove-DnsServerZone -Name rec.stress.test -Force
                Move-Item "$base/owned-forwarder.txt" "$base/removed-forwarder.txt"
            }
            $policy=Get-Content "$base/policy-original.json" -Raw | ConvertFrom-Json
            if ((Get-Acl $installed).Sddl -ne $policy.executable_sddl -or (& sc.exe sdshow EtwDnstap | Out-String) -ne $policy.scm_sddl -or (& sc.exe qsidtype EtwDnstap | Out-String) -ne $policy.sidtype -or (& sc.exe qprivs EtwDnstap | Out-String) -ne $policy.privileges) { throw 'Service policy changed' }
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
            if ($original.State -eq 'Running') { Start-Service EtwDnstap }
        }
        $inventory=$action -in 'Inventory','Backup','Prepare','Restore'
        [ordered]@{
            utc=[DateTime]::UtcNow.ToString('o');action=$action;run_id=$run;campaign_id=$campaignId
            service=Get-CimInstance Win32_Service -Filter "Name='EtwDnstap'" | Select-Object Name,State,ProcessId,PathName,StartName,ExitCode
            binary_sha256=(Get-FileHash $installed).Hash;free_bytes=(Get-Volume C).SizeRemaining
            available_memory_kib=(Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory
            ip=@(Get-NetIPAddress -AddressFamily IPv4 | Select-Object IPAddress,InterfaceAlias)
            authoritative_records=$(if ($inventory) {@(Get-DnsServerResourceRecord -ZoneName auth.stress.test -RRType A).Count})
            recursive_zone_count=$(if ($inventory) {@(Get-DnsServerZone | Where-Object ZoneName -eq 'rec.stress.test').Count})
            scm_sddl=(& sc.exe sdshow EtwDnstap | Out-String);sidtype=(& sc.exe qsidtype EtwDnstap | Out-String);privileges=(& sc.exe qprivs EtwDnstap | Out-String)
        }
    }
    $receipt=[ordered]@{vm=$vm | Select-Object Name,Id,State,ProcessorCount,MemoryAssigned,DynamicMemoryEnabled;guest=$result}
    $path=Join-Path $out "$RunId-$Action-$([DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')).json"
    $receipt | ConvertTo-Json -Depth 8 | Set-Content $path -Encoding UTF8
    if ($Action -in 'Backup','Fetch','Restore') {
        Copy-Item -FromSession $session -Path "C:/ProgramData/EtwDnstap/performance/$CampaignId/*" -Exclude '*.exe' -Destination $out -Force
    }
    $receipt | ConvertTo-Json -Depth 8
} finally { Remove-PSSession $session }
