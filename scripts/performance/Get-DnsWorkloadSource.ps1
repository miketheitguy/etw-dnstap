[CmdletBinding()]
param([string]$OutputDirectory = 'target/performance/workload')
$ErrorActionPreference = 'Stop'
$commit = '1e3a7423c117ea878679a29c760564fe442021c0'
$expected = '685fad386d19c3b7ecbcd18645606bc5c47a3b2339fa3d79f44c46bd9e79c1ef'
$base = "https://raw.githubusercontent.com/danielmiessler/SecLists/$commit"
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$source = Join-Path $OutputDirectory 'source-names.txt'
Invoke-WebRequest -Uri "$base/Discovery/DNS/subdomains-top1million-20000.txt" -OutFile $source
if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'Source checksum mismatch.' }
Invoke-WebRequest -Uri "$base/LICENSE" -OutFile (Join-Path $OutputDirectory 'LICENSE-SecLists')
@{ sha = $commit } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $OutputDirectory 'source-commit.json') -Encoding utf8
& (Join-Path $PSScriptRoot 'New-DnsWorkload.ps1') -SourceDirectory $OutputDirectory -OutputDirectory $OutputDirectory
