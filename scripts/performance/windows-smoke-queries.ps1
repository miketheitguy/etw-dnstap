param([ValidatePattern('^[A-Za-z0-9_-]{1,64}$')][string]$ProofName='smoke-query-proof',[string]$EvidenceRoot='target/performance/windows')
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$evidence = [IO.Path]::GetFullPath([IO.Path]::Combine($root,$EvidenceRoot))
if (-not $evidence.StartsWith($root + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw 'Evidence root must stay within the workspace' }
New-Item -ItemType Directory -Path $evidence -Force | Out-Null
$vm = Get-VM -Name DNSTap-WS2025
if ($vm.Id -ne [guid]'24bafedb-0998-4430-a0c6-7e90350cd9ac') { throw 'Wrong VM' }
$credential = Import-Clixml -LiteralPath (Join-Path $root 'target/lab-windows/private/administrator.clixml')
$result = Invoke-Command -VMId $vm.Id -Credential $credential -ScriptBlock {
    $ErrorActionPreference = 'Stop'
    $rows = @()
    foreach ($mode in 'auth','rec') {
        foreach ($label in 'www','bills') {
        foreach ($attempt in 1..3) {
            $answer = Resolve-DnsName -Name "$label.$mode.stress.test" -Server 127.0.0.1 -Type A -DnsOnly
            $rows += $answer | Select-Object Name,Type,TTL,IPAddress
            if ($mode -eq 'rec' -and @($answer | Where-Object { $_.Type -eq 'A' -and $_.TTL -eq 0 -and $_.IPAddress -eq '192.0.2.53' }).Count -ne 1) { throw 'Recursive TTL0 answer proof failed' }
        }
        }
    }
    $aCount = @(Get-DnsServerResourceRecord -ZoneName auth.stress.test -RRType A).Count
    if ($aCount -ne 5000) { throw "Expected 5000 authoritative A records, found $aCount" }
    $rows
}
$result | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $evidence "$ProofName.json")
$result | Format-Table Name,Type,TTL,IPAddress
