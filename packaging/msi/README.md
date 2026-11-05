# MSI package sources

> **Status:** Current · **Describes:** the MSI sources at commit `58fde72` · **Evidence dated:** 2026-11-03 (lab run on Windows Server 2025, build 26100) · **Reviewed:** 2026-11-03
>
> The guides that describe the MSI are in [docs/guide/install.md](../../docs/guide/install.md)
> and the plan is [docs/history/msi-installer-plan.md](../../docs/history/msi-installer-plan.md).

This directory holds the WiX v7 sources for the EtwDnstap MSI. The build script
is [scripts/build/Build-Msi.ps1](../../scripts/build/Build-Msi.ps1), which runs
the Rust build and the CI checks first.

| File | Content |
| --- | --- |
| `EtwDnstap.wixproj` | The project. `WixToolset.Sdk` 7.0.1 and `WixToolset.Util.wixext` 7.0.1, pinned. The WiX v7 OSMF EULA acceptance (`AcceptEula` = `wix7`) is in this file, with its source. |
| `Product.wxs` | The package: `MajorUpgrade` with `AllowSameVersionUpgrades="yes"` (every build has a new `ProductCode`, so a same-version rebuild replaces the installed build), launch conditions, the public properties and their remembered values, the install folder, the service with its SCM policy and ACL, the event source, the performance counter manifest, and `ScheduleReboot`. |
| `CustomActions.wxs` | The provisioning custom actions and their sequence rows. Install runs after `InstallServices`, and only when the executable component is installed in the same pass (`NOT REMOVE AND ($EtwDnstapExe = 3)`). A maintenance `msiexec /i` with no `REINSTALL` skips it, so the grant and the service's binary path never differ. Uninstall runs after `StopServices` and before `DeleteServices`. Rollback runs only for a failed first install, with the same component condition as the install. |
| `Invoke-EtwDnstapProvisioning.ps1` | The script that the custom actions run as SYSTEM with Windows PowerShell 5.1. It checks the receiver, creates or checks the data folders, manages the service SID's group membership, runs the grant script, and sets the service recovery policy with `sc.exe failure`. It reads and removes the group membership by the service SID (`NetLocalGroupGetMembers` and `NetLocalGroupDelMembers`), so the uninstall does not need `NT SERVICE\EtwDnstap` to resolve. A failure of that step on uninstall is a warning, and the uninstall continues. On uninstall it deletes only what EtwDnstap created. It also creates and removes the spool encryption key and the `Protect` registry key (see below). Its actions are `Install` and `Uninstall`. |

The `SESSION_NAME` property takes 1 to 128 ASCII letters, digits, underscore, dot,
or hyphen, and it is case-sensitive. The first character must not be a hyphen,
because the MSI passes the name on command lines where a leading `-` reads as a
parameter or option name. The rule is `^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}\z` with
no case folding, so a trailing newline and the letters U+212A and U+0130 do not
match. The provisioning script and `Grant-EtwDnstapEtwAccess.ps1` check it with
the same pattern, and the service's `--session-name` accepts the same names.

`Product.wxs` has one launch condition for the rule. It refuses a `SESSION_NAME`
that starts with `-`, with the message `SESSION_NAME must not start with '-'. Use 1
to 128 ASCII letters, digits, '_', '.', or '-', and do not start with '-'.` A
removal (`REMOVE="ALL"`) skips it. A launch condition cannot test the length or
the character set, so the scripts and the service still enforce the whole rule.
An invalid name that passes the launch condition fails the provisioning action,
and the install rolls back. The pattern uses the default `ValidatePattern` error
text, because Windows PowerShell 5.1 has no `ErrorMessage` option.

On install, the script checks the group membership by account name first
(`NT SERVICE\EtwDnstap` translated to a SID, then `Get-LocalGroupMember`). Only if
that throws does it read the members by SID, and it compiles the small
`Add-Type` class for that once. If the compile fails, the install fails with
`Could not read the members of <group>. By name: <error> By SID: <error>`. On
uninstall it uses only the SID path, because the service may be gone.

The spool encryption key has two parts. Both are created by the install steps and checked
after they are set.

| Item | Created | DACL |
| --- | --- | --- |
| CNG key `EtwDnstap Spool Key` | RSA-3072, Microsoft Software KSP, machine scope, export policy none. | Protected. SYSTEM and Administrators: all. `NT SERVICE\EtwDnstap`: read. The right is the `$SpoolKeyServiceRight` constant in the script. |
| `HKLM\SOFTWARE\ETW-DNSTapService\Protect` | The parent `HKLM\SOFTWARE\ETW-DNSTapService` is created only if missing, with the default ACL, which is never changed. `Protect` is created with its protected DACL in the `CreateSubKey` call, and the DACL is set and checked again on every install and repair. | Protected. SYSTEM and Administrators: full control. The service SID: `0x20003` (query value, set value, read control). |

The service keeps its wrapped spool key in the `WrappedKey` value (`REG_BINARY`) of `Protect`.
The installer never writes `WrappedKey`. `Protect` is outside the service key and outside
`HKLM\SOFTWARE\EtwDnstap`. Deleting the service (`DeleteServices`) deletes its whole registry
key, and the old product's removal deletes `SOFTWARE\EtwDnstap`. Neither touches
`SOFTWARE\ETW-DNSTapService`, so a major upgrade keeps the CNG key and `WrappedKey` without any
copy step. WiX does not author or remove `SOFTWARE\ETW-DNSTapService`: only the script does.

The installer does not create, read, or change anything under
`HKLM\SYSTEM\CurrentControlSet\Services\EtwDnstap\Parameters`, and never changes the ACL of
`Parameters`. An earlier build created `Parameters\SpoolKey`. Install deletes that obsolete
subkey if it exists (a failure is a warning).

If the install creates a new CNG key (none existed) and a `WrappedKey` value is already in
`Protect`, the old value cannot be unwrapped by the new key. The install deletes it and logs the
deletion. An existing CNG key is accepted and kept.

The uninstall steps (a real removal; they do not run during a major upgrade) delete `Protect`,
then `SOFTWARE\ETW-DNSTapService` only if it is then empty, then the CNG key. Each failure is a
`WARNING` line in the log, like the group-membership step, and the uninstall finishes. Delete
what is left by hand. A rollback of a failed first install runs the same uninstall action. The
script cannot tell at that point whether the install created the CNG key or accepted an existing
one, so a rollback also deletes a CNG key that the failed install only accepted. That needs a key
that was left behind by hand or by a failed uninstall.

The service depends on two services, both set in `Product.wxs` with `ServiceDependency`:

- `DNS`, the DNS Server service, as before.
- `KeyIso`, the CNG Key Isolation service. The service calls `NCryptOpenStorageProvider`,
  `NCryptOpenKey`, `NCryptDecrypt`, and `NCryptEncrypt` while it starts, and Microsoft Learn
  warns that a service must not call these from its start routine without `KeyIso`
  already running (it can deadlock). SCM starts `KeyIso` first. `KeyIso` is the service name of
  "CNG Key Isolation" and is present on Windows Server 2019 and later (it has existed since
  Windows Vista).

`ServiceInstall` writes the dependency list when it creates the service. A major upgrade removes
the old service and installs the new one, so the new list applies. A repair or `REINSTALL=ALL`
runs `InstallServices` again, which rewrites the service configuration. A plain maintenance
`msiexec /i` runs neither. An MSI from before this change has only `DNS`; upgrade it with a
major upgrade (any new build) or repair. Check with `sc.exe qc EtwDnstap`.

To test the key and registry functions without an install, run
`powershell.exe -NoProfile -File packaging\msi\Test-EtwDnstapProvisioningKeys.ps1`. It uses a
per-user key and a throwaway `HKCU` path, and deletes both.

| Run | Install steps | Uninstall steps |
| --- | --- | --- |
| First install | Run | Skip |
| Major upgrade, including a same-version rebuild | Run | Skip |
| Repair (`/fvomus`, `/fvamus`) or `REINSTALL=ALL` | Run | Skip |
| Maintenance `msiexec /i`, no `REINSTALL` | Skip | Skip |
| Uninstall | Skip | Run |

The same MSI file has the same `ProductCode`, so running it again is a
maintenance run, not an upgrade. A product installed from an MSI built before the
component condition keeps the old behavior until it is upgraded. See
[Upgrade, repair, or change settings with the MSI](../../docs/guide/upgrade.md#when-the-install-steps-run).

Lab validation: passed on one lab DNS Server (Windows Server 2025, build 26100)
on 2026-11-03: install, a failed install and its rollback, repair, a receiver
change, and uninstall, each with a restart. A major upgrade from an older MSI
has not been tested. See [the plan and its results](../../docs/history/msi-installer-plan.md).
The later changes to the component condition, the group removal by SID, same-version
upgrades, and the session-name rule were not run in the lab. Those runs are open
in [the findings fix plan](../../docs/history/code-review-findings-fix-plan.md#lab-validation-not-part-of-this-plans-agents).
The group removal by SID, the install fallback to the SID path, and the launch
condition message are unverified.

The guides that describe the MSI are [Install with the MSI](../../docs/guide/install.md#install-with-the-msi),
[Uninstall with the MSI](../../docs/guide/uninstall.md#uninstall-with-the-msi),
[Upgrade, repair, or change settings with the MSI](../../docs/guide/upgrade.md#upgrade-repair-or-change-settings-with-the-msi),
and [Build the MSI](../../docs/guide/build.md#build-the-msi).
The executable's version resource is written by [build.rs](../../build.rs), and
[tests/version_resource.rs](../../tests/version_resource.rs) checks it.

The grant script is not copied here. `Product.wxs` takes it from
[scripts/provision/Grant-EtwDnstapEtwAccess.ps1](../../scripts/provision/Grant-EtwDnstapEtwAccess.ps1).

Build output goes under `target\msi\`. The project sets `BaseIntermediateOutputPath`
and `BaseOutputPath` there, so no `obj` or `bin` folder is created in this directory.

To build without the Rust build, run the script with `-SkipCargoBuild` after a
release build. To build the WiX project directly, pass the same properties that
the script passes (`EtwVersion`, `EtwExePath`, `EtwRepoRoot`), with paths relative
to this directory.
