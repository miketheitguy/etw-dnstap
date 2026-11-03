# MSI installer plan

> **Status:** Executed on `plan/msi-installer`, not merged to `main` · **Evidence dated:** 2026-11-03 · **Commit:** written at `7903bb7`, executed through `531ae0d` · **Reviewed:** 2026-11-03

This plan adds a Windows Installer package (MSI), built with the WiX Toolset, that
does everything that [install.md](../guide/install.md) steps 2 to 6 do by hand:
it installs the files, sets the folder ACLs, registers and configures the
service, grants ETW access, registers the performance counters, and registers the
Application event log source. The uninstall reverses all of it. One version
number, the `version` field in `Cargo.toml`, flows into the executable's version
resource, the MSI `ProductVersion`, and the MSI file name.

## Decisions

The maintainer made these decisions on 2026-11-03:

1. **WiX v7, OSMF EULA accepted.** The build uses `WixToolset.Sdk` 7.x, pinned
   to an exact version. WiX v7 requires an explicit acceptance of the Open
   Source Maintenance Fee EULA (<https://wixtoolset.org/osmf/>). The maintainer
   accepted it. The build script passes the acceptance and the build guide
   says so.
2. **Uninstall deletes everything the product created under
   `C:\ProgramData\EtwDnstap`.** That is the `spool` folder (unsent DNS data)
   and the `backup` folder (the grant script's ETW snapshots), and then the
   `EtwDnstap` folder if it is empty. Content that the product did not create
   (for example the lab's `performance` folder) is kept and logged. A major
   upgrade deletes nothing.
3. **The lab guest stays MSI-installed.** After the install and removal tests,
   the lab test installs the MSI again with the lab's settings and leaves it.

Decisions made by the orchestrator, from the existing guides:

4. **No user interface.** The MSI is for `msiexec /qn` with properties. A
   double-click install shows only the basic progress dialog.
5. **Delayed automatic start, reboot required.** ETW applies the session GUID
   grant only at boot (install.md step 4). The MSI does not start the service.
   It sets `delayed-auto` and schedules a reboot (exit code 3010) on install and
   on uninstall. The service starts after the reboot. This replaces the guide's
   "Manual until step 7 passes" order. The SCM recovery policy limits the cost of
   a bad configuration to one restart every 60 seconds.
6. **No pre-release MSI.** MSI `ProductVersion` has three numeric fields and no
   pre-release part, so two pre-releases of one core version cannot upgrade each
   other. The build script refuses a version with a pre-release or build
   metadata part. The executable's version resource still carries the full
   SemVer string in `ProductVersion`, so a pre-release can be built and tested
   with `cargo`.
7. **Version stays `0.1.0`.** The repository has no release yet, and every
   change stays under `[Unreleased]` (CHANGELOG versioning policy). Adding the
   MSI is a non-breaking addition, a patch-level change under the local policy,
   and it is recorded under `[Unreleased]` without a version change.
8. **Unsigned.** The repository has no code-signing certificate. Signing the
   executable and the MSI is out of scope and listed under risks.

## Versioning

| Item | Source | Format | Example |
| --- | --- | --- | --- |
| Crate | `Cargo.toml` `version` | SemVer 2.0.0 | `0.1.0` |
| Executable `FILEVERSION` / `PRODUCTVERSION` (binary) | `CARGO_PKG_VERSION_MAJOR/MINOR/PATCH` in `build.rs` | `X,Y,Z,0` | `0,1,0,0` |
| Executable `FileVersion` string | same | `X.Y.Z` | `0.1.0` |
| Executable `ProductVersion` string | `CARGO_PKG_VERSION` | full SemVer | `0.1.0` |
| `capture started` `version` field | `CARGO_PKG_VERSION` (exists) | full SemVer | `0.1.0` |
| MSI `ProductVersion` | build script, from `cargo metadata` | `X.Y.Z` | `0.1.0` |
| MSI file | build script | `etw-dnstap-X.Y.Z-x64.msi` | `etw-dnstap-0.1.0-x64.msi` |
| MSI `UpgradeCode` | fixed, never changes | GUID | `{0F4D1A87-DAD8-4C37-ABD4-FEC4DA400FAB}` |
| MSI `ProductCode` | new for every build | GUID | generated |

MSI limits: major and minor 0 to 255, patch 0 to 65535. The build script refuses
other values. The build script checks after the build that the executable's
`FileVersion`, the MSI `ProductVersion`, and `Cargo.toml` agree.

## Constants

| Name | Value |
| --- | --- |
| Service name | `EtwDnstap` |
| Service SID (`sc.exe showsid EtwDnstap`) | `S-1-5-80-3892396922-4053150511-3308241269-1610546069-1753937115` |
| Install folder | `[ProgramFiles64Folder]EtwDnstap\` |
| Data folder | `[CommonAppDataFolder]EtwDnstap\` (`spool`, `backup`) |
| Service security descriptor | the step 4 string in install.md |
| Event source | `Application` log, source `EtwDnstap` |

## Changes

### Version resource (V)

- **V1. `VERSIONINFO` in the executable.** `build.rs` writes an `.rc` file into
  `OUT_DIR` from the `CARGO_PKG_VERSION_*` variables and compiles it with
  `embed-resource`, next to the existing counter string table. One
  `VERSIONINFO` only. String values: `CompanyName` and `LegalCopyright` from
  `LICENSE` (`miketheitguy`), `FileDescription` from the package description,
  `ProductName` `EtwDnstap`, `OriginalFilename` and `InternalName`
  `etw-dnstap.exe`, `FileVersion` `X.Y.Z`, `ProductVersion` full SemVer. The
  generated file must hold no build-host path. The counter strings must not
  change: `typeperf -q EtwDnstap` lists the same names.
- **V2. Test.** A test reads the built executable's version resource and checks
  every field against `CARGO_PKG_VERSION`. If that needs `unsafe`, follow the
  repository lint rules (`// SAFETY:` comments, one unsafe operation for each
  block).

### WiX package (W)

All WiX sources are in `packaging/msi/`.

- **W1. Project.** `packaging/msi/EtwDnstap.wixproj` uses
  `Sdk="WixToolset.Sdk/<exact 7.x>"` and `WixToolset.Util.wixext` with the same
  exact version. The OSMF EULA acceptance uses the mechanism that the official
  WiX v7 documentation names. Record the mechanism in the project file comment.
  `Version`, the executable path, and the output name come in as MSBuild
  properties from the build script. ICE validation stays on.
- **W2. Package.** `Package` with `Name="EtwDnstap"`, `Manufacturer="miketheitguy"`,
  `Version` from the build script, the fixed `UpgradeCode`, `Scope="perMachine"`,
  x64, compressed media embedded in the MSI. `MajorUpgrade` with a downgrade
  error. Add/Remove Programs fields: no modify button (`ARPNOMODIFY`), a help
  link to the repository.
- **W3. Launch conditions.** 64-bit Windows. The `DNS` service exists
  (registry search on `HKLM\SYSTEM\CurrentControlSet\Services\DNS`).
  `RECEIVER` is set, or a remembered value exists (W9).
- **W4. Files and the install folder.** The install folder holds
  `etw-dnstap.exe`, `etw-dnstap.man`,
  `Grant-EtwDnstapEtwAccess.ps1` (copied from `scripts/provision/`, not
  duplicated in the repository), and the provisioning script from W7. The folder
  gets `PermissionEx` with the SDDL
  `O:BAG:SYD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;<service SID>)`.
  That is install.md step 2 plus the step 3 read and execute grant. The
  manifest registers with `util:PerfCounterManifest`
  (`ResourceFileDirectory="[INSTALLFOLDER]"`), which is the `lodctr /m:` of
  step 3.
- **W5. Service.** `ServiceInstall`: `Name="EtwDnstap"`, own process,
  `Account="NT AUTHORITY\LocalService"`, `Start="auto"`, `ErrorControl="normal"`,
  `Arguments` `--receiver "[RECEIVER]" --session-name "[SESSION_NAME]" [SERVICE_ARGS]`,
  and `ServiceDependency Id="DNS"`. Children:
  - `ServiceConfig` (MSI 5.0 `MsiServiceConfig`): `DelayedAutoStart="yes"`,
    `ServiceSid="restricted"`, `RequiredPrivilege` `SeChangeNotifyPrivilege`
    only, `FailureActionsWhen="failedToStopOrReturnedError"` (the
    `failureflag 1`), on install and reinstall.
  - `ServiceConfigFailureActions`: `ResetPeriod="86400"`, restart after
    5000 ms, 5000 ms, and 60000 ms (install.md step 5).
  - `PermissionEx` with the install.md step 4 service SDDL
    (`MsiLockPermissionsEx` on the service).
  - `ServiceControl`: `Stop="both"`, `Remove="uninstall"`, `Wait="yes"`. No
    `Start` (decision 5).
- **W6. Event source.** `util:EventSource` for log `Application`, source
  `EtwDnstap`. `EventMessageFile` is the same file that `New-EventLog` writes:
  `[WindowsFolder]Microsoft.NET\Framework64\v4.0.30319\EventLogMessages.dll`.
  The lab test compares the MSI registration with the one `New-EventLog` made
  on the lab guest before it is removed.
- **W7. Provisioning script.** `packaging/msi/Invoke-EtwDnstapProvisioning.ps1`
  is installed to the install folder and run by `WixQuietExec64` custom actions:
  deferred, `Impersonate="no"`, Windows PowerShell 5.1 by full path
  (`[System64Folder]WindowsPowerShell\v1.0\powershell.exe -NoLogo -NoProfile
  -NonInteractive -ExecutionPolicy Bypass -File "[INSTALLFOLDER]..."`). The
  script writes one line for each change to the MSI log and exits non-zero on
  the first failure. It writes no secrets.
  - `-Action Install -SessionName <name> -Receiver <ip:port>` (after
    `InstallServices`, so that `NT SERVICE\EtwDnstap` resolves):
    1. Check the receiver as install.md step 1 does. Stop on a bad value.
    2. Create or check `ProgramData\EtwDnstap` and `spool` with the install.md
       `New-ProtectedFolder`, `Test-ProtectedFolder`, and `Test-SpoolFolder`
       rules. A folder that exists and passes its check is accepted (this
       covers upgrade and repair). A folder that exists and fails is never
       taken over: the install stops and rolls back.
    3. Add the service SID to the group with SID `S-1-5-32-559`. Resolve the
       group name from the SID, because the name is localized. Do nothing if
       it is already a member.
    4. Run `Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` from the install
       folder.
  - `-Action Uninstall -SessionName <name>` (after `StopServices`, before
    `RemoveFiles`):
    1. Stop the ETW session `<name>` if it exists (uninstall.md step 5).
    2. Remove the group membership.
    3. Run `Grant-EtwDnstapEtwAccess.ps1 -Remove`.
    4. Delete `spool` and `backup` under `ProgramData\EtwDnstap` (decision 2).
       Refuse a reparse point at any level, as uninstall.md step 7 does. Then
       delete `ProgramData\EtwDnstap` if it is empty. List what is kept.
  - Conditions: Install on install, repair, and major upgrade. Uninstall only
    on `REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE`, so the old product's
    removal during a major upgrade keeps the grants, the membership, and the
    spool. A rollback action runs `-Action Uninstall` only when a first install
    fails (`NOT Installed AND NOT WIX_UPGRADE_DETECTED`), so a failed upgrade
    does not deprovision a working installation.
- **W8. Reboot.** `ScheduleReboot` on install, upgrade, and uninstall. With
  `/qn /norestart`, `msiexec` exits with 3010.
- **W9. Properties.** `RECEIVER` (required), `SESSION_NAME` (default
  `EtwDnstap`), `SERVICE_ARGS` (default empty; other options from
  configuration.md, for example `--plaintext`). All three are secure public
  properties. Repair and upgrade reuse the values from the last install: they
  are stored under `HKLM\SOFTWARE\EtwDnstap\Installer` with an ACL for SYSTEM
  and Administrators only (`PermissionEx`), read back with `RegistrySearch`,
  and removed on uninstall.

### Build script (B)

`scripts/build/Build-Msi.ps1`, PowerShell 7, from the repository root:

1. Read the version with `cargo metadata --no-deps --format-version 1`. Check
   it against the SemVer 2.0.0 grammar. Refuse a pre-release or build metadata
   part (decision 6) and values outside the MSI limits.
2. `cargo build --locked --release`, with the output saved to
   `target\msi\release-build.log`. `-SkipCargoBuild` reuses an existing
   executable.
3. Run `scripts/ci/verify-link-exceptions.ps1` and `scripts/ci/verify-pe.ps1`
   on that build.
4. Check the executable's `FileVersion` (`X.Y.Z.0` binary, `X.Y.Z` string)
   and `ProductVersion` string.
5. `dotnet build packaging\msi\EtwDnstap.wixproj -c Release` with the version
   and paths, output to `target\msi\etw-dnstap-X.Y.Z-x64.msi`.
6. Read the MSI `Property` table (Windows Installer COM): `ProductVersion`,
   `UpgradeCode`, `ProductName`, `Manufacturer`. Stop on a difference.
7. Write `etw-dnstap-X.Y.Z-x64.msi.sha256` next to the MSI and print the path,
   version, and hash.

The script uses repository-relative paths only and writes nothing outside
`target\`. A failed step stops the script with a non-zero exit code.

### Lab tests (L)

Scripts in `scripts/lab/msi/`. The host script runs in Windows PowerShell 5.1,
imports `scripts/performance/LabCommon.psm1`, and reaches the guest through
PowerShell Direct (`Get-LabVm`, `Get-LabSession`). Results go to
`target\lab-msi\<yyyyMMdd-HHmmss>\` (ignored). The receiver on
DNSTap-Ubuntu2604 sends DNS-collector output to its null logger. Deliveries
are counted with `ss -tin` on the receiver, never from collector logs.

- `Invoke-MsiLabCycle.ps1`: the host driver for L0 to L7.
- `guest/Remove-ManualEtwDnstap.ps1`: removes a hand-provisioned install
  (uninstall.md steps 1 to 6 and 8). It leaves `ProgramData\EtwDnstap`
  content in place.
- `guest/Test-EtwDnstapInstalled.ps1`: the installed-state assertions.
- `guest/Test-EtwDnstapRemoved.ps1`: the removed-state assertions.

Each assertion prints `PASS` or `FAIL` with its name and the value found. A
script exits non-zero if any assertion fails.

| Step | Action | Expected |
| --- | --- | --- |
| L0 | Check the collector config. Take checkpoint `msi-installer-pre-<date>`. Save `sc.exe qc`, the event source registry key, and the folder ACLs. | Checkpoint exists. |
| L1 | Stop and remove the hand-provisioned install. Reboot. | `Test-EtwDnstapRemoved` passes, except for the kept data. |
| L2 | `msiexec /i` with no `RECEIVER`. | Fails (1603). Nothing changed. |
| L3 | `msiexec /i` with a hostname, `RECEIVER=dnstap-ubuntu:6000`. | Fails and rolls back. `Test-EtwDnstapRemoved` passes. |
| L4 | `msiexec /i` with the lab values (`RECEIVER`, `SESSION_NAME=EtwDnstap-Lab`, `SERVICE_ARGS` from the saved `ImagePath`). Reboot. | Exit 3010. `Test-EtwDnstapInstalled` passes. |
| L5 | `msiexec /fvomus` (repair) with no properties. Reboot. | Exit 0 or 3010. `ImagePath` unchanged. `Test-EtwDnstapInstalled` passes. |
| L6 | `msiexec /x`. Reboot. | Exit 3010. `Test-EtwDnstapRemoved` passes. |
| L7 | Install again with the lab values (decision 3). Reboot. | `Test-EtwDnstapInstalled` passes. The guest is left installed. |

`Test-EtwDnstapInstalled` checks:

- `sc.exe qc`: `BINARY_PATH_NAME` as expected, `START_TYPE` `AUTO_START
  (DELAYED)`, dependency `DNS`, `SERVICE_START_NAME`
  `NT AUTHORITY\LocalService`.
- `sc.exe qsidtype` `RESTRICTED`. `sc.exe qprivs` lists only
  `SeChangeNotifyPrivilege`.
- `sc.exe qfailure` (86400, restart 5000/5000/60000) and
  `sc.exe qfailureflag` 1.
- `sc.exe sdshow` equals the install.md step 4 string exactly.
- The install folder ACL is the W4 SDDL. The `ProgramData\EtwDnstap` and
  `spool` ACLs pass the install.md checks.
- Performance Log Users holds `NT SERVICE\EtwDnstap`.
- The ETW descriptor of the session GUID for `SESSION_NAME` holds an
  `0x20` allow ACE for the service SID, and no other descriptor holds an ACE
  for it.
- The event source key matches the L0 copy.
- `typeperf -q EtwDnstap` lists the counters.
- The executable `FileVersion` and the Add/Remove Programs `DisplayVersion`
  equal `Cargo.toml`'s version.
- The service reaches `RUNNING` within 5 minutes of boot. Event ID 2
  (`capture started`) has `version` equal to `Cargo.toml`'s version. After a
  few DNS queries in the guest, `bytes_received` on the receiver rises.

`Test-EtwDnstapRemoved` checks:

- `sc.exe query EtwDnstap` gives error 1060.
- The install folder does not exist.
- `ProgramData\EtwDnstap\spool` and `\backup` do not exist. The
  `EtwDnstap` folder does not exist, or holds only content that was there
  before L1 (the lab's `performance` folder).
- Performance Log Users does not hold `NT SERVICE\EtwDnstap`.
- No descriptor under `HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security`
  holds an ACE for the service SID.
- The `EtwDnstap` event source key does not exist.
- `typeperf -q EtwDnstap` lists nothing.
- `logman query -ets` shows no session named `SESSION_NAME`.
- No Add/Remove Programs entry has the `UpgradeCode`, and
  `HKLM\SOFTWARE\EtwDnstap` does not exist.

### Documentation (D)

- **D1. `docs/guide/install.md`.** A new first section, "Install with the
  MSI": prerequisites, the `msiexec` command line, the properties, the reboot,
  and a table that maps each MSI action to the manual step. The manual steps
  stay, as the reference and for hosts without the MSI.
- **D2. `docs/guide/uninstall.md`.** An "Uninstall with the MSI" section: what
  it removes (decision 2), what it keeps, and the reboot.
- **D3. `docs/guide/upgrade.md`.** Major upgrade with the MSI: properties
  reused, spool kept, reboot.
- **D4. `docs/guide/build.md`.** "Build the MSI": .NET SDK, WiX v7 restored
  from NuGet, the OSMF EULA (decision 1), `scripts/build/Build-Msi.ps1`, and
  the output.
- **D5. `CHANGELOG.md`.** `[Unreleased]` `Added` entries for the MSI and the
  version resource. In the versioning policy, say that the executable version
  resource and the MSI `ProductVersion` come from `Cargo.toml`, and that an MSI
  is not built from a pre-release version.
- **D6. `packaging/msi/README.md`** for the WiX sources, and a pointer in
  `docs/README.md`. Status headers of every changed guide.

## Execution

| Step | Owner | Branch | Where | Depends on |
| --- | --- | --- | --- | --- |
| 1. This plan | orchestrator | `plan/msi-installer` | main checkout | none |
| 2. V1, V2 | Rust agent | `msi/version` | worktree | 1 |
| 3. W1 to W9, B | build agent | `msi/package` | main checkout | 1; merges 2 when it is done |
| 4. L scripts and the lab run | lab agent | `msi/lab`, cut from 3 | main checkout | 3 |
| 5. D1 to D6 | documentation agent | `msi/docs`, cut from 3 | worktree | 3 |
| 6. Review, gates, merge | orchestrator | `plan/msi-installer` | main checkout | 4, 5 |

Steps 2 and 3 run at the same time, as do steps 4 and 5. The build and lab
agents run PowerShell and the lab credential, so they work in the main
checkout, one at a time. Worktree agents do not run PowerShell.

## Gates

Code changes pass these before they are handed back:

```powershell
cargo fmt --check
cargo test --locked
cargo test --locked --features diag-timing
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features diag-timing -- -D warnings
cargo clippy --locked --all-targets --features bench -- -D warnings
```

Step 3 and step 6 also pass `scripts/build/Build-Msi.ps1` end to end (it runs
`verify-pe.ps1` and `verify-link-exceptions.ps1`), with no ICE error. Every
PowerShell script passes `Invoke-ScriptAnalyzer` with no warnings or errors, if
PSScriptAnalyzer is installed. Every commit passes the `CLAUDE.md` host-path
check.

## Risks

- **Auto start before validation.** A bad `RECEIVER` or `SERVICE_ARGS` value
  makes the service fail at each boot. The recovery policy keeps the retry rate
  at one each 60 seconds, and Event ID 1 names the cause. The provisioning
  script checks `RECEIVER` before the commit.
- **A custom action runs PowerShell as SYSTEM.** The scripts are run from the
  protected install folder by full path, with no profile. No user-writable path
  is on that path.
- **Data deletion on uninstall.** Decision 2 deletes the ETW snapshots too, so
  `-Restore` is not possible after an uninstall. The uninstall's own `-Remove`
  is the reversal.
- **Lab exe replacement.** Performance scripts that copy a new executable over
  the installed one make the installed file differ from the MSI. A repair puts
  the MSI's file back.
- **Unsigned binaries.** SmartScreen and application control policies can
  block an unsigned MSI. Signing needs a certificate, which is a separate
  decision.
- **OSMF.** The EULA acceptance is recorded in the build. A change of the
  maintainer's organization can change whether the fee applies.

## Results

| Step | Commit | Result |
| --- | --- | --- |
| 1. Plan | `3ea6ba2` | Written. |
| 2. V1, V2 | `8964ad5` | As planned. `build.rs` compiles the generated `VERSIONINFO` `.rc` and the counter `.rc` separately. `tests/version_resource.rs` checks every field; it was shown to fail with a wrong expected value. |
| 3. W1 to W9, B | `80e2652` to `20e5b01`, `1aad4d0` | WiX `WixToolset.Sdk` and `WixToolset.Util.wixext` 7.0.1. The EULA is accepted with `<AcceptEula>wix7</AcceptEula>` (<https://docs.firegiant.com/wix/osmf/>). `1aad4d0` (orchestrator review) moved the provisioning rollback before `ProvisionInstall`, and the provisioning uninstall before `DeleteServices`. |
| 4. L scripts, lab run | `a81f420` to `531ae0d` | L0 to L7 and L5b passed in one run from the checkpoint. See below. |
| 5. D1 to D6 | `3af7033`, `f642f24`, merged in `244f47e` | As planned. The orchestrator then corrected the guides from the lab results. |
| 6. Gates | `244f47e` | All six cargo gates pass: 417 + 2 tests (default) and 422 + 2 tests (`diag-timing`), 7 ignored in each. `Build-Msi.ps1` end to end from a clean release build: link exceptions 6 checks, 0 failed; PE 21 checks, 0 failed; `FileVersion` and `ProductVersion` 0.1.0; MSI `ProductVersion` 0.1.0. Link check: 0 broken. |

### Lab run

Guest DNSTap-WS2025 (Windows Server 2025, build 26100). Checkpoint
`msi-installer-pre-2026-11-03` was taken first and is kept. The receiver routed
to its `devnull` logger, with its stdout and stderr sent to null. Results are in
`target\lab-msi\20261103-040548\` on the host (not tracked).

| Step | msiexec exit | Assertions |
| --- | --- | --- |
| L0 save state, checkpoint | none | 7 pass |
| L1 remove the hand-provisioned install, restart | none | 14 pass |
| L2 no `RECEIVER` | 1603 | 15 pass, nothing changed |
| L3 hostname `RECEIVER`, rollback | 1603 | 14 pass, removed state |
| L4 install, restart | 3010 | 34 pass |
| L5 repair (`/fvomus`), restart | 3010 | 35 pass, `ImagePath` unchanged |
| L5b receiver change | 3010 (each run) | 7 pass |
| L6 uninstall, restart | 3010 | 14 pass, removed state |
| L7 install again, restart | 3010 | 34 pass, left installed |

- The service reached `RUNNING` 122 to 124 seconds after boot (delayed start).
  `capture started` had `version="0.1.0"`. After DNS queries in the guest, the
  receiver's `bytes_received` rose (84 to 23524).
- `sc.exe sdshow` equals the install.md step 4 string, SACL included.
  `qsidtype` RESTRICTED, `qprivs` `SeChangeNotifyPrivilege` only,
  `AUTO_START (DELAYED)`, `qfailure` 86400 and 5000/5000/60000, and
  `FAILURE_ACTIONS_ON_NONCRASH_FAILURES: TRUE`.
- The grant script ran under SYSTEM for the grant and for `-Remove`. After L4,
  exactly one `0x20` allow ACE for the service SID was on the session GUID of
  `EtwDnstap-Lab`, and none elsewhere. After L6, none of 394 descriptors held an
  ACE for it.
- `typeperf -q EtwDnstap` listed 34 counters with the new executable.
- The L6 log shows `StopServices`, `ProvisionUninstall`, `DeleteServices`, then
  `RemoveFiles`. `NT SERVICE\EtwDnstap` resolved for the group removal and
  `-Remove`.
- In the L5 log, `AppSearch` set `RECEIVER_REMEMBERED` before `LaunchConditions`.

### Defects found in the lab

| Commit | Defect | Fix |
| --- | --- | --- |
| `a81f420` | `Grant-EtwDnstapEtwAccess.ps1` `Complete-Backup` joined two lists with `@() + @()`. Windows PowerShell 5.1 threw "Argument types do not match" when one was empty, so a run that changed a value failed at its end. This affects the manual steps too. | Join the `ToArray()` copies. |
| `aac2eff` | The provisioning script passed `BUILTIN\Performance Log Users` to `Get-LocalGroupMember` and `net.exe`, which reject it; splatted an array, so `-Remove` arrived as a positional string; and did not answer the grant script's `ConfirmImpact=High` prompt. | Bare group name, a hashtable splat, `-Confirm:$false`. |
| `addf37e` | The `MsiServiceConfigFailureActions` row failed with access denied, then error 1939, and the install rolled back. The other `MsiServiceConfig` operations succeeded. The root cause was not found. | The provisioning install action runs the install.md step 5 `sc.exe failure` command. The `ServiceConfig` row still sets the non-crash flag. |
| `2ee99f3` | `util:EventSource` with `Supports*` attributes wrote `TypesSupported`, which `New-EventLog` does not. | Attributes removed. The key now holds only `EventMessageFile` (`REG_EXPAND_SZ`), as `New-EventLog` writes it. |

### Differences from the plan

- The install folder DACL reads `D:P(...)`, not `D:PAI(...)`.
  `MsiLockPermissionsEx` writes a protected DACL without the auto-inherited
  flag. The ACEs are the W4 ACEs.
- L5 (repair) exits with 3010, because `ScheduleReboot` is unconditional. Repair
  and reinstall stop the service and do not start it again until the restart.
- **Receiver change (L5b).** Plain `msiexec /i <msi> RECEIVER=<new>` on the
  installed product exited 3010 but changed neither `ImagePath` nor the
  remembered value. The provisioning install action still ran.
  `REINSTALL=ALL REINSTALLMODE=vomus RECEIVER=<new>` changed both. upgrade.md
  documents the second form.
- WixQuietExec formats each output line as an MSI record, so the
  `[EtwDnstap]` prefix is read as an empty property and dropped. The log shows
  `WixQuietExec:   <text>`. The guides say so.
- **Snapshots deleted.** The L3 rollback and the L6 uninstall deleted
  `backup` folders that came before the MSI: grant-script snapshots from earlier
  lab work, including copies of `etw-dnstap.exe` and `service.reg` from
  2026-11-01. This follows decision 2. The checkpoint still holds them. The
  uninstall guide warns about it.

### Not covered

- A major upgrade from an older MSI version. There is only one version.
- A session name change through `REINSTALL=ALL`.
- The uninstall.md manual order (`sc.exe delete` before `net localgroup /delete`).

### Follow-up

- **Maintenance runs without `REINSTALL`.** The provisioning install action runs
  on every non-removal run (`NOT REMOVE`), including a plain `msiexec /i` on the
  installed product, which reinstalls nothing. With a new `SESSION_NAME`, that
  moves the ETW grant to the new GUID while `ImagePath` keeps the old name, and
  the service fails after the restart. upgrade.md tells operators to use
  `REINSTALL=ALL`. A fix in the package is to condition `ProvisionInstall` on the
  executable component being installed or reinstalled (`$EtwDnstapExe=3`), and
  `ScheduleReboot` with it. That needs a new lab run of L4, L5, and L5b.
- Signing the executable and the MSI (decision 8).
- A CI job that runs `Build-Msi.ps1`.
