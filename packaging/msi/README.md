# MSI package sources

> **Status:** Current · **Describes:** the code on branch `spool-encryption` · **Evidence dated:** 2026-11-03 (lab run on Windows Server 2025, build 26100) · **Reviewed:** 2026-11-05

This directory holds the WiX v7 sources for the EtwDnstap MSI. This file covers the sources. For the
steps that users follow, see the guides:

- [Install with the MSI](../../docs/guide/install.md#install-with-the-msi), including the spool key and its ACLs
- [Upgrade, repair, or change settings with the MSI](../../docs/guide/upgrade.md#upgrade-repair-or-change-settings-with-the-msi)
- [Uninstall with the MSI](../../docs/guide/uninstall.md#uninstall-with-the-msi)
- [Build the MSI](../../docs/guide/build.md#build-the-msi)

The build script is [scripts/build/Build-Msi.ps1](../../scripts/build/Build-Msi.ps1). It runs the Rust
build and the CI checks first.

| File | Content |
| --- | --- |
| `EtwDnstap.wixproj` | The project. `WixToolset.Sdk` 7.0.1 and `WixToolset.Util.wixext` 7.0.1, pinned. It sets `AcceptEula` to `wix7`. |
| `Product.wxs` | The package: `MajorUpgrade` with `AllowSameVersionUpgrades="yes"`, launch conditions, the public properties and their remembered values, the install folder, the service with its ACL and SCM policy, the event source, the performance counter manifest, and `ScheduleReboot`. |
| `CustomActions.wxs` | The provisioning custom actions and their sequence rows. See [Custom action sequence](#custom-action-sequence). |
| `Invoke-EtwDnstapProvisioning.ps1` | The script that the custom actions run as SYSTEM, with Windows PowerShell 5.1. Its actions are `Install` and `Uninstall`. It checks the receiver, creates or checks the data folders, creates and removes the spool key and the `Protect` registry key, manages the service SID's group membership, runs the grant script, and sets the recovery policy with `sc.exe failure`. |
| `Test-EtwDnstapProvisioningKeys.ps1` | Tests the key and registry functions without an install. See [Tests](#tests). |

## Custom action sequence

- `ProvisionInstall` runs after `InstallServices`, so that `NT SERVICE\EtwDnstap` resolves. It runs only in a
  pass that installs the executable: `NOT REMOVE AND ($EtwDnstapExe = 3)`. A maintenance `msiexec /i` with no
  `REINSTALL` skips it. The grant and the service's binary path therefore never differ.
- `ProvisionInstallRollback` runs only for a failed first install, with the same component condition.
  It must be scheduled before `ProvisionInstall`, because Windows Installer runs only the rollback actions that
  are already in the script.
- `ProvisionUninstall` runs after `StopServices` and before `DeleteServices`. It is skipped during a major
  upgrade (`UPGRADINGPRODUCTCODE`).

The script never writes `WrappedKey`. It deletes a stale `WrappedKey` only when the install created a new CNG key,
because the old value cannot be unwrapped by that key.

The component conditions above implement the run table in
[upgrade.md](../../docs/guide/upgrade.md#when-the-install-steps-run). A product installed from an MSI built before the

## Session name rule

`SESSION_NAME` takes 1 to 128 ASCII letters, digits, underscore, dot, or hyphen. It is case-sensitive, and it must
not start with a hyphen. The MSI passes the name on command lines, where a leading `-` reads as a parameter.

The rule is `^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}\z`, with no case folding. A trailing newline, and the letters U+212A
and U+0130, do not match. The provisioning script, `Grant-EtwDnstapEtwAccess.ps1`, and the service's `--session-name`
check the same pattern. The pattern uses the default `ValidatePattern` error text, because Windows PowerShell 5.1
has no `ErrorMessage` option.

`Product.wxs` has one launch condition for the leading hyphen. It refuses the name with the message
`SESSION_NAME must not start with '-'. ...`. A removal skips the check. A launch condition cannot test the length or the
character set, so the scripts and the service enforce the full rule. An invalid name that passes the launch condition fails
the provisioning action, and the install fails.

## Group membership

On install, the script checks the membership by account name first. It translates `NT SERVICE\EtwDnstap` to a SID, then calls
`Get-LocalGroupMember`. If that throws, the script reads the members by SID instead. It compiles the small `Add-Type` class for
that once. If the compile fails, the install fails with `Could not read the members of <group>. By name: <error> By SID: <error>`.

On uninstall, the script uses only the SID path, because the service may already be deleted. It reads and removes the membership
through `NetLocalGroupGetMembers` and `NetLocalGroupDelMembers` at level 0. A failure on uninstall is a warning, and the uninstall
continues. See [What the uninstall removes](../../docs/guide/uninstall.md#what-the-uninstall-removes).

## Spool key code

The script creates the CNG key `EtwDnstap Spool Key` and the registry key `HKLM\SOFTWARE\ETW-DNSTapService\Protect`, and it sets and
checks their DACLs on every install. The user-facing table is in [install.md](../../docs/guide/install.md#spool-encryption-key).
Code-level facts:

- `Protect` is outside the service key (`HKLM\SYSTEM\...\Services\EtwDnstap`) and outside `HKLM\SOFTWARE\EtwDnstap`. `DeleteServices`
  deletes the service key, and the old product's removal deletes `SOFTWARE\EtwDnstap`. Neither touches `SOFTWARE\ETW-DNSTapService`.
  So a major upgrade keeps the CNG key and `WrappedKey` without a copy step.
- WiX does not author or remove `SOFTWARE\ETW-DNSTapService`. Only the script does. The script creates the parent key only if it is missing, with
  the default ACL, and it never changes that ACL.
- The installer does not create, read, or change anything under `Services\EtwDnstap\Parameters`, and it never changes that ACL. Install deletes a
  `Parameters\SpoolKey` subkey if one exists. A failure is a warning.
- A rollback of a failed first install runs the uninstall action. The script cannot tell whether the install created the CNG key or accepted an
  existing one. So a rollback also deletes a CNG key that the failed install only accepted. That case needs a key left behind by hand or by a
  failed uninstall.

## Service dependencies

`Product.wxs` declares two `ServiceDependency` rows: `DNS` and `KeyIso` (CNG Key Isolation). The service calls
`NCryptOpenStorageProvider`, `NCryptOpenKey`, `NCryptDecrypt`, and `NCryptEncrypt` while it starts. Microsoft Learn warns that such
calls can deadlock unless `KeyIso` is already running. `KeyIso` exists on Windows Server 2019 and later.

`ServiceInstall` writes the dependency list when it creates the service. A major upgrade removes the old service and creates the
new one, so the new list applies. A repair or `REINSTALL=ALL` runs `InstallServices` again. A plain maintenance `msiexec /i` runs
neither. Check with `sc.exe qc EtwDnstap`.

## Tests

To test the key and registry functions without an install, run this from the repository root:

```powershell
powershell.exe -NoProfile -File packaging\msi\Test-EtwDnstapProvisioningKeys.ps1
```

It uses a per-user CNG key and a throwaway `HKCU` path, and it deletes both.

## Lab validation

Lab validation status is in [install.md](../../docs/guide/install.md#install-with-the-msi). In short, the lab ran install, a failed
install with rollback, repair, a receiver change, and uninstall. It did not run a major upgrade, the group removal by SID, the SID
fallback on install, the launch-condition message, or the later component-condition and same-version changes. Those open runs are listed in
[the findings fix plan](../../docs/history/code-review-findings-fix-plan.md#lab-validation-not-part-of-this-plans-agents).

## Version and grant script

The executable's version resource is written by [build.rs](../../build.rs), and
[tests/version_resource.rs](../../tests/version_resource.rs) checks it.

The grant script is not copied here. `Product.wxs` takes it from
[scripts/provision/Grant-EtwDnstapEtwAccess.ps1](../../scripts/provision/Grant-EtwDnstapEtwAccess.ps1).

## Build output

The project sets `BaseIntermediateOutputPath` and `BaseOutputPath` under `target\msi\`, so no `obj` or `bin` folder is created in
this directory.

To build without the Rust build, run the script with `-SkipCargoBuild` after a release build. To build the WiX project directly,
pass the same properties that the script passes (`EtwVersion`, `EtwExePath`, `EtwRepoRoot`), with paths relative to this directory.
