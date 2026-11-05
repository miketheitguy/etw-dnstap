# Pipeline animation

Builds `docs/assets/pipeline/etw-to-tcp-pipeline.gif`: the [data path](../../../docs/design/data-path.md) in six six-second stages. Stage 5: slow receiver (full FIFO spills to the disk spool, then replays). Stage 6: receiver outage (spool, replay after reconnect).

### Quick Start

PowerShell 7 on Windows and Python 3, no extra package. From the repository root:

```powershell
.\scripts\docs\pipeline-animation\Build-Animation.ps1
```

The last output line must contain `all verified`.

### Steps / Configuration

| File | Job |
| --- | --- |
| `render.cs` | Draws frames with GDI+, writes raw palette indexes |
| `make_animation.py` | Writes a GIF of changed areas, decodes it, checks every frame |
| `Build-Animation.ps1` | Runs both |

Frames depend only on time `t`. In `render.cs`:

- `StageTitle`, `StageSub`: stage captions.
- `BuildTimeline`: event timing. `RouteFrames` sends frames to the receiver, FIFO, or spool. `DownAt`, `UpAt`: the outage. `FullAt`, `Drained`, `ReplayFrom`: the stage 5 slow receiver, read by `Spilling` and `FifoFill`.
- `Boxes`, `ThreadStrip`, `Tokens`: the scene.

### Common Gotchas

- Raw frames and PNG previews go to the ignored `target\pipeline-animation\`. Check the previews before you commit the GIF.
- Keep captions in step with the [stage table](../../../docs/design/data-path.md#stages), and counter names and numbers the same as in the code.
