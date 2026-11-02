# Shared by the stage / run / restore scripts. Dot-sourced. Windows PowerShell 5.1 only.
$ErrorActionPreference = 'Stop'
$env:PSModulePath = "C:\Program Files\WindowsPowerShell\Modules;C:\Windows\system32\WindowsPowerShell\v1.0\Modules"   # Windows PowerShell 5.1 module directories; a pwsh parent pollutes the inherited value
Set-Location $Repo
Import-Module (Join-Path $Repo 'scripts\performance\LabCommon.psm1') -Force
$script:GuestDir = 'C:\ProgramData\EtwDnstap\performance\phase2-ab'
$script:Installed = 'C:\Program Files\EtwDnstap\etw-dnstap.exe'
$script:Service = 'EtwDnstap'
$script:EvRel = 'target/performance/phase2-lab-ab-2026-11-02'
$script:Images = @{
  base   = @{ file = 'etw-dnstap-base-e61179d.exe';   sha = '40E63F9DDD2B089B21B046EAFE0F21129B74DFCDE0A3DBE464D106C77775ED8E' }
  phase2 = @{ file = 'etw-dnstap-phase2-25742af.exe'; sha = 'EF31E6593FC91534493E9BFC2E0D75618C1176B28C140DEA3A81D91FA393B3D0' }
}
function Get-Sha([string]$p) { (Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash }
