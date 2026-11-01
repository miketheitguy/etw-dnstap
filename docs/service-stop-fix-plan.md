# Service shutdown fix plan

> **Status:** Superseded by [Diagnose and remediate receiver shutdown resets](service-stop-remediation.md). · **Evidence dated:** not recorded · **Commit:** not recorded · **Reviewed:** 2026-11-01

This plan gives maintainers the implementation boundaries and verification
criteria for diagnosing incomplete Frame Streams shutdown and configuring a
graceful DNS-collector close.

## Evidence and scope

The recorded failure is Windows error 10054 inside the STOP/FINISH exchange,
reported to Service Control Manager as error 1066 with service-specific code 1.
The Rust review found no confirmed intake, queue-drain, or ownership defect.
DNS-collector 3.2.3 inherits `reset-conn: true`; its pinned close helper uses
zero linger after sending FINISH. That can discard unacknowledged data, but
the failed connection's packet ordering has not been captured. See the
[existing shutdown analysis](pooled-vm-trace-analysis.md).

Implement shutdown diagnostics and regression coverage, and update the
documented receiver configuration. Preserve the original I/O error in its
source chain, strict FINISH validation, the shared two-second shutdown I/O
deadline, and SCM failure reporting. Do not add packet-path metrics, retries,
file output, new dependencies, or protocol changes to hide resets.

VM load testing remains stopped. This work permits local unit tests and
linting, but does not deploy binaries, edit live receiver settings, restart
services, or run the stress client. Keep historical measurements and the
existing untracked filtered-ramp report unchanged.

## Parallel assignments

The implementation tasks have separate file ownership:

| Task | Model and effort | Owned files | Required result |
|---|---|---|---|
| Shutdown diagnostics | Luna, medium | Production sections of `src/network.rs`; minimal framing seam only if needed | Distinguish final data flush, STOP write/flush, transport extraction, FINISH header/body reads, and validation without weakening errors or deadlines |
| Regression coverage | Sol, low | `src/network/shutdown_tests.rs` | Deterministic reset, truncation, invalid FINISH, and buffered final-data cases; assert preserved errors and no false success |
| Receiver guidance | Luna, low | `README.md`, `docs/service-stop-remediation.md`, documentation index | Explicit `reset-conn: false` in the receiver example; scoped validation and rollback instructions; no live changes |

The diagnostics owner adds the test module declaration and coordinates its
private helper signatures with the test owner. Tests may use private generic
I/O seams; production code must not gain a new public API for fixtures. Prefer
scripted bounded readers and writers to timing-sensitive reset races. Preserve
existing loopback tests for the real transport. Avoid duplicate Cargo runs
while files are changing; the coordinator runs the integrated checks.

Each owner returns changed files, behavior, assumptions, validation performed,
and unresolved risks. The documentation owner follows the technical
documentation skill and checks examples against the implementation and pinned
collector sources.

## Integration and independent review

Complete these steps after the parallel work:

1. Inspect the combined diff for file ownership conflicts and scope drift.
2. Run formatting, locked tests, and strict Clippy with the pinned toolchain.
   Preserve check output under `target/stop-failure-fix/`.
3. Assign Sol at high effort to independently review error-source preservation,
   final-write ordering, bounded reads, deadlines, test validity, and receiver
   configuration placement. The reviewer must identify findings by severity
   and file location, or explicitly report no findings and review limits.
4. Delegate any required corrections to the owning agent, rerun affected
   checks, and obtain review of those corrections.
5. Record the final verification and remaining interoperability uncertainty.

## Future receiver validation

When VM testing is resumed, preserve the receiver configuration and compare
its existing reset policy with explicit `reset-conn: false` using identical
binaries and workload. Capture the TCP STOP, FINISH, and close exchange, and
verify the Windows service stop result. Keep DNS record reconciliation
separate from protocol termination. Restore the saved configuration if the
change causes a regression. A passing local fixture does not establish that
the observed live reset is fixed.

## Completion record

Implementation is complete. `src/network.rs` adds shutdown-only phase context
and preserves the original I/O error through `anyhow`. Five regression test
functions cover ordering, write and flush resets, truncated FINISH frames,
malformed controls, and original error preservation. The README receiver
example sets `reset-conn: false`; the [remediation guide](service-stop-remediation.md)
describes future validation and rollback.

Integrated validation passed with the pinned toolchain:

- `cargo fmt` and `cargo fmt --check`.
- `cargo test --locked`: 58 passed, zero failed, and one manual benchmark ignored.
- `cargo clippy --locked --all-targets -- -D warnings`.
- `git diff --check`.

Test and Clippy logs are in `target/stop-failure-fix/`. Sol at high effort
independently reviewed the final code, tests, and corrected documentation and
approved them with no actionable findings. The reviewer inspected the logs
without rerunning tests.

The existing `BufWriter` drop behavior can attempt to write remaining buffered
bytes after a write failure. It remains within the original deadline and does
not change the returned failure into success. This work does not change that
behavior. FINISH parsing duplicates some handshake framing checks; both paths
retain the same bounds and validation rules.

The implementation and local-review phase did not change VM configuration,
deploy binaries, restart services, or generate load. After the user authorized
deployment, a separate bounded comparison deployed the binary and retained
the graceful receiver setting. See the
[validation results](service-stop-validation-results.md) for live outcomes
and remaining uncertainty.
