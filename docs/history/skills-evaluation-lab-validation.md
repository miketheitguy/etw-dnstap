# Skills evaluation lab validation

> **Status:** Historical · **Evidence dated:** 2026-11-01 (guest local time, America/Los_Angeles; Application log lines are UTC) · **Commit:** `77d7c47` plus an uncommitted fix to `scripts/provision/Grant-EtwDnstapEtwAccess.ps1` (see "Defect found") · **Reviewed:** 2026-11-01

This record covers the lab run for the changes made after the skills-driven
evaluation: the removal of the instance mutex, the GUID-checked session reclaim,
the DNS process handle, the pinned service descriptor, delayed automatic start,
and the backup and restore of ETW and service state. It follows
[the first LocalService record](localservice-restricted-validation.md).

## Environment

| Component | State |
| --- | --- |
| Windows VM | `DNSTap-WS2025` (`24bafedb-0998-4430-a0c6-7e90350cd9ac`), Windows Server 2025 Standard Evaluation, build 26100, DNS Server role. Identity checked with `Get-LabVm` before every action. |
| Receiver | DNS-collector on `DNSTap-Ubuntu2604`, `172.27.215.60:6000`. Not touched. The service reported `receiver_connected=true`. |
| Checkpoints | `pre-localservice-acl-test-2026-11-01` (kept) and `eval-pre-2026-11-01` (new, taken before any change) |
| Host commit | `77d7c47`. Working tree: only `scripts/provision/Grant-EtwDnstapEtwAccess.ps1`, `docs/guide/install.md` and `docs/guide/troubleshooting.md` modified by this run. |
| Service | `EtwDnstap`, `--receiver 172.27.215.60:6000 --session-name EtwDnstap-Lab` (binary path unchanged from the earlier lab) |
| Release exe (deployed) | SHA-256 `4D058D96D4E4FAC2F3BC87248C565F4E3D89A0FD06F93C09CE937B674CC44941`, built from `77d7c47` with `cargo build --locked --release` |
| Unit-test exe (guest) | `etw_dnstap-813bad2f42f8fb88.exe`, SHA-256 `1921419E8DEE8EAB2BD2B2DAB04F78BA686E371CA97A2B83F869669EF2D95590` (`77d7c47`) |
| Earlier builds | Test run 1 used the `700dd62` build: release `177632FDE67E181F12092B1F01036EC7FB5E080CC251A23299A33C9227EBFEFA`, test exe `744683C0E430E7B5C08AB1B6C81C1B9664C5908B79E420DB6D7E91354BF326B7`. Pre-eval installed exe: `E92DB5F185262900337686CDE874C85208173E0C3FA3BAE70E7231F1D0380156`. |
| PE check | `scripts/ci/verify-pe.ps1` on the deployed exe: 18 checks, 0 failed (x64, large-address aware, high-entropy ASLR, dynamic base, NX, CFG with 833 functions, EH continuation 11, CET marker, cookie, no PDB entry, no ASan, no dynamic CRT). |

The build used `CARGO_TARGET_DIR=target\elab`. The guest is a lab host. Standard
user tests used a temporary local user (`labstd`), removed afterwards.

## Results

"Expected" is what the changed code or documentation states. Rows marked
**Failed** or **Differs** are discussed below the table.

| # | Check | Command | Expected | Observed |
| --- | --- | --- | --- | --- |
| 1 | Elevated ignored tests | `unit.exe --ignored --test-threads=1` (both `700dd62` and `77d7c47` builds) | all pass, including `reclaim_replaces_leaked_real_time_session` | 6 passed, 0 failed for both builds |
| 2 | Snapshot block | install.md "Snapshot before a change" | folder with 12 files, ACL SYSTEM and Administrators only | as expected (`qc`, `qsidtype`, `qprivs`, `qfailure`, `qfailureflag`, `sdshow`, `service.reg`, `wmi-security.reg`, `install.acl`, `plu.txt`, `exe-hash.txt`, exe copy) |
| 3 | Replace the executable | stop, rename to `.bak`, copy, `icacls` | `NT SERVICE\EtwDnstap:(I)(RX)` on the new file | as expected, both times |
| 4 | Pinned service descriptor | `cmd.exe /c 'sc.exe sdset ...'`, then step 6 compare | `sdshow` equals the audit string | exact match, before and after a reboot |
| 5 | F-14 standard user | `sc.exe stop/start/config/control EtwDnstap 200` as `labstd` (Medium integrity, not an administrator) | error 5 for each; `query` works | error 5 for each; `query` and `qc` worked |
| 6 | F-14 control code as administrator | `sc.exe control EtwDnstap 200` | an error (audit expected 1052) | **Differs**: the command succeeded and printed the status (RUNNING). Harmless, but the handler accepts an unknown control code. |
| 7 | F-12 mutex squat | `labstd` held `Global\EtwDnstap-EtwDnstap-Lab` (`createdNew=True`), then `sc.exe start EtwDnstap` | service reaches RUNNING | RUNNING, `capture started` logged while the mutex was held |
| 8 | N-2 foreign session, administrator, file-backed | `logman start EtwDnstap-Lab ... -o x.etl -ets`, then start | start refused with the "session shape" message; foreign session untouched | refused (Event ID 1), foreign session still present |
| 9 | N-2 foreign session, administrator, real-time only | `logman start EtwDnstap-Lab -rt ... -ets`, then start | refused, not stopped (new GUID check) | refused (Event ID 1), foreign session still present. The old code would have stopped it. |
| 10 | N-2 standard user, then Performance Log Users member | `logman start ... -rt -ets` as `labstd` | denied without the group; allowed with it | "Access is denied" without the group. With the group, the session was created, and the service was refused (STOPPED). |
| 11 | Stale-session reclaim | unit test, plus kill and restart | reclaim works for the service's own session | unit test passed. The live kill-and-restart was not run (see Not covered). |
| 12 | Grant `-WhatIf` | `Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap-Lab -WhatIf` | no change, no backup folder | no change, no backup folder (count 1 before and after) |
| 13 | Grant run with nothing to do | same, without `-WhatIf` | "Already granted", no backup | as expected |
| 14 | `-Remove`, then `-Restore` (one-element `Changed`) | `-Remove`, `-Restore <dir> -WhatIf`, `-Restore <dir>` | `-WhatIf` changes nothing; the descriptor returns byte for byte; registry export equals the original | as expected; `ConvertFrom-Json` in Windows PowerShell 5.1 read the file; export diff 0 lines |
| 15 | Grant after `-Remove`, then `-Restore` (one-element `Created`, empty `Changed`) | grant, `-Restore <dir>` | the created value is deleted | as expected. The descriptor ETW seeded had the same SDDL as the original but different bytes. |
| 16 | **Failed first**: backup in the original script | `-Remove` on `700dd62` | backup written | `Argument types do not match` in `Write-ChangeLog`; no change was made (the backup precedes every change). See "Defect found". Passed after the fix. |
| 17 | Restore from a missing path | `-Restore C:\Lab\nonexistent` | clear error | `No changes.json at ...` |
| 18 | `icacls /save` and `/restore` | install.md snapshot, `icacls 'C:\Program Files' /restore ...` | ACL unchanged | 0 differing lines before and after |
| 19 | F-13 reboot | `Restart-Computer`, then export `WMI\Security` | only the session GUID value relates to the service SID | after restore and reboot, two new values (`2d0da110-...`, `4ba7efee-...`) appeared that other components create. Grant `-WhatIf` reported nothing to remove, so the service SID is only on the session GUID. |
| 20 | Service starts after the grant | `sc.exe start EtwDnstap` | RUNNING, `capture started` with the session GUID | RUNNING; `session_guid="43b7b2fc-777a-5214-b6ed-665c1ff9e972"`, keyword masks `0x3`, `keyword_fallback=false` |
| 21 | Step 6 identity checks | `qc`, `qsidtype`, `qprivs`, `net localgroup` | LocalService, RESTRICTED, only `SeChangeNotifyPrivilege`, service SID in the group | as expected |
| 22 | F-15(d) `obj=` and the SID type | `sc.exe config ... obj=` for the same account, and for LocalSystem and back | claim to verify | **SID type stayed RESTRICTED and the privilege list stayed `SeChangeNotifyPrivilege`** in both cases. |
| 23 | N-3 start type | `cmd.exe /c 'sc.exe config EtwDnstap start= delayed-auto depend= DNS'` | `AUTO_START  (DELAYED)`, `DEPENDENCIES : DNS` | as expected; `Get-Service` shows `Automatic` |
| 24 | N-3 after reboot | `Restart-Computer`, watch the System log | the service starts without help | DNS Server running at 03:04:17, `EtwDnstap` running at 03:06:18 (about 2 minutes later), capture started |
| 25 | Capture works | 100 queries to `www.auth.stress.test` | events captured | `received=400 dequeued=400 encoded=400`, all drop and reject counters 0, `receiver_connected=true` |
| 26 | SCM recovery with a DNS restart (no dependency) | `Restart-Service DNS -Force` | stop with a service-specific error, restart after 5 s, new PID | stopped 4 to 5 s after the restart (`the DNS Server service restarted (process ID 2528 -> 4692)`, exit code 1066 / service-specific 1), running again 9 to 10 s after the action. New capture used the new PID. The first run waited 60 s, because failures from my squat tests had raised the SCM failure count. |
| 27 | DNS restart with `depend= DNS` | `Restart-Service DNS` and `Restart-Service DNS -Force` | n/a | without `-Force`: "Cannot stop service ... because it has dependent services". With `-Force`: `EtwDnstap` stopped with exit code 0, DNS restarted, `EtwDnstap` restarted, capture running again in under 2 s. |
| 28 | Stop and start DNS with `depend= DNS` | `Stop-Service DNS -Force`, `Start-Service DNS` | n/a | `EtwDnstap` stays STOPPED (exit code 0), no recovery. Needs `Start-Service EtwDnstap`. |
| 29 | DNS crash | `Stop-Process -Name dns -Force` | stop and restart | `the DNS Server service stopped (it ran as process ID 3360)` after 4.9 s, restart 5 s later, and the dependency started DNS again. Capture running 10 s after the kill. |
| 30 | N-1 handle open (item 9) | Application log after each start | no `cannot hold the DNS Server process open` warning if `OpenProcess` works | **Failed**: the warning appears at every start. `OpenProcess(<dns pid>)` returns access denied (code 5) for the restricted LocalService token. The stop reason `the DNS Server process exited` never occurred. |
| 31 | Step 6 after reboot | `sdshow` compare, `qfailure`, `qfailureflag` | unchanged | unchanged; failure actions 5 s, 5 s, 60 s, flag TRUE |

## Defect found and fixed

- **The backup in the original `-BackupPath` code threw on the first change.**
  `@($script:ChangedValues)` on an empty `List[object]` raises `Argument types
  do not match` in Windows PowerShell 5.1. `Write-ChangeLog` now uses
  `.ToArray()`. The failed run made no change to the registry, because the
  backup runs before every change. The fix is in the working tree and is not
  committed. Rows 14, 15 and 19 used the fixed script.

## Findings that the code owners should see

- **N-1 has no effect on this host.** The restricted LocalService token cannot
  open `dns.exe` with `SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION`. The
  service falls back to SCM polling every time. The warning text suggests that
  the token needs `SC_MANAGER_CONNECT` and `SERVICE_QUERY_STATUS`. That is not
  the cause: the process object denies the open. The message should not blame
  the SCM rights.
- **`depend= DNS` changes DNS restart behavior.** See rows 27 to 29. The
  documentation now says so.
- **`sc.exe control EtwDnstap 200` succeeds for an administrator** (row 6).

## Not covered

- The stale-session reclaim after a live kill of the service process. The unit
  test passed, and the deployed service was not killed.
- The pre-`sdset` default descriptor of a new service. The guest service
  already carried a descriptor from the earlier lab, so the difference
  (`CR` for interactive and service logons) was not measured.
- Which access right the `dns.exe` object denies, and whether a different
  access mask would pass.
- Load. Only 100 queries were sent.
- Runtime handling of malformed ETW records (no ETW injector).
- Builds other than 26100, and domain-joined hosts.
- `dumpbin /imports` for `WINMM.dll` (F-17). `verify-pe.ps1` reported no
  dynamic runtime import, but the check does not list `WINMM.dll` by name.

## Guest state at the end

The service runs the `77d7c47` build (SHA-256 above) as LocalService with a
restricted SID type, only `SeChangeNotifyPrivilege`, the pinned descriptor,
failure actions 5 s, 5 s and 60 s with the non-crash flag, a delayed automatic
start, and a dependency on `DNS`. The `0x20` grant is on
`{43b7b2fc-777a-5214-b6ed-665c1ff9e972}` only. The snapshot folder
`C:\ProgramData\EtwDnstap\backup\20261101025017` holds the pre-evaluation state
and the old executable. The test user, its scheduled task, the temporary
batch-logon right and `C:\Lab` are removed. The VM is Running. Both checkpoints
are kept.
