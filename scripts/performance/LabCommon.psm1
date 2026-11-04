#requires -Version 5.1
<#
.SYNOPSIS
Shared helpers for the etw-dnstap performance lab scripts.

.DESCRIPTION
Centralizes the boilerplate that every windows-*.ps1 script used to repeat: lab
constants (lab.json), Hyper-V VM identity checks, the PowerShell Direct session
and credential, hash-verified file retrieval, uniform zone-ownership handling
and DNS answer checks.

Compatibility: Windows PowerShell 5.1 and PowerShell 7 on Windows. The scripts
target 5.1 because the Hyper-V module is Windows PowerShell only (under 7 it
runs through the implicit compatibility session and returns deserialized
objects, which breaks Remove-VMHardDiskDrive and VM object comparisons).

Files written by the host-side helpers are UTF-8 without a BOM on both
editions. Guest-side Set-Content calls use an explicit -Encoding.
#>
Set-StrictMode -Version Latest

$script:LabRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$script:LabConfig = $null
$script:Utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function Get-LabRoot {
    <#
    .SYNOPSIS
    Returns the repository root (two levels above scripts/performance).
    #>
    [CmdletBinding()] param()
    $script:LabRoot
}

function Get-LabConfig {
    <#
    .SYNOPSIS
    Loads scripts/performance/lab.json (cached). Use -Path to override, -Reload to refresh.
    #>
    [CmdletBinding()]
    param([string]$Path = (Join-Path $PSScriptRoot 'lab.json'), [switch]$Reload)
    if ($script:LabConfig -and -not $Reload) { return $script:LabConfig }
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Lab configuration not found: $Path" }
    $script:LabConfig = Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
    $script:LabConfig
}

function Resolve-LabPath {
    <#
    .SYNOPSIS
    Resolves a repository-relative path (it does not have to exist) and keeps it inside the repository.
    #>
    [CmdletBinding()] param([Parameter(Mandatory)][string]$RelativePath)
    $full = [IO.Path]::GetFullPath([IO.Path]::Combine($script:LabRoot, $RelativePath))
    $prefix = $script:LabRoot.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $full.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw "Path must stay within the repository: $RelativePath" }
    $full
}

function Write-LabText {
    <#
    .SYNOPSIS
    Writes text as UTF-8 without BOM (identical on Windows PowerShell 5.1 and 7).
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][AllowEmptyString()][string]$Text, [switch]$Append)
    $full = [IO.Path]::GetFullPath($Path)
    if ($Append) { [IO.File]::AppendAllText($full, $Text, $script:Utf8NoBom) } else { [IO.File]::WriteAllText($full, $Text, $script:Utf8NoBom) }
}

function Write-LabJson {
    <#
    .SYNOPSIS
    Serializes an object to JSON and writes it as UTF-8 without BOM. Refuses to overwrite unless -Force.
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)]$InputObject, [int]$Depth = 8, [switch]$Force)
    if ((Test-Path -LiteralPath $Path) -and -not $Force) { throw "Preserve existing evidence: $Path" }
    Write-LabText -Path $Path -Text ((ConvertTo-Json -InputObject $InputObject -Depth $Depth) + [Environment]::NewLine)
}

function Get-LabCredential {
    <#
    .SYNOPSIS
    Imports the guest administrator credential, with a helpful message if the file is missing.
    #>
    [CmdletBinding()] param()
    $path = Resolve-LabPath (Get-LabConfig).windows_vm.credential_path
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw ("Credential file not found: $path`n" +
            "Create it once, as the same Windows user that runs these scripts (DPAPI-protected, not portable):`n" +
            "  New-Item -ItemType Directory -Force (Split-Path '$path') | Out-Null`n" +
            "  Get-Credential Administrator | Export-Clixml -LiteralPath '$path'")
    }
    Import-Clixml -LiteralPath $path
}

function Get-LabVm {
    <#
    .SYNOPSIS
    Returns the Windows lab VM after verifying its identity (GUID) and, optionally, state/resources.
    #>
    [CmdletBinding()]
    param([switch]$RequireRunning, [switch]$CheckResources, [switch]$RequireFixedMemory,
          [ValidateSet('Windows', 'Ubuntu')][string]$Which = 'Windows')
    $config = Get-LabConfig
    $spec = if ($Which -eq 'Windows') { $config.windows_vm } else { $config.ubuntu_vm }
    $vm = Get-VM -Name $spec.name -ErrorAction Stop
    if ($vm.Id -ne [guid]$spec.id) { throw "Unexpected VM identity for $($spec.name): $($vm.Id)" }
    if ($RequireRunning -and $vm.State -ne 'Running') { throw "VM $($spec.name) is not running (state: $($vm.State))" }
    if ($CheckResources -and ($vm.ProcessorCount -ne $spec.processor_count -or $vm.MemoryAssigned -ne $spec.memory_bytes)) { throw 'VM resources changed' }
    if ($RequireFixedMemory -and $vm.DynamicMemoryEnabled) { throw 'VM must use fixed memory' }
    $vm
}

function Get-LabSession {
    <#
    .SYNOPSIS
    Opens a PowerShell Direct session to the Windows lab VM. Callers must Remove-PSSession it.
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory)]$Vm)
    New-PSSession -VMId $Vm.Id -Credential (Get-LabCredential) -ErrorAction Stop
}

function Get-LabGuestPrelude {
    <#
    .SYNOPSIS
    Returns the guest-side helper functions injected by Invoke-LabGuest.
    #>
    [CmdletBinding()] param()
    @'
$ErrorActionPreference = 'Stop'
function Get-LabOwnedZones {
    # Union of the uniform marker and the legacy per-script markers written by earlier campaigns.
    param([Parameter(Mandatory)][string]$Base)
    $owned = @()
    foreach ($name in 'owned-zones.txt', 'owned-rec-forwarder.txt', 'owned-forwarder.txt') {
        $marker = Join-Path $Base $name
        if (Test-Path -LiteralPath $marker) { $owned += @(Get-Content -LiteralPath $marker | ForEach-Object { $_.Trim() } | Where-Object { $_ }) }
    }
    @($owned | Select-Object -Unique)
}
function Add-LabOwnedZone {
    param([Parameter(Mandatory)][string]$Base, [Parameter(Mandatory)][string]$Zone)
    $marker = Join-Path $Base 'owned-zones.txt'
    $current = @(); if (Test-Path -LiteralPath $marker) { $current = @(Get-Content -LiteralPath $marker) }
    if ($current -notcontains $Zone) { @($current + $Zone) | Set-Content -LiteralPath $marker -Encoding UTF8 }
}
function Remove-LabOwnedZone {
    # The ONLY place that removes a DNS zone: the zone must be recorded as owned by this campaign.
    param([Parameter(Mandatory)][string]$Base, [Parameter(Mandatory)][string]$Zone, [switch]$SkipIfNotOwned)
    $owned = @(Get-LabOwnedZones -Base $Base)
    if ($owned -notcontains $Zone) {
        if ($SkipIfNotOwned) { return $false }
        throw "Zone '$Zone' is not listed in $Base/owned-zones.txt (or a legacy ownership marker); refusing to remove it."
    }
    if (@(Get-DnsServerZone | Where-Object ZoneName -eq $Zone).Count -eq 0) { return $false }
    Remove-DnsServerZone -Name $Zone -Force
    $remaining = @($owned | Where-Object { $_ -ne $Zone })
    $remaining | Set-Content -LiteralPath (Join-Path $Base 'owned-zones.txt') -Encoding UTF8
    Add-Content -LiteralPath (Join-Path $Base 'removed-zones.txt') -Value $Zone -Encoding UTF8
    foreach ($legacy in 'owned-rec-forwarder.txt', 'owned-forwarder.txt') {
        $marker = Join-Path $Base $legacy
        if (Test-Path -LiteralPath $marker) { Move-Item -LiteralPath $marker -Destination (Join-Path $Base ("removed-" + $legacy)) -Force }
    }
    $true
}
function Test-LabAnswerLocal {
    param([Parameter(Mandatory)][string]$Name, [string]$Server = '127.0.0.1', [Parameter(Mandatory)][string]$Address,
          [Parameter(Mandatory)][int]$Ttl)
    $answer = @(Resolve-DnsName -Name $Name -Server $Server -Type A -DnsOnly)
    $matching = @($answer | Where-Object { $_.Type -eq 'A' -and $_.IPAddress -eq $Address -and $_.TTL -eq $Ttl })
    [pscustomobject]@{ passed = ($matching.Count -eq 1); answers = @($answer | Select-Object Name, Type, TTL, IPAddress) }
}
'@
}

function Invoke-LabGuest {
    <#
    .SYNOPSIS
    Runs a script block in the guest with the lab helper functions (Remove-LabOwnedZone, ...) available.
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory)][Management.Automation.Runspaces.PSSession]$Session,
          [Parameter(Mandatory)][scriptblock]$ScriptBlock, [object[]]$ArgumentList = @())
    # The inner arguments travel as ONE array element of the outer list. After the CLIXML round trip
    # the wrapper re-splats that array, so the inner script block receives separate positional
    # parameters. @($innerArguments) keeps a single deserialized value from collapsing the splat.
    $wrapper = {
        param($prelude, $inner, $innerArguments)
        . ([scriptblock]::Create($prelude))
        $innerList = @($innerArguments)
        & ([scriptblock]::Create($inner)) @innerList
    }
    $outer = [object[]]::new(3)
    $outer[0] = Get-LabGuestPrelude
    $outer[1] = $ScriptBlock.ToString()
    $outer[2] = $ArgumentList
    Invoke-Command -Session $Session -ScriptBlock $wrapper -ArgumentList $outer -ErrorAction Stop
}

function Receive-LabFile {
    <#
    .SYNOPSIS
    Copies one guest file to the host with Copy-Item -FromSession and verifies its SHA-256.
    .DESCRIPTION
    An identical existing destination is kept; a different existing destination is never
    overwritten (preserve evidence) unless -Overwrite is given. A bad copy is removed.
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory)][Management.Automation.Runspaces.PSSession]$Session,
          [Parameter(Mandatory)][string]$GuestPath, [Parameter(Mandatory)][string]$Destination,
          [string]$ExpectedSha256, [switch]$Overwrite)
    if (-not $ExpectedSha256) {
        $ExpectedSha256 = Invoke-Command -Session $Session -ArgumentList $GuestPath -ErrorAction Stop -ScriptBlock { param($p) (Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash }
    }
    if (Test-Path -LiteralPath $Destination) {
        if ((Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash -eq $ExpectedSha256) { return [pscustomobject]@{ Path = $Destination; Sha256 = $ExpectedSha256; Copied = $false } }
        if (-not $Overwrite) { throw "Preserve existing mismatched evidence: $Destination" }
    }
    try { Copy-Item -LiteralPath $GuestPath -Destination $Destination -FromSession $Session -Force -ErrorAction Stop }
    catch {
        # Large files occasionally report a transport error after a complete copy; the hash check below is authoritative.
        if (-not (Test-Path -LiteralPath $Destination)) { throw }
    }
    $actual = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash
    if ($actual -ne $ExpectedSha256) {
        Remove-Item -LiteralPath $Destination -Force
        throw "Hash mismatch after copying $GuestPath (expected $ExpectedSha256, got $actual)"
    }
    [pscustomobject]@{ Path = $Destination; Sha256 = $actual; Copied = $true }
}

function Receive-LabDirectory {
    <#
    .SYNOPSIS
    Hash-verified copy of every file in a guest directory (non-recursive), skipping some extensions.
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory)][Management.Automation.Runspaces.PSSession]$Session,
          [Parameter(Mandatory)][string]$GuestDirectory, [Parameter(Mandatory)][string]$Destination,
          [string[]]$ExcludeExtension = @('.exe'), [string[]]$ExcludeName = @(), [switch]$Overwrite)
    $files = @(Invoke-Command -Session $Session -ArgumentList $GuestDirectory, $ExcludeExtension -ErrorAction Stop -ScriptBlock {
        param($dir, $skip)
        Get-ChildItem -LiteralPath $dir -File | Where-Object { $skip -notcontains $_.Extension } |
            Select-Object Name, Length, @{ n = 'Sha256'; e = { (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash } }
    })
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    foreach ($file in $files) {
        if ($ExcludeName -contains $file.Name) { continue }
        $null = Receive-LabFile -Session $Session -GuestPath "$GuestDirectory/$($file.Name)" -Destination (Join-Path $Destination $file.Name) -ExpectedSha256 $file.Sha256 -Overwrite:$Overwrite
    }
    $files
}

function Initialize-LabDnsZones {
    <#
    .SYNOPSIS
    Creates the lab zones in the guest and records ownership in <BaseDirectory>/owned-zones.txt.
    .DESCRIPTION
    Refuses to touch a zone that already exists (no ownership can be established). The
    authoritative zone is only created when -AuthoritativeZoneFile is given.
    #>
    [CmdletBinding(SupportsShouldProcess)]
    param([Parameter(Mandatory)][Management.Automation.Runspaces.PSSession]$Session,
          [Parameter(Mandatory)][string]$BaseDirectory, [Parameter(Mandatory)][string]$UpstreamAddress,
          [string]$AuthoritativeZoneFile)
    $dns = (Get-LabConfig).dns
    if (-not $PSCmdlet.ShouldProcess("$($dns.rec_zone) conditional forwarder -> $UpstreamAddress", 'Create DNS zones')) { return }
    Invoke-LabGuest -Session $Session -ArgumentList $BaseDirectory, $dns.auth_zone, $dns.rec_zone, $UpstreamAddress, $AuthoritativeZoneFile -ScriptBlock {
        param($base, $authZone, $recZone, $upstream, $zoneFile)
        foreach ($zone in @($recZone) + $(if ($zoneFile) { @($authZone) } else { @() })) {
            if (Get-DnsServerZone -Name $zone -ErrorAction SilentlyContinue) { throw "Zone $zone already exists; no ownership established" }
        }
        if ($zoneFile) { Add-DnsServerPrimaryZone -Name $authZone -ZoneFile $zoneFile -LoadExisting -DynamicUpdate None; Add-LabOwnedZone -Base $base -Zone $authZone }
        Add-DnsServerConditionalForwarderZone -Name $recZone -MasterServers $upstream -UseRecursion:$false
        Add-LabOwnedZone -Base $base -Zone $recZone
    }
}

function Invoke-LabNative {
    <#
    .SYNOPSIS
    Runs a native command from an argument array and throws on a non-zero exit code.
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$FilePath, [string[]]$ArgumentList = @())
    & $FilePath @ArgumentList
    if ($LASTEXITCODE -ne 0) { throw "$FilePath failed with exit code $LASTEXITCODE" }
}

function Join-LabArguments {
    <#
    .SYNOPSIS
    Quotes an argument array for Start-Process -ArgumentList (Windows PowerShell 5.1 does not quote elements).
    #>
    [CmdletBinding()] param([Parameter(Mandatory)][string[]]$Arguments)
    @($Arguments | ForEach-Object { if ($_ -match '[\s"]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ } }) -join ' '
}

function Get-LabServiceCommandLine {
    <#
    .SYNOPSIS
    Builds the SCM command line for a measured run from the lab constants (no string concatenation in callers).
    #>
    [CmdletBinding()]
    param([string[]]$ExtraArguments = @())
    $config = Get-LabConfig
    $exe = ($config.windows_vm.install_exe -replace '/', '\')
    $arguments = @('--receiver', "$($config.network.receiver_ip):$($config.network.receiver_port)", '--session-name', $config.windows_vm.session_name) + $ExtraArguments
    '"' + $exe + '" ' + (Join-LabArguments -Arguments $arguments)
}

function Get-LabSshArguments {
    # ssh.exe arguments shared by the receiver checks (config and alias come from lab.json).
    [CmdletBinding()] param()
    $config = Get-LabConfig
    @('-T', '-o', 'BatchMode=yes', '-F', (Resolve-LabPath $config.ubuntu_vm.ssh_config), $config.ubuntu_vm.ssh_alias)
}

function Assert-LabReceiverStopCaptureReady {
    <#
    .SYNOPSIS
    Read-only receiver preflight for the STOP/FINISH capture: python on the host, tcpdump on the receiver,
    and passwordless sudo. Throws when any is missing so the check can never pass by not running.
    #>
    [CmdletBinding()] param()
    if (-not (Get-Command python -ErrorAction SilentlyContinue)) { throw 'python is required on the host for the receiver STOP/FINISH check' }
    $ErrorActionPreference = 'Continue'  # ssh stderr text must not become a terminating error in 5.1
    $output = @(& ssh.exe @(Get-LabSshArguments) 'command -v tcpdump >/dev/null && sudo -n true && echo RECEIVER_STOP_CAPTURE_READY' 2>&1)
    if ($LASTEXITCODE -ne 0 -or ($output -join "`n") -notmatch 'RECEIVER_STOP_CAPTURE_READY') {
        throw "Receiver STOP/FINISH capture is not possible (tcpdump missing, or sudo needs a password): $($output -join ' ')"
    }
}

function Start-LabReceiverStopCapture {
    <#
    .SYNOPSIS
    Starts the receiver-side STOP/FINISH capture and returns its state. Call it just before stopping the service.
    .DESCRIPTION
    Runs, over ssh on the receiver, a tcpdump limited to TCP port 6000 packets with a 1 to 16 byte payload or a
    FIN/RST flag (the exact command is printed by receiver_stop_finish.py --print-command). Its text output goes
    to stdout and into <OutDirectory>/capture.txt on the host. The receiver writes no pcap and no stream data.
    Throws when the capture is not listening within 20 seconds; the capture is stopped first.
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$OutDirectory, [Parameter(Mandatory)][string]$ServiceIp)
    $command = (& python (Join-Path $PSScriptRoot 'receiver_stop_finish.py') --print-command | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $command) { throw 'Could not read the capture command from receiver_stop_finish.py' }
    New-Item -ItemType Directory -Path $OutDirectory -Force | Out-Null
    $state = [pscustomobject]@{
        Process = $null; OutDirectory = $OutDirectory; ServiceIp = $ServiceIp; Command = $command
        StartedUtc = [DateTime]::UtcNow.ToString('o'); Listening = $false; Stopped = $false
    }
    $arguments = @(Get-LabSshArguments) + @($command)
    $state.Process = Start-Process -FilePath 'ssh.exe' -ArgumentList (Join-LabArguments -Arguments $arguments) -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $OutDirectory 'capture.txt') -RedirectStandardError (Join-Path $OutDirectory 'capture-stderr.txt')
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while ([DateTime]::UtcNow -lt $deadline -and -not $state.Listening -and -not $state.Process.HasExited) {
        Start-Sleep -Milliseconds 250
        $stream = $null
        try {
            $stream = [IO.File]::Open((Join-Path $OutDirectory 'capture-stderr.txt'), 'Open', 'Read', 'ReadWrite')
            $text = (New-Object IO.StreamReader($stream)).ReadToEnd()
            if ($text -match 'listening on') { $state.Listening = $true }
        } catch { } finally { if ($stream) { $stream.Dispose() } }
    }
    if (-not $state.Listening) {
        $null = Stop-LabReceiverStopCapture -State $state -SettleSeconds 0
        throw 'Receiver STOP/FINISH capture did not start listening within 20 seconds'
    }
    $state
}

function Stop-LabReceiverStopCapture {
    <#
    .SYNOPSIS
    Stops the capture (always, including after errors), cleans up the remote tcpdump and writes the capture receipt.
    Never throws. Returns the receipt, which receiver_stop_finish.py turns into confirmed / not-seen.
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory)]$State, [int]$SettleSeconds = 5)
    if ($SettleSeconds -gt 0 -and $State.Listening) { Start-Sleep -Seconds $SettleSeconds }
    try { if ($State.Process -and -not $State.Process.HasExited) { Stop-Process -Id $State.Process.Id -Force -ErrorAction SilentlyContinue } } catch { }
    try {
        # The remote `timeout` bounds the capture if this fails; this removes it now. The [t] keeps
        # the pattern from matching the remote shell that carries this very command line.
        $ErrorActionPreference = 'Continue'
        $kill = "sudo -n pkill -INT -f '[t]cpdump -i any -l -nn -tt -x -s 128' || true"
        $null = & ssh.exe @(Get-LabSshArguments) $kill 2>&1
    } catch { }
    $State.Stopped = $true
    $receipt = [ordered]@{
        schema = 1; service_ip = $State.ServiceIp; command = $State.Command; capture_file = 'capture.txt'
        started_utc = $State.StartedUtc; stopped_utc = [DateTime]::UtcNow.ToString('o'); listening_confirmed = [bool]$State.Listening
    }
    try { Write-LabJson -Path (Join-Path $State.OutDirectory 'receiver-stop-capture.json') -InputObject $receipt -Force } catch { }
    $receipt
}

Export-ModuleMember -Function Get-LabRoot, Get-LabConfig, Resolve-LabPath, Write-LabText, Write-LabJson,
    Get-LabVm, Get-LabSession, Invoke-LabGuest, Receive-LabFile,
    Receive-LabDirectory, Initialize-LabDnsZones, Invoke-LabNative, Join-LabArguments, Get-LabServiceCommandLine,
    Assert-LabReceiverStopCaptureReady, Start-LabReceiverStopCapture, Stop-LabReceiverStopCapture
