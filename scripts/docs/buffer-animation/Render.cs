// Render.cs - frame loop, palette, and the delta-frame GIF writer.
//
// Render(outDir, gif) draws every frame with GDI+, builds a 256-colour palette by median
// cut over sample frames (with exact entries reserved for type, needles and lamps), maps
// each frame to palette indexes without dithering, and writes a GIF89a in which every
// frame after the first holds only the rectangle that changed; unchanged pixels inside
// it use the transparent index. It also writes frames.sha256, one hash per composited
// frame, which verify_animation.py checks against its own decode of the GIF.
//
// The static scenery of a step is drawn once and copied under every frame of that step,
// so only the moving parts cost rendering time, and only changed pixels cost GIF bytes.

using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;

public static partial class Watershed
{
    const int TransparentIndex = 255;
    static Color[] palette;
    static readonly Dictionary<int, byte> lookup = new Dictionary<int, byte>();

    static readonly Color[] Reserved = {
        Ivory, Ink, Paper, NeedleRed, LampGreen, LampRed, LampAmber, LampOff, Slate, SlateLight, PureWhite, PureBlack, OptionGold, Warn, Muted,
        C(26, 26, 30), BrassLight, BrassMid, BrassDark, Foam, WaterTop, WaterMid, WaterDeep, SteelLight, SteelMid, SteelDark };

    // ------------------------------------------------------------------ frames

    static int[] Pixels(Bitmap image)
    {
        var rect = new Rectangle(0, 0, image.Width, image.Height);
        var data = image.LockBits(rect, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        int[] argb = new int[image.Width * image.Height];
        for (int y = 0; y < image.Height; y++)
            Marshal.Copy(IntPtr.Add(data.Scan0, y * data.Stride), argb, y * image.Width, image.Width);
        image.UnlockBits(data);
        return argb;
    }

    static Bitmap statics; static int staticStep = -1;

    static void RenderFrame(double t, Bitmap target)
    {
        double u;
        int step = StepAt(t, out u);
        if (step != staticStep)
        {
            if (statics == null) statics = new Bitmap(Width, Height, PixelFormat.Format32bppArgb);
            using (g = Graphics.FromImage(statics)) DrawStatic(step);
            staticStep = step;
        }
        using (g = Graphics.FromImage(target))
        {
            g.DrawImageUnscaled(statics, 0, 0);
            DrawDynamic(t);
        }
    }

    // ------------------------------------------------------------------ palette (median cut)

    sealed class Box
    {
        public List<KeyValuePair<int, int>> Colors;    // rgb, count
        public long Weight;
        public int Rmin, Rmax, Gmin, Gmax, Bmin, Bmax;
        public void Bounds()
        {
            Rmin = Gmin = Bmin = 255; Rmax = Gmax = Bmax = 0; Weight = 0;
            foreach (var kv in Colors)
            {
                int r = kv.Key >> 16 & 255, gr = kv.Key >> 8 & 255, b = kv.Key & 255;
                if (r < Rmin) Rmin = r; if (r > Rmax) Rmax = r; if (gr < Gmin) Gmin = gr; if (gr > Gmax) Gmax = gr; if (b < Bmin) Bmin = b; if (b > Bmax) Bmax = b;
                Weight += kv.Value;
            }
        }
        public int Range { get { return Math.Max(Rmax - Rmin, Math.Max(Gmax - Gmin, Bmax - Bmin)); } }
    }

    static void BuildPalette(Dictionary<int, int> histogram, int count)
    {
        var boxes = new List<Box>();
        var first = new Box { Colors = new List<KeyValuePair<int, int>>(histogram) };
        first.Bounds();
        boxes.Add(first);
        while (boxes.Count < count)
        {
            // Split the box with the largest weight times range that still has more than one colour.
            Box best = null; double bestScore = -1;
            foreach (var b in boxes)
            {
                if (b.Colors.Count < 2) continue;
                double score = Math.Sqrt(b.Weight) * (b.Range + 1);
                if (score > bestScore) { bestScore = score; best = b; }
            }
            if (best == null) break;
            int rr = best.Rmax - best.Rmin, gg = best.Gmax - best.Gmin, bb = best.Bmax - best.Bmin;
            int shift = rr >= gg && rr >= bb ? 16 : gg >= bb ? 8 : 0;
            best.Colors.Sort((a, c) => (a.Key >> shift & 255).CompareTo(c.Key >> shift & 255));
            long half = best.Weight / 2, acc = 0; int cut = 0;
            for (; cut < best.Colors.Count - 1; cut++) { acc += best.Colors[cut].Value; if (acc >= half) { cut++; break; } }
            if (cut <= 0 || cut >= best.Colors.Count) cut = best.Colors.Count / 2;
            var other = new Box { Colors = best.Colors.GetRange(cut, best.Colors.Count - cut) };
            best.Colors.RemoveRange(cut, best.Colors.Count - cut);
            best.Bounds(); other.Bounds();
            boxes.Add(other);
        }
        var colors = new List<Color>();
        foreach (var c in Reserved) colors.Add(c);
        foreach (var b in boxes)
        {
            double r = 0, gr = 0, bl = 0;
            foreach (var kv in b.Colors) { r += (kv.Key >> 16 & 255) * (double)kv.Value; gr += (kv.Key >> 8 & 255) * (double)kv.Value; bl += (kv.Key & 255) * (double)kv.Value; }
            colors.Add(Color.FromArgb((int)Math.Round(r / b.Weight), (int)Math.Round(gr / b.Weight), (int)Math.Round(bl / b.Weight)));
        }
        while (colors.Count < TransparentIndex) colors.Add(Slate);
        if (colors.Count > TransparentIndex) colors.RemoveRange(TransparentIndex, colors.Count - TransparentIndex);
        colors.Add(Color.FromArgb(255, 0, 255));
        palette = colors.ToArray();
        lookup.Clear();
    }

    static byte Nearest(int argb)
    {
        int key = argb & 0xFFFFFF;
        byte value;
        if (lookup.TryGetValue(key, out value)) return value;
        int r = key >> 16 & 255, gr = key >> 8 & 255, b = key & 255;
        int min = int.MaxValue; value = 0;
        for (int c = 0; c < TransparentIndex; c++)
        {
            int dr = r - palette[c].R, dg = gr - palette[c].G, db = b - palette[c].B;
            int dist = dr * dr * 3 + dg * dg * 4 + db * db * 2;
            if (dist < min) { min = dist; value = (byte)c; }
        }
        lookup[key] = value;
        return value;
    }

    // ------------------------------------------------------------------ LZW

    static byte[] Lzw(byte[] pixels, int offset, int count)
    {
        const int minCode = 8, clear = 256, eoi = 257;
        var out_ = new MemoryStream();
        var table = new Dictionary<int, int>();
        int size = minCode + 1, next = eoi + 1;
        ulong acc = 0; int nbits = 0;
        Action<int> emit = code =>
        {
            acc |= (ulong)code << nbits; nbits += size;
            while (nbits >= 8) { out_.WriteByte((byte)(acc & 255)); acc >>= 8; nbits -= 8; }
        };
        emit(clear);
        int prefix = pixels[offset];
        for (int i = offset + 1; i < offset + count; i++)
        {
            int pixel = pixels[i];
            int key = prefix << 8 | pixel;
            int code;
            if (table.TryGetValue(key, out code)) { prefix = code; continue; }
            emit(prefix);
            if (next < 4096)
            {
                table[key] = next;
                if (next == 1 << size && size < 12) size++;
                next++;
            }
            else
            {
                emit(clear); table.Clear(); size = minCode + 1; next = eoi + 1;
            }
            prefix = pixel;
        }
        emit(prefix);
        if (next < 4096 && next == 1 << size && size < 12) size++;
        emit(eoi);
        if (nbits > 0) out_.WriteByte((byte)(acc & 255));
        return out_.ToArray();
    }

    static void SubBlocks(Stream s, byte[] data)
    {
        for (int i = 0; i < data.Length; i += 255)
        {
            int n = Math.Min(255, data.Length - i);
            s.WriteByte((byte)n);
            s.Write(data, i, n);
        }
        s.WriteByte(0);
    }

    static int Delay(int index)
    {
        // Centiseconds, so that every Fps frames add up to exactly 100.
        return (int)Math.Round((index + 1) * 100.0 / Fps) - (int)Math.Round(index * 100.0 / Fps);
    }

    static void U16(Stream s, int v) { s.WriteByte((byte)(v & 255)); s.WriteByte((byte)(v >> 8 & 255)); }

    // ------------------------------------------------------------------ build

    public static void Render(string outDir, string gifPath)
    {
        Directory.CreateDirectory(outDir);
        string previewDir = Path.Combine(outDir, "previews");
        Directory.CreateDirectory(previewDir);
        int frames = (int)Math.Round(Loop * Fps);

        // Pass 1: a histogram over three sample frames per step.
        var histogram = new Dictionary<int, int>();
        using (var image = new Bitmap(Width, Height, PixelFormat.Format32bppArgb))
        {
            for (int i = 0; i < Steps.Length; i++)
                foreach (double f in new[] { 0.15, 0.5, 0.85 })
                {
                    RenderFrame(StepStart(i) + f * Steps[i].Seconds, image);
                    foreach (int p in Pixels(image))
                    {
                        int key = p & 0xFFFFFF; int n;
                        histogram[key] = histogram.TryGetValue(key, out n) ? n + 1 : 1;
                    }
                }
        }
        Console.WriteLine("Sampled " + histogram.Count + " distinct colours");
        BuildPalette(histogram, TransparentIndex - Reserved.Length);

        // Pass 2: render, index, delta, write.
        var previews = new List<int>();
        for (int i = 0; i < Steps.Length; i++) { previews.Add((int)((StepStart(i) + 0.25 * Steps[i].Seconds) * Fps)); previews.Add((int)((StepStart(i) + 0.7 * Steps[i].Seconds) * Fps)); }
        var hashes = new StringBuilder();
        hashes.Append(Width).Append(' ').Append(Height).Append(' ').Append(frames).Append(' ').Append(Fps).Append('\n');
        int[] prevArgb = null; byte[] prevIdx = null;
        long bytes = 0;
        using (var sha = SHA256.Create())
        using (var image = new Bitmap(Width, Height, PixelFormat.Format32bppArgb))
        using (var gif = new FileStream(gifPath, FileMode.Create))
        {
            gif.Write(Encoding.ASCII.GetBytes("GIF89a"), 0, 6);
            U16(gif, Width); U16(gif, Height); gif.WriteByte(0xF7); gif.WriteByte(0); gif.WriteByte(0);
            foreach (var c in palette) { gif.WriteByte(c.R); gif.WriteByte(c.G); gif.WriteByte(c.B); }
            gif.Write(new byte[] { 0x21, 0xFF, 0x0B }, 0, 3);
            gif.Write(Encoding.ASCII.GetBytes("NETSCAPE2.0"), 0, 11);
            gif.Write(new byte[] { 3, 1, 0, 0, 0 }, 0, 5);
            var delta = new byte[Width * Height];
            for (int f = 0; f < frames; f++)
            {
                RenderFrame(f / (double)Fps, image);
                int[] argb = Pixels(image);
                var idx = new byte[argb.Length];
                int left = Width, top = Height, right = -1, bottom = -1;
                for (int i = 0; i < argb.Length; i++)
                {
                    if (prevArgb != null && argb[i] == prevArgb[i]) { idx[i] = prevIdx[i]; continue; }
                    idx[i] = Nearest(argb[i]);
                    if (prevIdx != null && idx[i] == prevIdx[i]) continue;
                    int x = i % Width, y = i / Width;
                    if (x < left) left = x; if (x > right) right = x; if (y < top) top = y; if (y > bottom) bottom = y;
                }
                if (prevIdx == null) { left = 0; top = 0; right = Width - 1; bottom = Height - 1; }
                else if (right < 0) { left = 0; top = 0; right = 0; bottom = 0; }
                int w = right - left + 1, h = bottom - top + 1;
                int n = 0;
                for (int y = top; y <= bottom; y++)
                    for (int x = left; x <= right; x++)
                    {
                        int i = y * Width + x;
                        delta[n++] = prevIdx != null && idx[i] == prevIdx[i] ? (byte)TransparentIndex : idx[i];
                    }
                gif.Write(new byte[] { 0x21, 0xF9, 4, 0x05 }, 0, 4);
                U16(gif, Delay(f)); gif.WriteByte(TransparentIndex); gif.WriteByte(0);
                gif.WriteByte(0x2C); U16(gif, left); U16(gif, top); U16(gif, w); U16(gif, h); gif.WriteByte(0);
                gif.WriteByte(8);
                SubBlocks(gif, Lzw(delta, 0, n));
                hashes.Append(BitConverter.ToString(sha.ComputeHash(idx)).Replace("-", "").ToLowerInvariant()).Append('\n');
                if (previews.Contains(f)) SavePreview(idx, Path.Combine(previewDir, "frame-" + f.ToString("D3") + ".png"));
                prevArgb = argb; prevIdx = idx;
                if (f % 60 == 0) Console.WriteLine("Rendered " + f + " / " + frames + ", " + gif.Length.ToString("#,0") + " bytes so far");
            }
            gif.WriteByte(0x3B);
            bytes = gif.Length;
        }
        File.WriteAllText(Path.Combine(outDir, "frames.sha256"), hashes.ToString());
        Console.WriteLine("Wrote " + gifPath + ": " + bytes.ToString("#,0") + " bytes, " + frames + " frames, " + Width + "x" + Height + " at " + Fps + " fps, " + Loop + " s");
    }

    // A preview PNG of the quantised frame, so the review sees what the GIF shows.
    static void SavePreview(byte[] idx, string path)
    {
        using (var bmp = new Bitmap(Width, Height, PixelFormat.Format32bppArgb))
        {
            var rect = new Rectangle(0, 0, Width, Height);
            var data = bmp.LockBits(rect, ImageLockMode.WriteOnly, PixelFormat.Format32bppArgb);
            int[] row = new int[Width];
            for (int y = 0; y < Height; y++)
            {
                for (int x = 0; x < Width; x++) row[x] = palette[idx[y * Width + x]].ToArgb();
                Marshal.Copy(row, 0, IntPtr.Add(data.Scan0, y * data.Stride), Width);
            }
            bmp.UnlockBits(data);
            bmp.Save(path, ImageFormat.Png);
        }
    }

    // Snap writes full-colour PNGs at the given seconds, without the palette, for quick layout checks.
    public static void Snap(string dir, double[] times)
    {
        Directory.CreateDirectory(dir);
        using (var image = new Bitmap(Width, Height, PixelFormat.Format32bppArgb))
            foreach (double t in times)
            {
                RenderFrame(t, image);
                string name = Path.Combine(dir, "snap-" + t.ToString("000.0", System.Globalization.CultureInfo.InvariantCulture).Replace('.', '_') + ".png");
                image.Save(name, ImageFormat.Png);
                Console.WriteLine("Wrote " + name);
            }
    }
}
