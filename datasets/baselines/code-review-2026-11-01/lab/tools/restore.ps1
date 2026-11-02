param([Parameter(Mandatory)][string]$Repo, [Parameter(Mandatory)][string]$StateFile, [Parameter(Mandatory)][string]$OutFile)
. (Join-Path $PSScriptRoot 'common.ps1')
$state = Get-Content -LiteralPath $StateFile -Raw | ConvertFrom-Json
$pre = $state.pre
$vm = Get-LabVm -RequireRunning
$s = Get-LabSession -Vm $vm
try {
  $post = Invoke-Command -Session $s -ArgumentList $GuestDir,$Installed,$Service,$pre -ScriptBlock {
    param($dir,$inst,$svcName,$pre)
    $ErrorActionPreference='Stop'
    Stop-Service $svcName -ErrorAction SilentlyContinue
    (Get-Service $svcName).WaitForStatus('Stopped',[TimeSpan]::FromSeconds(60))
    if ((Get-FileHash $inst).Hash -ne $pre.binary_sha256) {
      Copy-Item -LiteralPath "$dir\original.exe" -Destination $inst -Force
    }
    $svc = Get-CimInstance Win32_Service -Filter "Name='$svcName'"
    $r = [ordered]@{ state=$svc.State; path_name=$svc.PathName; start_name=$svc.StartName; start_mode=$svc.StartMode
      binary_sha256=(Get-FileHash $inst).Hash; executable_sddl=(Get-Acl $inst).Sddl
      scm_sddl=(& sc.exe sdshow $svcName | Out-String).Trim() }
    $ok = ($r.binary_sha256 -eq $pre.binary_sha256 -and $r.executable_sddl -eq $pre.executable_sddl -and $r.path_name -eq $pre.path_name -and $r.state -eq $pre.state -and $r.scm_sddl -eq $pre.scm_sddl -and $r.start_mode -eq $pre.start_mode -and $r.start_name -eq $pre.start_name)
    $r.matches_pre = $ok
    if ($ok) { Remove-Item -LiteralPath $dir -Recurse -Force; $r.guest_dir_removed = -not (Test-Path $dir) }
    $r
  }
  Write-LabJson -Path $OutFile -InputObject $post -Depth 5 -Force
  $post | ConvertTo-Json -Depth 5
  if (-not $post.matches_pre) { throw 'restore mismatch' }
} finally { Remove-PSSession $s }
