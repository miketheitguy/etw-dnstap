#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Historical Windows service campaign controller (pooled-VM and ring-fix harnesses, consolidated).

.DESCRIPTION
Replaces windows-pooled-campaign.ps1 (-Variant Pooled) and windows-ring-fix-campaign.ps1
(-Variant RingFix). Both used the same Inventory/Backup/Prepare/Deploy/Start/Stop/Status/
Restore/Proof/Fetch lifecycle against the Windows VM over PowerShell Direct, with their
dated campaign directory baked in; the directory is now the mandatory -CampaignId.

Backup, Prepare, Deploy and Start are disabled: the service no longer supports the JSONL
metrics these harnesses measured. Inventory, Stop, Status, Restore, Proof and Fetch remain
available so existing evidence can be cleaned up and collected.

Variant differences that are preserved:
  Pooled : capacity fixed at 8192, no --capacity argument on the service command line, the
           DNS inventory is limited to Inventory/Backup/Prepare/Deploy/Start/Proof so that
           Status/Stop/Fetch/Restore never perturb DNS, Stop verifies the SCM state.
  RingFix: capacity 1-16384 (default 1024) is passed as --capacity, the DNS inventory runs
           for every action, Restore can re-append --capacity unless -UseBinaryDefault.

Behavior changes from the originals: -ExpectedHash is mandatory (no baked-in hash); Fetch
keeps identical files and refuses to overwrite a different existing host file; the zone
removal in Restore requires the zone in owned-zones.txt (legacy markers are still read);
Backup refuses to run when the host evidence directory already exists.

.PARAMETER Action
Lifecycle step to run.
.PARAMETER CampaignId
Campaign directory name (guest: <data_dir>/<CampaignId>, host: target/performance/<CampaignId>/windows-campaign).
.PARAMETER Variant
Pooled or RingFix (see above).
.PARAMETER RunId
Run label used in metrics and receipt file names.
.PARAMETER ExpectedHash
SHA-256 of the service binary that must be installed (or staged, for Deploy).
.PARAMETER CandidatePath
Host path of the candidate binary (Deploy only; hash is verified before copy).
.PARAMETER ReviewGateApproved
Required together with -CandidatePath for Deploy.
.PARAMETER UseBinaryDefault
RingFix Restore: restore the original arguments without appending --capacity.

.OUTPUTS
JSON receipts under target/performance/<CampaignId>/windows-campaign.

.EXAMPLE
./archive/windows-service-campaign.ps1 -Action Restore -Variant Pooled -CampaignId <id> -ExpectedHash <sha256>
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Inventory','Backup','Prepare','Deploy','Start','Stop','Status','Restore','Proof','Fetch')][string]$Action,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$CampaignId,
    [Parameter(Mandatory)][ValidateSet('Pooled','RingFix')][string]$Variant,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ExpectedHash,
    [ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId = 'campaign',
    [ValidateRange(1,16384)][int]$Capacity = 1024,
    [string]$CandidatePath,
    [switch]$ReviewGateApproved,
    [switch]$UseBinaryDefault
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '../LabCommon.psm1') -Force
if ($Action -in 'Backup','Prepare','Deploy','Start') { throw 'Historical service-metrics harness: new campaigns are disabled because the TCP-only service no longer supports JSONL metrics. Existing evidence and cleanup actions remain available.' }
$isPooled = $Variant -eq 'Pooled'
if (-not $PSBoundParameters.ContainsKey('Capacity')) { $Capacity = if ($isPooled) { 8192 } else { 1024 } }
if ($isPooled -and $Capacity -ne 8192) { throw 'The Pooled profile only supports capacity 8192' }

$config = Get-LabConfig
$root = Get-LabRoot
$out = Resolve-LabPath "$($config.paths.evidence_root)/$CampaignId/windows-campaign"
if ($Action -eq 'Backup' -and (Test-Path -LiteralPath $out) -and @(Get-ChildItem -LiteralPath $out -Force).Count) { throw "Campaign evidence already exists; refusing to reuse CampaignId '$CampaignId': $out" }
New-Item -ItemType Directory -Path $out -Force | Out-Null
$vm = Get-LabVm -RequireRunning -CheckResources
if ($Action -in 'Backup','Prepare','Deploy','Start' -and (Get-PSDrive C).Free -lt 20GB) { throw 'Host disk guard' }
$guestBase = "$($config.windows_vm.data_dir)/$CampaignId"
$metricsPath = ($guestBase -replace '/', '\') + '\' + $RunId
$serviceArguments = @()
if (-not $isPooled) { $serviceArguments += @('--capacity', "$Capacity") }
$serviceArguments += @('--metrics', $metricsPath)
$measuredCommandLine = Get-LabServiceCommandLine -ExtraArguments $serviceArguments
$session = Get-LabSession -Vm $vm
try {
    if ($Action -eq 'Deploy') {
        if (-not $ReviewGateApproved -or -not $CandidatePath) { throw 'Candidate deployment requires reviewed gate and explicit path' }
        if ((Get-FileHash -LiteralPath $CandidatePath).Hash -ne $ExpectedHash) { throw 'Host candidate hash mismatch' }
        Copy-Item -LiteralPath $CandidatePath -Destination "$guestBase/candidate-staged.exe" -ToSession $session
    }
    if ($Action -eq 'Prepare') {
        Invoke-LabGuest -Session $session -ArgumentList $guestBase -ScriptBlock { param($base) if (-not (Test-Path -LiteralPath "$base/service-original.json")) { throw 'Missing current campaign backup' } }
        Initialize-LabDnsZones -Session $session -BaseDirectory $guestBase -UpstreamAddress $config.network.receiver_ip
    }
    $guestArguments = @($Action, $RunId, $Capacity, $ExpectedHash, [bool]$UseBinaryDefault, $guestBase, $isPooled, $measuredCommandLine,
        $config.windows_vm.install_exe, $config.windows_vm.service_name, $config.dns.auth_zone, $config.dns.rec_zone,
        [int]$config.dns.auth_record_count, $config.dns.answer_name, $config.dns.answer_ip, [int]$config.dns.answer_ttl)
    $result = Invoke-LabGuest -Session $session -ArgumentList $guestArguments -ScriptBlock {
        param($action,$run,$capacity,$expected,$useDefault,$base,$pooled,$measuredPath,$installed,$serviceName,$authZone,$recZone,$authCount,$answerName,$answerIp,$answerTtl)
        $svcFilter = "Name='$serviceName'"
        if ($action -in 'Backup','Prepare','Deploy','Start' -and (Get-Volume -DriveLetter C).SizeRemaining -lt 8GB) { throw 'Guest disk guard' }
        $service = Get-CimInstance Win32_Service -Filter $svcFilter
        $hash = (Get-FileHash -LiteralPath $installed).Hash
        if ($action -ne 'Deploy' -and $hash -ne $expected) { throw 'Installed service hash mismatch' }
        if ($service.StartName -ne 'LocalSystem') { throw 'Service identity changed' }
        # Pooled: DNS management inventory is restricted to pre-load/proof actions so that
        # Status/Stop/Fetch/Restore neither perturb DNS nor block cleanup.
        $aCount = $null
        if (-not $pooled -or $action -in 'Inventory','Backup','Prepare','Deploy','Start','Proof') {
            $aCount = @(Get-DnsServerResourceRecord -ZoneName $authZone -RRType A).Count
            if ($aCount -ne $authCount) { throw "Expected retained $authCount-record authoritative zone" }
        }
        if ($action -eq 'Backup') {
            if (Test-Path -LiteralPath $base) { throw 'Campaign backup exists; preserve it' }
            if ($service.State -ne 'Running' -or $service.PathName -match '--metrics') { throw 'Unexpected baseline service state' }
            if (@(Get-DnsServerZone | Where-Object ZoneName -eq $recZone).Count) { throw 'Temporary recursive zone unexpectedly present' }
            New-Item -ItemType Directory -Path $base | Out-Null
            & icacls.exe $base /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
            if ($LASTEXITCODE -ne 0) { throw 'Protect campaign backup failed' }
            $service | Select-Object Name,PathName,StartName,StartMode,State | ConvertTo-Json | Set-Content "$base/service-original.json" -Encoding UTF8
            Copy-Item -LiteralPath $installed -Destination "$base/service-original.exe"
            if ((Get-FileHash "$base/service-original.exe").Hash -ne $expected) { throw 'Guest baseline backup mismatch' }
            $hash | Set-Content "$base/baseline-sha256.txt" -Encoding ASCII
            (Get-Acl -LiteralPath $installed).Sddl | Set-Content "$base/executable-original-sddl.txt" -Encoding UTF8
            Get-DnsServerRecursion | Export-Clixml "$base/recursion-original.xml"
            Get-DnsServerForwarder | Export-Clixml "$base/forwarders-original.xml"
            Get-DnsServerCache | Export-Clixml "$base/cache-original.xml"
            & sc.exe qsidtype $serviceName | Set-Content "$base/sidtype-original.txt" -Encoding UTF8
            & sc.exe qprivs $serviceName | Set-Content "$base/privileges-original.txt" -Encoding UTF8
        }
        if ($action -eq 'Deploy') {
            if (-not (Test-Path "$base/service-original.exe")) { throw 'Missing protected guest baseline' }
            if ((Get-FileHash "$base/candidate-staged.exe").Hash -ne $expected) { throw 'Staged candidate hash mismatch' }
            Stop-Service $serviceName -ErrorAction SilentlyContinue
            $beforeAcl = (Get-Acl -LiteralPath $installed).Sddl
            Copy-Item -LiteralPath "$base/candidate-staged.exe" -Destination $installed -Force
            $hash = (Get-FileHash -LiteralPath $installed).Hash
            if ($hash -ne $expected -or (Get-Acl -LiteralPath $installed).Sddl -ne $beforeAcl) { throw 'Deployment hash or executable ACL mismatch' }
            @{utc=[DateTime]::UtcNow.ToString('o');sha256=$hash;executable_sddl_unchanged=$true} | ConvertTo-Json | Set-Content "$base/$run-deployment.json" -Encoding UTF8
        }
        if ($action -eq 'Start') {
            if ($service.State -ne 'Stopped') { throw 'Service must be stopped before a run' }
            if (@(Get-ChildItem -LiteralPath $base -Filter "$run.*.jsonl").Count) { throw 'Run metrics already exist; preserve prior run' }
            $changed = Invoke-CimMethod -InputObject $service -MethodName Change -Arguments @{PathName=$measuredPath}
            if ($changed.ReturnValue -ne 0) { throw 'Set measured service arguments failed' }
            Start-Service $serviceName
        }
        if ($action -eq 'Stop') {
            Stop-Service $serviceName
            if ($pooled) {
                $svcAfter = Get-CimInstance Win32_Service -Filter $svcFilter
                if ($svcAfter.State -ne 'Stopped') { throw 'SCM stop did not finish' }
            }
        }
        $lastMetrics = $null
        if ($action -eq 'Status') {
            $file = Get-ChildItem -LiteralPath $base -Filter "$run.*.jsonl" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
            if ($file) {
                $complete = @(Get-Content -LiteralPath $file.FullName -Tail 2 | ForEach-Object { try { $_ | ConvertFrom-Json -ErrorAction Stop } catch { } })
                $lastMetrics = $complete | Select-Object -Last 1
            }
        }
        if ($action -eq 'Restore') {
            Stop-Service $serviceName -ErrorAction SilentlyContinue
            $original = Get-Content "$base/service-original.json" -Raw | ConvertFrom-Json
            if ($original.PathName -match '--metrics|--capacity') { throw 'Unexpected original argument shape; inspect before restoration' }
            $normalPath = if ($pooled -or $useDefault) { $original.PathName } else { $original.PathName + ' --capacity ' + $capacity }
            $changed = Invoke-CimMethod -InputObject $service -MethodName Change -Arguments @{PathName=$normalPath}
            if ($changed.ReturnValue -ne 0) { throw 'Restore normal arguments failed' }
            $null = Remove-LabOwnedZone -Base $base -Zone $recZone
            $before = Import-Clixml "$base/recursion-original.xml"
            $after = Get-DnsServerRecursion
            foreach ($property in 'AdditionalTimeout','Enable','RetryInterval','SecureResponse','Timeout') {
                if ($before.$property -ne $after.$property) { throw "Recursion baseline changed: $property" }
            }
            Get-DnsServerCache | Export-Clixml "$base/cache-final.xml"
            Get-DnsServerForwarder | Export-Clixml "$base/forwarders-final.xml"
            & sc.exe qsidtype $serviceName | Set-Content "$base/sidtype-final.txt" -Encoding UTF8
            & sc.exe qprivs $serviceName | Set-Content "$base/privileges-final.txt" -Encoding UTF8
            & wpr.exe -status 2>&1 | Set-Content "$base/wpr-final.txt" -Encoding UTF8
            & logman.exe query | Set-Content "$base/logman-final.txt" -Encoding UTF8
            @{utc=[DateTime]::UtcNow.ToString('o');sha256=$hash;capacity=$capacity;uses_binary_default=($pooled -or $useDefault);normal_arguments=$normalPath;authoritative_a_records=$aCount} | ConvertTo-Json | Set-Content "$base/restoration-final.json" -Encoding UTF8
            if ($original.State -eq 'Running') { Start-Service $serviceName }
        }
        if ($action -eq 'Proof') {
            if ($service.State -ne 'Running' -or $service.PathName -match '--metrics') { throw 'Service not restored' }
            if (@(Get-DnsServerZone | Where-Object ZoneName -eq $recZone).Count) { throw 'Temporary recursive forwarder remains' }
            $proof = Test-LabAnswerLocal -Name $answerName -Address $answerIp -Ttl $answerTtl
            if (-not $proof.passed) { throw 'Final authoritative answer failed' }
            $proof.answers | ConvertTo-Json | Set-Content "$base/final-query-proof.json" -Encoding UTF8
        }
        [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');action=$action;run_id=$run;requested_capacity=$capacity;binary_sha256=$hash;authoritative_a_records=$aCount;last_metrics=$lastMetrics;service=Get-CimInstance Win32_Service -Filter $svcFilter | Select-Object Name,State,ProcessId,PathName,StartName,ExitCode}
    }
    $suffix = if ($Action -eq 'Status') { '-' + [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() } else { '' }
    Write-LabJson -Path (Join-Path $out "$RunId-$Action$suffix.json") -InputObject $result -Depth 5 -Force
    if ($Action -in 'Fetch','Restore','Proof') {
        $null = Receive-LabDirectory -Session $session -GuestDirectory $guestBase -Destination $out -ExcludeExtension '.exe'
    }
    $result | Format-List
} finally { Remove-PSSession $session }
