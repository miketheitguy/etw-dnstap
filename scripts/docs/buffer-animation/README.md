# Buffer animation

Builds `docs/assets/pipeline/buffers-as-a-watershed.gif`: the buffers of the [data path](../../../docs/design/data-path.md) drawn as rain, a row of glass-fronted rain barrels, a dam, a mill, a river, a pond and the sea, in seven steps of 18 to 20 seconds. The picture builds up one element per step: each element fades in when the story reaches it, and elements that are not part of the current step stay faint and still. The text version is [Buffers as a watershed](../../../docs/design/buffer-watershed.md). The picture is 1600 x 900 at 8 frames per second, 130 seconds per loop, about 7.3 MB.

### Quick Start

PowerShell 7 on Windows and Python 3, no extra package. From the repository root:

```powershell
.\scripts\docs\buffer-animation\Build-Animation.ps1
```

The build takes under a minute. The last output line must contain `all verified`: `verify_animation.py` decodes the finished GIF with its own LZW decoder and compares every composited frame with the SHA-256 that the renderer wrote to `target\buffer-animation\frames.sha256`.

### Steps / Configuration

| File | Job |
| --- | --- |
| `Paint.cs` | Skeuomorphic primitives: the sign (plain type with a soft shadow), the dark option plate, steel pipes, concrete, cavities and pits, the open-topped rain barrel with its glass front, tap and overflow spout, the gutter, water bodies, streams, falls, rain, the mill wheel, the clouds, and `Layer`, which draws an element through an offscreen bitmap at a given opacity. |
| `Scene.cs` | The content: the seven steps with their captions, the water model and the visibility of every element (`Compute`), the layout constants, the backdrop (`DrawStatic`) and the elements (`DrawDynamic`). |
| `Render.cs` | The frame loop, the median-cut palette with reserved colours for type and the key materials, the delta-frame LZW GIF writer, the frame hashes and the PNG previews. |
| `verify_animation.py` | Decodes the GIF and checks every frame, the loop extension and the frame delays. |
| `Build-Animation.ps1` | Runs the build. |

Everything on screen is a function of the time `t`, so every build gives the same GIF. Every step shows at most two signs, one at a time, and one plate. The steps:

| Step | Seconds | Signs | Plate | What moves |
| --- | --- | --- | --- | --- |
| 1 | 18 | ETW buffers, event channel | `--etw-buffer-kib 32` | Light rain. Four barrels fill from near empty to near the brim, each tap opens, the barrel drains. The lake fades in before the first tap opens. |
| 2 | 18 | none | `--etw-max-buffers 64` | The storm grows and the barrels cycle faster. Two more barrels are lowered onto the empty pads and fill. The faint lake rises to two thirds. |
| 3 | 18 | none | `etw_lost` | Illustration, not a lab result. Every barrel fills to the brim, water pours over the rims and the overflow spouts, every tap runs. |
| 4 | 20 | parser | `--etw-buffer-kib 128`, `--etw-max-buffers 128`, then `--capacity 131072` with `ring_dropped` | Restart with large barrels. The barrels go faint, the lake comes forward, the mill fades in, the slug fills the lake to the crest and the spill runs into the basin. |
| 5 | 18 | send queue | the 100,000 queries per second tier | Restart with the tier. The taller dam holds the slug at about half. The river fades in and swells close to its banks. |
| 6 | 20 | receiver, disk spool | `spooled` | The second lake and the sea fade in. The valve closes, the second lake fills, the pond fades in, and the sluice tips the whole lake into it. New water follows. |
| 7 | 18 | none | `replayed` | The valve opens. The pond drains through the replay pipe, oldest first. Then the sluice turns back. |

To change the storyboard, edit `Steps` in `Scene.cs` (seconds, caption) and the matching `case` in `Compute`, which sets each element to `Hidden`, `Faint`, `Full`, `FadeIn` or `FadeOut`, the rain, the barrel levels and taps, the lake levels, the gate, the sign and plate alphas and the counters; the sign texts and the plate texts are in `Signs`. Keep each caption to one sentence of at most 18 words. Option names, defaults and tier values in the plates are exact; every count is an illustration and the footnote says so.

Check the numbers and the layout without a full build:

```powershell
.\scripts\docs\buffer-animation\Build-Animation.ps1 -Report
.\scripts\docs\buffer-animation\Build-Animation.ps1 -Snap 4,12,70,104
```

`-Report` prints the barrel level range, the peak level of each lake and the final counters of every step. `-Snap` writes full-colour PNG frames at the given seconds to `target\buffer-animation\snaps\`. A full build also writes three quantised previews per step to `target\buffer-animation\previews\`; those show what the GIF shows. To check the water speed, snap two frames one second apart and compare them: a barrel surface rises about 20 pixels, and every drop, fleck and wheel moves only a few pixels.

### Common Gotchas

- Only changed pixels cost GIF bytes. The backdrop of a step is drawn once and copied under every frame; every element is redrawn on top each frame, and identical pixels cost nothing. A faint element is drawn with `Motion` 0, so its waves, streaks and foam stand still. A fade changes every pixel of its element for its two seconds, so keep fades few and short. The water gradient is anchored at the crest, not at the surface, so a rising level changes only the pixels near the surface. The verifier fails a GIF over 12 MB and notes one over 9 MB; the build prints the bytes at the end of each step.
- The palette is built from three sample frames per step. A new material with colours far from the existing ones may band; add its key colours to `Reserved` in `Render.cs` if type loses its edge.
- Keep captions, option names and defaults in step with [Tune capture for load](../../../docs/guide/tuning.md) and the [watershed page](../../../docs/design/buffer-watershed.md). Step 3 is an illustration, not a lab result: the default and tier runs never filled every ETW buffer, and both the step tag and the plate tag must keep saying so.
- The barrel count is not the buffer count. Four standing barrels and two pads stand for `--etw-min-buffers` and `--etw-max-buffers`; the footnote covers the scale. In steps 4 to 7 the taps deliver in an uneven rhythm and the barrels stay low; the picture gives no cause for the pause, on purpose.
- `Add-Type` compiles the three C# files on every run; a compiler error names the file and line. Warnings are ignored on purpose.
