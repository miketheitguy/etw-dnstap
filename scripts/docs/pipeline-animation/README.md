# Pipeline animation

These files build `docs/assets/pipeline/etw-to-tcp-pipeline.gif`. The GIF shows the [data path](../../../docs/design/data-path.md) from an ETW event to a TCP frame in six stages of six seconds. Stage 5 shows a slow receiver (full FIFO spills to the disk spool, then replays). Stage 6 shows a receiver outage (spool, then replay after reconnect).

| File | Job |
| --- | --- |
| `render.cs` | Draws every frame with GDI+ and writes raw palette indexes. |
| `make_animation.py` | Turns the raw frames into a GIF that stores only the changed area of each frame. It then decodes the GIF and checks every frame. |
| `Build-Animation.ps1` | Runs both steps. |

## Build the GIF

You need PowerShell 7 on Windows and Python 3, with no extra package.

1. Run the build script from the repository root:

   ```powershell
   .\scripts\docs\pipeline-animation\Build-Animation.ps1
   ```

2. Check that the last line contains `all verified`.

Raw frames and PNG previews go to the ignored `target\pipeline-animation\`. Check the previews before you commit the GIF.

## Change the animation

Frames depend only on the time `t`. Edit these parts of `render.cs`:

- `StageTitle` and `StageSub` hold the caption text of the six stages.
- `BuildTimeline` holds the timing of each event. `RouteFrames` sends each frame to the receiver, the FIFO, or the spool. `DownAt` and `UpAt` set the outage. `FullAt`, `Drained`, and `ReplayFrom` set the slow receiver in stage 5, which `Spilling` and `FifoFill` read.
- `Boxes`, `ThreadStrip`, and `Tokens` draw the scene.

Keep the caption text in step with the [stage table](../../../docs/design/data-path.md#stages). Keep each counter name and number the same as in the code.
