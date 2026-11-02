param(
  [Parameter(Mandatory)][string]$Repo,
  [Parameter(Mandatory)][string]$Label,
  [Parameter(Mandatory)][ValidateSet('base','phase2')][string]$Image,
  [Parameter(Mandatory)][int]$Qps,
  [Parameter(Mandatory)][int]$Duration,
  [int]$Threads = 4,
  [int]$Inflight = 1024
)
. (Join-Path $PSScriptRoot 'common.ps1')
$ClientExe = 'target/dns-stress-pacing/x86_64-pc-windows-msvc/release/dns-stress.exe'
$ClientSha = '82CFB758153DB8BF2E00E0FB25E932B84154DB2737B8D52CBA05A3D4E61D38A6'
if ((Get-Sha $ClientExe) -ne $ClientSha) { throw 'client hash mismatch' }
$names = 'target/performance/workload/auth-names.txt'
$out = Join-Path $Repo "$EvRel/$Label"
if (Test-Path $out) { throw "run dir exists: $out" }
New-Item -ItemType Directory -Path $out | Out-Null
$buildNames = 'cargo','rustc','link','cl','lld-link','rust-lld','cargo-clippy','clippy-driver','rustdoc','miri','cargo-miri'
function Get-Builders { @(Get-Process -Name $buildNames -ErrorAction SilentlyContinue | ForEach-Object { $_.ProcessName }) }
$b0 = Get-Builders
if ($b0.Count) { throw "build processes running on host: $($b0 -join ',')" }

$cs = @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class Thr {
  [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr OpenThread(uint a, bool i, uint t);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
  [DllImport("kernel32.dll")] static extern bool QueryThreadCycleTime(IntPtr h, out ulong c);
  [DllImport("kernel32.dll")] static extern int GetThreadDescription(IntPtr h, out IntPtr d);
  [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);
  public static string[] Snap(int[] tids) {
    var r = new List<string>();
    foreach (int t in tids) {
      IntPtr h = OpenThread(0x0800, false, (uint)t);
      if (h == IntPtr.Zero) continue;
      ulong c; string name = "";
      if (!QueryThreadCycleTime(h, out c)) c = 0;
      IntPtr d;
      if (GetThreadDescription(h, out d) >= 0 && d != IntPtr.Zero) { name = Marshal.PtrToStringUni(d); LocalFree(d); }
      CloseHandle(h);
      r.Add(t + ";" + name + ";" + c);
    }
    return r.ToArray();
  }
}
'@

$vm = Get-LabVm -RequireRunning
$s = Get-LabSession -Vm $vm
$snapBlock = {
  param($svcName, $cs, $full)
  $ErrorActionPreference = 'Stop'
  if (-not ('Thr' -as [type])) { Add-Type -TypeDefinition $cs }
  $svc = Get-CimInstance Win32_Service -Filter "Name='$svcName'"
  $p = Get-Process -Id $svc.ProcessId
  $dns = Get-Process -Name dns
  $o = [ordered]@{
    utc = [DateTime]::UtcNow.ToString('o'); pid = $svc.ProcessId; state = $svc.State
    cpu_seconds = $p.TotalProcessorTime.TotalSeconds; user_seconds = $p.UserProcessorTime.TotalSeconds
    private_bytes = $p.PrivateMemorySize64; working_set = $p.WorkingSet64; peak_working_set = $p.PeakWorkingSet64
    virtual_bytes = $p.VirtualMemorySize64; handles = $p.HandleCount; thread_count = $p.Threads.Count
    dns_cpu_seconds = $dns.TotalProcessorTime.TotalSeconds
    total_cpu_pct = (Get-Counter '\Processor(_Total)\% Processor Time').CounterSamples[0].CookedValue
  }
  if ($full) {
    $o.threads = @([Thr]::Snap([int[]]@($p.Threads | ForEach-Object { $_.Id })) | ForEach-Object { $f = $_ -split ';'; [ordered]@{ tid = [int]$f[0]; name = $f[1]; cycles = [uint64]$f[2] } })
    $names = '\EtwDnstap(*)\Events Received','\EtwDnstap(*)\Frames Sent','\EtwDnstap(*)\Events Dropped','\EtwDnstap(*)\Channel Full Drops','\EtwDnstap(*)\Pool Exhausted Drops','\EtwDnstap(*)\Jumbo Pool Exhausted Drops','\EtwDnstap(*)\ETW Events Lost','\EtwDnstap(*)\ETW Buffers Lost','\EtwDnstap(*)\Receiver Down Drops','\EtwDnstap(*)\Events Rejected','\EtwDnstap(*)\Callback Errors','\EtwDnstap(*)\Channel Peak Depth','\EtwDnstap(*)\Receiver Disconnects','\EtwDnstap(*)\Receiver Connected'
    $c = [ordered]@{}
    foreach ($sm in (Get-Counter -Counter $names).CounterSamples) { $c[($sm.Path -replace '^.*\\','')] = $sm.CookedValue }
    $o.counters = $c
  }
  $o
}
$ssCmd = "ss -tinH state established '( sport = :6000 )'"
function Get-RxBytes {
  $t = ssh -F target/lab-ubuntu/ssh_config dnstap-ubuntu $ssCmd 2>&1 | Out-String
  $m = [regex]::Matches($t, 'bytes_received:(\d+)')
  [ordered]@{ utc = [DateTime]::UtcNow.ToString('o'); connections = $m.Count; bytes_received = @($m | ForEach-Object { [long]$_.Groups[1].Value }); raw = $t.Trim() }
}
$result = [ordered]@{ label = $Label; image = $Image; image_sha256 = $Images[$Image].sha; qps_requested = $Qps; duration_s = $Duration; threads = $Threads; inflight = $Inflight; client_sha256 = $ClientSha; host_build_processes_at_start = $b0.Count }
$failure = $null
try {
  $begin = Invoke-Command -Session $s -ArgumentList $GuestDir,$Installed,$Service,$Image,$Images[$Image].sha -ScriptBlock {
    param($dir,$inst,$svcName,$img,$sha)
    $ErrorActionPreference='Stop'
    if ((Get-Service $svcName).Status -ne 'Stopped') { Stop-Service $svcName; (Get-Service $svcName).WaitForStatus('Stopped',[TimeSpan]::FromSeconds(60)) }
    $sddl = (Get-Acl $inst).Sddl
    $swapped = $false
    if ((Get-FileHash $inst).Hash -ne $sha) {
      Copy-Item -LiteralPath "$dir\$img.exe" -Destination $inst -Force
      $swapped = $true
    }
    if ((Get-FileHash $inst).Hash -ne $sha -or (Get-Acl $inst).Sddl -ne $sddl) { throw 'deploy hash/ACL mismatch' }
    Start-Sleep -Seconds 5
    $t0 = Get-Date
    Start-Service $svcName
    (Get-Service $svcName).WaitForStatus('Running',[TimeSpan]::FromSeconds(60))
    $svc = Get-CimInstance Win32_Service -Filter "Name='$svcName'"
    [ordered]@{ swapped=$swapped; installed_sha256=(Get-FileHash $inst).Hash; path_name=$svc.PathName; started_local=$t0.ToString('o'); pid=$svc.ProcessId }
  }
  $result.begin = $begin
  Start-Sleep -Seconds 8   # let the capture settle after start
  $pre = Invoke-Command -Session $s -ScriptBlock $snapBlock -ArgumentList $Service,$cs,$true
  $rxPre = Get-RxBytes
  $result.pre = $pre; $result.rx_pre = $rxPre

  $stressDir = "$EvRel/$Label/stress"
  $args_ = @('--server','172.27.215.163:53','--names',$names,'--mode','authoritative','--threads',"$Threads",'--duration',"$Duration",'--inflight',"$Inflight",'--timeout-ms','2000','--qps',"$Qps",'--output-dir',$stressDir)
  ($ClientExe + ' ' + ($args_ -join ' ')) | Set-Content (Join-Path $out 'client-cmd.txt')
  $t0 = [DateTime]::UtcNow
  $client = Start-Process -FilePath (Join-Path $Repo $ClientExe) -ArgumentList (Join-LabArguments -Arguments $args_) -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $out 'stress-stdout.txt') -RedirectStandardError (Join-Path $out 'stress-stderr.txt')
  $null = $client.Handle
  $samples = New-Object System.Collections.Generic.List[object]
  $maxBuilders = 0
  while (-not $client.HasExited) {
    Start-Sleep -Seconds 10
    $maxBuilders = [Math]::Max($maxBuilders, (Get-Builders).Count)
    if (-not $client.HasExited) {
      $x = Invoke-Command -Session $s -ScriptBlock $snapBlock -ArgumentList $Service,$cs,$false
      $x.host_total_cpu_pct = (Get-Counter '\Processor(_Total)\% Processor Time').CounterSamples[0].CookedValue
      $samples.Add($x)
    }
  }
  $client.WaitForExit()
  $t1 = [DateTime]::UtcNow
  $result.client_exit_code = $client.ExitCode
  $result.client_start_utc = $t0.ToString('o'); $result.client_end_utc = $t1.ToString('o')
  $result.polls = $samples
  Start-Sleep -Seconds 6   # drain
  $post = Invoke-Command -Session $s -ScriptBlock $snapBlock -ArgumentList $Service,$cs,$true
  $rxPost = Get-RxBytes
  $result.post = $post; $result.rx_post = $rxPost
  $result.host_build_processes_max_during = $maxBuilders
  $result.host_build_processes_at_end = (Get-Builders).Count
  $sumPath = Join-Path $Repo "$stressDir/summary.json"
  if (Test-Path $sumPath) { $sum = Get-Content $sumPath -Raw | ConvertFrom-Json; $result.client = [ordered]@{ sent=$sum.counts.sent; completed=$sum.counts.completed; successful=$sum.counts.successful; timeouts=$sum.counts.timeouts; send_errors=$sum.counts.send_errors; pacing_slots_skipped=$sum.counts.pacing_slots_skipped; measured_send_duration_ns=$sum.measured_send_duration_ns; qps_achieved=($sum.counts.sent / ($sum.measured_send_duration_ns / 1e9)); p50_ns=$sum.latency.p50_ns_upper; p99_ns=$sum.latency.p99_ns_upper } }
  Write-LabJson -Path (Join-Path $out 'exit.json') -InputObject ([ordered]@{ exit_code=$client.ExitCode; host_end_utc=$t1.ToString('o') }) -Depth 3
} catch { $failure = $_.ToString(); $result.failure = $failure }
finally {
  try {
    $stop = Invoke-Command -Session $s -ArgumentList $Service,$result.begin.started_local -ScriptBlock {
      param($svcName,$since)
      $ErrorActionPreference='Stop'
      Stop-Service $svcName -ErrorAction SilentlyContinue
      (Get-Service $svcName).WaitForStatus('Stopped',[TimeSpan]::FromSeconds(60))
      Start-Sleep -Seconds 1
      $svc = Get-CimInstance Win32_Service -Filter "Name='$svcName'"
      $st = [DateTime]::Parse($since)
      $ev = @(Get-WinEvent -FilterHashtable @{ LogName='Application'; ProviderName=$svcName; StartTime=$st.AddSeconds(-2) } -ErrorAction SilentlyContinue | Sort-Object TimeCreated | ForEach-Object { $_.Message -replace '\s+',' ' })
      [ordered]@{ state=$svc.State; exit_code=$svc.ExitCode; events=$ev }
    }
    $result.stop = $stop
  } catch { $result.stop_failure = $_.ToString() }
  Remove-PSSession $s
  Write-LabJson -Path (Join-Path $out 'run.json') -InputObject $result -Depth 8 -Force
}
if ($failure) { throw $failure }
"RUN_DONE $Label"
