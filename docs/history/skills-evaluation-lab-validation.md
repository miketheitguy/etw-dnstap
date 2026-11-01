# Skills evaluation lab validation

> **Status:** Historical · **Evidence dated:** 2026-11-01 (guest local time, America/Los_Angeles; Application log lines are UTC) · **Commit:** `77d7c47` plus an uncommitted fix to `scripts/provision/Grant-EtwDnstapEtwAccess.ps1` (see "Defect found") · **Reviewed:** 2026-11-01

This record covers the lab run for the changes made after the skills-driven
evaluation: the removal of the instance mutex, the GUID-checked session reclaim,
the DNS process handle, the pinned service descriptor, delayed automatic start,
and the backup and restore of ETW and service state. It follows
[the first LocalService record](localservice-restricted-validation.md). It is the lab record for the [skills-driven evaluation plan](skills-evaluation-plan.md).

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

## Post-review re-run (2026-11-01, after the final code review)

The final review found a local privilege escalation in the backup and restore
feature (H1), an unconditional restore (M1), a pinned descriptor that was wider
than the default (L1), and a grant on an unparseable descriptor (L3). This re-run
tests the fixes. It does not change the observed results above. Those used the
earlier script and the earlier pinned descriptor (`AU` instead of `IU` and `SU`).
The standard user in rows 5 and 7 was a batch logon, which `AU` covers and
`IU` and `SU` do not.

| Item | Value |
| --- | --- |
| Commit | `78d1ca0` plus uncommitted edits to `scripts/provision/Grant-EtwDnstapEtwAccess.ps1`, `scripts/provision/README.md`, `docs/guide/install.md` and `docs/guide/uninstall.md` |
| Checkpoint | `eval-post-review-2026-11-01` (taken first; the two earlier checkpoints are kept) |
| Release exe | SHA-256 `8FAD752D69B17A9CC35AFCBABDAFD181D5A612770D4F7DA868C3ADD675609A18`. `verify-pe.ps1`: 21 checks, 0 failed. |
| Unit-test exe | SHA-256 `14FF8CC8121E2F17759A41B73B63F5A76E77480200D252AF4908CF329900F528` |
| Script checks | Windows PowerShell 5.1 and PowerShell 7 parse with 0 errors. ScriptAnalyzer reports one warning (`PSUseShouldProcessForStateChangingFunctions` on the existing `Remove-SidAce`). |

| # | Check | Command | Expected | Observed |
| --- | --- | --- | --- | --- |
| R1 | Ignored tests, including M2 | `unit.exe --ignored --test-threads=1` | 6 pass | 6 passed, 0 failed. `reclaim_refuses_session_that_writes_log_file` and `reclaim_refuses_sequential_log_file_session` start their sessions with the GUID that `session_guid` returns, assert that a QUERY reports that GUID, and then assert the refusal. They now reach the log-file check. |
| R2 | Replace the executable | install.md procedure | `(I)(RX)` for the service SID | as expected |
| R3 | Pinned descriptor with `IU` and `SU` | `sc.exe sdset`, then `sdshow` | prints the exact string | exact match, also after the reboot |
| R4 | L1 batch logon | `sc.exe query`, `qc`, `sdshow`, `stop` as a standard user in a scheduled task | error 5 | error 5 for each, including `query` and `qc` |
| R5 | L1 administrator | `sc.exe qc EtwDnstap` | works | worked (Administrators hold full control) |
| R6 | H1: untrusted existing folder | script `-WhatIf` on the `EtwDnstap` folder that an earlier administrator run had made (inherited ACE for CREATOR OWNER and Users) | refuse | refused: `lets S-1-3-0 change it`. The doc function `New-ProtectedBackupFolder` refused too. After the operator locked the folders by hand, both worked. |
| R7 | H1: standard user pre-creates `C:\ProgramData\EtwDnstap` and `backup` | script `-WhatIf`, script `-Remove`, doc function, `-Restore` of a planted folder with a planted `changes.json` | refuse, change nothing | all four refused: `owned by S-1-5-21-...-1001, not by SYSTEM or Administrators`. Clean-up restored the original folder. |
| R8 | H1: junction as the `backup` level | `mklink`-style junction, script `-WhatIf` | refuse | refused: `is a reparse point` |
| R9 | H1: junction as the backup folder | `-BackupPath <junction>` | refuse | refused: `is a reparse point` |
| R10 | H1: never reuse a folder | `-BackupPath` of an existing folder | refuse | refused: `already exists. The script never reuses one` |
| R11 | H1: path outside the root | `-BackupPath C:\Lab\out` | refuse | refused: `is not below ...\EtwDnstap\backup` |
| R12 | `-WhatIf` leaves no folder | count folders before and after | equal | 4 and 4 |
| R13 | M1: value changed after the run | `-Remove`, write other bytes to the value, `-Restore` | skip with a warning | skipped with a warning, summary `0 restored, 0 deleted, 1 skipped` |
| R14 | M1: `-Restore -Force` | same folder | restore | `1 restored`, bytes equal the original |
| R15 | M1: created value changed | grant, change the value, `-Restore`, then `-Restore -Force` | skip, then delete | skipped, then `1 deleted` |
| R16 | Validate before apply | `changes.json` edited to a bad name, then to bad descriptor bytes | refuse, change nothing | both refused with a clear message. The registry value did not change. |
| R17 | L3 | session descriptor set to 5 invalid bytes, then the grant | stop with an error, no `AddDACL` | warning, then `exists but cannot be parsed. The script does not change it.` The value kept its 5 bytes. |
| R18 | Round trip and final grant | `-Remove`, `-Restore`, grant | registry export equals the start | 0 differing lines |
| R19 | Stale-session reclaim, live | `Stop-Process -Id <service pid> -Force` | recovery restart, reclaim | service left RUNNING at 0.3 s and was RUNNING again at 5.2 s. The log shows `reclaimed stale ETW session`, then `capture started`. This closes the earlier "not covered" item. |
| R20 | Reboot | `Restart-Computer` | delayed start | DNS running at 03:37:28, `EtwDnstap` running at 03:39:28; `sdshow` as expected; RESTRICTED; `SeChangeNotifyPrivilege` only |
| R21 | Capture | 100 queries after the reboot | events captured | `received=200 encoded=200 ring_dropped=0 foreign_pid_rejected=0` in the next stats line |

Problems seen in this re-run:

- **Same-second backup folder names collided** in my test sequence. The default
  folder name had a one-second resolution, so two runs in one second made the
  second one fail with "already exists". That is the intended refusal, but it is
  a nuisance. The default name now has milliseconds (`yyyyMMddHHmmssfff`).
- **A test script hung** on a `Remove-Item` confirmation prompt for a junction
  (my test, not the product). I removed the junction with `rmdir`.
- A standard user in the Users group on the interactive console (an `IU` logon)
  was not tested. No interactive session is available through PowerShell
  Direct. An `IU` logon is expected to keep query rights, by the descriptor.

Not covered by the re-run: a root-owned-by-SYSTEM `EtwDnstap` folder (the
owner check accepts SYSTEM, but I only created Administrator-owned ones),
domain-joined hosts, and any build other than 26100.

Guest state at the end: the `78d1ca0` build (hash above), running, delayed
automatic start, dependency on `DNS`, the `IU` and `SU` descriptor, the `0x20`
grant only on `{43b7b2fc-777a-5214-b6ed-665c1ff9e972}`. Snapshot folders
`20261101025017` and `20261101033112` remain under
`C:\ProgramData\EtwDnstap\backup`. The test user, task, batch-logon right and
`C:\Lab` are removed. The VM is Running.
