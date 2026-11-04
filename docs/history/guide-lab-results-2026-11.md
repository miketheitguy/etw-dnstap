# Guide lab results, 2026-11

> **Status:** Historical · **Evidence dated:** 2026-11-01, except the flush-era runs, which are not dated · **Commit:** not recorded · **Reviewed:** 2026-11-03

These lab results were stated in the operator guides, [Tune capture for load](../guide/tuning.md) and [Log events and counters](../guide/monitoring.md), before the guides were trimmed. Most appear in no other record. Each section gives one run or one result, and it names the source where the result is also recorded. A value that the source does not give is written as not recorded. The raw run data is lab evidence and is not in the repository.

## 60-second run with the defaults at 10,000 queries per second

| Item | Value |
| --- | --- |
| Date | 2026-11-01 |
| Rate | 10,000 queries per second, about 20,000 events per second, 60 seconds |
| Settings | Defaults: `--etw-buffer-kib 16 --etw-max-buffers 64 --capacity 24576` |
| Result | Every event captured, with 0 ring drops and 0 `etw_lost`. ETW delivered late four times. Each time about 35,000 to 41,000 events arrived in one second. The channel peaked at 8,800 events. ETW had at most 5 buffers in use and allocated at most 18. |
| Host and binary | Not recorded. The source says "the lab". |
| Source | Commit `19aa979`: the commit message and `docs/guide/configuration.md`. The CHANGELOG entry for the default change gives 0 ring drops and 0 `etw_lost` for one 60-second run at the same rate. |

Method: one 60-second run at the target rate with the default settings. The client and receiver are not recorded.

## Two 60-second runs at 10,000 queries per second with the previous channel

| Item | Value |
| --- | --- |
| Date | 2026-11-01 |
| Rate | 10,000 queries per second, 60 seconds per run, two runs |
| Settings | `--etw-buffer-kib 16 --etw-max-buffers 32 --capacity 8192` |
| Result | Channel drops of 10,751 and 12,747 events. Each drop followed a late ETW real-time delivery: a second with about 1,500 events, then about 40,000 events the next second. ETW lost nothing (`etw_lost` 0). |
| Host and binary | Not recorded. The [Windows performance counters V2 plan](windows-performance-counters-v2-plan.md#lab-results) records a 60-second run with the same settings and 10,751 drops, on DNSTap-WS2025. It does not say whether that is the first of these two runs. |
| Source | Commit `19aa979`: the commit message and `docs/guide/configuration.md`. The commit message gives the 8,192 capacity and both counts. It does not give the maximum-buffer value. |

Method: two 60-second runs at the target rate, with the settings above.

## 10-minute run at 10,000 queries per second

| Item | Value |
| --- | --- |
| Date | 2026-11-01 |
| Rate | 10,000 queries per second, 10 minutes |
| Settings | 16 KiB buffers. Maximum buffers and channel capacity: not recorded. |
| Result | Up to 16 ETW buffers in use and up to 21 allocated. Channel drops and `etw_lost`: not recorded. |
| Host and binary | Not recorded. |
| Source | Commit `19aa979`: `docs/guide/configuration.md`. |

Method: one 10-minute run at the target rate. The source gives only the buffer counts.

## 60-second counter run at 10,000 queries per second

| Item | Value |
| --- | --- |
| Date | 2026-11-01 |
| Rate | 10,000 queries per second, 60 seconds |
| Settings | `--etw-buffer-kib 16 --etw-max-buffers 32 --capacity 8192` |
| Host and binary | DNSTap-WS2025, 6 vCPUs, LocalService with a restricted service SID. Binary not recorded. |
| Result | The by-reference counter totals matched the `capture stopped` event: Events Received 1,200,000, Channel Full Drops 10,751, Channel Peak Depth 8,193. Events Received/sec read 19,700 to 21,000 in steady seconds, which is 2 events per query. The instance disappeared at stop. |
| Source | Also recorded in the [Windows performance counters V2 plan](windows-performance-counters-v2-plan.md#lab-results). Commit `19aa979` states it in the commit message. |

Method: one 60-second run. The counter totals were compared with the final `capture stopped` event.

## 200,000 queries per second with 32 buffers and a 24,576-event channel

| Item | Value |
| --- | --- |
| Date | 2026-11-01 |
| Rate | 200,000 queries per second |
| Settings | `--etw-max-buffers 32 --capacity 24576`. Buffer size, duration, and client: not recorded. |
| Result | One run: 37,133 ring drops and 351 `etw_lost`. Another run: 3,219 `etw_lost`. Its ring drops: not recorded. |
| Host and binary | Not recorded. |
| Source | Commit `19aa979`: `docs/guide/configuration.md`. The [monitoring guide](../guide/monitoring.md) alert rationale also cites these runs. The source refers to "the lab notes", and no such file is in the repository. These are not the runs in [200,000 QPS validation](200k-qps-validation.md), which used 256 buffers and a 262,144-event channel and lost nothing. |

Method: two runs at the target rate. The source gives only the counts above.

## No periodic flush: channel loss of 59 to 79% at 10,000 to 20,000 queries per second

| Item | Value |
| --- | --- |
| Date | Not recorded. The source says the result predates the buffer wake (commit `7f91264`, 2026-11-01). |
| Rate | 10,000 to 20,000 queries per second |
| Settings | 64 KiB × 256 ETW buffers, `--capacity 8192`, periodic flush off |
| Result | The channel dropped 59 to 79% of events. |
| Host and binary | Not recorded. |
| Source | `docs/ring-buffer.md` at commit `e59425f` states "59-79% loss without it" for 10k and 20k queries per second. The current record set has no entry for this case. |

Method: not recorded beyond the settings above. The source gives the rate range and the loss range.

## Periodic flush: 0 ring drops at 10,000 and 20,000 queries per second

| Item | Value |
| --- | --- |
| Date | Not recorded. The flush was removed in commit `54067ac` (2026-11-01). |
| Rate and flush interval | 10,000 queries per second with a 100 ms flush. 20,000 queries per second with a 50 ms flush. |
| Settings | 64 KiB × 256 ETW buffers, periodic flush on. Channel size: not recorded. |
| Result | 0 ring drops at both rates. |
| Host and binary | Not recorded. |
| Source | `docs/design/ring-buffer.md` at commit `54067ac`, in the "Removed periodic flush" section. The same table gives 50,000 queries per second with a 100 ms flush: 0.04 to 0.91% loss and `etw_lost` 0. The guides do not cite that row. |

Method: the runs as the design table records them. The source does not give the run length or the channel size.

## Stop-failure workload: parser-filtered-auth-80000-60s-1

| Item | Value |
| --- | --- |
| Date | Not recorded. |
| Rate | 80,000 queries per second, 60 seconds |
| Settings | Filtered authoritative DNS. Binary, receiver, filter, and client: not recorded. |
| Result | The guide names this run as the one that reproduced the Windows service-stop failure. Its counts and outcome are not recorded. |
| Host and binary | Not recorded. |
| Source | `docs/service-stop-remediation.md` at commit `e59425f`, the first version of the [service shutdown remediation guide](../guide/service-stop-remediation.md). A different 80,000 queries per second, 60-second campaign, with other run names, is in [Service shutdown validation results](service-stop-validation-results.md). |

Method: not recorded.
