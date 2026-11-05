# MSI package sources

> **Status:** Current · **Describes:** the code on branch `spool-encryption` · **Evidence dated:** 2026-11-03 (lab run on Windows Server 2025, build 26100) · **Reviewed:** 2026-11-05

This directory holds the WiX v7 sources for the EtwDnstap MSI. User steps are in the guides:

- [Install with the MSI](../../docs/guide/install.md#install-with-the-msi), including the spool key and its ACLs
- [Upgrade, repair, or change settings with the MSI](../../docs/guide/upgrade.md#upgrade-repair-or-change-settings-with-the-msi)
- [Uninstall with the MSI](../../docs/guide/uninstall.md#uninstall-with-the-msi)
- [Build the MSI](../../docs/guide/build.md#build-the-msi)

[scripts/build/Build-Msi.ps1](../../scripts/build/Build-Msi.ps1) runs the Rust build and the CI checks first, then builds the MSI.

| File | Content |
| --- | --- |
| `EtwDnstap.wixproj` | The project. `WixToolset.Sdk` 7.0.1 and `WixToolset.Util.wixext` 7.0.1, pinned. It sets `AcceptEula` to `wix7`. |
| `Product.wxs` | The package: `MajorUpgrade` with `AllowSameVersionUpgrades="yes"`, launch conditions, public properties, install folder, service with ACL and SCM policy, event source, counter manifest, and `ScheduleReboot`. |
| `CustomActions.wxs` | The provisioning custom actions and sequence rows. See [Custom action sequence](#custom-action-sequence). |
| `Invoke-EtwDnstapProvisioning.ps1` | Run by the custom actions as SYSTEM in Windows PowerShell 5.1 (`Install`, `Uninstall`). It checks the receiver, creates the data folders, creates and removes the spool key and `Protect` registry key, manages the service SID's group membership, runs the grant script, and sets recovery with `sc.exe failure`. |
| `Test-EtwDnstapProvisioningKeys.ps1` | Tests the key and registry functions without an install. See [Tests](#tests). |

## Custom action sequence

- `ProvisionInstall` runs after `InstallServices`, so that `NT SERVICE\EtwDnstap` resolves. It runs in a pass that installs the executable: `NOT REMOVE AND ($EtwDnstapExe = 3)`. A maintenance `msiexec /i` with no `REINSTALL` skips it, so the grant and the service's binary path stay in step.
- `ProvisionInstallRollback` runs for a failed first install, with the same component condition. Schedule it before `ProvisionInstall`, because Windows Installer runs only the rollback actions already in the script.
- `ProvisionUninstall` runs after `StopServices` and before `DeleteServices`. A major upgrade (`UPGRADINGPRODUCTCODE`) skips it.

The script deletes a stale `WrappedKey` only when the install created a new CNG key.

The component conditions implement the run table in [upgrade.md](../../docs/guide/upgrade.md#when-the-install-steps-run).

## Session name rule

`SESSION_NAME` takes 1 to 128 ASCII letters, digits, underscore, dot, or hyphen, with no leading hyphen. The match is case-sensitive: `^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}\z`. A trailing newline, U+212A, and U+0130 do not match. The provisioning script, `Grant-EtwDnstapEtwAccess.ps1`, and the service's `--session-name` check the same pattern, with the default `ValidatePattern` error text (Windows PowerShell 5.1 has no `ErrorMessage` option).

`Product.wxs` has one launch condition, for the leading hyphen (a leading `-` reads as a parameter on command lines). It refuses the name with `SESSION_NAME must not start with '-'. ...`. A removal skips it. The scripts and the service enforce length and character set, and an invalid name fails the provisioning action and the install.

## Group membership

On install, the script translates `NT SERVICE\EtwDnstap` to a SID and calls `Get-LocalGroupMember`. If that throws, it reads the members by SID through a small `Add-Type` class. If the compile fails, the install fails with `Could not read the members of <group>. By name: <error> By SID: <error>`.

On uninstall, the script uses the SID path only, because the service may be deleted. It uses `NetLocalGroupGetMembers` and `NetLocalGroupDelMembers` at level 0. A failure is a warning. See [What the uninstall removes](../../docs/guide/uninstall.md#what-the-uninstall-removes).

## Spool key code

The script creates the CNG key `EtwDnstap Spool Key` and the registry key `HKLM\SOFTWARE\ETW-DNSTapService\Protect`, and sets and checks their DACLs on every install. The user-facing table is in [install.md](../../docs/guide/install.md#spool-encryption-key). Code-level facts:

- `Protect` sits outside the service key (`HKLM\SYSTEM\...\Services\EtwDnstap`) and outside `HKLM\SOFTWARE\EtwDnstap`. `DeleteServices` deletes the service key, and the old product's removal deletes `SOFTWARE\EtwDnstap`. Both leave `SOFTWARE\ETW-DNSTapService` alone, so a major upgrade keeps the CNG key and `WrappedKey` without a copy step.
- Only the script creates or removes `SOFTWARE\ETW-DNSTapService`. It creates the parent key if missing, with the default ACL, and leaves that ACL unchanged.
- Install deletes a `Parameters\SpoolKey` subkey under `Services\EtwDnstap` if one exists (a failure is a warning). It leaves the rest of `Parameters` and its ACL as is.
- A rollback runs the uninstall action. It cannot tell a created CNG key from an accepted one, so it also deletes an accepted key (one left behind by hand or a failed uninstall).

## Service dependencies

`Product.wxs` declares two `ServiceDependency` rows: `DNS` and `KeyIso` (CNG Key Isolation). The service calls `NCryptOpenStorageProvider`, `NCryptOpenKey`, `NCryptDecrypt`, and `NCryptEncrypt` at start, which can deadlock unless `KeyIso` is running. `KeyIso` exists on Windows Server 2019 and later.

`ServiceInstall` writes the list when it creates the service. A major upgrade, repair, or `REINSTALL=ALL` applies it. A plain maintenance `msiexec /i` does not. Check with `sc.exe qc EtwDnstap`.

## Tests

To test the key and registry functions without an install, run from the repository root:

```powershell
powershell.exe -NoProfile -File packaging\msi\Test-EtwDnstapProvisioningKeys.ps1
```

It uses a per-user CNG key and a throwaway `HKCU` path, and deletes both.

## Lab validation

The lab ran install, failed install with rollback, repair, receiver change, and uninstall ([install.md](../../docs/guide/install.md#install-with-the-msi)). Open runs: major upgrade, group removal by SID, SID fallback, the launch-condition message, and the later component-condition and same-version changes. See [the findings fix plan](../../docs/history/code-review-findings-fix-plan.md#lab-validation-not-part-of-this-plans-agents).

## Version and grant script

[build.rs](../../build.rs) writes the executable's version resource, and [tests/version_resource.rs](../../tests/version_resource.rs) checks it. `Product.wxs` takes the grant script from [scripts/provision/Grant-EtwDnstapEtwAccess.ps1](../../scripts/provision/Grant-EtwDnstapEtwAccess.ps1).

## Build output

Output goes under `target\msi\`. Run the script with `-SkipCargoBuild` after a release build to skip the Rust build. To build the WiX project directly, pass `EtwVersion`, `EtwExePath`, and `EtwRepoRoot`, with paths relative to this directory.
