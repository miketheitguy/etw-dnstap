// Paint.cs - skeuomorphic drawing primitives for the buffer animation.
//
// Light comes from the top left. Every shape gets a highlight on its top and left
// edges, a shade on its bottom and right edges, and a soft drop shadow offset to the
// bottom right. Metal is a gradient with a brushed grain, glass is a dark cavity with
// a vertical highlight, water is a gradient from a light surface to a deep floor with
// a moving meniscus line.
//
// Everything here is a pure function of its arguments, so a frame rendered twice is
// identical. The noise and grain textures use a fixed seed.

using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Text;

public static partial class Watershed
{
    static Graphics g;

    // ------------------------------------------------------------------ colours

    static Color C(int r, int gr, int b) { return Color.FromArgb(r, gr, b); }
    static Color A(int alpha, Color c) { return Color.FromArgb(Math.Max(0, Math.Min(255, alpha)), c.R, c.G, c.B); }
    static Color Mix(Color a, Color b, double u)
    {
        u = Math.Max(0, Math.Min(1, u));
        return Color.FromArgb((int)(a.R + (b.R - a.R) * u), (int)(a.G + (b.G - a.G) * u), (int)(a.B + (b.B - a.B) * u));
    }

    // Exact colours that the GIF palette must hold, so type and needles stay crisp.
    public static readonly Color Ivory = C(244, 238, 222), Ink = C(38, 30, 18), Paper = C(238, 232, 214),
        NeedleRed = C(190, 32, 28), LampGreen = C(70, 200, 100), LampRed = C(230, 60, 50), LampAmber = C(255, 190, 70),
        LampOff = C(70, 60, 50), Slate = C(30, 30, 34), SlateLight = C(52, 52, 58), PureWhite = C(255, 255, 255), PureBlack = C(0, 0, 0),
        OptionGold = C(255, 222, 140), Warn = C(255, 150, 80), Muted = C(180, 176, 168);

    static readonly Color SteelLight = C(188, 194, 202), SteelMid = C(128, 136, 146), SteelDark = C(66, 72, 80),
        BrassLight = C(240, 212, 130), BrassMid = C(192, 152, 62), BrassDark = C(118, 86, 30),
        IronLight = C(110, 112, 118), IronMid = C(64, 66, 72), IronDark = C(30, 32, 36),
        ConcreteLight = C(176, 174, 166), ConcreteDark = C(112, 110, 102),
        WaterTop = C(112, 184, 228), WaterMid = C(44, 120, 190), WaterDeep = C(18, 60, 120), Foam = C(236, 246, 252),
        SkyTop = C(132, 168, 205), SkyLow = C(206, 218, 228), GrassLight = C(118, 160, 84), GrassDark = C(74, 112, 56),
        EarthLight = C(128, 98, 64), EarthDark = C(72, 54, 36), StoneLight = C(166, 160, 148), StoneDark = C(96, 92, 84),
        WoodLight = C(170, 120, 68), WoodDark = C(96, 62, 30), WoodMid = C(134, 92, 48);

    // ------------------------------------------------------------------ fonts and text

    static Font F(string family, float px, bool bold = false)
    {
        return new Font(family, px, bold ? FontStyle.Bold : FontStyle.Regular, GraphicsUnit.Pixel);
    }
    const string Serif = "Georgia", Sans = "Segoe UI", Mono = "Consolas";

    static SizeF Measure(string s, string family, float px, bool bold = false)
    {
        using (var f = F(family, px, bold)) return g.MeasureString(s, f, 4000, StringFormat.GenericTypographic);
    }

    static void Text(string s, float x, float y, string family, float px, Color color, bool bold = false)
    {
        using (var f = F(family, px, bold)) using (var b = new SolidBrush(color))
            g.DrawString(s, f, b, x, y, StringFormat.GenericTypographic);
    }

    static void TextCenter(string s, float cx, float y, string family, float px, Color color, bool bold = false)
    {
        float w = Measure(s, family, px, bold).Width;
        Text(s, cx - w / 2, y, family, px, color, bold);
    }

    // Engraved type: a light copy one pixel down and right, then the dark letters.
    static void Engrave(string s, float x, float y, string family, float px, bool bold = false, Color? ink = null)
    {
        Text(s, x + 1, y + 1, family, px, A(150, PureWhite), bold);
        Text(s, x, y, family, px, ink ?? Ink, bold);
    }
    static void EngraveCenter(string s, float cx, float y, string family, float px, bool bold = false, Color? ink = null)
    {
        float w = Measure(s, family, px, bold).Width;
        Engrave(s, cx - w / 2, y, family, px, bold, ink);
    }

    // ------------------------------------------------------------------ shapes

    static GraphicsPath Round(float x, float y, float w, float h, float r)
    {
        var p = new GraphicsPath();
        r = Math.Min(r, Math.Min(w, h) / 2);
        if (r < 0.5f) { p.AddRectangle(new RectangleF(x, y, w, h)); return p; }
        p.AddArc(x, y, r * 2, r * 2, 180, 90);
        p.AddArc(x + w - 2 * r, y, 2 * r, 2 * r, 270, 90);
        p.AddArc(x + w - 2 * r, y + h - 2 * r, 2 * r, 2 * r, 0, 90);
        p.AddArc(x, y + h - 2 * r, 2 * r, 2 * r, 90, 90);
        p.CloseFigure();
        return p;
    }

    static void FillGrad(GraphicsPath p, RectangleF r, Color a, Color b, float angle)
    {
        if (r.Width < 1 || r.Height < 1) return;
        using (var br = new LinearGradientBrush(r, a, b, angle)) g.FillPath(br, p);
    }
    static void FillGradRect(RectangleF r, Color a, Color b, float angle)
    {
        if (r.Width < 1 || r.Height < 1) return;
        using (var br = new LinearGradientBrush(r, a, b, angle)) g.FillRectangle(br, r);
    }
    static void FillRect(RectangleF r, Color c) { using (var b = new SolidBrush(c)) g.FillRectangle(b, r); }
    static void Fill(GraphicsPath p, Color c) { using (var b = new SolidBrush(c)) g.FillPath(b, p); }
    static void Stroke(GraphicsPath p, Color c, float w) { using (var pen = new Pen(c, w)) g.DrawPath(pen, p); }
    static void Line(float x1, float y1, float x2, float y2, Color c, float w)
    {
        using (var pen = new Pen(c, w)) g.DrawLine(pen, x1, y1, x2, y2);
    }

    // A soft drop shadow for a rounded box. Light is top left, so the shadow falls bottom right.
    static void Shadow(float x, float y, float w, float h, float r, float dx = 5, float dy = 7, int alpha = 60)
    {
        for (int i = 3; i >= 0; i--)
        {
            using (var p = Round(x + dx - i * 1.5f, y + dy - i * 1.5f, w + i * 3, h + i * 3, r + i))
                Fill(p, A(alpha / 4, PureBlack));
        }
    }
    static void ShadowEllipse(float cx, float cy, float r, float dx = 5, float dy = 7, int alpha = 70)
    {
        for (int i = 3; i >= 0; i--)
            using (var b = new SolidBrush(A(alpha / 4, PureBlack)))
                g.FillEllipse(b, cx - r - i * 1.5f + dx, cy - r - i * 1.5f + dy, 2 * r + i * 3, 2 * r + i * 3);
    }

    // Bevel: light top/left edge, dark bottom/right edge, inside a rounded box.
    static void Bevel(float x, float y, float w, float h, float r, int strength = 110, float width = 1.5f)
    {
        if (w < 4 || h < 4) return;
        using (var p = Round(x + 0.75f, y + 0.75f, w - 1.5f, h - 1.5f, r))
        using (var br = new LinearGradientBrush(new RectangleF(x, y, Math.Max(2, w), Math.Max(2, h)), A(strength, PureWhite), A(strength, PureBlack), 60f))
        using (var pen = new Pen(br, width))
            g.DrawPath(pen, p);
    }

    // Brushed grain: faint horizontal lines with a seeded random alpha. Static layer only.
    static void Grain(float x, float y, float w, float h, int seed, int alpha = 22, float step = 2f)
    {
        var rnd = new Random(seed);
        var old = g.SmoothingMode;
        g.SmoothingMode = SmoothingMode.None;
        for (float yy = y; yy < y + h; yy += step)
        {
            int a = rnd.Next(alpha);
            float x0 = x + rnd.Next((int)Math.Max(1, w / 3));
            float len = w - (x0 - x) - rnd.Next((int)Math.Max(1, w / 3));
            if (len <= 1) continue;
            using (var pen = new Pen(A(a, rnd.Next(2) == 0 ? PureWhite : PureBlack), 1))
                g.DrawLine(pen, x0, yy, x0 + len, yy);
        }
        g.SmoothingMode = old;
    }

    // Speckle: seeded dots for concrete and stone. Static layer only.
    static void Speckle(GraphicsPath clip, RectangleF r, int seed, int count, int alpha)
    {
        var rnd = new Random(seed);
        var old = g.Clip;
        g.SetClip(clip);
        for (int i = 0; i < count; i++)
        {
            float px = r.X + (float)(rnd.NextDouble() * r.Width), py = r.Y + (float)(rnd.NextDouble() * r.Height);
            float s = 1 + rnd.Next(3);
            using (var b = new SolidBrush(A(rnd.Next(alpha), rnd.Next(2) == 0 ? PureWhite : PureBlack)))
                g.FillEllipse(b, px, py, s, s);
        }
        g.Clip = old;
    }

    // ------------------------------------------------------------------ materials

    // A brushed steel panel with rivets at the corners.
    static void SteelPanel(float x, float y, float w, float h, float r, int seed, bool rivets = true)
    {
        Shadow(x, y, w, h, r);
        using (var p = Round(x, y, w, h, r))
        {
            FillGrad(p, new RectangleF(x, y, w, h), SteelLight, SteelDark, 90f);
            var old = g.Clip;
            g.SetClip(p);
            Grain(x, y, w, h, seed, 26);
            using (var sheen = new LinearGradientBrush(new RectangleF(x, y, w, h), A(0, PureWhite), A(70, PureWhite), 35f))
            {
                sheen.Blend = new Blend(5) { Positions = new[] { 0f, 0.3f, 0.5f, 0.7f, 1f }, Factors = new[] { 0f, 0.1f, 1f, 0.1f, 0f } };
                g.FillRectangle(sheen, x, y, w, h);
            }
            g.Clip = old;
            Stroke(p, A(160, IronDark), 1.5f);
        }
        Bevel(x + 1.5f, y + 1.5f, w - 3, h - 3, r - 1, 120);
        if (rivets)
        {
            Rivet(x + 14, y + 14); Rivet(x + w - 14, y + 14); Rivet(x + 14, y + h - 14); Rivet(x + w - 14, y + h - 14);
        }
    }

    static void Rivet(float cx, float cy, float r = 4.5f)
    {
        using (var b = new SolidBrush(A(90, PureBlack))) g.FillEllipse(b, cx - r + 1, cy - r + 1.5f, 2 * r, 2 * r);
        Sphere(cx, cy, r, SteelLight, SteelDark);
    }

    // A brass name plate with engraved text and two screws. Width follows the text.
    static float BrassPlate(float cx, float y, string text, float px = 14, string family = Sans, bool bold = false, float padX = 14, Color? ink = null)
    {
        float tw = Measure(text, family, px, bold).Width;
        float w = tw + padX * 2 + 16, h = px + 12;
        BrassPlateBox(cx - w / 2, y, w, h);
        Engrave(text, cx - tw / 2, y + 5, family, px, bold, ink);
        return w;
    }

    static void BrassPlateBox(float x, float y, float w, float h, float r = 4)
    {
        Shadow(x, y, w, h, r, 3, 4, 70);
        using (var p = Round(x, y, w, h, r))
        {
            using (var br = new LinearGradientBrush(new RectangleF(x, y, w, h), BrassLight, BrassDark, 70f))
            {
                br.InterpolationColors = new ColorBlend(5)
                {
                    Colors = new[] { BrassMid, BrassLight, BrassMid, BrassDark, BrassMid },
                    Positions = new[] { 0f, 0.25f, 0.5f, 0.8f, 1f }
                };
                g.FillPath(br, p);
            }
            Stroke(p, A(170, BrassDark), 1f);
        }
        Bevel(x + 1, y + 1, w - 2, h - 2, r - 1, 120, 1f);
        Screw(x + 7, y + h / 2); Screw(x + w - 7, y + h / 2);
    }

    static void Screw(float cx, float cy, float r = 3f)
    {
        Sphere(cx, cy, r, BrassLight, BrassDark);
        Line(cx - r * 0.6f, cy - r * 0.6f, cx + r * 0.6f, cy + r * 0.6f, A(180, Ink), 1f);
    }

    // A horizontal or vertical riveted steel pipe. Cylinder shading runs across the axis.
    static void Pipe(float x, float y, float w, float h, bool flanges = true)
    {
        bool horizontal = w >= h;
        float thick = horizontal ? h : w;
        Shadow(x, y, w, h, thick / 2, 4, 5, 50);
        var r = new RectangleF(x, y, w, h);
        using (var br = new LinearGradientBrush(r, SteelLight, SteelDark, horizontal ? 90f : 0f))
        {
            br.InterpolationColors = new ColorBlend(4)
            {
                Colors = new[] { SteelMid, SteelLight, SteelMid, SteelDark },
                Positions = new[] { 0f, 0.25f, 0.6f, 1f }
            };
            g.FillRectangle(br, r);
        }
        using (var pen = new Pen(A(170, IronDark), 1f)) g.DrawRectangle(pen, x, y, w, h);
        if (!flanges) return;
        float fl = Math.Max(6, thick * 0.28f);
        if (horizontal)
        {
            Flange(x, y - 3, fl, h + 6); Flange(x + w - fl, y - 3, fl, h + 6);
        }
        else
        {
            Flange(x - 3, y, w + 6, fl); Flange(x - 3, y + h - fl, w + 6, fl);
        }
    }

    static void Flange(float x, float y, float w, float h)
    {
        var r = new RectangleF(x, y, w, h);
        using (var br = new LinearGradientBrush(r, SteelLight, SteelDark, w >= h ? 90f : 0f)) g.FillRectangle(br, r);
        using (var pen = new Pen(A(190, IronDark), 1f)) g.DrawRectangle(pen, x, y, w, h);
        if (w < h) { Rivet(x + w / 2, y + 6, 2.2f); Rivet(x + w / 2, y + h - 6, 2.2f); }
        else { Rivet(x + 6, y + h / 2, 2.2f); Rivet(x + w - 6, y + h / 2, 2.2f); }
    }

    // A cast iron hand-wheel seen from the front, with spokes at the given angle.
    static void HandWheel(float cx, float cy, float r, double angle)
    {
        ShadowEllipse(cx, cy, r, 3, 4, 50);
        using (var pen = new Pen(IronMid, r * 0.28f)) g.DrawEllipse(pen, cx - r, cy - r, 2 * r, 2 * r);
        using (var pen = new Pen(A(140, IronLight), 1.5f)) g.DrawArc(pen, cx - r - r * 0.1f, cy - r - r * 0.1f, 2 * r + r * 0.2f, 2 * r + r * 0.2f, 190, 110);
        using (var pen = new Pen(IronDark, 1f)) g.DrawEllipse(pen, cx - r * 0.86f, cy - r * 0.86f, 2 * r * 0.86f, 2 * r * 0.86f);
        using (var pen = new Pen(IronMid, Math.Max(2f, r * 0.14f)))
            for (int i = 0; i < 4; i++)
            {
                double a = angle + i * Math.PI / 2;
                g.DrawLine(pen, cx, cy, cx + (float)(Math.Cos(a) * r * 0.9), cy + (float)(Math.Sin(a) * r * 0.9));
            }
        Sphere(cx, cy, r * 0.22f, BrassLight, BrassDark);
    }

    static void Sphere(float cx, float cy, float r, Color light, Color dark)
    {
        if (r < 0.5f) return;
        using (var p = new GraphicsPath())
        {
            p.AddEllipse(cx - r, cy - r, 2 * r, 2 * r);
            using (var br = new PathGradientBrush(p))
            {
                br.CenterPoint = new PointF(cx - r * 0.35f, cy - r * 0.4f);
                br.CenterColor = light;
                br.SurroundColors = new[] { dark };
                g.FillPath(br, p);
            }
        }
    }

    // ------------------------------------------------------------------ water

    static double Frac(double v) { return v - Math.Floor(v); }
    static double Clamp01(double v) { return v < 0 ? 0 : v > 1 ? 1 : v; }
    static double Smooth(double a, double b, double v)
    {
        double u = Clamp01((v - a) / (b - a));
        return u * u * (3 - 2 * u);
    }

    // A body of water inside the rectangle, from surfaceY down to the bottom. The surface
    // is a gentle moving sine. Highlights are a few soft streaks under the surface.
    static void Water(float x, float w, float surfaceY, float bottomY, double t, double amp = 1.5, double speed = 1.0, int alpha = 255, bool highlights = true)
    {
        if (bottomY - surfaceY < 1.5f || w < 2) return;
        float depth = bottomY - surfaceY;
        using (var body = new GraphicsPath())
        {
            var pts = new List<PointF>();
            pts.Add(new PointF(x, bottomY));
            for (float px = x; px <= x + w + 0.01f; px += 4)
            {
                float xx = Math.Min(px, x + w);
                float yy = surfaceY + (float)(Math.Sin(xx * 0.09 + t * 2.1 * speed) * amp + Math.Sin(xx * 0.031 - t * 1.3 * speed) * amp * 0.6);
                pts.Add(new PointF(xx, Math.Min(bottomY, yy)));
            }
            pts.Add(new PointF(x + w, bottomY));
            body.AddPolygon(pts.ToArray());
            var r = new RectangleF(x, surfaceY - 4, w, depth + 8);
            using (var br = new LinearGradientBrush(r, A(alpha, WaterTop), A(alpha, WaterDeep), 90f))
            {
                br.InterpolationColors = new ColorBlend(3)
                {
                    Colors = new[] { A(alpha, WaterTop), A(alpha, WaterMid), A(alpha, WaterDeep) },
                    Positions = new[] { 0f, 0.35f, 1f }
                };
                g.FillPath(br, body);
            }
            var old = g.Clip;
            g.SetClip(body);
            using (var pen = new Pen(A(200, Foam), 1.6f))
            {
                var line = pts.GetRange(1, pts.Count - 2).ToArray();
                if (line.Length > 1) g.DrawLines(pen, line);
            }
            if (highlights && depth > 8 && w > 30)
            {
                for (int i = 0; i < 3; i++)
                {
                    double ph = Frac(t * 0.08 * speed + i * 0.33);
                    float hx = x + (float)(ph * w);
                    float hw = Math.Min(w * 0.22f, 60), hh = Math.Min(depth * 0.35f, 6);
                    using (var hp = new GraphicsPath())
                    {
                        hp.AddEllipse(hx - hw / 2, surfaceY + 3 + i * 2, hw, hh);
                        using (var br = new PathGradientBrush(hp))
                        {
                            br.CenterColor = A(70, PureWhite);
                            br.SurroundColors = new[] { A(0, PureWhite) };
                            g.FillPath(br, hp);
                        }
                    }
                }
            }
            g.Clip = old;
        }
    }

    // Flowing water in a channel: a translucent band with foam flecks that slide along it.
    static void Stream(float x, float y, float w, float h, double t, double flow, bool horizontal = true, float speedPx = 60)
    {
        if (flow <= 0.01 || w < 1 || h < 1) return;
        int alpha = (int)(120 + 100 * Clamp01(flow));
        var r = new RectangleF(x, y, w, h);
        using (var br = new LinearGradientBrush(r, A(alpha, WaterTop), A(alpha, WaterMid), horizontal ? 90f : 0f)) g.FillRectangle(br, r);
        var old = g.Clip;
        g.SetClip(r);
        float spacing = 26;
        float shift = (float)Frac(t * speedPx / spacing) * spacing;
        using (var pen = new Pen(A(150, Foam), 1.4f))
        {
            if (horizontal)
                for (float cx = x - spacing + shift; cx < x + w; cx += spacing)
                    g.DrawArc(pen, cx, y + h * 0.15f, 10, Math.Max(2, h * 0.7f), 300, 120);
            else
                for (float cy = y - spacing + shift; cy < y + h; cy += spacing)
                    g.DrawArc(pen, x + w * 0.15f, cy, Math.Max(2, w * 0.7f), 10, 30, 120);
        }
        g.Clip = old;
    }

    // A falling sheet of water from yTop to yBottom with foam at the foot.
    static void Fall(float x, float w, float yTop, float yBottom, double t, double flow)
    {
        if (flow <= 0.01 || yBottom <= yTop) return;
        int alpha = (int)(110 + 110 * Clamp01(flow));
        var r = new RectangleF(x, yTop, w, yBottom - yTop);
        using (var br = new LinearGradientBrush(r, A(alpha, Foam), A(alpha, WaterMid), 0f))
        {
            br.InterpolationColors = new ColorBlend(3) { Colors = new[] { A(alpha, WaterMid), A(alpha, Foam), A(alpha, WaterMid) }, Positions = new[] { 0f, 0.4f, 1f } };
            g.FillRectangle(br, r);
        }
        var old = g.Clip;
        g.SetClip(r);
        float spacing = 14;
        float shift = (float)Frac(t * 7) * spacing;
        using (var pen = new Pen(A(170, Foam), 1.3f))
            for (float cy = yTop - spacing + shift; cy < yBottom; cy += spacing)
                g.DrawLine(pen, x + w * 0.3f, cy, x + w * 0.3f, cy + spacing * 0.45f);
        g.Clip = old;
        FoamBlobs(x - w * 0.3f, yBottom - 3, w * 1.6f, t, flow);
    }

    static void FoamBlobs(float x, float y, float w, double t, double flow)
    {
        int n = (int)(3 + 5 * Clamp01(flow));
        for (int i = 0; i < n; i++)
        {
            double ph = Frac(t * 1.7 + i * 0.37 + i * i * 0.05);
            float fx = x + (float)((i + 0.5) / n * w) + (float)(Math.Sin(t * 3 + i) * 2);
            float fy = y - (float)(ph * 5);
            float r = 2.5f + (float)(Math.Sin(ph * Math.PI) * 2.5);
            using (var b = new SolidBrush(A((int)(220 * Math.Sin(ph * Math.PI)), Foam))) g.FillEllipse(b, fx - r, fy - r, 2 * r, 2 * r);
        }
    }

    // Rain over a region: streaks that fall with t, and splashes at the ground line.
    static void Rain(float x, float y, float w, float h, double t, double intensity, int seed = 7)
    {
        if (intensity <= 0.01) return;
        var rnd = new Random(seed);
        int n = (int)(8 + 44 * Clamp01(intensity));
        float len = 10 + 10 * (float)Clamp01(intensity);
        using (var pen = new Pen(A(200, Mix(Foam, WaterTop, 0.35)), 1.6f))
            for (int i = 0; i < n; i++)
            {
                float ox = (float)rnd.NextDouble();
                double ph0 = rnd.NextDouble();
                double speed = 0.9 + rnd.NextDouble() * 0.4;
                double ph = Frac(t * speed * (0.8 + 0.6 * intensity) + ph0);
                float px = x + ox * w, py = y + (float)(ph * h);
                float l = Math.Min(len, y + h - py);
                if (l < 2) continue;
                g.DrawLine(pen, px, py, px - 1.5f, py + l);
            }
        int splashes = (int)(3 + 9 * intensity);
        for (int i = 0; i < splashes; i++)
        {
            double ph = Frac(t * 2.3 + i * 0.61 + rnd.NextDouble());
            float sx = x + (float)((i + 0.5) / splashes * w);
            float sr = (float)(ph * 7) + 0.5f;
            using (var pen = new Pen(A((int)(200 * (1 - ph)), Foam), 1.2f)) g.DrawArc(pen, sx - sr, y + h - sr * 0.4f, sr * 2, sr * 0.8f, 180, 180);
        }
    }

    // Drops that fall past the tanks onto the ground (ETW overflow).
    static void LostDrops(float x, float w, float yTop, float yBottom, double t, double intensity)
    {
        if (intensity <= 0.01) return;
        int n = (int)(6 * intensity) + 2;
        for (int i = 0; i < n; i++)
        {
            double ph = Frac(t * 1.1 + i * 0.29 + i * i * 0.07);
            float px = x + (float)((i + 0.5) / n * w), py = yTop + (float)(ph * (yBottom - yTop));
            using (var b = new SolidBrush(A(230, WaterTop))) g.FillEllipse(b, px - 2.5f, py - 4, 5, 8);
            using (var b = new SolidBrush(A(200, Foam))) g.FillEllipse(b, px - 1.5f, py - 3, 2, 3);
        }
        FoamBlobs(x, yBottom, w, t, intensity);
    }

    // ------------------------------------------------------------------ glass tank

    // A glass tank in a steel frame. dug = false draws an empty socket with a ghost outline.
    static void TankFrame(float x, float y, float w, float h, bool dug)
    {
        if (!dug)
        {
            using (var p = Round(x, y, w, h, 3))
            {
                Fill(p, A(28, PureBlack));
                using (var pen = new Pen(A(150, SteelDark), 1.2f) { DashStyle = DashStyle.Dash }) g.DrawPath(pen, p);
            }
            return;
        }
        Shadow(x, y, w, h, 3, 3, 4, 60);
        using (var p = Round(x, y, w, h, 3))
        {
            FillGrad(p, new RectangleF(x, y, w, h), C(40, 48, 58), C(18, 22, 28), 0f);
            Stroke(p, SteelMid, 1.8f);
        }
        FillGradRect(new RectangleF(x - 2, y - 3, w + 4, 4), SteelLight, SteelDark, 90f);
        FillGradRect(new RectangleF(x - 2, y + h - 1, w + 4, 4), SteelLight, SteelDark, 90f);
    }

    static void TankWater(float x, float y, float w, float h, double level, double t)
    {
        if (level < 0.01) return;
        float inner = h - 4;
        float surface = y + 2 + inner * (float)(1 - Clamp01(level));
        var old = g.Clip;
        using (var clip = Round(x + 2, y + 2, w - 4, h - 4, 2))
        {
            g.SetClip(clip);
            Water(x + 2, w - 4, surface, y + h - 2, t, 0.8, 1.4, 255, false);
            g.Clip = old;
        }
    }

    // Glass highlight over a tank or window: a soft vertical sheen on the left, a thin one on the right.
    static void Glass(float x, float y, float w, float h)
    {
        var old = g.Clip;
        using (var clip = Round(x + 2, y + 2, w - 4, h - 4, 2))
        {
            g.SetClip(clip);
            float sw = Math.Max(3, w * 0.22f);
            FillGradRect(new RectangleF(x + 3, y + 3, sw, h - 6), A(110, PureWhite), A(10, PureWhite), 0f);
            FillRect(new RectangleF(x + w - 5, y + 4, 1.5f, h - 8), A(70, PureWhite));
            g.Clip = old;
        }
    }

    // ------------------------------------------------------------------ instruments

    // A round gauge with a brass bezel, a cream face and ticks. The needle is drawn separately.
    static void GaugeFace(float cx, float cy, float r, string[] marks)
    {
        ShadowEllipse(cx, cy, r, 4, 6, 70);
        using (var p = new GraphicsPath())
        {
            p.AddEllipse(cx - r, cy - r, 2 * r, 2 * r);
            using (var br = new LinearGradientBrush(new RectangleF(cx - r, cy - r, 2 * r, 2 * r), BrassLight, BrassDark, 60f))
            {
                br.InterpolationColors = new ColorBlend(4) { Colors = new[] { BrassLight, BrassMid, BrassDark, BrassMid }, Positions = new[] { 0f, 0.4f, 0.8f, 1f } };
                g.FillPath(br, p);
            }
            Stroke(p, A(200, BrassDark), 1f);
        }
        float fr = r * 0.82f;
        using (var p = new GraphicsPath())
        {
            p.AddEllipse(cx - fr, cy - fr, 2 * fr, 2 * fr);
            using (var br = new PathGradientBrush(p))
            {
                br.CenterPoint = new PointF(cx - fr * 0.3f, cy - fr * 0.3f);
                br.CenterColor = PureWhite;
                br.SurroundColors = new[] { Paper };
                g.FillPath(br, p);
            }
            Stroke(p, A(120, IronDark), 1f);
        }
        for (int i = 0; i <= 20; i++)
        {
            double a = (225 - 270 * i / 20.0) * Math.PI / 180;
            bool major = i % 5 == 0;
            float r1 = fr * (major ? 0.78f : 0.86f), r2 = fr * 0.93f;
            Line(cx + (float)(Math.Cos(a) * r1), cy - (float)(Math.Sin(a) * r1), cx + (float)(Math.Cos(a) * r2), cy - (float)(Math.Sin(a) * r2), Ink, major ? 1.6f : 1f);
        }
        if (marks != null)
            for (int i = 0; i < marks.Length; i++)
            {
                double a = (225 - 270 * i / (marks.Length - 1.0)) * Math.PI / 180;
                float mr = fr * 0.58f;
                TextCenter(marks[i], cx + (float)(Math.Cos(a) * mr), cy - (float)(Math.Sin(a) * mr) - 6, Sans, 11, Ink);
            }
    }

    static void GaugeNeedle(float cx, float cy, float r, double value, Color? color = null)
    {
        float fr = r * 0.82f;
        double a = (225 - 270 * Clamp01(value)) * Math.PI / 180;
        float nx = cx + (float)(Math.Cos(a) * fr * 0.8), ny = cy - (float)(Math.Sin(a) * fr * 0.8);
        float tx = cx - (float)(Math.Cos(a) * fr * 0.18), ty = cy + (float)(Math.Sin(a) * fr * 0.18);
        Line(cx + 1.5f, cy + 2.5f, nx + 1.5f, ny + 2.5f, A(60, PureBlack), 2.2f);
        Line(tx, ty, nx, ny, color ?? NeedleRed, 2.2f);
        Sphere(cx, cy, r * 0.09f, SteelLight, IronDark);
    }

    // Glass over the gauge: an arc of light at the top left.
    static void GaugeGlass(float cx, float cy, float r)
    {
        float fr = r * 0.82f;
        var old = g.Clip;
        using (var clip = new GraphicsPath())
        {
            clip.AddEllipse(cx - fr, cy - fr, 2 * fr, 2 * fr);
            g.SetClip(clip);
            using (var hp = new GraphicsPath())
            {
                hp.AddEllipse(cx - fr * 1.1f, cy - fr * 1.5f, fr * 1.7f, fr * 1.4f);
                using (var br = new PathGradientBrush(hp))
                {
                    br.CenterColor = A(80, PureWhite);
                    br.SurroundColors = new[] { A(0, PureWhite) };
                    g.FillPath(br, hp);
                }
            }
            g.Clip = old;
        }
    }

    // An odometer window: dark drums with ivory digits behind glass, in a brass bezel.
    static void OdometerBox(float x, float y, float w, float h)
    {
        Shadow(x, y, w, h, 4, 3, 4, 70);
        using (var p = Round(x, y, w, h, 4))
        {
            FillGrad(p, new RectangleF(x, y, w, h), BrassLight, BrassDark, 80f);
            Stroke(p, A(200, BrassDark), 1f);
        }
        using (var p = Round(x + 4, y + 4, w - 8, h - 8, 3))
        {
            FillGrad(p, new RectangleF(x, y, w, h), C(26, 26, 30), C(52, 52, 60), 90f);
            Stroke(p, A(200, PureBlack), 1f);
        }
    }

    static void OdometerDigits(float x, float y, float w, float h, string text, float px = 16)
    {
        float tw = Measure(text, Mono, px, true).Width;
        Text(text, x + w - 10 - tw, y + (h - px) / 2 - 1, Mono, px, Ivory, true);
    }

    // An indicator lamp: a domed jewel in a steel collar.
    static void Lamp(float cx, float cy, float r, Color color, bool on)
    {
        ShadowEllipse(cx, cy, r + 3, 2, 3, 50);
        Sphere(cx, cy, r + 3, SteelLight, SteelDark);
        Color body = on ? color : Mix(LampOff, color, 0.25);
        Sphere(cx, cy, r, on ? Mix(color, PureWhite, 0.55) : Mix(body, PureWhite, 0.15), on ? Mix(color, PureBlack, 0.3) : Mix(body, PureBlack, 0.5));
        using (var b = new SolidBrush(A(170, PureWhite))) g.FillEllipse(b, cx - r * 0.55f, cy - r * 0.6f, r * 0.5f, r * 0.35f);
    }

    // Cast concrete block with a speckle and a stone crest, for dams and walls.
    static void Concrete(float x, float y, float w, float h, int seed)
    {
        Shadow(x, y, w, h, 2, 4, 6, 60);
        using (var p = Round(x, y, w, h, 2))
        {
            FillGrad(p, new RectangleF(x, y, w, h), ConcreteLight, ConcreteDark, 20f);
            Speckle(p, new RectangleF(x, y, w, h), seed, (int)(w * h / 25), 70);
            Stroke(p, A(160, StoneDark), 1f);
        }
        Bevel(x + 1, y + 1, w - 2, h - 2, 1, 90, 1f);
    }

    // A row of stones, each a small rounded block with its own shade.
    static void StoneRow(float x, float y, float w, float h, int seed)
    {
        var rnd = new Random(seed);
        float sx = x;
        while (sx < x + w - 2)
        {
            float sw = Math.Min(x + w - sx, 10 + rnd.Next(12));
            using (var p = Round(sx, y, sw, h, 3))
            {
                FillGrad(p, new RectangleF(sx, y, sw, h), Mix(StoneLight, PureWhite, rnd.NextDouble() * 0.2), StoneDark, 70f);
                Stroke(p, A(150, StoneDark), 1f);
            }
            sx += sw + 1.5f;
        }
    }

    // A wooden water-mill wheel. angle in radians.
    static void MillWheel(float cx, float cy, float r, double angle)
    {
        ShadowEllipse(cx, cy, r, 5, 7, 60);
        using (var pen = new Pen(WoodMid, r * 0.18f)) g.DrawEllipse(pen, cx - r, cy - r, 2 * r, 2 * r);
        using (var pen = new Pen(WoodDark, 1f)) { g.DrawEllipse(pen, cx - r - r * 0.09f, cy - r - r * 0.09f, 2 * r * 1.09f, 2 * r * 1.09f); g.DrawEllipse(pen, cx - r * 0.91f, cy - r * 0.91f, 2 * r * 0.91f, 2 * r * 0.91f); }
        using (var pen = new Pen(A(120, WoodLight), 1.5f)) g.DrawArc(pen, cx - r, cy - r, 2 * r, 2 * r, 200, 90);
        int spokes = 8;
        for (int i = 0; i < spokes; i++)
        {
            double a = angle + i * 2 * Math.PI / spokes;
            float ex = cx + (float)(Math.Cos(a) * r * 0.9), ey = cy + (float)(Math.Sin(a) * r * 0.9);
            Line(cx, cy, ex, ey, WoodDark, Math.Max(2, r * 0.07f));
            Line(cx, cy, ex, ey, WoodLight, Math.Max(1, r * 0.03f));
            // Paddle: a short board at the rim, tangent to the circle.
            double pa = a + Math.PI / 2;
            float px = cx + (float)(Math.Cos(a) * r * 1.02), py = cy + (float)(Math.Sin(a) * r * 1.02);
            float dx = (float)(Math.Cos(pa) * r * 0.16), dy = (float)(Math.Sin(pa) * r * 0.16);
            Line(px - dx, py - dy, px + dx, py + dy, WoodDark, Math.Max(3, r * 0.1f));
            Line(px - dx, py - dy, px + dx, py + dy, WoodMid, Math.Max(1.5f, r * 0.05f));
        }
        Sphere(cx, cy, r * 0.17f, BrassLight, BrassDark);
    }

    // A puffy grey cloud. Light on the top left of each puff.
    static void Cloud(float cx, float cy, float scale, bool dark)
    {
        float[] ox = { -1.3f, -0.6f, 0.2f, 0.9f, 1.5f, 0.0f, -0.9f };
        float[] oy = { 0.3f, -0.3f, -0.5f, -0.2f, 0.3f, 0.35f, 0.4f };
        float[] rr = { 0.55f, 0.7f, 0.8f, 0.7f, 0.5f, 0.6f, 0.5f };
        Color light = dark ? C(150, 156, 168) : C(236, 240, 246), shade = dark ? C(74, 80, 96) : C(150, 162, 184);
        for (int i = 0; i < ox.Length; i++)
        {
            float r = rr[i] * scale;
            using (var b = new SolidBrush(A(50, PureBlack))) g.FillEllipse(b, cx + ox[i] * scale - r + 4, cy + oy[i] * scale - r + 6, 2 * r, 2 * r);
        }
        for (int i = 0; i < ox.Length; i++)
            Sphere(cx + ox[i] * scale, cy + oy[i] * scale, rr[i] * scale, light, shade);
    }
}
