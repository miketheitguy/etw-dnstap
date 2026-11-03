#requires -Version 5.1
<#
.SYNOPSIS
Host driver for the MSI lab tests L0 to L7 (docs/plans/msi-installer-plan.md, "Lab tests").

.DESCRIPTION
Runs in Windows PowerShell 5.1 (Hyper-V module). Imports scripts/performance/LabCommon.psm1,
reaches the guest through PowerShell Direct, copies the guest scripts and the MSI in, runs
msiexec with verbose logging, reboots the guest, runs the assertion scripts, and counts
deliveries on the receiver with `ss -tin` (never from collector logs).

Results go to target\lab-msi\<yyyyMMdd-HHmmss>\ :
  driver.log                      what the driver did
  pre-state\                      L0: sc.exe output, ImagePath, event source key, ACLs, folder listing
  <step>-msiexec.log              msiexec /l*v log
  <step>-<script>.txt             guest script output (PASS/FAIL lines)
  <step>-assertions.txt           host-side assertions
  <step>-evidence.txt             action sequence and provisioning lines from the msiexec log
  summary.json                    one record for each step

Steps:
  L0  check the collector config, take the checkpoint, save the pre-test state
  L1  remove the hand-provisioned install, reboot, Test-EtwDnstapRemoved
  L2  msiexec /i without RECEIVER (expect 1603)
  L3  msiexec /i with a hostname as RECEIVER (expect 1603 and a rollback)
  L4  msiexec /i with the lab values, reboot, Test-EtwDnstapInstalled (expect 3010)
  L5  msiexec /fvomus, reboot, Test-EtwDnstapInstalled (expect 0 or 3010)
  L6  msiexec /x, reboot, Test-EtwDnstapRemoved (expect 3010)
  L7  install again with the lab values, reboot, Test-EtwDnstapInstalled (left installed)
  Restore  apply the checkpoint (not part of All)

The collector standing rule: its output goes to the devnull logger. L0 checks that and stops
if it does not hold.

.PARAMETER Step
All (default), or one or more of L0..L7 and Restore, separated by ',' or ';'. A failed
step can be rerun alone after a fix.

.PARAMETER RunFolder
Name of a folder under target\lab-msi to use. Default: a new folder for All and L0, the
newest existing folder for other steps.

.PARAMETER CheckpointName
Default msi-installer-pre-<yyyy-MM-dd>. L0 creates it unless it exists. Nothing deletes it.

.PARAMETER MsiPath
Default target\msi\etw-dnstap-<version>-x64.msi, the version read from Cargo.toml.

.PARAMETER RebootTimeoutSeconds
How long to wait for the guest to answer PowerShell Direct after a restart.

.EXAMPLE
powershell.exe -NoProfile -File scripts\lab\msi\Invoke-MsiLabCycle.ps1 -Step L4
#>
[CmdletBinding()]
param(
    [string]$Step = 'All',
    [string]$RunFolder = '',
    [string]$CheckpointName = ('msi-installer-pre-' + (Get-Date -Format 'yyyy-MM-dd')),
    [string]$MsiPath = '',
    [int]$RebootTimeoutSeconds = 600
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Import-Module (Join-Path $PSScriptRoot '..\..\performance\LabCommon.psm1') -Force

$repoRoot = Get-LabRoot
$config = Get-LabConfig
$allSteps = 'L0', 'L1', 'L2', 'L3', 'L4', 'L5', 'L5b', 'L6', 'L7'
$guestScriptRoot = Join-Path $PSScriptRoot 'guest'
$sessionName = $config.windows_vm.session_name
$receiver = "$($config.network.receiver_ip):$($config.network.receiver_port)"
$upgradeCode = '{0F4D1A87-DAD8-4C37-ABD4-FEC4DA400FAB}'

# ---- steps to run ---------------------------------------------------------------------------
$requested = @($Step -split '[,;]' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if ($requested -contains 'All') { $requested = $allSteps }
foreach ($name in $requested) {
    if (($allSteps + 'Restore') -notcontains $name) { throw "Unknown step '$name'. Use All, L0 to L7, or Restore." }
}

# ---- run folder -----------------------------------------------------------------------------
$resultsRoot = Resolve-LabPath 'target\lab-msi'
if (-not (Test-Path -LiteralPath $resultsRoot)) { New-Item -ItemType Directory -Path $resultsRoot | Out-Null }
if ($RunFolder) { $runDir = Join-Path $resultsRoot $RunFolder }
elseif ($requested -contains 'L0') { $runDir = Join-Path $resultsRoot (Get-Date -Format 'yyyyMMdd-HHmmss') }
else {
    $latest = @(Get-ChildItem -LiteralPath $resultsRoot -Directory | Sort-Object Name -Descending | Select-Object -First 1)
    if ($latest.Count -eq 0) { throw 'No earlier run folder. Run L0 first.' }
    $runDir = $latest[0].FullName
}
if (-not (Test-Path -LiteralPath $runDir)) { New-Item -ItemType Directory -Path $runDir | Out-Null }
$preStateDir = Join-Path $runDir 'pre-state'
$logFile = Join-Path $runDir 'driver.log'
$summaryFile = Join-Path $runDir 'summary.json'

function Write-Log([string]$Text) {
    $line = '{0} {1}' -f (Get-Date -Format 'HH:mm:ss'), $Text
    Write-Information -MessageData $line -InformationAction Continue
    Add-Content -LiteralPath $logFile -Value $line -Encoding UTF8
}

# ---- Cargo version and MSI path ---------------------------------------------------------------
$cargoText = Get-Content -LiteralPath (Join-Path $repoRoot 'Cargo.toml') -Raw -Encoding UTF8
if ($cargoText -notmatch '(?m)^version\s*=\s*"([^"]+)"') { throw 'No version in Cargo.toml.' }
$version = $Matches[1]
if (-not $MsiPath) { $MsiPath = Join-Path $repoRoot "target\msi\etw-dnstap-$version-x64.msi" }
if (-not (Test-Path -LiteralPath $MsiPath)) { throw "MSI not found: $MsiPath. Build it with scripts/build/Build-Msi.ps1." }

# ---- per-step counters and summary ------------------------------------------------------------
$script:StepPass = 0
$script:StepFail = 0
$script:StepNotes = New-Object System.Collections.Generic.List[string]
$script:StepMsiexec = New-Object System.Collections.Generic.List[object]

function Add-Assertion([string]$StepName, [string]$Name, [bool]$Ok, $Value) {
    $text = if ($null -eq $Value) { '' } else { (@($Value) | ForEach-Object { [string]$_ }) -join ' | ' }
    $line = "{0} {1} : {2}" -f $(if ($Ok) { 'PASS' } else { 'FAIL' }), $Name, $text
    if ($Ok) { $script:StepPass++ } else { $script:StepFail++ }
    Write-Log "[$StepName] $line"
    Add-Content -LiteralPath (Join-Path $runDir "$StepName-assertions.txt") -Value $line -Encoding UTF8
}

function Save-StepSummary([string]$StepName) {
    $records = @{}
    if (Test-Path -LiteralPath $summaryFile) {
        $existing = Get-Content -LiteralPath $summaryFile -Raw -Encoding UTF8 | ConvertFrom-Json
        foreach ($record in @($existing.steps)) { $records[$record.step] = $record }
    }
    $records[$StepName] = [pscustomobject]@{
        step       = $StepName
        msiexec    = [object[]]$script:StepMsiexec.ToArray()
        pass       = $script:StepPass
        fail       = $script:StepFail
        notes      = [string[]]$script:StepNotes.ToArray()
        finishedAt = (Get-Date).ToString('o')
    }
    $ordered = @($records.Keys | Sort-Object | ForEach-Object { $records[$_] })
    $summary = [pscustomobject]@{
        runFolder  = (Split-Path -Leaf $runDir)
        version    = $version
        checkpoint = $CheckpointName
        msi        = (Split-Path -Leaf $MsiPath)
        steps      = $ordered
    }
    Write-LabJson -Path $summaryFile -InputObject $summary -Force
}

# ---- guest access -----------------------------------------------------------------------------
$vm = Get-LabVm   # no -CheckResources: lab.json lists 2 vCPU, the guest has 6
$script:Session = $null
$script:GuestStage = $null

function Get-GuestSession {
    if ($null -ne $script:Session -and $script:Session.State -eq 'Opened') { return $script:Session }
    if ($null -ne $script:Session) { Remove-PSSession -Session $script:Session -ErrorAction SilentlyContinue }
    $script:Session = Get-LabSession -Vm $vm
    return $script:Session
}

function Wait-GuestSession([int]$TimeoutSeconds, [Nullable[datetime]]$BootedAfter = $null) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        try {
            $session = Get-LabSession -Vm $vm
            $boot = Invoke-Command -Session $session -ScriptBlock { (Get-CimInstance Win32_OperatingSystem).LastBootUpTime }
            if ($null -eq $BootedAfter -or $boot -gt $BootedAfter) {
                if ($null -ne $script:Session) { Remove-PSSession -Session $script:Session -ErrorAction SilentlyContinue }
                $script:Session = $session
                return $boot
            }
            Remove-PSSession -Session $session -ErrorAction SilentlyContinue
        }
        catch { Start-Sleep -Seconds 5 }
        Start-Sleep -Seconds 5
    }
    throw "The guest did not answer PowerShell Direct within $TimeoutSeconds seconds."
}

function Invoke-GuestRestart {
    $session = Get-GuestSession
    $before = Invoke-Command -Session $session -ScriptBlock { (Get-CimInstance Win32_OperatingSystem).LastBootUpTime }
    Write-Log "Restarting the guest (last boot $before)."
    $started = Get-Date
    try { Invoke-Command -Session $session -ScriptBlock { Restart-Computer -Force } -ErrorAction Stop } catch { Write-Log "Restart command ended the session: $($_.Exception.Message)" }
    Start-Sleep -Seconds 20
    $script:Session = $null
    $boot = Wait-GuestSession -TimeoutSeconds $RebootTimeoutSeconds -BootedAfter $before
    $seconds = [int]((Get-Date) - $started).TotalSeconds
    Write-Log "The guest answers again $seconds seconds after the restart command (boot time $boot)."
    $script:StepNotes.Add("reboot: guest answered $seconds s after the restart command")
}

function Initialize-GuestStage {
    $session = Get-GuestSession
    $script:GuestStage = Invoke-Command -Session $session -ScriptBlock {
        $dir = Join-Path $env:SystemRoot 'Temp\EtwDnstapMsiLab'
        if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
        $dir
    }
    foreach ($file in Get-ChildItem -LiteralPath $guestScriptRoot -Filter '*.ps1') {
        Copy-Item -LiteralPath $file.FullName -Destination $script:GuestStage -ToSession $session -Force
    }
    Copy-Item -LiteralPath $MsiPath -Destination (Join-Path $script:GuestStage 'etw-dnstap.msi') -ToSession $session -Force
    Copy-Item -LiteralPath (Join-Path $repoRoot 'scripts\provision\Grant-EtwDnstapEtwAccess.ps1') -Destination $script:GuestStage -ToSession $session -Force
}

# Runs a copied guest script with named parameters. Returns Output (lines) and ExitCode.
function Invoke-GuestScript([string]$StepName, [string]$ScriptName, [hashtable]$Parameters = @{}) {
    $session = Get-GuestSession
    $path = Join-Path $script:GuestStage $ScriptName
    $result = Invoke-Command -Session $session -ArgumentList $path, $Parameters -ScriptBlock {
        param($scriptPath, $scriptParameters)
        $global:LASTEXITCODE = 0
        $lines = @(& $scriptPath @scriptParameters)
        [pscustomobject]@{ Output = @($lines | ForEach-Object { [string]$_ }); ExitCode = $LASTEXITCODE }
    }
    $outFile = Join-Path $runDir ("$StepName-" + [IO.Path]::GetFileNameWithoutExtension($ScriptName) + '.txt')
    Write-LabText -Path $outFile -Text ((@($result.Output) -join [Environment]::NewLine) + [Environment]::NewLine)
    return $result
}

# Counts the PASS and FAIL lines of an assertion script and copies them to the log.
function Add-GuestAssertion([string]$StepName, $Result, [string]$What) {
    foreach ($line in @($Result.Output)) {
        if ($line -match '^PASS ') { $script:StepPass++; Write-Log "[$StepName] $line" }
        elseif ($line -match '^FAIL ') { $script:StepFail++; Write-Log "[$StepName] $line" }
        else { Write-Log "[$StepName] $line" }
    }
    $summaryLine = @($Result.Output | Where-Object { $_ -match '^SUMMARY ' }) | Select-Object -Last 1
    if ($summaryLine) { $script:StepNotes.Add("$What : $summaryLine") }
    Add-Assertion $StepName "$What exit code equals its FAIL count" ($Result.ExitCode -eq @($Result.Output | Where-Object { $_ -match '^FAIL ' }).Count) "exit=$($Result.ExitCode)"
}

# ---- msiexec ----------------------------------------------------------------------------------
function Invoke-Msiexec([string]$StepName, [string]$Mode, [string]$Properties, [int[]]$ExpectedExitCodes, [string]$AssertionStep = '') {
    if (-not $AssertionStep) { $AssertionStep = $StepName }
    $session = Get-GuestSession
    $guestLog = Join-Path $script:GuestStage "$StepName-msiexec.log"
    $guestMsi = Join-Path $script:GuestStage 'etw-dnstap.msi'
    $arguments = switch ($Mode) {
        'install' { "/i `"$guestMsi`" /qn /norestart /l*v `"$guestLog`" $Properties" }
        'repair' { "/fvomus `"$guestMsi`" /qn /norestart /l*v `"$guestLog`"" }
        'uninstall' { "/x `"$guestMsi`" /qn /norestart /l*v `"$guestLog`"" }
    }
    $arguments = $arguments.Trim()
    Write-Log "[$StepName] msiexec $arguments"
    $started = Get-Date
    $exit = Invoke-Command -Session $session -ArgumentList $arguments, $guestLog -ScriptBlock {
        param($argumentText, $logPath)
        if (Test-Path -LiteralPath $logPath) { Remove-Item -LiteralPath $logPath -Force }
        $process = Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\msiexec.exe') -ArgumentList $argumentText -Wait -PassThru
        $process.ExitCode
    }
    $seconds = [int]((Get-Date) - $started).TotalSeconds
    $hostLog = Join-Path $runDir "$StepName-msiexec.log"
    if (Test-Path -LiteralPath $hostLog) { Remove-Item -LiteralPath $hostLog -Force }
    $null = Receive-LabFile -Session $session -GuestPath $guestLog -Destination $hostLog
    $script:StepMsiexec.Add([pscustomobject]@{
            mode = $Mode; arguments = ($arguments -replace [regex]::Escape($script:GuestStage), '<stage>')
            exitCode = $exit; expected = $ExpectedExitCodes; seconds = $seconds; log = (Split-Path -Leaf $hostLog)
        })
    Add-Assertion $AssertionStep "msiexec $Mode ($StepName) exit code" ($ExpectedExitCodes.Count -eq 0 -or $ExpectedExitCodes -contains $exit) "exit=$exit expected=$(if ($ExpectedExitCodes.Count -eq 0) { 'recorded only' } else { $ExpectedExitCodes -join ' or ' }) seconds=$seconds"
    Save-LogEvidence $StepName $hostLog
    return $exit
}

# Keeps the lines of the verbose log that explain the order of actions and what the
# provisioning script and the service configuration did.
function Save-LogEvidence([string]$StepName, [string]$LogPath) {
    $patterns = 'Action start .*: (InstallInitialize|StopServices|ProvisionUninstall|DeleteServices|RemoveFiles|InstallFiles|InstallServices|StartServices|ProvisionInstall|ProvisionInstallRollback|MsiConfigureServices|MsiServiceConfig|SchedServiceConfig|MsiLockPermissionsEx|SchedLockPermissionsEx|SchedPerfCounterManifestInstall|SchedPerfCounterManifestUninstall|InstallFinalize)',
        '\[EtwDnstap\]', 'WixQuietExec', 'ServiceConfig', 'WIX1149', 'Service SID for', 'Return value 3', 'Rollback', 'Action ended .*ProvisionInstall',
        'Action ended .*ProvisionUninstall', 'ConfigureService', 'Failure', 'Creating folder', 'Product: EtwDnstap -- ', 'RECEIVER_REMEMBERED', 'Action start .*: (AppSearch|LaunchConditions|FindRelatedProducts|SetSessionName|MigrateFeatureStates)', 'Property.S.: (RECEIVER|SESSION_NAME|SERVICE_ARGS) '
    $encoding = [Text.Encoding]::Unicode
    $bytes = [IO.File]::ReadAllBytes($LogPath)
    $text = if ($bytes.Length -ge 2 -and $bytes[0] -eq 0xFF -and $bytes[1] -eq 0xFE) { $encoding.GetString($bytes, 2, $bytes.Length - 2) } else { [Text.Encoding]::Default.GetString($bytes) }
    $lines = $text -split "`r?`n"
    $kept = New-Object System.Collections.Generic.List[string]
    for ($i = 0; $i -lt $lines.Count; $i++) {
        foreach ($pattern in $patterns) {
            if ($lines[$i] -match $pattern) { $kept.Add(('{0,6}: {1}' -f ($i + 1), $lines[$i])); break }
        }
    }
    Write-LabText -Path (Join-Path $runDir "$StepName-evidence.txt") -Text (($kept -join [Environment]::NewLine) + [Environment]::NewLine)
}

# ---- receiver ---------------------------------------------------------------------------------
$sshConfig = Resolve-LabPath $config.ubuntu_vm.ssh_config
$sshAlias = $config.ubuntu_vm.ssh_alias

function Invoke-Receiver([string]$Command) {
    Push-Location -LiteralPath $repoRoot
    try {
        $output = & ssh.exe -F $sshConfig $sshAlias $Command 2>&1
        if ($LASTEXITCODE -ne 0) { throw "ssh failed (exit $LASTEXITCODE): $($output -join ' ')" }
        return @($output | ForEach-Object { [string]$_ })
    }
    finally { Pop-Location }
}

function Get-ReceiverByteCount {
    $lines = Invoke-Receiver "ss -tin state established '( sport = :$($config.network.receiver_port) )'"
    $total = [int64]0
    $connections = 0
    foreach ($line in $lines) {
        if ($line -match 'bytes_received:(\d+)') { $total += [int64]$Matches[1]; $connections++ }
    }
    return [pscustomobject]@{ Bytes = $total; Connections = $connections }
}

function Test-ReceiverDelivery([string]$StepName) {
    $before = Get-ReceiverByteCount
    $session = Get-GuestSession
    $answered = Invoke-Command -Session $session -ArgumentList $config.dns.answer_name, $config.dns.auth_zone -ScriptBlock {
        param($answerName, $zone)
        $ok = 0
        for ($i = 1; $i -le 100; $i++) {
            $name = if ($i % 2 -eq 0) { $answerName } else { "msi-lab-$i.$zone" }
            try { if (@(Resolve-DnsName -Name $name -Server 127.0.0.1 -Type A -DnsOnly -ErrorAction Stop).Count -gt 0) { $ok++ } } catch { $null = $_ }
        }
        $ok
    }
    $after = $before
    $deadline = (Get-Date).AddSeconds(60)
    do {
        Start-Sleep -Seconds 3
        $after = Get-ReceiverByteCount
    } while ($after.Bytes -le $before.Bytes -and (Get-Date) -lt $deadline)
    Add-Assertion $StepName 'receiver bytes_received rises after 100 DNS queries (ss -tin)' ($after.Bytes -gt $before.Bytes) "before=$($before.Bytes) after=$($after.Bytes) connections=$($after.Connections) queries-answered=$answered"
}

# ---- state helpers ----------------------------------------------------------------------------
function Read-JsonFile([string]$Path) { Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json }

function Get-GuestState([string]$StepName) {
    $result = Invoke-GuestScript $StepName 'Get-EtwDnstapState.ps1'
    return (($result.Output -join [Environment]::NewLine) | ConvertFrom-Json)
}

function Get-ServiceArgsFromImagePath([string]$ImagePath) {
    $rest = $ImagePath.Trim()
    if ($rest.StartsWith('"')) { $rest = $rest.Substring($rest.IndexOf('"', 1) + 1) }
    else { $rest = ($rest -split '\s+', 2)[1] }
    $tokens = @([regex]::Matches($rest, '"[^"]*"|\S+') | ForEach-Object { $_.Value })
    $kept = New-Object System.Collections.Generic.List[string]
    $i = 0
    while ($i -lt $tokens.Count) {
        if ($tokens[$i] -eq '--receiver' -or $tokens[$i] -eq '--session-name') { $i += 2; continue }
        $kept.Add($tokens[$i]); $i++
    }
    return ($kept -join ' ')
}

function Get-LabValue {
    $imageFile = Join-Path $preStateDir 'imagepath.txt'
    if (-not (Test-Path -LiteralPath $imageFile)) { throw 'No saved ImagePath. Run L0 in this run folder first.' }
    $imagePath = (Get-Content -LiteralPath $imageFile -Raw -Encoding UTF8).Trim()
    return [pscustomobject]@{
        ImagePath   = $imagePath
        ServiceArgs = Get-ServiceArgsFromImagePath $imagePath
    }
}

function Get-DataFolderName([string]$FileName) {
    $path = Join-Path $runDir $FileName
    if (-not (Test-Path -LiteralPath $path)) { throw "Missing $FileName. Run L1 in this run folder first." }
    return @((Read-JsonFile $path) | ForEach-Object { $_.name })
}

function Get-InstalledAssertionParameter {
    $lab = Get-LabValue
    $parameters = @{
        ExpectedVersion = $version
        Receiver        = $receiver
        SessionName     = $sessionName
        ServiceArgs     = $lab.ServiceArgs
        UpgradeCode     = $upgradeCode
    }
    $baseline = Join-Path $script:GuestStage 'pre-state.json'
    $parameters['EventSourceBaseline'] = $baseline
    return $parameters
}

function Get-LabProperty {
    $lab = Get-LabValue
    $properties = "RECEIVER=`"$receiver`" SESSION_NAME=`"$sessionName`""
    if ($lab.ServiceArgs) { $properties += " SERVICE_ARGS=`"$($lab.ServiceArgs)`"" }
    return $properties
}

function Send-PreState {
    $session = Get-GuestSession
    Copy-Item -LiteralPath (Join-Path $preStateDir 'state.json') -Destination (Join-Path $script:GuestStage 'pre-state.json') -ToSession $session -Force
}

function Save-DataFolderListing([string]$StepName, [string]$FileName) {
    $state = Get-GuestState $StepName
    Write-LabJson -Path (Join-Path $runDir $FileName) -InputObject @($state.dataFolder) -Force
    return $state
}

# ---- the steps --------------------------------------------------------------------------------
function Invoke-StepL0 {
    $check = 'L0'
    # Collector rule: output to the devnull logger only.
    $configText = (Invoke-Receiver 'sudo cat /etc/dnscollector/config.yml') -join "`n"
    Write-LabText -Path (Join-Path $runDir 'collector-config.yml') -Text ($configText + "`n")
    $keys = @()
    foreach ($chunk in ($configText -split '(?m)^  - name:' | Select-Object -Skip 1)) {
        foreach ($m in [regex]::Matches($chunk, '(?m)^    ([a-z0-9-]+):\s*$')) { if ($m.Groups[1].Value -ne 'routing-policy') { $keys += $m.Groups[1].Value } }
    }
    $others = @($keys | Where-Object { $_ -ne 'dnstap' -and $_ -ne 'devnull' })
    Add-Assertion $check 'collector config: every pipeline is dnstap or devnull' ($keys -contains 'devnull' -and $others.Count -eq 0) "pipelines=[$($keys -join ', ')] other=[$($others -join ', ')]"
    $unit = (Invoke-Receiver 'systemctl show dnscollector -p StandardOutput -p StandardError') -join ' '
    Add-Assertion $check 'collector service: StandardOutput and StandardError are null' ($unit -match 'StandardOutput=null' -and $unit -match 'StandardError=null') $unit
    if ($script:StepFail -gt 0) { throw 'The collector is not routed to null. Fix it on the receiver (with sudo, keep a backup of the config) and rerun L0.' }

    # Checkpoint.
    $existing = Get-VMSnapshot -VMName $vm.Name -Name $CheckpointName -ErrorAction SilentlyContinue
    if ($null -eq $existing) {
        Checkpoint-VM -VM $vm -SnapshotName $CheckpointName
        Write-Log "Created checkpoint '$CheckpointName'."
    }
    else { Write-Log "Checkpoint '$CheckpointName' exists already (created $($existing.CreationTime))." }
    $snapshot = $null
    for ($attempt = 0; $attempt -lt 10 -and $null -eq $snapshot; $attempt++) {
        $snapshot = Get-VMSnapshot -VMName $vm.Name -Name $CheckpointName -ErrorAction SilentlyContinue
        if ($null -eq $snapshot) { Start-Sleep -Seconds 3 }
    }
    Add-Assertion $check "checkpoint '$CheckpointName' exists" ($null -ne $snapshot) $(if ($snapshot) { $snapshot.CreationTime } else { 'missing' })

    # Pre-test state.
    Initialize-GuestStage
    if (-not (Test-Path -LiteralPath $preStateDir)) { New-Item -ItemType Directory -Path $preStateDir | Out-Null }
    $state = Get-GuestState $check
    Write-LabJson -Path (Join-Path $preStateDir 'state.json') -InputObject $state -Depth 10 -Force
    Write-LabText -Path (Join-Path $preStateDir 'sc-qc.txt') -Text ((@($state.scQc) + @($state.scQsidtype) + @($state.scQprivs) + @($state.scQfailure) + @($state.scQfailureflag) + @($state.scSdshow)) -join [Environment]::NewLine)
    Write-LabText -Path (Join-Path $preStateDir 'imagepath.txt') -Text ([string]$state.imagePath + [Environment]::NewLine)
    Write-LabJson -Path (Join-Path $preStateDir 'eventsource.json') -InputObject $state.eventSource -Force
    Write-LabJson -Path (Join-Path $preStateDir 'acls.json') -InputObject @($state.folders) -Force
    Write-LabJson -Path (Join-Path $preStateDir 'programdata-listing.json') -InputObject @($state.dataFolder) -Force
    Write-LabJson -Path (Join-Path $preStateDir 'installfolder-listing.json') -InputObject @($state.installFolder) -Force
    Write-LabJson -Path (Join-Path $runDir 'before-L1-datafolder.json') -InputObject @($state.dataFolder) -Force

    $lab = Get-LabValue
    Add-Assertion $check 'saved ImagePath is the lab service (receiver and session name match lab.json)' ($lab.ImagePath -match [regex]::Escape($receiver) -and $lab.ImagePath -match [regex]::Escape($sessionName)) $lab.ImagePath
    Add-Assertion $check 'SERVICE_ARGS derived from the saved ImagePath' ($true) "SERVICE_ARGS=[$($lab.ServiceArgs)]"
    Add-Assertion $check 'event source key was saved' ($null -ne $state.eventSource) $(if ($state.eventSource) { ($state.eventSource.values | ForEach-Object { "$($_.name)($($_.kind))=$($_.data)" }) -join '; ' } else { 'none' })
    Add-Assertion $check 'ProgramData\EtwDnstap content was listed' (@($state.dataFolder).Count -gt 0) ((@($state.dataFolder) | ForEach-Object { "$($_.name)[$($_.fileCount) files]" }) -join ', ')
}

function Invoke-StepL1 {
    $check = 'L1'
    Initialize-GuestStage
    $before = Get-DataFolderName 'before-L1-datafolder.json'
    $remove = Invoke-GuestScript $check 'Remove-ManualEtwDnstap.ps1' @{ SessionName = $sessionName; GrantScript = (Join-Path $script:GuestStage 'Grant-EtwDnstapEtwAccess.ps1') }
    Add-Assertion $check 'Remove-ManualEtwDnstap exit code 0' ($remove.ExitCode -eq 0) "exit=$($remove.ExitCode) last=[$(@($remove.Output)[-1])]"
    if ($remove.ExitCode -ne 0) { throw 'The hand removal failed. Not restarting. See L1-Remove-ManualEtwDnstap.txt.' }
    Invoke-GuestRestart
    Initialize-GuestStage
    # The hand removal leaves ProgramData\EtwDnstap alone, so spool and backup may remain.
    $kept = ($before + 'spool' + 'backup' | Sort-Object -Unique) -join ';'
    $test = Invoke-GuestScript $check 'Test-EtwDnstapRemoved.ps1' @{ SessionName = $sessionName; KeptNames = $kept; UpgradeCode = $upgradeCode }
    Add-GuestAssertion $check $test 'Test-EtwDnstapRemoved'
    $null = Save-DataFolderListing $check 'after-L1-datafolder.json'
}

function Invoke-StepL2 {
    $check = 'L2'
    Initialize-GuestStage
    $kept = (Get-DataFolderName 'after-L1-datafolder.json') -join ';'
    $beforeState = Save-DataFolderListing $check 'before-L2-datafolder.json'
    $null = Invoke-Msiexec $check 'install' '' @(1603)
    $test = Invoke-GuestScript $check 'Test-EtwDnstapRemoved.ps1' @{ SessionName = $sessionName; KeptNames = $kept; UpgradeCode = $upgradeCode }
    Add-GuestAssertion $check $test 'Test-EtwDnstapRemoved'
    $afterState = Save-DataFolderListing $check 'after-L2-datafolder.json'
    $same = (ConvertTo-Json -InputObject @($beforeState.dataFolder) -Depth 5) -ceq (ConvertTo-Json -InputObject @($afterState.dataFolder) -Depth 5)
    Add-Assertion $check 'nothing changed under ProgramData\EtwDnstap' $same ((@($afterState.dataFolder) | ForEach-Object { "$($_.name)[$($_.fileCount)]" }) -join ', ')
}

function Invoke-StepL3 {
    $check = 'L3'
    Initialize-GuestStage
    $afterL1 = Get-DataFolderName 'after-L1-datafolder.json'
    $kept = ($afterL1 | Where-Object { $_ -ne 'spool' -and $_ -ne 'backup' }) -join ';'
    $beforeState = Save-DataFolderListing $check 'before-L3-datafolder.json'
    Write-Log ("[L3] ProgramData\EtwDnstap before: " + ((@($beforeState.dataFolder) | ForEach-Object { "$($_.name)[$($_.fileCount) files, $($_.bytes) bytes]" }) -join ', '))
    $script:StepNotes.Add('before L3: ' + ((@($beforeState.dataFolder) | ForEach-Object { "$($_.name)[$($_.fileCount) files]" }) -join ', '))
    $null = Invoke-Msiexec $check 'install' 'RECEIVER="dnstap-ubuntu:6000"' @(1603)
    $afterState = Save-DataFolderListing $check 'after-L3-datafolder.json'
    Write-Log ("[L3] ProgramData\EtwDnstap after: " + ((@($afterState.dataFolder) | ForEach-Object { "$($_.name)[$($_.fileCount) files]" }) -join ', '))
    $script:StepNotes.Add('after L3: ' + ((@($afterState.dataFolder) | ForEach-Object { "$($_.name)[$($_.fileCount) files]" }) -join ', '))
    $test = Invoke-GuestScript $check 'Test-EtwDnstapRemoved.ps1' @{ SessionName = $sessionName; KeptNames = $kept; UpgradeCode = $upgradeCode }
    Add-GuestAssertion $check $test 'Test-EtwDnstapRemoved'
}

function Invoke-InstalledCheck([string]$StepName) {
    Initialize-GuestStage
    Send-PreState
    $test = Invoke-GuestScript $StepName 'Test-EtwDnstapInstalled.ps1' (Get-InstalledAssertionParameter)
    Add-GuestAssertion $StepName $test 'Test-EtwDnstapInstalled'
    if ($test.Output -match 'service-RUNNING-within-boot-window') { Test-ReceiverDelivery $StepName }
}

function Save-GuestImagePath([string]$FileName) {
    $state = Get-GuestState 'imagepath'
    Write-LabText -Path (Join-Path $runDir $FileName) -Text ([string]$state.imagePath + [Environment]::NewLine)
    return [string]$state.imagePath
}

function Invoke-StepL4 {
    $check = 'L4'
    Initialize-GuestStage
    $null = Invoke-Msiexec $check 'install' (Get-LabProperty) @(3010)
    $null = Save-GuestImagePath 'L4-imagepath.txt'
    Invoke-GuestRestart
    Invoke-InstalledCheck $check
}

function Invoke-StepL5 {
    $check = 'L5'
    Initialize-GuestStage
    $expected = (Get-Content -LiteralPath (Join-Path $runDir 'L4-imagepath.txt') -Raw -Encoding UTF8).Trim()
    $null = Invoke-Msiexec $check 'repair' '' @(0, 3010)
    $after = Save-GuestImagePath 'L5-imagepath.txt'
    Add-Assertion $check 'ImagePath unchanged by the repair' ($after.Trim() -ceq $expected) "after=[$after] before=[$expected]"
    Invoke-GuestRestart
    Invoke-InstalledCheck $check
}

function Get-GuestInstallState {
    $session = Get-GuestSession
    return Invoke-Command -Session $session -ScriptBlock {
        $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Services\EtwDnstap')
        $image = if ($null -ne $key) { [string]$key.GetValue('ImagePath', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) } else { '' }
        if ($null -ne $key) { $key.Dispose() }
        $remembered = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\EtwDnstap\Installer' -ErrorAction SilentlyContinue
        $receiverValue = if ($null -ne $remembered) { [string]$remembered.Receiver } else { '' }
        $status = (Get-Service -Name EtwDnstap -ErrorAction SilentlyContinue)
        [pscustomobject]@{ ImagePath = $image; Receiver = $receiverValue; Service = $(if ($status) { [string]$status.Status } else { 'absent' }) }
    }
}

# L5b: change the receiver on an installed product. Both command forms are tried and the
# effect of each is recorded: the plain form (maintenance mode, no REINSTALL) and the
# form with REINSTALL=ALL REINSTALLMODE=vomus. The receiver is then set back to the lab value.
function Invoke-StepL5b {
    $check = 'L5b'
    Initialize-GuestStage
    $l4Image = (Get-Content -LiteralPath (Join-Path $runDir 'L4-imagepath.txt') -Raw -Encoding UTF8).Trim()
    $newReceiver = "$($config.network.receiver_ip):$([int]$config.network.receiver_port + 1)"
    $forms = @(
        @{ Name = 'plain'; Properties = "RECEIVER=`"$newReceiver`"" },
        @{ Name = 'reinstall'; Properties = "REINSTALL=ALL REINSTALLMODE=vomus RECEIVER=`"$newReceiver`"" }
    )
    $working = $null
    foreach ($form in $forms) {
        $sub = "L5b-$($form.Name)"
        $before = Get-GuestInstallState
        $exit = Invoke-Msiexec $sub 'install' $form.Properties @() -AssertionStep $check
        $after = Get-GuestInstallState
        $logText = [Text.Encoding]::Default.GetString([IO.File]::ReadAllBytes((Join-Path $runDir "$sub-msiexec.log")))
        $ran = [regex]::Matches($logText, '\[EtwDnstap\] Receiver .* accepted').Count -gt 0
        $changed = $after.ImagePath -match [regex]::Escape($newReceiver)
        if ($changed -and $null -eq $working) { $working = $form }
        $observation = "form=$($form.Name) exit=$exit ImagePath-has-new-receiver=$changed ImagePath=[$($after.ImagePath)] registry-Receiver=[$($after.Receiver)] provisioning-install-ran=$ran service=$($after.Service) (before: Receiver=[$($before.Receiver)] service=$($before.Service))"
        Add-Assertion $check "observed $($form.Name) form" $true $observation
        $script:StepNotes.Add($observation)
    }
    if ($null -ne $working) {
        $restoreProps = $working.Properties -replace [regex]::Escape($newReceiver), $receiver
        $null = Invoke-Msiexec 'L5b-restore' 'install' $restoreProps @(0, 3010) -AssertionStep $check
        $script:StepNotes.Add("restore command properties: $restoreProps")
    }
    $final = Get-GuestInstallState
    Add-Assertion $check 'ImagePath equals the L4 value after the restore' ($final.ImagePath.Trim() -ceq $l4Image) "found=[$($final.ImagePath)] L4=[$l4Image]"
    Add-Assertion $check 'remembered Receiver is the lab receiver after the restore' ($final.Receiver -ceq $receiver) $final.Receiver
    $script:StepNotes.Add("service state after L5b: $($final.Service)")
}

function Invoke-StepL6 {
    $check = 'L6'
    Initialize-GuestStage
    $kept = (Get-DataFolderName 'after-L1-datafolder.json' | Where-Object { $_ -ne 'spool' -and $_ -ne 'backup' }) -join ';'
    $beforeState = Save-DataFolderListing $check 'before-L6-datafolder.json'
    $script:StepNotes.Add('before L6: ' + ((@($beforeState.dataFolder) | ForEach-Object { "$($_.name)[$($_.fileCount) files]" }) -join ', '))
    $null = Invoke-Msiexec $check 'uninstall' '' @(3010)
    Invoke-GuestRestart
    Initialize-GuestStage
    $test = Invoke-GuestScript $check 'Test-EtwDnstapRemoved.ps1' @{ SessionName = $sessionName; KeptNames = $kept; UpgradeCode = $upgradeCode }
    Add-GuestAssertion $check $test 'Test-EtwDnstapRemoved'
    $afterState = Save-DataFolderListing $check 'after-L6-datafolder.json'
    $script:StepNotes.Add('after L6: ' + ((@($afterState.dataFolder) | ForEach-Object { "$($_.name)[$($_.fileCount) files]" }) -join ', '))
}

function Invoke-StepL7 {
    $check = 'L7'
    Initialize-GuestStage
    $null = Invoke-Msiexec $check 'install' (Get-LabProperty) @(3010)
    $null = Save-GuestImagePath 'L7-imagepath.txt'
    Invoke-GuestRestart
    Invoke-InstalledCheck $check
}

function Invoke-StepRestore {
    $snapshot = Get-VMSnapshot -VMName $vm.Name -Name $CheckpointName -ErrorAction Stop
    Write-Log "Applying checkpoint '$CheckpointName'."
    Restore-VMSnapshot -VMSnapshot $snapshot -Confirm:$false
    Start-Sleep -Seconds 10
    if ((Get-VM -Name $vm.Name).State -ne 'Running') { Start-VM -Name $vm.Name }
    $script:Session = $null
    $null = Wait-GuestSession -TimeoutSeconds $RebootTimeoutSeconds
    Write-Log 'The guest answers again.'
}

# ---- run --------------------------------------------------------------------------------------
Write-Log "Run folder: $(Split-Path -Leaf $runDir). Steps: $($requested -join ', '). Version $version."
$totalFail = 0
try {
    foreach ($name in $requested) {
        $script:StepPass = 0
        $script:StepFail = 0
        $script:StepNotes = New-Object System.Collections.Generic.List[string]
        $script:StepMsiexec = New-Object System.Collections.Generic.List[object]
        Write-Log "===== $name ====="
        $stepFile = Join-Path $runDir "$name-assertions.txt"
        if (Test-Path -LiteralPath $stepFile) { Remove-Item -LiteralPath $stepFile -Force }
        try {
            if ($name -eq 'Restore') { Invoke-StepRestore; continue }
            & "Invoke-Step$name"
        }
        catch {
            Add-Assertion $name 'step completed without an error' $false $_.Exception.Message
            $script:StepNotes.Add("error: $($_.Exception.Message)")
        }
        Save-StepSummary $name
        Write-Log "[$name] pass=$($script:StepPass) fail=$($script:StepFail)"
        $totalFail += $script:StepFail
        if ($script:StepFail -gt 0) { Write-Log "Stopping after $name. Fix the cause, then rerun from this step with -Step $name."; break }
    }
}
finally {
    if ($null -ne $script:Session) { Remove-PSSession -Session $script:Session -ErrorAction SilentlyContinue }
}
Write-Log "Done. Failed assertions: $totalFail. Results: $(Split-Path -Leaf $runDir)"
if ($totalFail -gt 0) { exit 1 }
exit 0
