param([ValidateSet('Attach','Detach')][string]$Action='Attach')
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$evidence = Join-Path $root 'target/performance/pooled-vm-20261029-0942/ubuntu'
$vmGuid = [guid]'d5ca1bde-da5c-4c08-9625-e845c1443b0b'
$dataPath = 'D:\VMs\DNSTap-Ubuntu2604\pooled-vm-20261029-0942-data.vhdx'
$osPath = 'D:\VMs\DNSTap-Ubuntu2604\DNSTap-Ubuntu2604.vhdx'
$controllerLocation = 2
$vm = Get-VM -Id $vmGuid
if ($vm.Name -ne 'DNSTap-Ubuntu2604' -or $vm.State -ne 'Running' -or $vm.ProcessorCount -ne 2 -or $vm.MemoryStartup -ne 4GB) { throw 'Unexpected VM identity/resources' }
if ((Get-VMMemory -VM $vm).DynamicMemoryEnabled) { throw 'Expected fixed memory' }
$drives = @(Get-VMHardDiskDrive -VM $vm)
$dvds = @(Get-VMDvdDrive -VM $vm)
if (@($drives | Where-Object { $_.ControllerNumber -eq 0 -and $_.ControllerLocation -eq 0 -and $_.Path -eq $osPath }).Count -ne 1) { throw 'OS disk identity changed' }
$slot = @($drives | Where-Object { $_.ControllerType -eq 'SCSI' -and $_.ControllerNumber -eq 0 -and $_.ControllerLocation -eq $controllerLocation })
if ($Action -eq 'Attach') {
    $receiptPath = Join-Path $evidence 'temporary-data-disk-host.json'
    $occupied = @(@($drives)+@($dvds) | Where-Object { $_.ControllerNumber -eq 0 -and $_.ControllerLocation -eq $controllerLocation })
    if ((Test-Path -LiteralPath $receiptPath) -or $occupied.Count) { throw 'Receipt or SCSI slot already exists' }
    $volume = Get-Volume -DriveLetter D
    if ($volume.SizeRemaining -lt 80GB) { throw 'Insufficient host storage headroom' }
    if (Test-Path -LiteralPath $dataPath) {
        $failed = Get-Content (Join-Path $evidence 'temporary-data-disk-attach-failed.json') -Raw | ConvertFrom-Json
        $existing = Get-VHD -Path $dataPath
        if ($failed.state -ne 'created_but_unattached' -or $failed.vm_guid -ne $vmGuid.ToString() -or $failed.vhdx_path -ne $dataPath -or $existing.Attached -or $existing.DiskIdentifier.ToString() -ne $failed.vhdx_disk_identifier) { throw 'Existing VHDX is not the verified newly created task disk' }
    } else {
        $created = New-VHD -Path $dataPath -Dynamic -SizeBytes 64GB
    }
    $disk = Get-VHD -Path $dataPath
    if ($disk.Size -ne 64GB -or $disk.VhdType -ne 'Dynamic' -or -not $disk.DiskIdentifier) { throw 'Unexpected newly created VHDX' }
    Add-VMHardDiskDrive -VM $vm -ControllerType SCSI -ControllerNumber 0 -ControllerLocation $controllerLocation -Path $dataPath
    $after = @(Get-VMHardDiskDrive -VM $vm)
    if (@($after | Where-Object { $_.ControllerNumber -eq 0 -and $_.ControllerLocation -eq $controllerLocation -and $_.Path -eq $dataPath }).Count -ne 1) { throw 'New disk attachment identity failed' }
    if ((@(Get-VMDvdDrive -VM $vm | Select-Object ControllerNumber,ControllerLocation,Path) | ConvertTo-Json -Compress) -ne (@($dvds | Select-Object ControllerNumber,ControllerLocation,Path) | ConvertTo-Json -Compress)) { throw 'DVD configuration changed' }
    $receipt = [ordered]@{
        action='Attach'; utc=[DateTime]::UtcNow.ToString('o'); vm_name=$vm.Name; vm_guid=$vmGuid.ToString();
        vhdx_path=$dataPath; vhdx_disk_identifier=$disk.DiskIdentifier.ToString(); virtual_size_bytes=[uint64]$disk.Size;
        vhd_type=$disk.VhdType.ToString(); controller_type='SCSI'; controller_number=0; controller_location=$controllerLocation;
        os_vhdx_path=$osPath; host_volume='D:'; host_free_before_bytes=[uint64]$volume.SizeRemaining;
        vcpus=2; fixed_memory_bytes=[uint64]$vm.MemoryStartup;
        before_drives=@($drives | Select-Object ControllerType,ControllerNumber,ControllerLocation,Path);
        unchanged_dvds=@($dvds | Select-Object ControllerNumber,ControllerLocation,Path);
        after_drives=@($after | Select-Object ControllerType,ControllerNumber,ControllerLocation,Path)
    }
} else {
    $receiptPath = Join-Path $evidence 'temporary-data-disk-detached.json'
    if (Test-Path -LiteralPath $receiptPath) { throw 'Detach receipt already exists' }
    $unmounted = Get-Content (Join-Path $evidence 'temporary-data-disk-unmounted.json') -Raw | ConvertFrom-Json
    $attached = Get-Content (Join-Path $evidence 'temporary-data-disk-host.json') -Raw | ConvertFrom-Json
    if ($unmounted.unmounted -ne $true -or $unmounted.raw_file_absent -ne $true -or @($unmounted.open_fd_matches).Count -ne 0 -or $unmounted.umount_exit_code -ne 0 -or $unmounted.vhdx_path -ne $dataPath -or $slot.Count -ne 1 -or $slot[0].Path -ne $dataPath) { throw 'Missing actual unmount/idle proof or wrong attached disk' }
    $disk = Get-VHD -Path $dataPath
    if ($disk.DiskIdentifier.ToString() -ne $attached.vhdx_disk_identifier) { throw 'VHDX disk identifier changed' }
    Remove-VMHardDiskDrive -VMHardDiskDrive $slot[0]
    if (@(Get-VMHardDiskDrive -VM $vm | Where-Object Path -eq $dataPath).Count) { throw 'Task disk remains attached' }
    $receipt = [ordered]@{action='Detach';utc=[DateTime]::UtcNow.ToString('o');vm_guid=$vmGuid.ToString();vhdx_path=$dataPath;vhdx_retained=(Test-Path -LiteralPath $dataPath);vhdx_disk_identifier=$disk.DiskIdentifier.ToString();guest_unmount_receipt='temporary-data-disk-unmounted.json';os_vhdx_path=$osPath}
}
$receipt | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $receiptPath -Encoding utf8
$receipt | ConvertTo-Json -Depth 8
