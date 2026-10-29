$ErrorActionPreference='Stop'
$root=(Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$out=Join-Path $root 'target/performance/pooled-vm-20261029-0942/build/process-handle-check.json'
if (Test-Path -LiteralPath $out) { throw 'Preserve existing process-handle check' }
$results=@()
foreach ($expected in 0,7) {
    $process=Start-Process powershell.exe -ArgumentList @('-NoProfile','-Command',"exit $expected") -WindowStyle Hidden -PassThru
    $retainedHandle=$process.Handle
    $deadline=[DateTime]::UtcNow.AddSeconds(10)
    while (-not $process.HasExited) {
        if ([DateTime]::UtcNow -gt $deadline) { Stop-Process -Id $process.Id; throw 'Nonload child completion deadline' }
        Start-Sleep -Milliseconds 50
        $process.Refresh()
    }
    $process.WaitForExit()
    $actual=$process.ExitCode
    $results += [ordered]@{expected=$expected;actual=$actual;retained_handle_nonzero=($retainedHandle -ne [IntPtr]::Zero);pass=($null -ne $actual -and $actual -eq $expected)}
    if ($null -eq $actual -or $actual -ne $expected) { throw 'Retained process handle did not preserve exact exit code' }
}
[ordered]@{utc=[DateTime]::UtcNow.ToString('o');powershell_version=$PSVersionTable.PSVersion.ToString();nonload=$true;checks=$results} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $out
Get-Content -LiteralPath $out -Raw
