# Buffers as a watershed

> **Status:** Current · **Describes:** the buffers and counters in `src/etw/`, `src/pipeline/` and `src/sender/` on `main` at commit `d03b00f` as of 2026-11-05 · **Evidence dated:** 2026-11-04 for the sizing runs (explanation, from the source and [tuning.md](../guide/tuning.md)) · **Reviewed:** 2026-11-05

### Quick Start

![Animation in seven steps. Rain from dns.exe falls from a roof gutter into a row of galvanised rain barrels, the ETW buffers. Open taps run the water into a trough that pours into a lake behind a dam, the event channel. A mill wheel (the parser) feeds the send queue and the receiver. Step 2: ETW sets down more barrels, up to the maximum. Step 3: an illustration of every barrel full, with lost events counted in etw_lost. Step 4: large barrels pass the storm on, the lake tops the dam, and ring_dropped counts the spill. Step 5: a taller dam holds the same storm. Step 6: a slow receiver fills the last lake, which spills into the disk spool pond. Step 7: replay drains the pond oldest first.](../assets/pipeline/buffers-as-a-watershed.gif)

```text
rain -> ETW barrels -> flush taps -> channel lake -> parser mill -> send queue -> receiver
        etw_lost                   ring_dropped                   \-> disk spool (spill, replay)
```

Every lake is a buffer. A buffer smooths a burst so the next stage can work at a steady pace. When a lake is full, new water goes over the spillway and a counter records it. The send queue is the exception: with a spool, the router drains it to the disk pond first.

ETW is Event Tracing for Windows. A FIFO is a first-in, first-out queue.

### Steps / Configuration

#### What the picture means

| Picture | What it is | Setting | Loss counter |
| --- | --- | --- | --- |
| Rain | ETW events 256, 257, 258 from `dns.exe` | | |
| A rain barrel | One ETW buffer | `--etw-buffer-kib` (default 32) sets its size | |
| Standing barrels | Buffers ETW allocates at start | `--etw-min-buffers` (default 12) | |
| Empty pads | Barrels ETW sets down on demand | `--etw-max-buffers` (default 64) sets the limit | `etw_lost` (events), `etw_buffers_lost` (buffers) |
| Taps | ETW hands a buffer on when it is full or the flush timer expires | `--etw-flush-seconds` (default 1) | |
| Lake behind a dam | The bounded event channel | `--capacity` (default 131,072 events) | `ring_dropped` |
| Spillway over the crest | A full channel drops the newest event | | `ring_dropped` |
| Mill wheel | `dnstap-parser`: decode, encode, queue blocks | | `rejected`, `queue_dropped` |
| Send queue | Parser-to-router queue (256 blocks) and link FIFO (256 blocks) | | |
| Disk spool | Encrypted overflow pond for a slow or absent receiver | `--spool-max-mib` (default 4,096) | `spool_dropped` |
| Sea | The TCP receiver | | |

ETW allocates a new buffer when the existing ones fill faster than ETW can flush them, up to the maximum. Past the maximum, ETW discards events until a buffer is free.

Microsoft documents this behavior for `EVENT_TRACE_PROPERTIES`. ETW raises the minimum to two buffers per logical processor, so on a large host the real counts are higher. Limits and ranges: [Configuration reference](../guide/configuration.md#options).

#### The seven steps

The animation runs for 130 seconds and builds the picture up one element at a time. Each step stays on screen for 18 to 20 seconds, and a caption under the picture states the step in one sentence. Steps 1 to 3 are one continuous run. Steps 4 and 5 restart the service with new settings, because ETW buffer settings and `--capacity` apply at the next start. Steps 6 and 7 continue step 5.

1. **Light rain:** the defaults at a calm rate. Rain from a roof gutter fills the ETW barrels, the taps pass it on, and no counter moves.
2. **More barrels:** the rain outpaces the taps, so ETW sets down more barrels, up to `--etw-max-buffers`.
3. **ETW overflow (illustration):** with every barrel full at the maximum, new rain is lost and `etw_lost` counts it. The picture says "illustration, not a lab result".
4. **The dam spills:** large barrels (128 KiB) pass the storm on. The full burst reaches the lake at the default `--capacity 131072`, and `ring_dropped` counts the newest events that spill over the crest.
5. **The taller dam:** the 100,000 queries per second tier: `--etw-buffer-kib 128 --etw-min-buffers 12 --etw-max-buffers 128 --capacity 524288`. The lake holds the same storm and nothing spills ([tier settings](../guide/tuning.md#recommended-settings-for-higher-rates)).
6. **A slow receiver:** the last lake (the link FIFO) fills, and the router spills the whole FIFO into the disk spool ([slow receiver](slow-receiver.md)).
7. **Replay in order:** the receiver recovers and replay drains the spool, oldest first, before new water passes it.

The animation does not show `--etw-flush-seconds` (the taps open when a barrel is full or the timer ends) or the `--no-spool` case. The last gotcha below describes `--no-spool`.

#### Choose which lake to enlarge

| Counter that rises | Lake that is full | Act |
| --- | --- | --- |
| `etw_lost` | ETW buffers, or a CPU-starved host | Raise `--etw-max-buffers`, then `--etw-buffer-kib`. If `ETW Buffers Allocated` is far below the maximum, check CPU first |
| `ring_dropped` | Event channel | Raise `--capacity` |
| `queue_dropped` | Send queue | Check the receiver and the spool disk ([the receiver is slow](../guide/troubleshooting.md#the-receiver-is-slow)). The queue size isn't an option |

Read [Tune capture for load](../guide/tuning.md) first. Changing any ETW value (`--etw-flush-seconds` included) or `--capacity` turns off the one-time rate warning (Event ID 3), so watch the counters yourself.

#### Regenerate the animation

Edit the [animation sources](../../scripts/docs/buffer-animation/README.md) when a setting, counter, or step changes, and rebuild the GIF.

### Common Gotchas

- **The animation is an illustration.** Barrel counts, lake sizes, rates, and counter values aren't to scale. The settings and counter names are exact.
- **The 2026-11-04 default and tier runs never filled every ETW buffer.** The default run peaked at 28 of 64 buffers allocated, and the 200,000 queries per second run at 72 of 128. Step 3 shows what the maximum protects against. It isn't a measured failure of these settings. Their measured losses were at the event channel, as in step 4 ([record](../history/slow-receiver-validation.md#default-and-tier-sizing-runs-on-12-vcpus-2026-11-04)). Earlier runs with smaller buffers, and a CPU-starved guest, did lose events in ETW.
- **The cause of the delivery gaps is unknown.** At 100,000 queries per second, ETW delivered events 1 to 3 seconds late, then in a burst. A smaller buffer didn't remove the gaps. The animation shows bursts but gives no cause.
- **The barrels stand in one row.** Real ETW buffers are per logical processor, and about one was in use for each processor in the lab.
- **Spill and replay need a spool.** With `--no-spool`: a full link FIFO stops the router, the router channel fills, and the parser counts `queue_dropped`.
