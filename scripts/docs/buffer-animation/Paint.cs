// Paint.cs - skeuomorphic drawing primitives for the buffer animation.
//
// Light comes from the top left. Every shape gets a highlight on its top and left
// edges, a shade on its bottom and right edges, and a soft drop shadow to the bottom
// right. Water is a gradient from a light surface to a deep floor with a slow sine
// surface. The rain barrels are galvanised, open at the top, with a glass front panel
// that shows the water, a brass tap at the foot and an overflow spout at the rim.
//
// Everything here is a pure function of its arguments, so a frame rendered twice is
// identical. Motion (0..1) scales every wave, streak and foam fleck: an element that is
// not part of the current step is drawn with Motion 0, so its water stands still and
// its pixels do not change from frame to frame.

using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Drawing.Text;

public static partial class Watershed
{
    static Graphics g;
    static double Motion = 1;
    static bool Glass;

    // ------------------------------------------------------------------ colours

    static Color C(int r, int gr, int b) { return Color.FromArgb(r, gr, b); }
    static Color A(int alpha, Color c) { return Color.FromArgb(Math.Max(0, Math.Min(255, alpha)), c.R, c.G, c.B); }
    static Color Mix(Color a, Color b, double u)
    {
        u = Math.Max(0, Math.Min(1, u));
        return Color.FromArgb((int)(a.R + (b.R - a.R) * u), (int)(a.G + (b.G - a.G) * u), (int)(a.B + (b.B - a.B) * u));
    }

    // Exact colours that the GIF palette must hold, so type stays crisp.
    public static readonly Color Ivory = C(244, 238, 222), Ink = C(38, 30, 18),
        Slate = C(30, 30, 34), SlateLight = C(52, 52, 58), PureWhite = C(255, 255, 255), PureBlack = C(0, 0, 0),
        OptionGold = C(255, 222, 140), Warn = C(255, 150, 80), Muted = C(180, 176, 168);

    static readonly Color SteelLight = C(188, 194, 202), SteelMid = C(128, 136, 146), SteelDark = C(66, 72, 80),
        ZincLight = C(226, 230, 234), ZincMid = C(162, 168, 176), ZincDark = C(84, 90, 98),
        BrassLight = C(240, 212, 130), BrassMid = C(192, 152, 62), BrassDark = C(118, 86, 30),
        IronLight = C(110, 112, 118), IronMid = C(64, 66, 72), IronDark = C(30, 32, 36),
        ConcreteLight = C(186, 184, 176), ConcreteDark = C(120, 118, 110),
        WaterTop = C(112, 184, 228), WaterMid = C(44, 120, 190), WaterDeep = C(18, 60, 120), Foam = C(236, 246, 252),
        GlassTop = C(150, 216, 252), GlassMid = C(40, 156, 234), GlassDeep = C(14, 96, 186),
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

    // A sign: large cream type straight on the scene with a soft dark shadow. alpha fades it.
    static void Sign(string s, float cx, float y, double alpha, float px = 32)
    {
        if (alpha <= 0.01) return;
        float w = Measure(s, Sans, px, true).Width, x = cx - w / 2;
        int sh = (int)(26 * alpha);
        for (int dx = -1; dx <= 2; dx++)
            for (int dy = 1; dy <= 3; dy++)
                Text(s, x + dx, y + dy, Sans, px, A(sh, PureBlack), true);
        Text(s, x, y, Sans, px, A((int)(255 * alpha), Ivory), true);
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

    static void Modes()
    {
        g.SmoothingMode = SmoothingMode.AntiAlias;
        g.TextRenderingHint = TextRenderingHint.AntiAliasGridFit;
        g.PixelOffsetMode = PixelOffsetMode.HighQuality;
    }

    // Draws through an offscreen bitmap and composites it at the given opacity. Every
    // element of the scene goes through here, so a fade and a full draw share one code
    // path and one rounding, and a fade ends without a visible step.
    static void Layer(RectangleF r, double opacity, Action draw)
    {
        if (opacity <= 0.004) return;
        int x0 = (int)Math.Floor(r.X) - 6, y0 = (int)Math.Floor(r.Y) - 6;
        int w = (int)Math.Ceiling(r.Width) + 12, h = (int)Math.Ceiling(r.Height) + 12;
        using (var bmp = new Bitmap(w, h, PixelFormat.Format32bppArgb))
        {
            var outer = g;
            using (g = Graphics.FromImage(bmp))
            {
                Modes();
                g.TranslateTransform(-x0, -y0);
                draw();
            }
            g = outer;
            using (var ia = new ImageAttributes())
            {
                ia.SetColorMatrix(new ColorMatrix { Matrix33 = (float)Math.Min(1, opacity) });
                g.DrawImage(bmp, new Rectangle(x0, y0, w, h), 0, 0, w, h, GraphicsUnit.Pixel, ia);
            }
        }
    }

    // ------------------------------------------------------------------ materials

    // A plain steel panel: gradient, a soft sheen, a bevel.
    static void SteelPanel(float x, float y, float w, float h, float r)
    {
        Shadow(x, y, w, h, r);
        using (var p = Round(x, y, w, h, r))
        {
            FillGrad(p, new RectangleF(x, y, w, h), SteelLight, SteelDark, 90f);
            var old = g.Clip;
            g.SetClip(p);
            using (var sheen = new LinearGradientBrush(new RectangleF(x, y, w, h), A(0, PureWhite), A(60, PureWhite), 35f))
            {
                sheen.Blend = new Blend(5) { Positions = new[] { 0f, 0.3f, 0.5f, 0.7f, 1f }, Factors = new[] { 0f, 0.1f, 1f, 0.1f, 0f } };
                g.FillRectangle(sheen, x, y, w, h);
            }
            g.Clip = old;
            Stroke(p, A(160, IronDark), 1.5f);
        }
        Bevel(x + 1.5f, y + 1.5f, w - 3, h - 3, r - 1, 120);
    }

    // A dark plate with a brass rim for the one option or counter that a step is about.
    // Each line is a pair of texts: the left part in gold, the right part in ivory.
    static void Plate(float cx, float y, string[] gold, string[] ivory, float px = 22, string tag = null, bool warn = false)
    {
        float w = 0;
        for (int i = 0; i < gold.Length; i++) w = Math.Max(w, Measure(gold[i] + ivory[i], Mono, px, true).Width);
        if (tag != null) w = Math.Max(w, Measure(tag, Sans, 16, true).Width);
        w += 44;
        float lineH = px + 8;
        float h = gold.Length * lineH + 14 + (tag == null ? 0 : 24);
        float x = cx - w / 2;
        Shadow(x, y, w, h, 7, 4, 5, 80);
        using (var p = Round(x, y, w, h, 7))
        {
            FillGrad(p, new RectangleF(x, y, w, h), SlateLight, Slate, 90f);
            Stroke(p, BrassMid, 2f);
        }
        Bevel(x + 2, y + 2, w - 4, h - 4, 5, 80, 1f);
        for (int i = 0; i < gold.Length; i++)
        {
            float gw = Measure(gold[i], Mono, px, true).Width, iw = Measure(ivory[i], Mono, px, true).Width;
            float tx = cx - (gw + iw) / 2, ty = y + 9 + i * lineH;
            Text(gold[i], tx, ty, Mono, px, OptionGold, true);
            Text(ivory[i], tx + gw, ty, Mono, px, Ivory, true);
        }
        if (tag != null) TextCenter(tag, cx, y + 9 + gold.Length * lineH + 3, Sans, 16, warn ? Warn : Muted, true);
    }

    // A steel pipe. Cylinder shading runs across the axis; plain rings at both ends.
    static void Pipe(float x, float y, float w, float h)
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
        float ring = Math.Max(4, thick * 0.3f);
        if (horizontal) { FillRect(new RectangleF(x, y - 1, ring, h + 2), A(60, PureBlack)); FillRect(new RectangleF(x + w - ring, y - 1, ring, h + 2), A(60, PureBlack)); }
        else { FillRect(new RectangleF(x - 1, y, w + 2, ring), A(60, PureBlack)); FillRect(new RectangleF(x - 1, y + h - ring, w + 2, ring), A(60, PureBlack)); }
    }

    // A cast iron hand-wheel seen from the front, with spokes at the given angle.
    static void HandWheel(float cx, float cy, float r, double angle)
    {
        ShadowEllipse(cx, cy, r, 3, 4, 50);
        using (var pen = new Pen(IronMid, r * 0.28f)) g.DrawEllipse(pen, cx - r, cy - r, 2 * r, 2 * r);
        using (var pen = new Pen(A(140, IronLight), 1.5f)) g.DrawArc(pen, cx - r - r * 0.1f, cy - r - r * 0.1f, 2 * r + r * 0.2f, 2 * r + r * 0.2f, 190, 110);
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

    // Cast concrete block.
    static void Concrete(float x, float y, float w, float h)
    {
        Shadow(x, y, w, h, 2, 4, 6, 60);
        using (var p = Round(x, y, w, h, 2))
        {
            FillGrad(p, new RectangleF(x, y, w, h), ConcreteLight, ConcreteDark, 20f);
            Stroke(p, A(160, StoneDark), 1f);
        }
        Bevel(x + 1, y + 1, w - 2, h - 2, 1, 90, 1f);
    }

    // A channel cut into the earth: a dark cavity with stone walls and a stone floor. A pit
    // is the same cut in lighter earth, for the large bodies that are often nearly empty.
    static void Pit(float x, float y, float w, float h) { Cavity(x, y, w, h, true); }
    static void Cavity(float x, float y, float w, float h, bool pit = false)
    {
        using (var p = Round(x, y, w, h, 3))
            FillGrad(p, new RectangleF(x, y, w, h), pit ? C(104, 84, 58) : C(52, 46, 40), pit ? C(58, 44, 30) : C(26, 24, 22), 90f);
        FillGradRect(new RectangleF(x - 4, y, 4, h), StoneDark, StoneLight, 0f);
        FillGradRect(new RectangleF(x + w, y, 4, h), StoneLight, StoneDark, 0f);
        FillGradRect(new RectangleF(x - 4, y + h - 3, w + 8, 6), StoneLight, StoneDark, 90f);
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
            double pa = a + Math.PI / 2;
            float px = cx + (float)(Math.Cos(a) * r * 1.02), py = cy + (float)(Math.Sin(a) * r * 1.02);
            float dx = (float)(Math.Cos(pa) * r * 0.16), dy = (float)(Math.Sin(pa) * r * 0.16);
            Line(px - dx, py - dy, px + dx, py + dy, WoodDark, Math.Max(3, r * 0.1f));
            Line(px - dx, py - dy, px + dx, py + dy, WoodMid, Math.Max(1.5f, r * 0.05f));
        }
        Sphere(cx, cy, r * 0.17f, BrassLight, BrassDark);
    }

    // A puffy cloud. Light on the top left of each puff.
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

    // ------------------------------------------------------------------ water

    static double Frac(double v) { return v - Math.Floor(v); }
    static double Clamp01(double v) { return v < 0 ? 0 : v > 1 ? 1 : v; }
    static double Smooth(double a, double b, double v)
    {
        double u = Clamp01((v - a) / (b - a));
        return u * u * (3 - 2 * u);
    }

    // A body of water inside the rectangle, from surfaceY down to the bottom. The surface
    // is a slow, low sine. The body gradient is anchored at gradTop (or the surface), so a
    // rising level changes only the pixels near the surface. A light band follows the surface.
    static void Water(float x, float w, float surfaceY, float bottomY, double t, double amp = 1.2, double speed = 1.0, int alpha = 255, bool highlights = true, float gradTop = float.NaN)
    {
        if (bottomY - surfaceY < 1.5f || w < 2) return;
        amp *= Motion;
        Color top_ = Glass ? GlassTop : WaterTop, mid = Glass ? GlassMid : WaterMid, deep = Glass ? GlassDeep : WaterDeep;
        float top = float.IsNaN(gradTop) ? surfaceY - 4 : Math.Min(gradTop, surfaceY - 4);
        using (var body = new GraphicsPath())
        {
            var pts = new List<PointF>();
            pts.Add(new PointF(x, bottomY));
            for (float px = x; px <= x + w + 0.01f; px += 4)
            {
                float xx = Math.Min(px, x + w);
                float yy = surfaceY + (float)(Math.Sin(xx * 0.05 + t * 0.55 * speed) * amp + Math.Sin(xx * 0.021 - t * 0.35 * speed) * amp * 0.6);
                pts.Add(new PointF(xx, Math.Min(bottomY, yy)));
            }
            pts.Add(new PointF(x + w, bottomY));
            body.AddPolygon(pts.ToArray());
            var r = new RectangleF(x, top, w, bottomY - top + 4);
            using (var br = new LinearGradientBrush(r, A(alpha, top_), A(alpha, deep), 90f))
            {
                br.InterpolationColors = new ColorBlend(3)
                {
                    Colors = new[] { A(alpha, top_), A(alpha, mid), A(alpha, deep) },
                    Positions = new[] { 0f, 0.35f, 1f }
                };
                g.FillPath(br, body);
            }
            var old = g.Clip;
            g.SetClip(body);
            float band = Math.Min(Glass ? 22 : 14, bottomY - surfaceY);
            if (band > 3) FillGradRect(new RectangleF(x, surfaceY - 3, w, band + 3), A(Glass ? 150 : 90, Glass ? PureWhite : WaterTop), A(0, WaterTop), 90f);
            using (var pen = new Pen(A(Glass ? 240 : 200, Foam), Glass ? 3f : 1.6f))
            {
                var line = pts.GetRange(1, pts.Count - 2).ToArray();
                if (line.Length > 1) g.DrawLines(pen, line);
            }
            if (highlights && Motion > 0.01 && bottomY - surfaceY > 10 && w > 40)
            {
                for (int i = 0; i < 2; i++)
                {
                    double ph = Frac(t * 0.02 * speed + i * 0.5);
                    float hx = x + (float)(ph * w);
                    float hw = Math.Min(w * 0.25f, 70), hh = 6;
                    using (var hp = new GraphicsPath())
                    {
                        hp.AddEllipse(hx - hw / 2, surfaceY + 4 + i * 3, hw, hh);
                        using (var br = new PathGradientBrush(hp))
                        {
                            br.CenterColor = A((int)(60 * Motion), PureWhite);
                            br.SurroundColors = new[] { A(0, PureWhite) };
                            g.FillPath(br, hp);
                        }
                    }
                }
            }
            g.Clip = old;
        }
    }

    // Flowing water in a channel or pipe: a translucent band with foam flecks that drift along it.
    static void Stream(float x, float y, float w, float h, double t, double flow, bool horizontal = true, float speedPx = 18)
    {
        if (flow <= 0.01 || w < 1 || h < 1) return;
        int alpha = (int)(120 + 100 * Clamp01(flow));
        var r = new RectangleF(x, y, w, h);
        using (var br = new LinearGradientBrush(r, A(alpha, WaterTop), A(alpha, WaterMid), horizontal ? 90f : 0f)) g.FillRectangle(br, r);
        if (Motion <= 0.01) return;
        var old = g.Clip;
        g.SetClip(r);
        float spacing = 32;
        float shift = (float)Frac(t * speedPx / spacing) * spacing;
        using (var pen = new Pen(A((int)(140 * Motion), Foam), 1.4f))
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

    // A falling sheet of water from yTop to yBottom with a little foam at the foot.
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
        if (Motion <= 0.01) return;
        var old = g.Clip;
        g.SetClip(r);
        float spacing = 16;
        float shift = (float)Frac(t * 1.3) * spacing;
        using (var pen = new Pen(A((int)(170 * Motion), Foam), 1.3f))
            for (float cy = yTop - spacing + shift; cy < yBottom; cy += spacing)
                g.DrawLine(pen, x + w * 0.3f, cy, x + w * 0.3f, cy + spacing * 0.45f);
        g.Clip = old;
        FoamBlobs(x - w * 0.4f, yBottom - 2, w * 1.8f, t, flow);
    }

    static void FoamBlobs(float x, float y, float w, double t, double flow)
    {
        if (Motion <= 0.01) return;
        int n = (int)(2 + 4 * Clamp01(flow));
        for (int i = 0; i < n; i++)
        {
            double ph = Frac(t * 0.45 + i * 0.37 + i * i * 0.05);
            float fx = x + (float)((i + 0.5) / n * w) + (float)(Math.Sin(t * 0.7 + i) * 1.5);
            float fy = y - (float)(ph * 4);
            float r = 2 + (float)(Math.Sin(ph * Math.PI) * 2.2);
            using (var b = new SolidBrush(A((int)(200 * Motion * Math.Sin(ph * Math.PI)), Foam))) g.FillEllipse(b, fx - r, fy - r, 2 * r, 2 * r);
        }
    }

    // Rain over a region: a few streaks that fall slowly, and soft splashes at the foot.
    static void Rain(float x, float y, float w, float h, double t, double intensity, int seed = 7)
    {
        if (intensity <= 0.01) return;
        var rnd = new Random(seed);
        int n = (int)(7 + 24 * Clamp01(intensity));
        float len = 9 + 6 * (float)Clamp01(intensity);
        double pxPerSec = 28 + 26 * Clamp01(intensity);
        using (var pen = new Pen(A(190, Mix(Foam, WaterTop, 0.4)), 1.8f))
            for (int i = 0; i < n; i++)
            {
                float ox = (float)rnd.NextDouble();
                double ph0 = rnd.NextDouble();
                double sp = 0.85 + rnd.NextDouble() * 0.3;
                double ph = Frac(t * pxPerSec * sp / h + ph0);
                float px = x + ox * w, py = y + (float)(ph * h);
                float l = Math.Min(len, y + h - py);
                if (l < 2) continue;
                g.DrawLine(pen, px, py, px - 0.5f, py + l);
            }
        int splashes = (int)(2 + 5 * intensity);
        for (int i = 0; i < splashes; i++)
        {
            double ph = Frac(t * 0.55 + i * 0.37 + rnd.NextDouble());
            float sx = x + (float)((i + 0.5) / splashes * w) + (float)(Math.Sin(i * 2.3) * w * 0.05);
            float sr = (float)(ph * 6) + 1f;
            using (var pen = new Pen(A((int)(170 * (1 - ph)), Foam), 1.3f)) g.DrawArc(pen, sx - sr, y + h - sr * 0.35f, sr * 2, sr * 0.7f, 180, 180);
        }
    }

    // ------------------------------------------------------------------ rain barrel

    // A galvanised rain barrel, open at the top, with a glass front panel. (x, yBase) is
    // the bottom left of the body, w its width, h its height. level (0..1) is the water
    // behind the glass. inflow runs the downspout from spoutY into the rim and the fall
    // inside the glass. tap (0..1) opens the brass tap at the foot and runs its stream down
    // to troughY. overflow (0..1) brims the rim, sheets down both sides and runs the
    // overflow spout at the top left down to the ground.
    static void Barrel(float x, float yBase, float w, float h, double level, double t, double tap, double inflow, double overflow, float troughY, float spoutY)
    {
        float top = yBase - h;
        float rim = w * 0.09f, band = w * 0.08f;
        using (var b = new SolidBrush(A(70, PureBlack))) g.FillEllipse(b, x + 4, yBase - 5, w + 14, 11);
        var body = new RectangleF(x, top + rim * 0.5f, w, yBase - top - rim * 0.5f);
        using (var br = new LinearGradientBrush(body, ZincLight, ZincDark, 0f))
        {
            br.InterpolationColors = new ColorBlend(5)
            {
                Colors = new[] { ZincDark, ZincLight, ZincMid, ZincDark, Mix(ZincDark, PureBlack, 0.3) },
                Positions = new[] { 0f, 0.22f, 0.55f, 0.88f, 1f }
            };
            g.FillRectangle(br, body);
        }
        var old = g.SmoothingMode;
        g.SmoothingMode = SmoothingMode.None;
        for (float yy = top + rim + 12; yy < yBase - band - 10; yy += 12)
        {
            Line(x + 1, yy, x + w - 1, yy, A(70, PureWhite), 1f);
            Line(x + 1, yy + 4, x + w - 1, yy + 4, A(60, PureBlack), 1f);
        }
        g.SmoothingMode = old;
        // Rim: a rolled band, a little wider than the body, with the dark inside edge of the open top.
        var rimR = new RectangleF(x - 3, top, w + 6, rim);
        using (var br = new LinearGradientBrush(rimR, ZincLight, ZincDark, 90f)) g.FillRectangle(br, rimR);
        Line(x - 3, top + 1.5f, x + w + 3, top + 1.5f, A(170, PureWhite), 1.5f);
        FillRect(new RectangleF(x - 3, top + rim - 2, w + 6, 3), A(110, PureBlack));
        using (var pen = new Pen(A(180, IronDark), 1f)) g.DrawRectangle(pen, rimR.X, rimR.Y, rimR.Width, rimR.Height);
        // Foot band.
        FillGradRect(new RectangleF(x - 1, yBase - band, w + 2, band), ZincMid, Mix(ZincDark, PureBlack, 0.25), 90f);
        Line(x - 1, yBase - band + 1, x + w + 1, yBase - band + 1, A(110, PureWhite), 1f);
        using (var pen = new Pen(A(180, IronDark), 1f)) g.DrawRectangle(pen, x, top + rim * 0.5f, w, yBase - top - rim * 0.5f);

        // The glass panel with the water behind it.
        float wx = x + w * 0.15f, ww = w * 0.70f, wy = top + rim + 10, wh = yBase - band - w * 0.2f - wy;
        using (var p = Round(wx - 4, wy - 4, ww + 8, wh + 8, 7)) { FillGrad(p, new RectangleF(wx - 4, wy - 4, ww + 8, wh + 8), SteelMid, SteelDark, 90f); Stroke(p, A(200, IronDark), 1.2f); }
        using (var p = Round(wx, wy, ww, wh, 4)) { FillGrad(p, new RectangleF(wx, wy, ww, wh), C(28, 36, 46), C(10, 14, 20), 0f); }
        float surface = wy + 3 + (wh - 6) * (float)(1 - Clamp01(level));
        var clip = g.Clip;
        using (var p = Round(wx + 1, wy + 1, ww - 2, wh - 2, 3))
        {
            g.SetClip(p);
            if (level > 0.01)
            {
                Glass = true;
                Water(wx, ww, surface, wy + wh, t, 0.6, 1.0, 255, true, wy);
                Glass = false;
            }
            if (inflow > 0.01 && level < 0.985) Fall(wx + ww * 0.5f - 3, 6, wy - 2, Math.Max(wy + 2, surface + 1), t, inflow);
            g.Clip = clip;
        }
        FillGradRect(new RectangleF(wx + 2, wy + 2, Math.Max(2, ww * 0.2f), wh - 4), A(120, PureWhite), A(0, PureWhite), 0f);
        FillRect(new RectangleF(wx + ww - 5, wy + 6, 2, wh - 12), A(60, PureWhite));
        Bevel(wx - 3, wy - 3, ww + 6, wh + 6, 6, 100, 1f);

        // The downspout water into the open top.
        if (inflow > 0.01) Fall(x + w * 0.5f - 3, 6, spoutY, top + 3, t, inflow);

        // Overflow: the brim, the sheets down both sides, and the spout at the top left.
        float tl = w * 0.17f, tt = w * 0.10f;
        float oy = top + rim + 4;
        FillGradRect(new RectangleF(x - tl * 0.8f, oy, tl * 0.8f + 2, tt), ZincLight, ZincDark, 90f);
        FillGradRect(new RectangleF(x - tl * 0.8f, oy, tt, tt * 1.9f), ZincLight, ZincDark, 0f);
        if (overflow > 0.01)
        {
            int a = (int)(220 * overflow);
            FillGradRect(new RectangleF(x - 5, top - 5, w + 10, rim + 4), A(a, GlassTop), A(a, GlassMid), 90f);
            Line(x - 5, top - 4, x + w + 5, top - 4, A(a, Foam), 2.5f);
            Fall(x - 7, 6, top - 2, yBase + 8, t, overflow);
            Fall(x + w + 1, 6, top - 2, yBase + 8, t, overflow);
            Fall(x - tl * 0.8f - 1, tt + 1, oy + tt * 1.9f, yBase + 10, t, overflow);
        }

        // Tap at the foot: a brass spout under the glass, a handle that turns when the tap is open.
        float ty = yBase - band - w * 0.18f, tx = x + w * 0.5f - tt * 0.8f;
        FillGradRect(new RectangleF(tx, ty, tt * 1.6f, w * 0.14f), BrassLight, BrassDark, 0f);
        FillGradRect(new RectangleF(tx - 3, ty, tt * 1.6f + 6, tt * 0.6f), BrassLight, BrassDark, 90f);
        float hx = tx + tt * 1.6f + 3, hy = ty + tt * 0.6f, hl = w * 0.15f;
        double ang = -Math.PI / 2 * (1 - Clamp01(tap));
        Line(hx, hy, hx + (float)(Math.Cos(ang) * hl), hy + (float)(Math.Sin(ang) * hl), BrassDark, Math.Max(3f, tt * 0.5f));
        Sphere(hx, hy, Math.Max(2.5f, tt * 0.45f), BrassLight, BrassDark);
        if (tap > 0.01) Fall(tx + tt * 0.3f, tt, ty + w * 0.14f, troughY, t, tap);
    }

    // A concrete pad for a barrel. The top face catches the light; the front face is in shade.
    static void Pad(float x, float y, float w)
    {
        float px = x - 5, pw = w + 10;
        Shadow(px, y, pw, 10, 2, 3, 4, 50);
        FillGradRect(new RectangleF(px, y, pw, 4), Mix(ConcreteLight, PureWhite, 0.25), ConcreteLight, 90f);
        FillGradRect(new RectangleF(px, y + 4, pw, 6), ConcreteDark, Mix(ConcreteDark, PureBlack, 0.3), 90f);
        using (var pen = new Pen(A(150, StoneDark), 1f)) g.DrawRectangle(pen, px, y, pw, 10);
    }

    // A half-round roof gutter seen from the side: dark inside the lip, light on the belly.
    static void Gutter(float x, float y, float w, float h)
    {
        Shadow(x, y, w, h, h / 2, 3, 4, 50);
        var r = new RectangleF(x, y, w, h);
        using (var br = new LinearGradientBrush(r, ZincDark, ZincLight, 90f))
        {
            br.InterpolationColors = new ColorBlend(4) { Colors = new[] { ZincDark, ZincMid, ZincLight, ZincDark }, Positions = new[] { 0f, 0.3f, 0.7f, 1f } };
            using (var p = Round(x, y, w, h, h / 2)) g.FillPath(br, p);
        }
        using (var p = Round(x, y, w, h, h / 2)) Stroke(p, A(170, IronDark), 1f);
        Line(x + 2, y + 1.5f, x + w - 2, y + 1.5f, A(150, PureWhite), 1f);
    }
}
