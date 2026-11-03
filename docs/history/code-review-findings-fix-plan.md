# Code review findings fix plan

> **Status:** Executed. Merged to `main`. Lab validation open · **Evidence dated:** 2026-11-03 (engineering review of the documentation alignment findings) · **Commit:** written on `plan/msi-installer` after `58fde72`; the work is not committed yet · **Reviewed:** 2026-11-03

This plan helps the maintainer fix the defects that the 2026-11-03
documentation alignment review found, then bring the documents in line with the
fixed code. It lists each finding, the decision, the work package that owns the
change, and how to verify it. Each work package changes disjoint files, so the
packages can run in parallel where the table says so.

## Findings and decisions

The review compared every current document with the code. An engineering review
of the 12 resulting questions confirmed some as defects and cleared others.

| ID | Finding | Decision | Package |
| --- | --- | --- | --- |
| F1 | A transient `TdhGetEventInformation` failure is cached as a rejected key for the life of the process (`src/etw/schema.rs`, `remember_rejected`). All queries or all responses are lost until a restart. | Fix. Keep shape and plan failures cached. Retry a Win32 load failure after a fixed number of rejected events. | R1 |
| F2 | With TLS on, a certificate failure that follows a start while the receiver was down is not logged until failure 61. Reconnect-thread death and spawn failure are counted but never logged (`src/network.rs`). | Fix. Log whenever the error class changes. Log the thread failures. | R1 |
| F3 | The spool free-space block refuses frames for 10 s with no log line (`src/spool.rs`). | Fix. Add a bounded warning. | R1 |
| F4 | The pre-argument startup guard is armed as `shutdown` (`src/service.rs`). The capture-start watchdog reuses `SHUTDOWN_TIMEOUT` (`src/pipeline/supervisor.rs`). | Fix. Arm the guard as `startup`. Add `CAPTURE_START_TIMEOUT`. | R2 |
| F5 | The parser `rejected` warning logs only the first four rejections, with no every-60th cadence (`src/pipeline/parser.rs`). | Fix. Use the same cadence as the other counters. | R2 |
| F6 | The rate warning says "about 20,000 events/s" while the threshold is 22,000. | Reword the message to state both figures. | R2 |
| F7 | Session-name limits disagree: CLI 1 to 200 UTF-16 units, `Capture::start` 1,024, MSI and provisioning script `[A-Za-z0-9_.-]{1,128}`, grant script no pattern. | Fix. One rule everywhere: 1 to 128 ASCII letters, digits, underscore, dot, or hyphen, and no leading hyphen (`^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}$`). The MSI also gets a launch condition for the leading hyphen. Keep 1,024 in `Capture::start` as a backstop. Do not change the GUID hash. | R2, M1 |
| F8 | A maintenance `msiexec /i` with a new `SESSION_NAME` moves the ETW grant but leaves the service `ImagePath` on the old name. The service then fails every start with access denied. | Fix. Run `ProvisionInstall` only when the executable component is installed in the same pass. | M1 |
| F9 | If someone deleted the service by hand, `Test-ServiceInGroup` throws and the uninstall fails and rolls back on every attempt. | Fix. Resolve the group member by the service SID text. In the uninstall path, turn membership failures into warnings. | M1 |
| F10 | `MajorUpgrade` has `AllowSameVersionUpgrades=no`. A same-version rebuild installs side by side as a second product. | Fix. Set `AllowSameVersionUpgrades="yes"`. | M1 |
| F11 | `docs/guide/upgrade.md` says a same-version rebuild is "probably refused". `/fvomus` repair is described too broadly. | Docs. Correct to match F10 and the real repair semantics. | D1 |
| F12 | Name matching for an IP-literal server name is delegated to CryptoAPI. Whether it matches an `iPAddress` SAN or falls back to the CN is undocumented. | Docs. State that the certificate should carry an `iPAddress` SAN. A lab test is a follow-up, not part of this plan. | D1 |

Reviewed and left unchanged, with the reason:

- **Null `UserContext` in `native_etw_callback`.** ETW copies the context from
  `EVENT_TRACE_LOGFILEW.Context`, which is always the non-null box. It cannot
  happen.
- **Identity check before the stored arguments are parsed.** It fails closed
  before the service acts on configuration. The order is a safety property.
- **Unreachable `SpoolDirError` shape variants.** They are defensive code, so
  `check_spool_dir` stands alone.
- **Parent-folder owner and DACL checks for a custom `--spool-dir`.** The MSI
  layout is covered by the provisioning script. This stays an optional later
  hardening.

## Work packages

| Package | Scope | Files | Agent | Runs |
| --- | --- | --- | --- | --- |
| R1 | F1, F2, F3 with unit tests | `src/etw/schema.rs`, `src/etw/schema_tests.rs`, `src/network.rs`, `src/network/*_tests.rs`, `src/spool.rs`, `src/spool/spool_tests.rs` | Rust engineer, Opus | Wave 1 |
| M1 | F7 (scripts), F8, F9, F10 | `packaging/msi/Product.wxs`, `packaging/msi/CustomActions.wxs`, `packaging/msi/Invoke-EtwDnstapProvisioning.ps1`, `scripts/provision/Grant-EtwDnstapEtwAccess.ps1`, `scripts/lab/msi/**` only if a lab script encodes the old behaviour | PowerShell and WiX engineer, Opus | Wave 1 |
| R2 | F4, F5, F6, F7 (Rust) with unit tests | `src/service.rs`, `src/pipeline/supervisor.rs`, `src/pipeline/watchdog.rs`, `src/pipeline/parser.rs`, `src/pipeline/config.rs`, `src/pipeline/*_tests.rs`, `src/etw/session.rs` (comment only) | Rust engineer, Sonnet | Wave 2, after R1 |
| D1 | Documents for all fixes, F11, F12, the changelog, this plan's status, the index | `docs/**` (not `docs/history/` results), `README.md`, `CONTRIBUTING.md`, `CHANGELOG.md`, `packaging/msi/README.md`, `scripts/provision/README.md` | Technical writer, Sonnet | Wave 3, after R1, R2, and M1 |
| V1 | Independent review of the whole diff | Read-only | Code reviewer, Opus | Wave 4 |

R1 and R2 both build the crate, so they do not run at the same time. M1 does not
run cargo, so it runs next to R1.

## Rules for every package

1. The code is the source of truth. A package does not change behaviour outside
   its findings.
2. Each behaviour change has a test, or a stated reason why none is possible.
3. No host directory path appears in any repository file (see
   [CLAUDE.md](../../CLAUDE.md)).
4. Packages do not commit. The maintainer commits after V1.
5. Log events keep the existing event IDs. New log text uses the existing field
   style: a short message with structured fields.

## Package details

### R1: runtime behaviour

**F1.** In `src/etw/schema.rs`, store a rejected key together with the reason
class. A shape or plan failure (decoding source, provider or event mismatch,
plan compile) stays cached. A Win32 status from `TdhGetEventInformation` is
cached with a retry budget. After 65,536 further rejected events for that key,
the parser tries the load again. The retry count needs no clock read. Change the
warning text from "schema is unsupported" to a text that says the load failed and
will be retried, and keep the logging bounded. Counters keep their names. Tests
cover: a permanent failure is not retried, a transient failure is retried after
the budget, and a retry that succeeds clears the key.

**F2.** In `src/network.rs`, log a reconnect failure when its error kind differs
from the previous failure, in addition to the existing first-four-then-every-60th
rule. For an `InvalidData` failure on a link that has never connected, log once
at error level and name the trust store and `--tls-server-name` as the checks.
Log a dead reconnect thread and a thread spawn failure. Tests cover the
error-class change using the existing reconnect test harness.

**F3.** In `src/spool.rs`, log a warning when the spool enters the free-space
block: the first four entries, then every 60th by the same rule as the other
counters. Include the free and the needed MiB.

### M1: installer

**F8.** Condition `SetProvisionInstall` and `ProvisionInstall` so they run only
when the executable component is installed in the same pass
(`NOT REMOVE AND ($EtwDnstapExe = 3)` or the equivalent for the component
name in `Product.wxs`). Confirm that a fresh install and a major upgrade still
run both actions. Confirm that a maintenance run with a changed `SESSION_NAME`
and no `REINSTALL` does not move the grant.

**F9.** In `Invoke-EtwDnstapProvisioning.ps1`, do not call `.Translate()` on
`NT SERVICE\EtwDnstap`. Use the service SID text constant. Remove group
membership by SID (ADSI `WinNT://` provider or an equivalent that does not
throw on an orphaned SID). In the uninstall path a failure to read or remove
the membership writes a warning and continues. The grant removal and the spool
removal still run.

**F10.** Set `AllowSameVersionUpgrades="yes"` on `MajorUpgrade` and keep the
downgrade error. Record the ICE61 result and whether the build suppresses it.

**F7 (scripts).** Add `[ValidatePattern('^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}$')]`
to each `SessionName` parameter of the grant script, and use the same pattern in
the provisioning script. Add a launch condition to `Product.wxs` that refuses a
`SESSION_NAME` that starts with `-`, and skip it on a removal. The GUID
derivation does not change.

M1 builds the MSI with `scripts/build/Build-Msi.ps1` if the WiX toolchain is
present, and parses every edited script. M1 does not install the MSI on any
machine. Lab validation is a separate step (see below).

### R2: small fixes

**F4.** In `src/service.rs`, arm the pre-argument guard with
`ShutdownWatchdog::arm_for("startup", pipeline::STARTUP_BASE_BUDGET)`. In
`src/pipeline/watchdog.rs`, add `CAPTURE_START_TIMEOUT` (10 s) and use it for
the inner capture-start guard in `src/pipeline/supervisor.rs`.

**F5.** In `src/pipeline/parser.rs`, log a `schema_rejected` warning for the
first four and then every 60th, using the same cadence helper as the other
counters if one exists.

**F6.** In `src/pipeline/supervisor.rs`, reword the rate warning so it says the
service was validated at about 20,000 events/s and warns above 22,000.

**F7 (Rust).** In `src/pipeline/config.rs`, make `check_session_name` accept
only `^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}$` with a clear error. Update the CLI tests. In
`src/etw/session.rs`, leave the 1,024 check as a backstop and say so in its
comment.

## Documentation package (D1)

D1 starts after the code packages and reads the final code, not this plan.

| Document | Change |
| --- | --- |
| [`docs/guide/upgrade.md`](../guide/upgrade.md) | Same-version rebuilds are upgrades in place (F10). Repair `/fvomus` works only with the same ProductCode, restores missing or older files, and `/fvamus` forces replacement. A maintenance run that changes `SESSION_NAME` does not move the grant (F8). |
| [`docs/guide/uninstall.md`](../guide/uninstall.md) | An uninstall after a hand-deleted service completes with a warning (F9). |
| [`docs/guide/configuration.md`](../guide/configuration.md) | The session-name rule (F7). The certificate should carry an `iPAddress` SAN when the server name is the receiver IP (F12). |
| [`docs/guide/receiver.md`](../guide/receiver.md) | The `iPAddress` SAN note (F12). |
| [`docs/guide/monitoring.md`](../guide/monitoring.md) and [`docs/guide/troubleshooting.md`](../guide/troubleshooting.md) | The new log lines and their causes (F1, F2, F3, F5). Log cadence reads "first four, then the 61st, 121st, and so on". The rate-warning text (F6). The `startup` watchdog label (F4). |
| [`docs/design/single-pass-decoder.md`](../design/single-pass-decoder.md) and [`docs/design/pooled-intake.md`](../design/pooled-intake.md) | A transient schema-load failure is retried (F1). |
| [`docs/guide/install.md`](../guide/install.md) and [`packaging/msi/README.md`](../../packaging/msi/README.md) | The conditions on the provisioning actions (F8) and the session-name rule. |
| [`CHANGELOG.md`](../../CHANGELOG.md) | One `[Unreleased]` entry per fix. |
| [`CONTRIBUTING.md`](../../CONTRIBUTING.md) | The new test totals. |
| [`docs/README.md`](../README.md) | Index row for this plan. |
| This plan | Set the status to Executed and record the verification results. |

## Verification

Run these after the code packages and again after D1.

```powershell
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo test --locked --features diag-timing
cargo test --locked --features bench
python -I scripts/ci/linkcheck.py .
# Then run the host-path check from CLAUDE.md on the staged changes.
```

The privacy check lists only lines the change added. Any match is a defect.

### Lab validation (not part of this plan's agents)

The MSI fixes change install behaviour, so they need a lab run on the
Windows Server guest before the plan counts as validated. The runs below stay
open until a maintainer performs them:

1. Fresh install, then a maintenance `msiexec /i` with a changed `SESSION_NAME`
   and no `REINSTALL`. The service starts and the grant is unchanged (F8).
2. Delete the service with `sc.exe delete`, then `msiexec /x`. The uninstall
   completes with a warning (F9).
3. Install a same-version rebuild over an installed build. One product is
   listed (F10).
4. Confirm the platform verifier matches an `iPAddress` SAN, a CN-only
   certificate, and an IPv6 receiver (F12).
5. Run `msiexec /i` with `SESSION_NAME=-x` and confirm that the launch condition
   stops the install with the message `SESSION_NAME must not start with '-'. Use
   1 to 128 ASCII letters, digits, '_', '.', or '-', and do not start with '-'.`
   Confirm that `msiexec /x` of an installed product is not affected (F7).
6. Install on a host where `csc.exe` is blocked, so `Add-Type` cannot compile.
   Confirm that the install passes the group check by name without the
   fallback. Then add an orphaned member SID to Performance Log Users, so the
   name-based check throws, and confirm that the install prints the fallback
   line, fails with `Could not read the members of <group>`, and rolls back.
   Uninstall on the same host and confirm the group step prints the warning
   (F9).

## Results

The packages ran on `plan/msi-installer` after `58fde72`. The result is committed
in commit `c972593`, which is merged to `main`.

| Package | Findings | What changed |
| --- | --- | --- |
| R1 | F1, F2, F3 | `src/etw/schema.rs`: a schema failure is now `Unsupported` (never retried) or `Load` (a Win32 status from `TdhGetEventInformation`). A rejected key keeps its class. A load failure is retried after `SCHEMA_RETRY_EVENTS` (65,536) further rejected events of the key, and the budget counts events, so the reject path reads no clock. A retry that loads clears the key, and a retry that fails restarts the budget and counts in `schema_rejected`. `src/network.rs`: reconnect failures carry a `cause` class (`connect`, `timeout`, `tls`, `protocol`) and the first failure of each class since the last established session is always logged. The first rejection before any session logs one error. The thread spawn failure and the thread death are logged with the first-four-then-every-60th cadence. `src/spool.rs`: the free-space block logs a warning with `free_mib`, `needed_mib`, and `pause_s`. Unit tests are in `src/etw/schema_tests.rs`, `src/network/reconnect_tests.rs`, `src/network/tls_tests.rs`, and `src/spool/spool_tests.rs`. |
| R2 | F4, F5, F6, F7 (Rust) | `src/service.rs` arms the pre-argument guard as `startup`. `src/pipeline/watchdog.rs` has `CAPTURE_START_TIMEOUT` (10 s), which `src/pipeline/supervisor.rs` uses for the capture-start guard. `src/pipeline/parser.rs` logs a rejected event for the first four and then every 60th, with the `rejected` count. The rate warning is the constant `RATE_WARNING`, which states 22,000 and about 20,000 events/s. `check_session_name` in `src/pipeline/config.rs` accepts `^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}$` only (no leading hyphen), and `Capture::start` keeps its 1,024-unit check as a backstop. Tests are in `src/pipeline/cli_tests.rs`, `config_tests.rs`, `parser_tests.rs`, and `supervisor_tests.rs`. |
| M1 | F7 (scripts), F8, F9, F10 | `CustomActions.wxs` runs `ProvisionInstall` and `SetProvisionInstall` only when `NOT REMOVE AND ($EtwDnstapExe = 3)`, and the rollback actions carry the same component condition. `Invoke-EtwDnstapProvisioning.ps1` reads and removes the group membership by the service SID, through `NetLocalGroupGetMembers` and `NetLocalGroupDelMembers` at level 0, and a failure in the uninstall path is a warning. On install, the script checks the membership by name first and reads it by SID only if that throws. `Product.wxs` sets `AllowSameVersionUpgrades="yes"` and has a launch condition that refuses a `SESSION_NAME` that starts with `-`. Both scripts check `SessionName` with `^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}\z`. |
| D1 | F11, F12, and the documents for every fix | The guides, the design pages, the two README files for the MSI and the provisioning scripts, the changelog, the index, and this plan. |

Differences from the plan:

- **F9.** The plan named the ADSI `WinNT://` provider. M1 used `netapi32` through a
  small `Add-Type` class, because a `WinNT://S-1-...` path binds as a domain name
  and `Get-LocalGroupMember` in Windows PowerShell 5.1 fails on an orphaned member.
  Only the uninstall depends on that class. On install, the script uses the
  original name-based check first (the service name translated to a SID, then
  `Get-LocalGroupMember`), and reads the members by SID only if that throws. It
  compiles the class once. If the compile fails, the install fails with `Could
  not read the members of <group>`, with both errors.
- **F7 (scripts).** The plan gave `^[A-Za-z0-9_.-]{1,128}$`. The scripts use `\z`
  and `Options = 'None'`, because the default `ValidatePattern` ignores case and
  `$` accepts a trailing newline. The rule also refuses a leading hyphen, in the
  scripts, the service, and an MSI launch condition (see [Review round](#review-round)).
  A condition cannot test the length or the character set, so an invalid name
  that passes it still fails the provisioning action and the install rolls back.
  The scripts use the default `ValidatePattern` error text, because Windows
  PowerShell 5.1 has no `ErrorMessage` option.
- **F2.** The plan asked for a log when the error class changes. The code logs a
  failure that is among the first four, or the 61st, 121st, and so on, or the
  first of its `cause` class since a session was last established. The set of
  classes seen is emptied when a session is established, at startup or on a
  reconnect, so each later outage reports each of its causes once. It is not
  emptied when a session is lost, because the line
  `dnstap receiver connection lost` marks each outage. A failed connection at
  startup adds its class, because the startup warning already reported it.
  Alternating causes add at most one extra line per class per outage. The code
  also splits the first-rejection error into a TLS variant and a Frame Streams
  variant. A TLS alert that arrives after the handshake, such as
  `CertificateRequired` from a receiver that requires a client certificate, is
  classed as `tls`, and the TLS variant tells you that the service sends no
  client certificate.
- **F1.** The reject path scans a list of at most eight keys only on a cache miss,
  so an accepted event does no extra work.

Test totals, run after the last code change and verified by the coordinator:
443 passed with the default features (441 library tests plus `console_output`
and `version_resource`), 448 with `diag-timing`, and 447 with `bench`, with 7
ignored in each. The coordinator runs the other gates in
[Verification](#verification) after the last documentation edit.

The test
`sender::sender_tests::e2e::a_run_stopped_in_the_middle_of_a_replay_leaves_a_cursor_the_next_run_resumes_from`
is flaky. It failed about 2 times in 60 runs on the unmodified base commit. This
work does not change it, and fixing it is out of scope. This work did fix two
test-infrastructure races that it found. The first is the callsite-interest race
in the shared log-capture helper, which is now one global subscriber with a
thread-routed capture slot. The second is the reuse of an unused port in the
network tests (`RefusedPort`).

After the D1 edits, `python -I scripts/ci/linkcheck.py .` reported 95 files, 866
relative links, and 379 anchors checked, with `broken=0`. The checker reads
tracked files only, so it skips this plan until it is committed. The links in
this plan were checked by hand. The privacy check found no added line with a host
path.

Open: the six lab runs in
[Lab validation](#lab-validation-not-part-of-this-plans-agents) have not been
performed. Until they pass, these statements in the documents are not
lab-validated: the skipped install steps on a maintenance run (F8), the
uninstall after a hand-deleted service (F9), the install fallback to the SID
path and the blocked compile (F9), the `SESSION_NAME` launch-condition message
(F7), the same-version upgrade (F10), the error 1605 for `/f` with a rebuilt MSI
(F11), and the `iPAddress` SAN match (F12). The documents say so where they
state each one.

### Review round

An independent review (V1) of the whole diff found no critical or high defects.
It made four findings. Each is fixed, and the documents describe the fixed
behaviour:

- **Alternating-cause log volume.** A rule that logged each change from the
  previous failure wrote one line per attempt when two causes alternated. The
  reconnect log now writes the first failure of each cause once per outage, in
  addition to the first four and every 60th.
- **Leading hyphen in the MSI path.** A `SESSION_NAME` that starts with `-`
  passed the old pattern, and `powershell.exe -File` and the service's command
  line read it as a parameter or an option. The service, both scripts, and a new
  MSI launch condition now refuse it. The earlier advice to pass such a name with
  an equals sign is gone.
- **Install dependence on `Add-Type`.** The install path compiled a helper class
  that application control can block. The install now checks the group
  membership by name first and compiles the class only if that check throws. The
  uninstall still uses only the SID path.
- **Post-handshake TLS alert classed as protocol.** A TLS alert that arrives
  after the handshake, such as `CertificateRequired` from a receiver that
  requires a client certificate, now counts as a `tls` failure. The first
  rejection error also says that the service sends no client certificate.

The review also found three wrong statements in the first documentation pass.
They are corrected: the certificate "must" carry an `iPAddress` SAN (it should),
the schema warning described as never repeated for a key (it can appear again
after the rejected-key list is cleared or when a retry fails as unsupported), and
the earlier test totals.
