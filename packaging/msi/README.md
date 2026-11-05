# MSI package sources

> **Status:** Current · **Describes:** the code on branch `spool-encryption` · **Evidence dated:** 2026-11-03 (lab run on Windows Server 2025, build 26100) · **Reviewed:** 2026-11-05

WiX v7 sources for the EtwDnstap MSI. User steps: [Install](../../docs/guide/install.md#install-with-the-msi), [Upgrade, repair, or change settings](../../docs/guide/upgrade.md#upgrade-repair-or-change-settings-with-the-msi), [Uninstall](../../docs/guide/uninstall.md#uninstall-with-the-msi), [Build](../../docs/guide/build.md#build-the-msi).

### Quick Start

```powershell
pwsh -NoProfile -File scripts/build/Build-Msi.ps1
```

[Build-Msi.ps1](../../scripts/build/Build-Msi.ps1) runs the Rust build and CI checks, then builds into `target\msi\`. `-SkipCargoBuild` skips the Rust build after a release build. To build the WiX project directly, pass `EtwVersion`, `EtwExePath`, `EtwRepoRoot` (paths relative to this directory).

### Steps / Configuration

| File | Content |
| --- | --- |
| **EtwDnstap.wixproj** | `WixToolset.Sdk`, `WixToolset.Util.wixext` 7.0.1 pinned; `AcceptEula` is `wix7` |
| **Product.wxs** | `MajorUpgrade` (`AllowSameVersionUpgrades="yes"`), launch conditions, public properties, install folder, service ACL and SCM policy, event source, counter manifest, `ScheduleReboot`; takes [Grant-EtwDnstapEtwAccess.ps1](../../scripts/provision/Grant-EtwDnstapEtwAccess.ps1) |
| **CustomActions.wxs** | Provisioning actions, sequence rows |
| **Invoke-EtwDnstapProvisioning.ps1** | SYSTEM, Windows PowerShell 5.1 (`Install`, `Uninstall`): checks receiver, creates data folders, creates and removes spool key and `Protect` registry key, manages service SID group membership, runs grant script, `sc.exe failure` recovery |
| **Test-EtwDnstapProvisioningKeys.ps1** | Key and registry tests, no install |

[build.rs](../../build.rs) writes the version resource; [tests/version_resource.rs](../../tests/version_resource.rs) checks it. Key test (per-user CNG key, throwaway `HKCU` path, both deleted), from the repository root:

```powershell
powershell.exe -NoProfile -File packaging\msi\Test-EtwDnstapProvisioningKeys.ps1
```

#### Custom action sequence

- `ProvisionInstall`: after `InstallServices` (so `NT SERVICE\EtwDnstap` resolves). Condition `NOT REMOVE AND ($EtwDnstapExe = 3)`. A maintenance `msiexec /i` with no `REINSTALL` skips it, keeping the grant and the binary path in step.
- `ProvisionInstallRollback`: failed first install, same condition.
- `ProvisionUninstall`: after `StopServices`, before `DeleteServices`. Major upgrades (`UPGRADINGPRODUCTCODE`) skip it.

A stale `WrappedKey` is deleted only when the install created a new CNG key. Run table: [upgrade.md](../../docs/guide/upgrade.md#when-the-install-steps-run).

#### Session name rule

`SESSION_NAME`: 1 to 128 ASCII letters, digits, `_`, `.`, `-`; no leading hyphen; case-sensitive. `^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}\z`. A trailing newline, U+212A, and U+0130 don't match. The provisioning script, `Grant-EtwDnstapEtwAccess.ps1`, and the service's `--session-name` use the same pattern and the default `ValidatePattern` error text (Windows PowerShell 5.1 has no `ErrorMessage`).

One launch condition covers the leading hyphen: `SESSION_NAME must not start with '-'. ...`. A removal skips it. The scripts and service enforce length and characters; an invalid name fails the provisioning action and the install.

#### Group membership

- **Install:** translate `NT SERVICE\EtwDnstap` to a SID, call `Get-LocalGroupMember`. On a throw, read members by SID through a small `Add-Type` class. If the compile fails: `Could not read the members of <group>. By name: <error> By SID: <error>`.
- **Uninstall:** SID path only (the service may be gone): `NetLocalGroupGetMembers`, `NetLocalGroupDelMembers` at level 0. Failure is a warning. See [What the uninstall removes](../../docs/guide/uninstall.md#what-the-uninstall-removes).

#### Spool key code

The script creates CNG key `EtwDnstap Spool Key` and registry key `HKLM\SOFTWARE\ETW-DNSTapService\Protect`, setting and checking DACLs on every install ([install.md](../../docs/guide/install.md#spool-encryption-key)).

- `Protect` sits outside `HKLM\SYSTEM\...\Services\EtwDnstap` (deleted by `DeleteServices`) and `HKLM\SOFTWARE\EtwDnstap` (deleted by the old product's removal). A major upgrade keeps the CNG key and `WrappedKey` with no copy step.
- Only the script creates or removes `SOFTWARE\ETW-DNSTapService`. A missing parent gets the default ACL, unchanged.
- Install deletes a `Parameters\SpoolKey` subkey under `Services\EtwDnstap` (failure is a warning). The rest of `Parameters` and its ACL stay.

#### Service dependencies

`Product.wxs` declares `DNS` and `KeyIso` (CNG Key Isolation, Windows Server 2019 and later) as `ServiceDependency` rows. At start the service calls `NCryptOpenStorageProvider`, `NCryptOpenKey`, `NCryptDecrypt`, `NCryptEncrypt`; without `KeyIso` running it can deadlock.

#### Lab validation

Run: install, failed install with rollback, repair, receiver change, uninstall. Open: major upgrade, group removal by SID, SID fallback, launch-condition message, later component-condition and same-version changes ([plan](../../docs/history/code-review-findings-fix-plan.md#lab-validation-not-part-of-this-plans-agents)).

### Common Gotchas

- Rollback can delete an accepted key: it runs the uninstall action and can't tell a created CNG key from one left by hand or a failed uninstall.
- Schedule `ProvisionInstallRollback` before `ProvisionInstall`: Windows Installer runs only rollback actions already in the script.
- `ServiceInstall` writes the dependency list at service creation. A major upgrade, repair, or `REINSTALL=ALL` applies it; a plain maintenance `msiexec /i` doesn't. Check with `sc.exe qc EtwDnstap`.
