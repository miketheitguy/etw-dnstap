#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Historical pooled-VM WPR recording helper (probe/start/status/stop/fetch) for the Windows VM.

.DESCRIPTION
Starts and stops a task-owned WPR recording in the guest from pooled-vm.wprp, never
cancelling an unrelated recording, and fetches the artifacts with hash verification. The
campaign directory that used to be baked in is now the mandatory -CampaignId.

.PARAMETER Action
Probe, Start, Status, Stop or Fetch.
.PARAMETER CampaignId
Campaign directory name (guest: <data_dir>/<CampaignId>/<RunId>-wpr; host: target/performance/<CampaignId>/capture/<RunId>).
.PARAMETER RunId
Run label.
.PARAMETER CompressTransfer
Compress the ETL in the guest and verify a lossless round trip on the host.

.OUTPUTS
Fetched WPR artifacts, manifest.json and (with -CompressTransfer) transfer-roundtrip.json.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Probe','Start','Status','Stop','Fetch')][string]$Action,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$CampaignId,
    [ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$RunId = 'probe',
    [switch]$CompressTransfer
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '../LabCommon.psm1') -Force
$config = Get-LabConfig
$out = Resolve-LabPath "$($config.paths.evidence_root)/$CampaignId/capture/$RunId"
New-Item -ItemType Directory -Path $out -Force | Out-Null
if ($Action -in 'Probe','Start' -and (Get-PSDrive C).Free -lt 20GB) { throw 'Host disk guard' }
$vm = Get-LabVm -RequireRunning -CheckResources
$session = Get-LabSession -Vm $vm
try {
    $directory = "$($config.windows_vm.data_dir)/$CampaignId/$RunId-wpr"
    if ($Action -in 'Probe','Start') {
        Invoke-LabGuest -Session $session -ArgumentList $directory -ScriptBlock {
            param($dir)
            if (Test-Path -LiteralPath $dir) { throw 'WPR directory exists' }
            if ((Get-Volume C).SizeRemaining -lt 8GB) { throw 'Guest disk guard' }
            New-Item -ItemType Directory -Path "$dir/temp" | Out-Null
        }
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot '../pooled-vm.wprp') -Destination "$directory/profile.wprp" -ToSession $session
    }
    Invoke-LabGuest -Session $session -ArgumentList $Action,$directory,$RunId,([bool]$CompressTransfer) -ScriptBlock {
        param($action,$dir,$run,$compressTransfer)
        $ErrorActionPreference = 'Stop'
        $profile = "$dir/profile.wprp!PooledVM"
        function Record-Wpr {
            param([string]$Name,[string[]]$ToolArgs,[switch]$AllowFailure)
            $priorPreference=$ErrorActionPreference
            $ErrorActionPreference='Continue'
            & wpr.exe @ToolArgs 2>&1 | Out-File -LiteralPath "$dir/$Name.txt" -Encoding utf8
            $code = $LASTEXITCODE
            $ErrorActionPreference=$priorPreference
            $code | Set-Content "$dir/$Name-exit.txt" -Encoding UTF8
            if ($code -ne 0 -and -not $AllowFailure) { throw "WPR $Name failed ($code)" }
        }
        if ($action -in 'Probe','Start') {
            Record-Wpr 'status-before' @('-status')
            if ((Get-Content "$dir/status-before.txt" -Raw) -notmatch 'not recording') { throw 'Preserve unrelated active WPR recording' }
            (Get-Item (Get-Command wpr.exe).Source).VersionInfo | Select-Object FileVersion,ProductVersion | ConvertTo-Json | Set-Content "$dir/wpr-version.json" -Encoding UTF8
            Record-Wpr 'profint' @('-profint')
            Record-Wpr 'profiles' @('-profiles',"$dir/profile.wprp")
            Record-Wpr 'profiledetails' @('-profiledetails',$profile,'-filemode')
            [DateTime]::UtcNow.ToString('o') | Set-Content "$dir/start-begin-utc.txt" -Encoding UTF8
            Record-Wpr 'start' @('-start',$profile,'-filemode','-recordtempto',"$dir/temp")
            [DateTime]::UtcNow.ToString('o') | Set-Content "$dir/start-end-utc.txt" -Encoding UTF8
            Record-Wpr 'status-active' @('-status','collectors','-details')
            'PooledVM' | Set-Content "$dir/owned-recording.txt" -Encoding UTF8
        }
        if ($action -eq 'Probe') { Start-Sleep -Seconds 10 }
        if ($action -in 'Probe','Stop') {
            if ((Get-Content "$dir/owned-recording.txt") -ne 'PooledVM') { throw 'Missing task ownership marker' }
            Record-Wpr 'status-prestop' @('-status','collectors','-details')
            [DateTime]::UtcNow.ToString('o') | Set-Content "$dir/stop-begin-utc.txt" -Encoding UTF8
            Record-Wpr 'stop' @('-stop',"$dir/trace.etl","EtwDnstap pooled VM $run native recording",'-skipPdbGen')
            [DateTime]::UtcNow.ToString('o') | Set-Content "$dir/stop-end-utc.txt" -Encoding UTF8
            Record-Wpr 'status-final' @('-status')
            if ($compressTransfer) {
                $tracePath="$dir/trace.etl"
                $compressedPath="$dir/trace.etl.gz"
                if (Test-Path -LiteralPath $compressedPath) { throw 'Preserve existing compressed trace' }
                $began=[DateTime]::UtcNow
                $originalHash=(Get-FileHash -LiteralPath $tracePath).Hash
                $inputStream=[IO.File]::OpenRead($tracePath)
                $outputStream=[IO.File]::Open($compressedPath,[IO.FileMode]::CreateNew)
                $gzip=$null
                try {
                    $gzip=New-Object IO.Compression.GZipStream($outputStream,[IO.Compression.CompressionLevel]::Fastest)
                    $inputStream.CopyTo($gzip,1MB)
                } finally {
                    if ($gzip) { $gzip.Dispose() }
                    $outputStream.Dispose()
                    $inputStream.Dispose()
                }
                if ((Get-FileHash -LiteralPath $tracePath).Hash -ne $originalHash) { throw 'Guest original trace changed during compression' }
                [ordered]@{method='System.IO.Compression.GZipStream Fastest; after recording stop';begin_utc=$began.ToString('o');end_utc=[DateTime]::UtcNow.ToString('o');original_sha256=$originalHash;original_bytes=(Get-Item $tracePath).Length;compressed_sha256=(Get-FileHash -LiteralPath $compressedPath).Hash;compressed_bytes=(Get-Item $compressedPath).Length;guest_original_preserved=$true} | ConvertTo-Json | Set-Content "$dir/transfer-compression.json" -Encoding UTF8
            }
        }
        if ($action -eq 'Status') {
            $stamp=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
            Record-Wpr "status-$stamp" @('-status','collectors','-details')
        }
        [ordered]@{utc=[DateTime]::UtcNow.ToString('o');directory=$dir;files=@(Get-ChildItem -LiteralPath $dir -File | Select-Object Name,Length);temporary_files=@(Get-ChildItem "$dir/temp" -File -Recurse -Force | Select-Object FullName,Length);temporary_bytes=(@(Get-ChildItem "$dir/temp" -File -Recurse -Force | Measure-Object Length -Sum).Sum);guest_free_bytes=(Get-Volume C).SizeRemaining} | ConvertTo-Json -Depth 5
    }
    if ($Action -in 'Probe','Stop','Fetch') {
        $files = Invoke-Command -Session $session -ArgumentList $directory -ScriptBlock {param($dir) Get-ChildItem -LiteralPath $dir -File | Select-Object Name,Length,@{n='Sha256';e={(Get-FileHash -LiteralPath $_.FullName).Hash}} }
        foreach ($file in $files) {
            if ($CompressTransfer -and $file.Name -eq 'trace.etl') { continue }
            $null = Receive-LabFile -Session $session -GuestPath "$directory/$($file.Name)" -Destination (Join-Path $out $file.Name) -ExpectedSha256 $file.Sha256
        }
        if ($CompressTransfer) {
            $compression=Get-Content -LiteralPath (Join-Path $out 'transfer-compression.json') -Raw | ConvertFrom-Json
            $destination=Join-Path $out 'trace.etl'
            if (Test-Path -LiteralPath $destination) { throw 'Preserve existing host ETL' }
            $inputStream=[IO.File]::OpenRead((Join-Path $out 'trace.etl.gz'))
            $outputStream=[IO.File]::Open($destination,[IO.FileMode]::CreateNew)
            $gzip=$null
            try {
                $gzip=New-Object IO.Compression.GZipStream($inputStream,[IO.Compression.CompressionMode]::Decompress)
                $gzip.CopyTo($outputStream,1MB)
            } finally {
                if ($gzip) { $gzip.Dispose() }
                $outputStream.Dispose()
                $inputStream.Dispose()
            }
            $actualHash=(Get-FileHash -LiteralPath $destination).Hash
            if ($actualHash -ne $compression.original_sha256 -or (Get-Item $destination).Length -ne $compression.original_bytes) { throw 'Lossless trace transfer identity verification failed' }
            [ordered]@{utc=[DateTime]::UtcNow.ToString('o');source_original_sha256=$compression.original_sha256;host_decompressed_sha256=$actualHash;byte_length=$compression.original_bytes;identical=$true;compressed_artifact_preserved=$true} | ConvertTo-Json | Set-Content (Join-Path $out 'transfer-roundtrip.json') -Encoding UTF8
        }
        $files | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $out 'manifest.json') -Encoding UTF8
    }
} finally { Remove-PSSession $session }
