#requires -Version 5.1
#requires -Modules Hyper-V
<#
.SYNOPSIS
Attaches or detaches the temporary performance data disk of the Ubuntu lab VM (Hyper-V host side).

.DESCRIPTION
Attach creates a 64 GiB dynamic VHDX (or reuses the verified one left by a failed attach) and
adds it to SCSI controller 0, location 2 of the Ubuntu VM. Detach removes it from the VM after
the guest proved (temporary-data-disk-unmounted.json) that the filesystem is unmounted with no
open descriptors; the VHDX file is always retained.

Both actions change VM hardware and support -WhatIf/-Confirm; use -WhatIf first. Identity
checks (VM GUID, name, state, vCPU, fixed memory, OS disk slot, free host space) run before
any change, and receipts refuse to be overwritten.

.PARAMETER Action
Attach (default) or Detach.
.PARAMETER CampaignId
Campaign identifier; names the VHDX (<CampaignId>-data.vhdx) and the evidence directory
target/performance/<CampaignId>/ubuntu.

.EXAMPLE
./ubuntu-performance-data-disk.ps1 -Action Attach -CampaignId my-campaign -WhatIf

.OUTPUTS
temporary-data-disk-host.json (Attach) or temporary-data-disk-detached.json (Detach).
#>
[CmdletBinding(SupportsShouldProcess, ConfirmImpact = 'High')]
param(
    [ValidateSet('Attach','Detach')][string]$Action = 'Attach',
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$CampaignId
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'LabCommon.psm1') -Force
$config = Get-LabConfig
$ubuntu = $config.ubuntu_vm
$evidence = Resolve-LabPath "$($config.paths.evidence_root)/$CampaignId/ubuntu"
$vmGuid = [guid]$ubuntu.id
$dataPath = ($ubuntu.vhdx_dir -replace '/', '\') + "\$CampaignId-data.vhdx"
$osPath = $ubuntu.os_vhdx -replace '/', '\'
$controllerLocation = 2
$vm = Get-LabVm -Which Ubuntu -RequireRunning
if ($vm.ProcessorCount -ne $ubuntu.processor_count -or $vm.MemoryStartup -ne $ubuntu.memory_bytes) { throw 'Unexpected VM identity/resources' }
if ((Get-VMMemory -VM $vm).DynamicMemoryEnabled) { throw 'Expected fixed memory' }
$drives = @(Get-VMHardDiskDrive -VM $vm)
$dvds = @(Get-VMDvdDrive -VM $vm)
if (@($drives | Where-Object { $_.ControllerNumber -eq 0 -and $_.ControllerLocation -eq 0 -and $_.Path -eq $osPath }).Count -ne 1) { throw 'OS disk identity changed' }
$slot = @($drives | Where-Object { $_.ControllerType -eq 'SCSI' -and $_.ControllerNumber -eq 0 -and $_.ControllerLocation -eq $controllerLocation })
New-Item -ItemType Directory -Force -Path $evidence | Out-Null
if ($Action -eq 'Attach') {
    $receiptPath = Join-Path $evidence 'temporary-data-disk-host.json'
    $occupied = @(@($drives) + @($dvds) | Where-Object { $_.ControllerNumber -eq 0 -and $_.ControllerLocation -eq $controllerLocation })
    if ((Test-Path -LiteralPath $receiptPath) -or $occupied.Count) { throw 'Receipt or SCSI slot already exists' }
    $volume = Get-Volume -DriveLetter ($dataPath.Substring(0, 1))
    if ($volume.SizeRemaining -lt 80GB) { throw 'Insufficient host storage headroom' }
    if (Test-Path -LiteralPath $dataPath) {
        $failedPath = Join-Path $evidence 'temporary-data-disk-attach-failed.json'
        if (-not (Test-Path -LiteralPath $failedPath)) { throw "VHDX already exists and no failed-attach receipt was found: $failedPath" }
        $failed = Get-Content -LiteralPath $failedPath -Raw -Encoding UTF8 | ConvertFrom-Json
        $existing = Get-VHD -Path $dataPath
        if ($failed.state -ne 'created_but_unattached' -or $failed.vm_guid -ne $vmGuid.ToString() -or $failed.vhdx_path -ne $dataPath -or $existing.Attached -or $existing.DiskIdentifier.ToString() -ne $failed.vhdx_disk_identifier) { throw 'Existing VHDX is not the verified newly created task disk' }
    } elseif ($PSCmdlet.ShouldProcess($dataPath, 'Create 64 GiB dynamic VHDX')) {
        $null = New-VHD -Path $dataPath -Dynamic -SizeBytes 64GB
    } else { return }
    $disk = Get-VHD -Path $dataPath
    if ($disk.Size -ne 64GB -or $disk.VhdType -ne 'Dynamic' -or -not $disk.DiskIdentifier) { throw 'Unexpected newly created VHDX' }
    if (-not $PSCmdlet.ShouldProcess("$($vm.Name) SCSI 0:$controllerLocation", "Attach $dataPath")) { return }
    Add-VMHardDiskDrive -VM $vm -ControllerType SCSI -ControllerNumber 0 -ControllerLocation $controllerLocation -Path $dataPath
    $after = @(Get-VMHardDiskDrive -VM $vm)
    if (@($after | Where-Object { $_.ControllerNumber -eq 0 -and $_.ControllerLocation -eq $controllerLocation -and $_.Path -eq $dataPath }).Count -ne 1) { throw 'New disk attachment identity failed' }
    if ((@(Get-VMDvdDrive -VM $vm | Select-Object ControllerNumber,ControllerLocation,Path) | ConvertTo-Json -Compress) -ne (@($dvds | Select-Object ControllerNumber,ControllerLocation,Path) | ConvertTo-Json -Compress)) { throw 'DVD configuration changed' }
    $receipt = [ordered]@{
        action='Attach'; utc=[DateTime]::UtcNow.ToString('o'); vm_name=$vm.Name; vm_guid=$vmGuid.ToString();
        vhdx_path=$dataPath; vhdx_disk_identifier=$disk.DiskIdentifier.ToString(); virtual_size_bytes=[uint64]$disk.Size;
        vhd_type=$disk.VhdType.ToString(); controller_type='SCSI'; controller_number=0; controller_location=$controllerLocation;
        os_vhdx_path=$osPath; host_volume=($dataPath.Substring(0, 2)); host_free_before_bytes=[uint64]$volume.SizeRemaining;
        vcpus=$ubuntu.processor_count; fixed_memory_bytes=[uint64]$vm.MemoryStartup;
        before_drives=@($drives | Select-Object ControllerType,ControllerNumber,ControllerLocation,Path);
        unchanged_dvds=@($dvds | Select-Object ControllerNumber,ControllerLocation,Path);
        after_drives=@($after | Select-Object ControllerType,ControllerNumber,ControllerLocation,Path)
    }
} else {
    $receiptPath = Join-Path $evidence 'temporary-data-disk-detached.json'
    if (Test-Path -LiteralPath $receiptPath) { throw 'Detach receipt already exists' }
    foreach ($required in 'temporary-data-disk-unmounted.json', 'temporary-data-disk-host.json') {
        if (-not (Test-Path -LiteralPath (Join-Path $evidence $required))) { throw "Missing $required in $evidence; run the guest unmount (unmount-ubuntu-data-disk.py --apply) and copy its receipt first." }
    }
    $unmounted = Get-Content -LiteralPath (Join-Path $evidence 'temporary-data-disk-unmounted.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $attached = Get-Content -LiteralPath (Join-Path $evidence 'temporary-data-disk-host.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($unmounted.unmounted -ne $true -or $unmounted.raw_file_absent -ne $true -or @($unmounted.open_fd_matches).Count -ne 0 -or $unmounted.umount_exit_code -ne 0 -or $unmounted.vhdx_path -ne $dataPath -or $slot.Count -ne 1 -or $slot[0].Path -ne $dataPath) { throw 'Missing actual unmount/idle proof or wrong attached disk' }
    $disk = Get-VHD -Path $dataPath
    if ($disk.DiskIdentifier.ToString() -ne $attached.vhdx_disk_identifier) { throw 'VHDX disk identifier changed' }
    if (-not $PSCmdlet.ShouldProcess("$($vm.Name) SCSI 0:$controllerLocation", "Detach $dataPath (VHDX file is retained)")) { return }
    Remove-VMHardDiskDrive -VMHardDiskDrive $slot[0]
    if (@(Get-VMHardDiskDrive -VM $vm | Where-Object Path -eq $dataPath).Count) { throw 'Task disk remains attached' }
    $receipt = [ordered]@{action='Detach';utc=[DateTime]::UtcNow.ToString('o');vm_guid=$vmGuid.ToString();vhdx_path=$dataPath;vhdx_retained=(Test-Path -LiteralPath $dataPath);vhdx_disk_identifier=$disk.DiskIdentifier.ToString();guest_unmount_receipt='temporary-data-disk-unmounted.json';os_vhdx_path=$osPath}
}
Write-LabJson -Path $receiptPath -InputObject $receipt -Depth 8
$receipt | ConvertTo-Json -Depth 8
