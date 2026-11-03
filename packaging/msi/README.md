# MSI package sources

> **Status:** Current for the sources in this directory. The guides that describe
> the MSI are in [docs/guide/install.md](../../docs/guide/install.md) and the
> plan is [docs/plans/msi-installer-plan.md](../../docs/plans/msi-installer-plan.md).

This directory holds the WiX v7 sources for the EtwDnstap MSI. The build script
is [scripts/build/Build-Msi.ps1](../../scripts/build/Build-Msi.ps1), which runs
the Rust build and the CI checks first.

| File | Content |
| --- | --- |
| `EtwDnstap.wixproj` | The project. `WixToolset.Sdk` 7.0.1 and `WixToolset.Util.wixext` 7.0.1, pinned. The WiX v7 OSMF EULA acceptance (`AcceptEula` = `wix7`) is in this file, with its source. |
| `Product.wxs` | The package: `MajorUpgrade`, launch conditions, the public properties and their remembered values, the install folder, the service with its SCM policy and ACL, the event source, the performance counter manifest, and `ScheduleReboot`. |
| `CustomActions.wxs` | The provisioning custom actions and their sequence rows. Install runs after `InstallServices`. Uninstall runs before `RemoveFiles`. Rollback runs only for a failed first install. |
| `Invoke-EtwDnstapProvisioning.ps1` | The script that the custom actions run as SYSTEM with Windows PowerShell 5.1. It checks the receiver, creates or checks the data folders, manages the service SID's group membership, and runs the grant script. On uninstall it deletes only what EtwDnstap created. |

Lab validation: passed on one lab DNS Server (Windows Server 2025, build 26100)
on 2026-11-03: install, a failed install and its rollback, repair, a receiver
change, and uninstall, each with a restart. A major upgrade from an older MSI
has not been tested. See [the plan and its results](../../docs/plans/msi-installer-plan.md).
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
