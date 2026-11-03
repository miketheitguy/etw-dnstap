# Commit hash mapping after the 2026-11-03 history rewrite

> **Status:** Historical · **Evidence dated:** 2026-11-03 · **Commit:** the commit that adds this record · **Reviewed:** 2026-11-03

On 2026-11-03 the history from commit `253df79` (old `8588619`) onward was
rewritten on every branch. The rewrite changed one line in `LICENSE` and
one line in `README.md`: the copyright holder is now `miketheitguy`. No other
file, author, date, or message changed. Every commit hash from that point
changed as a result.

This table maps each old hash to the new one, in commit order. Use it to
resolve an old hash in these places:

- the files under `datasets/`, which keep their recorded hashes so that their
  `sha256.txt` manifests stay valid;
- commit messages and old copies of the repository.

The guides, plans, history records, diagrams, and `CHANGELOG.md` were
updated to the new hashes.

| Old | New | Subject |
| --- | --- | --- |
| `8588619` | `253df79` | Restructure documentation and add project files |
| `6e62b2e` | `f1475d8` | Harden and consolidate lab performance scripts |
| `f0b51f4` | `b46da39` | Reconnect receiver, filter DNS PID, and run as virtual account |
| `54eafae` | `a2ae289` | Reorganize docs into guide, design, performance, plans and history |
| `b7f23ba` | `ca1fe2d` | Document reconnect, PID filter, virtual account and runtime stats |
| `12cce7a` | `4603bc8` | Add property tests, fuzz targets, golden vectors and CI |
| `6718863` | `1a54272` | Run as LocalService with a restricted service SID |
| `e8560ac` | `87c8d6d` | Scope the ETW grant to the service's own session GUID |
| `9ab6dad` | `65b6e7f` | Add skills-driven evaluation plan |
| `14d6e6a` | `abf6538` | Add a property test for copy_record admission and copying |
| `df6fd9c` | `b70702a` | Reconcile CHANGELOG, guides and security notes with the code |
| `9fce762` | `cb0d290` | Drop the global instance mutex, pin the DNS process, check session GUID |
| `700dd62` | `39c4337` | Back up and restore ETW and service state; pin the service DACL |
| `47206d4` | `f3f134b` | Verify the release PE in CI and widen supply-chain checks |
| `77d7c47` | `72cc41e` | Trim build-host paths from the release binary; narrow windows-sys |
| `d721569` | `50fb489` | Record lab validation of the evaluation fixes; fix grant script on PS 5.1 |
| `5f610f5` | `39a5ce4` | Remove the DNS process handle; document the PID-reuse window |
| `f3b4a6b` | `c988b32` | Document the evaluation changes; move the executed plan to history |
| `987ec0f` | `fc4e33c` | Make the log-file reclaim tests reach the log-file check |
| `78d1ca0` | `9837085` | Make verify-pe.ps1 fail when it parses nothing |
| `b8d0427` | `3f15814` | Close the backup-folder privilege escalation; guard restore |
| `efb47af` | `685f869` | Record the backup hardening and narrower service DACL in CHANGELOG and SECURITY |
| `2fcc4a2` | `a691c10` | Remove manifest keys that repeat Cargo defaults; pin CI tools |
| `8c79f69` | `26cd08d` | Add the obsolete and legacy code cleanup plan |
| `63dd477` | `18a29a9` | Remove unreferenced lab scripts and unused LabCommon exports |
| `015d385` | `aa7e199` | Archive finished-campaign Python tools; fix parser-ramp monitor |
| `0865e19` | `9146740` | Rename ring-era names, share test scaffolding, require the DNS PID |
| `40a6b65` | `6dac61f` | Move executed plans and retired designs to history; fix doc status |
| `21e31c0` | `d538e35` | Record the cleanup validation; fix review findings; move the plan to history |
| `69db97d` | `ee94de6` | Remove the per-event TDH fallback reader |
| `dde7188` | `7f91264` | Wake the parser once per delivered ETW buffer |
| `7bc711f` | `8031e9e` | Restructure current docs in plain STE style; add the asd-ste100 skill |
| `63bcb0c` | `67fde6c` | Bump serde from 1.0.228 to 1.0.229 in /tools/dns-stress |
| `06b8965` | `bcf659e` | Bump serde_json from 1.0.150 to 1.0.154 in /tools/dns-stress |
| `dc28e45` | `3a2d56f` | Track the asd-ste100 skill once, under .agents/ |
| `6f7ef84` | `b885c1c` | Keep host directory paths out of the repository |
| `ef46c85` | `50523a8` | Connect to the dnstap receiver over TLS 1.2 or 1.3 by default |
| `6d49577` | `2073411` | Pace dns-stress on an absolute schedule so sleep overshoot is recovered |
| `fcecb56` | `54067ac` | Remove the explicit periodic ETW flush and --etw-flush-ms |
| `7ae60a6` | `7c62552` | Document ETW buffer and queue settings for 200,000 QPS |
| `335e501` | `19aa979` | Add performance counters, --etw-min-buffers, and larger buffer defaults |
| `fc983ab` | `1d66263` | Capture DNS Server RESPONSE_FAILURE (event 258) and drop the level filter |
| `9e6b21a` | `e80546f` | Add the code review remediation plan |
| `65d9aeb` | `02d0aa9` | Reference the host-path check instead of quoting its pattern |
| `30a93a8` | `758bb74` | State the receiver revocation check as end-entity only and soft-fail |
| `033f530` | `7152236` | Assert the LNK2047 link-exception set in CI and record the ring decision |
| `ec9ce45` | `6db89b3` | Split unsafe blocks so each holds one operation |
| `decf431` | `dd66e94` | Use QueryProperties for STOP and a byte for the event-ID FilterIn |
| `446de3f` | `842e6c1` | Type the session query status and the target change |
| `99491ce` | `005cf84` | Wait on conditions instead of sleeping in the network tests |
| `40a7dc5` | `b3da920` | Make eligible functions const and rename shadowed FFI bindings |
| `735bc04` | `07b8622` | Name the protobuf tags, property-flag mask, and enable timeout |
| `803292e` | `7bf6f70` | Add a lab check that starts the service at the maximum capacity |
| `ce32423` | `a327b52` | Report SCM start progress during pool allocation and derive wait hints |
| `c21df7b` | `8e70d6e` | Implement EventProducer on Capture directly |
| `4f9ef0a` | `75df9a9` | Add a criterion benchmark of the per-event hot path |
| `777f21b` | `03fccdc` | Record the code review baseline at fc983ab |
| `3bf9fcd` | `9f329fd` | Tighten the revocation wording after security review |
| `478207b` | `3e98b48` | Start short error messages with a lowercase word |
| `b7c0614` | `a66e583` | Merge step 1.2: revocation wording |
| `e96a4dd` | `e7b62aa` | Clear the remaining production Clippy hits in both feature sets |
| `5cc41b5` | `dfedcff` | Restore the installed binary and path when a deploy step fails midway |
| `af516c6` | `e01f9ea` | Pass Invoke-LabGuest arguments as separate positional parameters |
| `5db3a45` | `744bb74` | Document how to update the link-exception allowlist |
| `dbaf472` | `741304c` | Merge step 1.1: SCM start progress, wait hints, lab startup check |
| `ee40d02` | `0dd7cbe` | Merge step 1.4: code cleanups |
| `64521f8` | `0e11543` | Record steps 1.1, 1.2, and 1.4 in the changelog and the plan |
| `4d3632c` | `f44f25b` | Add an advisory Miri job for the pure-Rust modules |
| `b34f53d` | `a6cfb59` | Tighten the reconnect tests and the Miri job after review |
| `8ee43a4` | `34db8c6` | Extend the hot-path benchmark with cold-pool, two-thread, and decomposition groups |
| `12f15b6` | `dc6216a` | Check the bench feature in CI and document how to run the benchmark |
| `33ff12d` | `181a80b` | Re-record the code review baseline from repeated sessions |
| `da06e4e` | `b104f7f` | Merge step 1.5: hot-path benchmark and Phase 0 baseline |
| `1236fe7` | `3aa0b28` | Build the benchmark pool with the production constructor |
| `b936f18` | `14ae1f8` | Record step 1.5 in the changelog and the plan |
| `a442727` | `4369d46` | Merge step 1.6: condition-based test waits and advisory Miri job |
| `1fabf32` | `85e4fdd` | Merge step 1.3: LNK2047 link-exception detector |
| `e61179d` | `0d991e3` | Record steps 1.3 and 1.6 and close Phase 1 in the plan |
| `e396871` | `4669686` | Link the release profile with fat LTO and one codegen unit |
| `8d1b172` | `50e0b2a` | Lease pool slots from a leaked static pool instead of an Arc |
| `2ec1796` | `e9ef0cf` | Pass the benchmark pool by value |
| `eecc74e` | `aa9acb3` | Store extended-data item tables at the end of the slot bytes |
| `05941c1` | `7dbe71d` | Add a socket write group to the hot-path benchmark |
| `50e9298` | `93f608e` | Skip set_read_timeout and set_write_timeout when the time left is close |
| `ded1faa` | `5b34a4b` | Record the Phase 2 performance results in the baseline record |
| `29f821d` | `72419cd` | Make the pool own the item-cell reservation invariant |
| `fd8e167` | `bbd7ba0` | Record the build-host verify-pe and link-exception results |
| `25742af` | `39cbcdb` | Merge Phase 2: performance experiments (items 9 to 12) |
| `4b1b17e` | `9c62a17` | Record the Phase 2 lab comparison and close Phase 2 in the plan |
| `7f33dec` | `55fdbe5` | Split src/pipeline.rs into a pipeline module directory |
| `98af7b5` | `b161d38` | Replace the per-line diag-timing attributes with a DiagTiming type |
| `6d9f474` | `25b87bc` | Merge Phase 3: pipeline module split and DiagTiming (items 14 and 15) |
| `909b0b8` | `7ec888d` | Enable three more Clippy lints and clear their hits |
| `0937487` | `cf40864` | Record Phases 3 and 4 in the remediation plan and changelog |
| `d20e96b` | `4bbdc21` | Merge Phase 4: three more Clippy lints |
| `22bc1bb` | `3f99558` | Record the Phase 4 merge commit in the plan |
| `907f728` | `0651f9b` | Restructure the documentation by reader task |
| `f3f1eee` | `0f6db1e` | Add the data-path animation and align the docs with the code |
| `6a38e71` | `e6b4d35` | Bump crossbeam-channel from 0.5.15 to 0.5.17 |
