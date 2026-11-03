# Pipeline animation

These files build `docs/assets/pipeline/etw-to-tcp-pipeline.gif`. The GIF shows
the [data path](../../../docs/design/data-path.md) from an ETW event to a TCP
frame, in six stages of six seconds. Stage 6 shows a receiver outage: frames go
to the disk spool and are replayed after the reconnect.

| File | Job |
| --- | --- |
| `render.cs` | Draws every frame with GDI+ and writes the frames as raw palette indexes. |
| `make_animation.py` | Turns the raw frames into a GIF. It stores only the changed area of each frame. It then decodes the GIF and checks that every frame is exact. |
| `Build-Animation.ps1` | Runs both steps. |

## Build the GIF

You need PowerShell 7 on Windows and Python 3. The build needs no other
package.

1. Run the build script from the repository root:

   ```powershell
   .\scripts\docs\pipeline-animation\Build-Animation.ps1
   ```

2. Check that the last line says `all verified`.

The script writes the raw frames and PNG previews to `target\pipeline-animation\`.
Git ignores that folder. Open the previews to check the layout before you commit
the GIF.

## Change the animation

The animation is a function of the time `t`, so every build gives the same
frames. Edit these parts of `render.cs`:

- `StageTitle` and `StageSub` hold the caption text of the six stages.
- `BuildTimeline` holds the timing of each event. `RouteFrames` sends each
  frame to the receiver or to the spool. `DownAt` and `UpAt` set the outage.
- The functions `Boxes`, `ThreadStrip`, and `Tokens` draw the scene.

Keep the caption text in step with the
[stage table](../../../docs/design/data-path.md#stages). Keep each counter name
and each number the same as in the code.
