#requires -Version 5.1
<#
.SYNOPSIS
Downloads the pinned SecLists subdomain list and generates the lab DNS workload from it.

.DESCRIPTION
Fetches subdomains-top1million-20000.txt and the license from a fixed SecLists commit,
verifies the SHA-256 of the source list, records the commit, then runs New-DnsWorkload.ps1.
This is the only script that touches the network; it never queries DNS.

.PARAMETER OutputDirectory
Destination directory (default target/performance/workload under the repository, regardless
of the current directory).

.OUTPUTS
source-names.txt, LICENSE-SecLists, source-commit.json and the New-DnsWorkload.ps1 outputs.
#>
[CmdletBinding()]
param([string]$OutputDirectory)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'LabCommon.psm1') -Force
if (-not $OutputDirectory) { $OutputDirectory = Resolve-LabPath (Get-LabConfig).paths.workload_dir }
$commit = '1e3a7423c117ea878679a29c760564fe442021c0'
$expected = '685fad386d19c3b7ecbcd18645606bc5c47a3b2339fa3d79f44c46bd9e79c1ef'
$base = "https://raw.githubusercontent.com/danielmiessler/SecLists/$commit"
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$source = Join-Path $OutputDirectory 'source-names.txt'
Invoke-WebRequest -Uri "$base/Discovery/DNS/subdomains-top1million-20000.txt" -OutFile $source
if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'Source checksum mismatch.' }
Invoke-WebRequest -Uri "$base/LICENSE" -OutFile (Join-Path $OutputDirectory 'LICENSE-SecLists')
Write-LabJson -Path (Join-Path $OutputDirectory 'source-commit.json') -InputObject @{ sha = $commit } -Force
& (Join-Path $PSScriptRoot 'New-DnsWorkload.ps1') -SourceDirectory $OutputDirectory -OutputDirectory $OutputDirectory
