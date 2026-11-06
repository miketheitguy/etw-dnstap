# Buffer animation

Builds `docs/assets/pipeline/buffers-as-a-watershed.gif`: the buffers of the [data path](../../../docs/design/data-path.md) as rain, glass tanks, a dam, a mill, a river, a pond and the sea, in nine steps of 10 to 12 seconds. The text version is [Buffers as a watershed](../../../docs/design/buffer-watershed.md). The picture is 1600 x 900 at 6 frames per second, 96 seconds per loop, about 7 MB.

### Quick Start

PowerShell 7 on Windows and Python 3, no extra package. From the repository root:

```powershell
.\scripts\docs\buffer-animation\Build-Animation.ps1
```

The build takes about half a minute. The last output line must contain `all verified`: `verify_animation.py` decodes the finished GIF with its own LZW decoder and compares every composited frame with the SHA-256 that the renderer wrote to `target\buffer-animation\frames.sha256`.

### Steps / Configuration

| File | Job |
| --- | --- |
| `Paint.cs` | Skeuomorphic primitives: brushed steel, brass plates with engraved type, glass tanks, water with a moving meniscus, rain, gauges, odometers, lamps, the mill wheel, concrete and stone. |
| `Scene.cs` | The content: the nine steps with their captions and settings, the water model (`Compute`), the layout constants, the static scenery (`DrawStatic`) and the moving parts (`DrawDynamic`). |
| `Render.cs` | The frame loop, the median-cut palette with reserved colours for type and needles, the delta-frame LZW GIF writer, the frame hashes and the PNG previews. |
| `verify_animation.py` | Decodes the GIF and checks every frame, the loop extension and the frame delays. |
| `Build-Animation.ps1` | Runs the build. |

Everything on screen is a function of the time `t`, so every build gives the same GIF. To change the storyboard, edit `Steps` in `Scene.cs` (title, seconds, caption, settings, storm label) and the matching `case` in `Compute`, which sets the water levels, the gate rhythm, the counters, the lamps and the event-log text for that step. Option names, defaults and tier values in the plates are exact; every counter value is an illustration and the footnote says so.

Check the numbers and the layout without a full build:

```powershell
.\scripts\docs\buffer-animation\Build-Animation.ps1 -Report
.\scripts\docs\buffer-animation\Build-Animation.ps1 -Snap 5,44.8,68
```

`-Report` prints the peak level of each lake and the final counters of every step. `-Snap` writes full-colour PNG frames at the given seconds to `target\buffer-animation\snaps\`. A full build also writes two quantised previews per step to `target\buffer-animation\previews\`; those show what the GIF shows.

### Common Gotchas

- Only changed pixels cost GIF bytes. The scenery of a step is drawn once and copied under every frame, and the moving parts are drawn on top. Keep water surfaces thin and textures static, or the delta frames bloat. The verifier fails a GIF over 12 MB and notes one over 9 MB.
- The palette is built from three sample frames per step. A new material with colours far from the existing ones may band; add its key colours to `Reserved` in `Render.cs` if type or needles lose their edge.
- Keep captions, option names and defaults in step with [Tune capture for load](../../../docs/guide/tuning.md) and the [watershed page](../../../docs/design/buffer-watershed.md). Step 4 is an illustration, not a lab result: the default and tier runs never filled every ETW buffer, and the banner on that step must stay.
- `Add-Type` compiles the three C# files on every run; a compiler error names the file and line. Warnings are ignored on purpose.
