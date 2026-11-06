# Buffers as a watershed

> **Status:** Current · **Describes:** the buffers and counters in `src/etw/`, `src/pipeline/` and `src/sender/` on `main` at commit `d03b00f` as of 2026-11-05 · **Evidence dated:** 2026-11-04 for the sizing runs (explanation, from the source and [tuning.md](../guide/tuning.md)) · **Reviewed:** 2026-11-05

### Quick Start

![Animation in nine steps, drawn as a water works with brass gauges, glass tanks and a concrete dam. Rain from dns.exe fills a row of ETW buffer tanks. A flush gate passes full tanks to the event channel, a lake behind a dam. A mill wheel (the parser) feeds the send queue, the link FIFO and the TCP receiver, the sea. Steps 1 to 4: the calm river, the tanks and the flush timer, a burst that digs more tanks up to the maximum, and an illustration of ETW overflow counted in etw_lost. Step 5: with large tanks, a full burst spills over the crest and ring_dropped counts the loss. Step 6: a taller dam holds the same burst. Steps 7 and 8: a slow receiver fills the link FIFO, the whole lake spills into the disk spool pond, and replay empties the pond oldest first after the receiver recovers. Step 9: with no spool, a full link FIFO stops the sluice and queue_dropped counts the loss.](../assets/pipeline/buffers-as-a-watershed.gif)

```text
rain -> ETW tanks -> flush gate -> channel lake -> parser mill -> send queue -> receiver
        etw_lost                   ring_dropped                   \-> disk spool (spill, replay)
```

Every lake is a buffer. A buffer smooths a burst so the next stage can work at a steady pace. When a lake is full, new water goes over the spillway and a counter records it. The send queue is the exception: with a spool, the router drains it to the disk pond first.

ETW is Event Tracing for Windows. A FIFO is a first-in, first-out queue.

### Steps / Configuration

#### What the picture means

| Picture | What it is | Setting | Loss counter |
| --- | --- | --- | --- |
| Rain | ETW events 256, 257, 258 from `dns.exe` | | |
| A tank | One ETW buffer | `--etw-buffer-kib` (default 32) sets its size | |
| Solid tanks | Buffers ETW allocates at start | `--etw-min-buffers` (default 12) | |
| Dashed tanks | Buffers ETW adds on demand | `--etw-max-buffers` (default 64) sets the limit | `etw_lost` (events), `etw_buffers_lost` (buffers) |
| Flush gate | ETW hands a buffer on when it is full or the flush timer expires | `--etw-flush-seconds` (default 1) | |
| Lake behind a dam | The bounded event channel | `--capacity` (default 131,072 events) | `ring_dropped` |
| Spillway over the crest | A full channel drops the newest event | | `ring_dropped` |
| Mill wheel | `dnstap-parser`: decode, encode, queue blocks | | `rejected`, `queue_dropped` |
| Send queue | Parser-to-router queue (256 blocks) and link FIFO (256 blocks) | | |
| Disk spool | Encrypted overflow pond for a slow or absent receiver | `--spool-max-mib` (default 4,096) | `spool_dropped` |
| Sea | The TCP receiver | | |

ETW allocates a new buffer when the existing ones fill faster than ETW can flush them, up to the maximum. Past the maximum, ETW discards events until a buffer is free.

Microsoft documents this behavior for `EVENT_TRACE_PROPERTIES`. ETW raises the minimum to two buffers per logical processor, so on a large host the real counts are higher. Limits and ranges: [Configuration reference](../guide/configuration.md#options).

#### The nine steps

The animation runs for 96 seconds. Each step stays on screen for 10 to 12 seconds, and a caption under the picture states the step in one sentence. Steps 1 to 4 are one continuous run. Steps 5, 6, and 9 restart the service with new settings, because ETW buffer settings and `--capacity` apply at the next start. Steps 7 and 8 continue step 6.

1. **The whole river:** the defaults at a calm rate. Each lake stays low and no counter moves.
2. **The ETW tanks:** a detail inset shows one tank and the flush timer. ETW opens the gate when a tank is full or `--etw-flush-seconds` ends.
3. **A burst digs tanks:** the tanks fill faster than the gate opens, so ETW adds tanks up to `--etw-max-buffers`.
4. **ETW overflow (illustration):** with every tank full at the maximum, ETW discards new events and `etw_lost` counts them. The picture carries the banner "Illustration, not a lab result".
5. **The dam spills:** large tanks (128 KiB) pass the storm on. The full burst reaches the channel lake at the default `--capacity`, and `ring_dropped` counts the newest events that spill over the crest.
6. **The taller dam:** the 100,000 queries per second tier: `--etw-buffer-kib 128 --etw-min-buffers 12 --etw-max-buffers 128 --capacity 524288`. The lake holds the same burst and no event spills ([tier settings](../guide/tuning.md#recommended-settings-for-higher-rates)).
7. **A slow sea:** the receiver slows, the link FIFO fills, and the router spills the whole FIFO into the disk spool. New water follows it ([slow receiver](slow-receiver.md)).
8. **Replay in order:** the receiver recovers and replay empties the spool, oldest first, before new water passes it.
9. **No pond:** with `--no-spool`, a full link FIFO stops the sluice, and the parser counts the new blocks in `queue_dropped`.

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

- **The animation is an illustration.** Tank counts, lake sizes, rates, and counter values aren't to scale. The settings and counter names are exact.
- **The 2026-11-04 default and tier runs never filled every ETW buffer.** The default run peaked at 28 of 64 buffers allocated, and the 200,000 queries per second run at 72 of 128. Step 4 shows what the maximum protects against. It isn't a measured failure of these settings. Their measured losses were at the event channel, as in step 5 ([record](../history/slow-receiver-validation.md#default-and-tier-sizing-runs-on-12-vcpus-2026-11-04)). Earlier runs with smaller buffers, and a CPU-starved guest, did lose events in ETW.
- **The cause of the delivery gaps is unknown.** At 100,000 queries per second, ETW delivered events 1 to 3 seconds late, then in a burst. A smaller buffer didn't remove the gaps. The animation shows bursts but gives no cause.
- **The tanks fill one after another.** Real ETW buffers are per logical processor, and about one was in use for each processor in the lab.
- **Spill and replay need a spool.** Step 9 shows `--no-spool`: a full link FIFO stops the router, the router channel fills, and the parser counts `queue_dropped`.
