#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Starts the lab service with a large --capacity and records the SCM start progress until Running.

.DESCRIPTION
Checks that the service reaches Running when the event pool is large (default --capacity
262144; the maximum is 524288). The pool allocation takes longer than the fixed 15-second wait hint that
builds before the checkpointed startup reported, so the SCM must see the checkpoint advance.

The script appends --capacity to the service binary path, starts the service without waiting,
samples sc.exe queryex (state, checkpoint, wait hint, process ID) every 250 ms until the state
settles, keeps the service running for a few seconds, stops it, and restores the original binary
path. With -CandidatePath it first installs the hash-verified build over the installed
executable (ACL preserved) and restores the original executable afterwards. A service that was
Running is stopped first and started again at the end. The restore runs in a finally block.

The check passes when the service reaches Running inside the startup budget (10 s plus 1 s per
16,384 slots, rounded up), the sampled checkpoint advanced past the first report, and the stop
is clean (Stopped, exit code 0). The receiver is not needed: an unreachable receiver is logged
and retried, it does not fail startup.

Use -WhatIf to run the preflight and print the planned changes without touching the guest.

.PARAMETER Capacity
Value for --capacity (default 262144; the maximum is 524288).
.PARAMETER CandidatePath
Host path of a release build to install for the check. The original executable is restored.
.PARAMETER ExpectedHash
SHA-256 of CandidatePath. Required with CandidatePath.
.PARAMETER ProofName
Output file name without extension (default startup-capacity-proof).
.PARAMETER EvidenceRoot
Repository-relative evidence directory (default target/performance/windows); must stay inside the repository.

.OUTPUTS
<EvidenceRoot>/<ProofName>.json
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [ValidateRange(1024, 524288)][int]$Capacity = 262144,
    [string]$CandidatePath,
    [ValidatePattern('^(|[A-Fa-f0-9]{64})$')][string]$ExpectedHash = '',
    [ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$ProofName = 'startup-capacity-proof',
    [string]$EvidenceRoot = 'target/performance/windows'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'LabCommon.psm1') -Force
$config = Get-LabConfig
$evidence = Resolve-LabPath $EvidenceRoot
New-Item -ItemType Directory -Path $evidence -Force -WhatIf:$false | Out-Null
$serviceName = $config.windows_vm.service_name
$installed = $config.windows_vm.install_exe
$guestBase = "$($config.windows_vm.data_dir)/startup-capacity"
# Mirrors pipeline::startup_budget: 10 s plus 1 s per PoolManager::PROGRESS_INTERVAL slots.
$budgetSeconds = 10 + [math]::Ceiling($Capacity / 16384)
if ($CandidatePath) {
    if (-not $ExpectedHash) { throw 'ExpectedHash is required with CandidatePath' }
    # Get-FileHash returns nothing under -WhatIf in Windows PowerShell 5.1, so hash with .NET (read-only).
    $candidateFull = (Resolve-Path -LiteralPath $CandidatePath).ProviderPath
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($candidateFull)
    try { $candidateHash = -join ($sha.ComputeHash($stream) | ForEach-Object { $_.ToString('X2') }) } finally { $stream.Dispose(); $sha.Dispose() }
    if ($candidateHash -ne $ExpectedHash) { throw "Candidate hash mismatch: $CandidatePath" }
}
$vm = Get-LabVm -RequireRunning
$session = Get-LabSession -Vm $vm
$pre = $null
$newPath = $null
$mutated = $false
$deployed = $false
$pathChanged = $false
$stoppedForCheck = $false
$failure = $null
$run = $null
$stop = $null
$restore = $null
try {
    $pre = Invoke-Command -Session $session -ErrorAction Stop -ArgumentList $serviceName, $installed -ScriptBlock {
        param($serviceName, $installed)
        $ErrorActionPreference = 'Stop'
        $svc = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
        if (-not $svc) { throw "Service $serviceName is not installed" }
        if ($svc.State -notin 'Running', 'Stopped') { throw "Service must be Running or Stopped before the check (state: $($svc.State))" }
        if ($svc.PathName -match '--foreground') { throw 'Unexpected service arguments' }
        $os = Get-CimInstance Win32_OperatingSystem
        [ordered]@{
            path_name = $svc.PathName; start_name = $svc.StartName; state = $svc.State
            binary_sha256 = (Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash
            executable_sddl = (Get-Acl -LiteralPath $installed).Sddl
            free_memory_kib = $os.FreePhysicalMemory; total_memory_kib = $os.TotalVisibleMemorySize
            logical_processors = (Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
            os_version = $os.Version
        }
    }
    # The maximum pool is about 640 MB; keep a margin above it.
    if ($pre.free_memory_kib -lt 1.5MB) { throw "Guest free memory is below 1.5 GiB ($($pre.free_memory_kib) KiB)" }
    $newPath = (($pre.path_name -replace '\s--capacity\s+\d+', '') + " --capacity $Capacity")
    $plan = "Start $serviceName with --capacity $Capacity (budget $budgetSeconds s)" + $(if ($CandidatePath) { ", after installing $ExpectedHash" } else { '' })
    if (-not $PSCmdlet.ShouldProcess("$serviceName on $($vm.Name)", $plan)) {
        [pscustomobject]@{ preflight = $pre; planned_path_name = $newPath; budget_seconds = $budgetSeconds } | ConvertTo-Json -Depth 4
        return
    }
    $mutated = $true
    if ($pre.state -eq 'Running') {
        Invoke-Command -Session $session -ErrorAction Stop -ArgumentList $serviceName -ScriptBlock { param($serviceName) $ErrorActionPreference = 'Stop'; Stop-Service $serviceName; if ((Get-Service $serviceName).Status -ne 'Stopped') { throw 'Service did not stop before the check' } }
        $stoppedForCheck = $true
    }
    if ($CandidatePath) {
        Invoke-Command -Session $session -ErrorAction Stop -ArgumentList $guestBase -ScriptBlock { param($base) $ErrorActionPreference = 'Stop'; New-Item -ItemType Directory -Path $base -Force | Out-Null }
        Copy-Item -LiteralPath $CandidatePath -Destination "$guestBase/candidate.exe" -ToSession $session -Force
        # Step 1: verify the guest copy and take the backup. Nothing installed has changed yet.
        Invoke-Command -Session $session -ErrorAction Stop -ArgumentList $guestBase, $installed, $ExpectedHash, $pre.binary_sha256 -ScriptBlock {
            param($base, $installed, $expected, $originalHash)
            $ErrorActionPreference = 'Stop'
            if ((Get-FileHash -LiteralPath "$base/candidate.exe" -Algorithm SHA256).Hash -ne $expected) { throw 'Guest candidate hash mismatch' }
            if (Test-Path -LiteralPath "$base/original.exe") {
                # A backup left by an earlier run is kept only if it is the executable the preflight saw.
                if ((Get-FileHash -LiteralPath "$base/original.exe" -Algorithm SHA256).Hash -ne $originalHash) { throw "Refusing to overwrite an existing backup that differs from the installed executable: $base/original.exe" }
            } else {
                Copy-Item -LiteralPath $installed -Destination "$base/original.exe"
            }
            if ((Get-FileHash -LiteralPath "$base/original.exe" -Algorithm SHA256).Hash -ne $originalHash) { throw 'Backup of the installed executable failed' }
        }
        # Step 2: overwrite the installed executable. The flag is set BEFORE the call that mutates, so
        # a failure inside it (including the hash/ACL check after the copy) still reaches the restore.
        $deployed = $true
        Invoke-Command -Session $session -ErrorAction Stop -ArgumentList $guestBase, $installed, $ExpectedHash, $pre.executable_sddl -ScriptBlock {
            param($base, $installed, $expected, $sddl)
            $ErrorActionPreference = 'Stop'
            Copy-Item -LiteralPath "$base/candidate.exe" -Destination $installed -Force
            if ((Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash -ne $expected -or (Get-Acl -LiteralPath $installed).Sddl -ne $sddl) { throw 'Deployment hash/ACL mismatch' }
        }
    }
    # Set before the call: Change may take effect even when the remote call then throws. The restore is idempotent.
    $pathChanged = $true
    Invoke-Command -Session $session -ErrorAction Stop -ArgumentList $serviceName, $newPath -ScriptBlock {
        param($serviceName, $path)
        $ErrorActionPreference = 'Stop'
        $svc = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
        $change = Invoke-CimMethod -InputObject $svc -MethodName Change -Arguments @{ PathName = $path }
        if ($change.ReturnValue) { throw "Change PathName returned $($change.ReturnValue)" }
    }
    $run = Invoke-Command -Session $session -ErrorAction Stop -ArgumentList $serviceName, $budgetSeconds -ScriptBlock {
        param($serviceName, $budgetSeconds)
        $ErrorActionPreference = 'Stop'
        function Read-ScmStatus {
            param($name)
            $ErrorActionPreference = 'Stop'
            $text = (& sc.exe queryex $name | Out-String)
            $row = [ordered]@{ state = $null; checkpoint = $null; wait_hint_ms = $null; pid = $null }
            if ($text -match 'STATE\s*:\s*\d+\s+(\w+)') { $row.state = $Matches[1] }
            if ($text -match 'CHECKPOINT\s*:\s*0x([0-9a-fA-F]+)') { $row.checkpoint = [Convert]::ToInt64($Matches[1], 16) }
            if ($text -match 'WAIT_HINT\s*:\s*0x([0-9a-fA-F]+)') { $row.wait_hint_ms = [Convert]::ToInt64($Matches[1], 16) }
            if ($text -match 'PID\s*:\s*(\d+)') { $row.pid = [int]$Matches[1] }
            $row
        }
        $svc = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
        $started = [DateTime]::UtcNow
        $start = Invoke-CimMethod -InputObject $svc -MethodName StartService
        if ($start.ReturnValue -ne 0) { throw "StartService returned $($start.ReturnValue)" }
        $deadline = $started.AddSeconds($budgetSeconds + 10)
        $samples = @()
        $running = $null
        $last = $null
        while ($true) {
            $status = Read-ScmStatus -name $serviceName
            $elapsed = [int]([DateTime]::UtcNow - $started).TotalMilliseconds
            $last = $status.state
            $samples += [pscustomobject]@{ elapsed_ms = $elapsed; state = $status.state; checkpoint = $status.checkpoint; wait_hint_ms = $status.wait_hint_ms; pid = $status.pid }
            if ($status.state -eq 'RUNNING') { $running = $elapsed; break }
            if ($status.state -eq 'STOPPED' -or [DateTime]::UtcNow -gt $deadline) { break }
            Start-Sleep -Milliseconds 250
        }
        if ($null -ne $running) { Start-Sleep -Seconds 3 }
        $after = Read-ScmStatus -name $serviceName
        $pending = @($samples | Where-Object { $_.state -eq 'START_PENDING' })
        $maxCheckpoint = 0
        if ($pending.Count) { $maxCheckpoint = ($pending | Measure-Object -Property checkpoint -Maximum).Maximum }
        $events = @(Get-WinEvent -FilterHashtable @{ LogName = 'Application'; ProviderName = $serviceName; StartTime = $started.ToLocalTime() } -ErrorAction SilentlyContinue |
            Select-Object TimeCreated, Id, LevelDisplayName, Message)
        [ordered]@{
            started_utc = $started.ToString('o'); budget_seconds = $budgetSeconds
            reached_running = ($null -ne $running); running_elapsed_ms = $running; last_sampled_state = $last
            max_start_pending_checkpoint = $maxCheckpoint
            start_pending_wait_hints_ms = @($pending | ForEach-Object { $_.wait_hint_ms } | Select-Object -Unique)
            state_after_settle = $after.state
            samples = $samples; events = $events
        }
    }
    if (-not $run.reached_running) { throw "Service did not reach Running (last state: $($run.last_sampled_state))" }
    if ($run.running_elapsed_ms -gt $budgetSeconds * 1000) { throw "Running after $($run.running_elapsed_ms) ms, above the $budgetSeconds s startup budget" }
    if ($run.max_start_pending_checkpoint -lt 2) { throw "The START_PENDING checkpoint did not advance (maximum sampled: $($run.max_start_pending_checkpoint))" }
    if ($run.state_after_settle -ne 'RUNNING') { throw "Service left Running shortly after start (state: $($run.state_after_settle))" }
} catch {
    $failure = $_.ToString()
} finally {
    if ($mutated) {
        try {
            $stop = Invoke-Command -Session $session -ErrorAction Stop -ArgumentList $serviceName -ScriptBlock {
                param($serviceName)
                $ErrorActionPreference = 'Stop'
                Stop-Service $serviceName -ErrorAction SilentlyContinue
                Get-CimInstance Win32_Service -Filter "Name='$serviceName'" | Select-Object Name, State, ExitCode, ServiceSpecificExitCode, ProcessId
            }
            # No receiver takes part in this check, so there is no STOP/FINISH to confirm (receiver_stop_finish is
            # not-checked). An unreachable receiver must not fail the stop: the service has to reach Stopped with
            # exit code 0 regardless, and that is the whole stop requirement here.
            if ($run -and ($stop.State -ne 'Stopped' -or $stop.ExitCode -ne 0)) { $failure = "$failure; SCM stop not clean (state $($stop.State), exit code $($stop.ExitCode))".TrimStart('; ') }
        } catch { $failure = "$failure; SCM stop: $_".TrimStart('; ') }
        try {
            if ($pathChanged -or $deployed) {
                $restore = Invoke-Command -Session $session -ErrorAction Stop -ArgumentList $serviceName, $installed, $guestBase, $pre.path_name, $pre.binary_sha256, $pre.executable_sddl, $pathChanged, $deployed -ScriptBlock {
                    param($serviceName, $installed, $base, $originalPath, $originalHash, $sddl, $restorePath, $restoreBinary)
                    $ErrorActionPreference = 'Stop'
                    if ($restorePath) {
                        $svc = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
                        $change = Invoke-CimMethod -InputObject $svc -MethodName Change -Arguments @{ PathName = $originalPath }
                        if ($change.ReturnValue) { throw "Restore PathName returned $($change.ReturnValue)" }
                    }
                    if ($restoreBinary) {
                        # Idempotent: copy the original back only if the backup exists. Without a backup the
                        # installed executable was never replaced, and the check below still has to match.
                        if (Test-Path -LiteralPath "$base/original.exe") {
                            Copy-Item -LiteralPath "$base/original.exe" -Destination $installed -Force
                        }
                        if ((Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash -ne $originalHash -or (Get-Acl -LiteralPath $installed).Sddl -ne $sddl) { throw 'Restore of the installed executable failed' }
                        Remove-Item -LiteralPath "$base/candidate.exe", "$base/original.exe" -Force -ErrorAction SilentlyContinue
                    }
                    $svc = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
                    [ordered]@{ path_name = $svc.PathName; binary_sha256 = (Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash; executable_sddl = (Get-Acl -LiteralPath $installed).Sddl }
                }
                if ($restore.path_name -ne $pre.path_name -or $restore.binary_sha256 -ne $pre.binary_sha256 -or $restore.executable_sddl -ne $pre.executable_sddl) { throw 'Service configuration differs from the preflight after the restore' }
            }
            if ($stoppedForCheck) {
                Invoke-Command -Session $session -ErrorAction Stop -ArgumentList $serviceName -ScriptBlock { param($serviceName) $ErrorActionPreference = 'Stop'; Start-Service $serviceName }
            }
        } catch { $failure = "$failure; restore: $_".TrimStart('; ') }
    }
    Remove-PSSession $session -WhatIf:$false
    if ($mutated -or $failure) {
        $proof = [ordered]@{
            utc = [DateTime]::UtcNow.ToString('o'); vm = $vm | Select-Object Name, Id, State, ProcessorCount, MemoryAssigned
            capacity = $Capacity; budget_seconds = $budgetSeconds
            candidate_sha256 = $(if ($CandidatePath) { $ExpectedHash } else { $null })
            preflight = $pre; path_name_under_test = $newPath; run = $run; stop = $stop; restore = $restore
            receiver_stop_finish = 'not-checked'; restarted_original = $stoppedForCheck; failure = $failure; passed = ($null -eq $failure)
        }
        Write-LabJson -Path (Join-Path $evidence "$ProofName.json") -InputObject $proof -Depth 8 -Force
    }
}
if ($failure) { throw $failure }
if (-not $mutated) { return }
$run.samples | Format-Table elapsed_ms, state, checkpoint, wait_hint_ms, pid -AutoSize
Write-Output "STARTUP_CAPACITY_CHECK_PASSED capacity=$Capacity running_after_ms=$($run.running_elapsed_ms) max_checkpoint=$($run.max_start_pending_checkpoint) budget_s=$budgetSeconds"
