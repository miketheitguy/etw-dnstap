# Buffer animation

Builds `docs/assets/pipeline/buffers-as-a-watershed.gif`: the buffers of the [data path](../../../docs/design/data-path.md) drawn as rain, a row of rain barrels, a dam, a mill, a river, a pond and the sea, in seven steps of 12 to 14 seconds. The text version is [Buffers as a watershed](../../../docs/design/buffer-watershed.md). The picture is 1600 x 900 at 8 frames per second, 92 seconds per loop, about 7 MB.

### Quick Start

PowerShell 7 on Windows and Python 3, no extra package. From the repository root:

```powershell
.\scripts\docs\buffer-animation\Build-Animation.ps1
```

The build takes about a minute. The last output line must contain `all verified`: `verify_animation.py` decodes the finished GIF with its own LZW decoder and compares every composited frame with the SHA-256 that the renderer wrote to `target\buffer-animation\frames.sha256`.

### Steps / Configuration

| File | Job |
| --- | --- |
| `Paint.cs` | Skeuomorphic primitives: brass labels, the dark option plate, steel pipes, concrete, the galvanised rain barrel with its lid, sight window, tap and overflow spout, the gutter, water bodies, streams, falls, rain, the mill wheel and the clouds. |
| `Scene.cs` | The content: the seven steps with their captions and plates, the water model (`Compute`), the layout constants, the static scenery (`DrawStatic`) and the moving parts (`DrawDynamic`). |
| `Render.cs` | The frame loop, the median-cut palette with reserved colours for type and the key materials, the delta-frame LZW GIF writer, the frame hashes and the PNG previews. |
| `verify_animation.py` | Decodes the GIF and checks every frame, the loop extension and the frame delays. |
| `Build-Animation.ps1` | Runs the build. |

Everything on screen is a function of the time `t`, so every build gives the same GIF. The steps:

| Step | Seconds | Plate | What moves |
| --- | --- | --- | --- |
| 1 | 12 | `--etw-buffer-kib 32` | Light rain. Two barrels fill and drain through their taps. Every lake stays low. |
| 2 | 12 | `--etw-max-buffers 64` | Heavy rain. Every barrel fills. Two more barrels settle on their pads. The lake rises to two thirds. |
| 3 | 12 | `etw_lost` | Illustration, not a lab result. Every barrel is full, every tap runs, the overflow spouts spill onto the ground. |
| 4 | 14 | `--capacity 131072`, `ring_dropped` | Restart with large barrels. Each slug fills the lake to the crest and the spill runs into the basin. |
| 5 | 14 | `--capacity 524288` and the tier ETW values | Restart with the 100,000 queries per second tier. The taller dam holds each slug at about half. |
| 6 | 14 | `spooled` | The outlet valve closes. The second lake fills, then tips whole into the pond. New water follows it. |
| 7 | 14 | `replayed` | The valve opens. The pond drains through the replay pipe, oldest first. Then the sluice turns back. |

To change the storyboard, edit `Steps` in `Scene.cs` (seconds, caption, plate text, counter) and the matching `case` in `Compute`, which sets the rain, the barrel levels and taps, the lake levels, the gate and the counters. Keep each caption to one sentence of at most 18 words, and keep at most eight labels and plates on screen in any step. Option names, defaults and tier values in the plates are exact; every count is an illustration and the footnote says so.

Check the numbers and the layout without a full build:

```powershell
.\scripts\docs\buffer-animation\Build-Animation.ps1 -Report
.\scripts\docs\buffer-animation\Build-Animation.ps1 -Snap 5,40.5,70
```

`-Report` prints the peak level of each lake, the barrel count and the final counters of every step. `-Snap` writes full-colour PNG frames at the given seconds to `target\buffer-animation\snaps\`. A full build also writes two quantised previews per step to `target\buffer-animation\previews\`; those show what the GIF shows. To check the water speed, snap two frames one second apart and compare them: every surface, drop and wheel must move only a few pixels.

### Common Gotchas

- Only changed pixels cost GIF bytes. The scenery of a step is drawn once and copied under every frame, and the moving parts are drawn on top. The water gradient is anchored at the crest, not at the surface, so a rising level changes only the pixels near the surface. Keep textures static or the delta frames bloat. The verifier fails a GIF over 12 MB and notes one over 9 MB.
- The palette is built from three sample frames per step. A new material with colours far from the existing ones may band; add its key colours to `Reserved` in `Render.cs` if type loses its edge.
- Keep captions, option names and defaults in step with [Tune capture for load](../../../docs/guide/tuning.md) and the [watershed page](../../../docs/design/buffer-watershed.md). Step 3 is an illustration, not a lab result: the default and tier runs never filled every ETW buffer, and both the step tag and the counter plate must keep saying so.
- The barrel count is not the buffer count. Five standing barrels and two pads stand for `--etw-min-buffers` and `--etw-max-buffers`; the footnote covers the scale. The taps deliver in an uneven rhythm in the storm steps, and the picture gives no cause for the pause, on purpose.
- `Add-Type` compiles the three C# files on every run; a compiler error names the file and line. Warnings are ignored on purpose.
