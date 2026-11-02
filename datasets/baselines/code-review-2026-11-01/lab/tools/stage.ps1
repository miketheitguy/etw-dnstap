param([Parameter(Mandatory)][string]$Repo, [Parameter(Mandatory)][string]$OutFile)
. (Join-Path $PSScriptRoot 'common.ps1')
foreach ($k in $Images.Keys) {
  $p = Join-Path $Repo "target\lab-ab\$($Images[$k].file)"
  if ((Get-Sha $p) -ne $Images[$k].sha) { throw "host image hash mismatch: $k" }
}
$vm = Get-LabVm -RequireRunning
$s = Get-LabSession -Vm $vm
try {
  $pre = Invoke-Command -Session $s -ArgumentList $GuestDir,$Installed,$Service -ScriptBlock {
    param($dir,$inst,$svcName)
    $ErrorActionPreference='Stop'
    $svc = Get-CimInstance Win32_Service -Filter "Name='$svcName'"
    if ($svc.State -ne 'Stopped') { throw "service not Stopped: $($svc.State)" }
    if (Test-Path $dir) { throw "guest dir exists: $dir" }
    New-Item -ItemType Directory -Path $dir | Out-Null
    Copy-Item -LiteralPath $inst -Destination "$dir\original.exe"
    [ordered]@{ state=$svc.State; start_name=$svc.StartName; path_name=$svc.PathName; start_mode=$svc.StartMode
      binary_sha256=(Get-FileHash $inst).Hash; backup_sha256=(Get-FileHash "$dir\original.exe").Hash
      executable_sddl=(Get-Acl $inst).Sddl; dir_sddl=(Get-Acl (Split-Path $inst)).Sddl
      scm_sddl=(& sc.exe sdshow $svcName | Out-String).Trim()
      dir_acl_dir=(Get-Acl $dir).Sddl
      utc=[DateTime]::UtcNow.ToString('o') }
  }
  if ($pre.binary_sha256 -ne $pre.backup_sha256) { throw 'backup hash mismatch' }
  foreach ($k in $Images.Keys) {
    Copy-Item -LiteralPath (Join-Path $Repo "target\lab-ab\$($Images[$k].file)") -Destination "$GuestDir\$k.exe" -ToSession $s
  }
  $hashes = Invoke-Command -Session $s -ArgumentList $GuestDir -ScriptBlock { param($d) Get-ChildItem $d -Filter *.exe | % { [pscustomobject]@{name=$_.Name; sha=(Get-FileHash $_.FullName).Hash; len=$_.Length} } }
  foreach ($k in $Images.Keys) { $h = $hashes | ? name -eq "$k.exe"; if ($h.sha -ne $Images[$k].sha) { throw "guest image hash mismatch: $k" } }
  # keep a host copy of the original installed binary
  $dest = Join-Path $Repo ("target\lab-ab\original-installed-" + $pre.binary_sha256.Substring(0,8) + '.exe')
  $null = Receive-LabFile -Session $s -GuestPath "$GuestDir\original.exe" -Destination $dest -ExpectedSha256 $pre.binary_sha256
  $o = [ordered]@{ pre=$pre; staged=$hashes | % { @{name=$_.name; sha=$_.sha; len=$_.len} } }
  New-Item -ItemType Directory -Force (Split-Path $OutFile) | Out-Null
  Write-LabJson -Path $OutFile -InputObject $o -Depth 6 -Force
  $o | ConvertTo-Json -Depth 6
} finally { Remove-PSSession $s }
