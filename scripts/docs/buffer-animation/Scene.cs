// Scene.cs - "Buffers as a watershed": the storyboard, the water model and the picture.
//
// Seven steps. Steps 1 to 3 are one run at the defaults, step 4 starts after a service
// restart with large ETW buffers, step 5 after a restart with the 100,000 queries per
// second tier, and steps 6 and 7 continue the step 5 run. The water and the counters are
// a function of the time t, so every build gives the same GIF.
//
// Every count on screen is an illustration (see the footnote). Option names, defaults
// and tier values are exact and come from docs/guide/tuning.md, docs/guide/configuration.md,
// src/pipeline/config.rs, src/etw/session.rs, src/etw/intake.rs, src/sender.rs and
// src/sender/router.rs. The companion text is docs/design/buffer-watershed.md.
//
// The picture builds up step by step. Rain from dns.exe falls into a roof gutter that
// feeds a row of rain barrels (the ETW buffers). The brass taps (the flush) run into a
// trough that pours into a lake behind a dam (the event channel). A culvert turns a mill
// wheel (the parser). The river (the send queue) passes a sluice (the router) into a
// second lake (the link FIFO) and over an outlet into the sea (the TCP receiver). Under
// the second lake lies the pond (the encrypted disk spool). Each element appears in the
// step that is about it; elements that are not part of the current step are faint and
// their water stands still.

using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Globalization;

public static partial class Watershed
{
    public const int Width = 1600, Height = 900, Fps = 8;

    // Motion runs at half speed: every wave, streak, drop and wheel takes twice as long.
    const double Slow = 0.5;
    // Opacity of an element that is drawn but is not part of the current step.
    const double FaintVis = 0.35;

    // ------------------------------------------------------------------ storyboard

    sealed class Step
    {
        public string Caption;
        public double Seconds;
        public bool Tier, TallDam, Banner;
    }

    static readonly Step[] Steps = {
        new Step { Seconds = 18,
            Caption = "Rain from dns.exe fills the ETW buffers, and each tap passes its water on to the event channel." },
        new Step { Seconds = 18,
            Caption = "When the barrels fill faster than the taps drain them, ETW adds barrels up to the maximum." },
        new Step { Seconds = 18, Banner = true,
            Caption = "With every barrel full at the maximum, ETW discards new events, and etw_lost counts them." },
        new Step { Seconds = 20, Tier = true,
            Caption = "Large barrels pass the whole storm on, so the lake tops the dam and the newest events spill." },
        new Step { Seconds = 18, Tier = true, TallDam = true,
            Caption = "A taller dam holds the same storm, nothing spills, and the parser fills the send queue." },
        new Step { Seconds = 20, Tier = true, TallDam = true,
            Caption = "When the receiver slows, the full second lake spills into the disk spool, and new water follows it." },
        new Step { Seconds = 18, Tier = true, TallDam = true,
            Caption = "When the receiver recovers, replay empties the disk spool oldest first, before new water passes it." },
    };

    const string TitleText = "Buffers as a watershed";
    const string Footnote = "Illustration. Counts and sizes are not to scale.";
    const string BannerText = "ILLUSTRATION, NOT A LAB RESULT";

    static double Loop { get { double s = 0; foreach (var st in Steps) s += st.Seconds; return s; } }

    static int StepAt(double t, out double u)
    {
        double s = 0;
        for (int i = 0; i < Steps.Length; i++)
        {
            if (t < s + Steps[i].Seconds || i == Steps.Length - 1) { u = Clamp01((t - s) / Steps[i].Seconds); return i; }
            s += Steps[i].Seconds;
        }
        u = 0; return 0;
    }

    static double StepStart(int step) { double s = 0; for (int i = 0; i < step; i++) s += Steps[i].Seconds; return s; }

    // ------------------------------------------------------------------ the state of the water

    const int Positions = 6, Standing = 4;

    // How an element is drawn: Vis is its opacity, Live scales its motion (0 stands still).
    struct El { public double Vis, Live; }
    static readonly El Hidden = new El(), Faint = new El { Vis = FaintVis }, Full = new El { Vis = 1, Live = 1 };
    static El FadeIn(double a, double b, double v, bool fromFaint) { double k = Smooth(a, b, v); return new El { Vis = (fromFaint ? FaintVis : 0) * (1 - k) + k, Live = k }; }
    static El FadeOut(double a, double b, double v) { double k = Smooth(a, b, v); return new El { Vis = 1 - (1 - FaintVis) * k, Live = 1 - k }; }

    sealed class State
    {
        public int Step; public double T, L;
        public double Rain, CloudDark, Overflow, Trough, Lake, Spill, WheelAngle, Race;
        public double River, RiverFlow, Lake2, Outlet, Valve, Gate, SpillPipe, ReplayFlow, Pond;
        public double[] Level = new double[Positions], Tap = new double[Positions], Inflow = new double[Positions], Present = new double[Positions];
        public long EtwLost, RingDropped, Spooled, Replayed;
        public El Barrels, LakeEl, Mill, RiverEl, Router, Link, Sea, PondEl;
        public double Sign1, Sign2, PlateA, PlateB;
    }

    static double Window(double a, double b, double c, double d, double v) { return Smooth(a, b, v) * (1 - Smooth(c, d, v)); }

    // The integral of Smooth(0, d, x) from 0 to v.
    static double RampIntegral(double v, double d)
    {
        double u = Math.Min(1, v / d);
        double r = d * (u * u * u - u * u * u * u / 2);
        return v > d ? r + v - d : r;
    }

    // The barrel clock of the first run: it runs at rate 1 in step 1 and speeds up to 1.8
    // over the first three seconds of step 2, when the rain grows. One barrel cycle is 12
    // clock units, so the barrels fill faster in the storm without a jump in any level.
    static double BarrelClock(int step, double L)
    {
        if (step == 0) return L;
        double b = Steps[0].Seconds;
        if (step == 1) return b + L + 0.8 * RampIntegral(L, 3);
        return b + Steps[1].Seconds + 0.8 * RampIntegral(Steps[1].Seconds, 3);
    }

    // One barrel cycle on the clock c: a slow rise from near empty to near the brim, the
    // tap opens, the barrel drains, the tap closes, and the cycle repeats.
    static void Cycle(double c, out double level, out double tap)
    {
        if (c < 0) { level = 0.06; tap = 0; return; }
        c = Frac(c / 12) * 12;
        level = 0.06 + 0.86 * Smooth(0, 7.4, c) - 0.86 * Smooth(7.9, 10.8, c);
        tap = Window(7.5, 8.0, 10.5, 11.2, c);
    }

    // The storm rhythm of the tier runs: two small beats from two taps, then one large
    // slug from every tap. The picture does not show where the water waits in between,
    // and the barrels stay low.
    static void Storm(State s, double L, double smallA, double smallB, double bigA, double bigB)
    {
        double big = Window(bigA, bigA + 0.8, bigB - 0.8, bigB, L);
        double small = 0.35 * (Window(smallA, smallA + 0.5, smallA + 1.3, smallA + 1.8, L) + Window(smallB, smallB + 0.5, smallB + 1.3, smallB + 1.8, L));
        double maxTap = 0;
        for (int i = 0; i < Standing; i++)
        {
            s.Tap[i] = Math.Max(big, i < 2 ? small : 0);
            s.Inflow[i] = 1;
            if (i < 2) s.Level[i] -= 0.03 * Smooth(smallA, smallA + 1.5, L) + 0.03 * Smooth(smallB, smallB + 1.5, L);
            maxTap = Math.Max(maxTap, s.Tap[i]);
        }
        s.Trough = maxTap;
    }

    static double MillLive(double t)
    {
        double u; int step = StepAt(t, out u); double L = t - StepStart(step);
        return step == 3 ? Smooth(8, 10, L) : step == 4 ? 1 : 0;
    }

    // The wheel turns only while the mill is live, at 0.35 radians per second.
    static double WheelAngle(double t)
    {
        double angle = 0;
        for (int f = 0; f < (int)(t * Fps); f++) angle += 0.35 * MillLive(f / (double)Fps) / Fps;
        return angle;
    }

    static State Compute(double t)
    {
        var s = new State();
        double u;
        int step = StepAt(t, out u);
        s.Step = step; s.T = t; s.L = t - StepStart(step);
        double L = s.L;

        s.Barrels = s.LakeEl = s.Mill = s.RiverEl = s.Router = s.Link = s.Sea = s.PondEl = Faint;
        for (int i = 0; i < Positions; i++) { s.Present[i] = i < Standing ? 1 : 0; s.Level[i] = 0.2; }
        s.Rain = 0.8; s.CloudDark = 1; s.Lake = 0.30; s.River = 0.30; s.RiverFlow = 0.4; s.Race = 0.6; s.Lake2 = 0.15; s.Valve = 1; s.Outlet = 0.6;

        switch (step)
        {
            case 0:
                {
                    // Calm. Each barrel fills slowly, its tap opens, it drains. The lake appears
                    // just before the first tap opens.
                    s.Rain = 0.3; s.CloudDark = 0;
                    s.Barrels = Full; s.LakeEl = FadeIn(5.5, 7.5, L, false);
                    s.Mill = s.RiverEl = s.Router = s.Link = s.Sea = s.PondEl = Hidden;
                    double clock = BarrelClock(0, L), maxTap = 0;
                    for (int i = 0; i < Standing; i++)
                    {
                        Cycle(clock - 3 * i, out s.Level[i], out s.Tap[i]);
                        s.Inflow[i] = 1;
                        maxTap = Math.Max(maxTap, s.Tap[i]);
                    }
                    s.Trough = maxTap;
                    s.Lake = 0.10 + 0.18 * Smooth(8, 17, L);
                    s.Sign1 = Window(0.5, 1.5, 6.5, 7.5, L);
                    s.Sign2 = Window(8, 9, 14, 15, L);
                    s.PlateA = 1;
                    break;
                }
            case 1:
                {
                    // The storm grows. The barrels cycle faster, and two more barrels are lowered
                    // onto the empty pads and start to fill.
                    s.Rain = 0.3 + 0.7 * Smooth(0, 3, L); s.CloudDark = Smooth(0, 3, L);
                    s.Barrels = Full; s.LakeEl = Faint;
                    s.Mill = s.RiverEl = s.Router = s.Link = s.Sea = s.PondEl = Hidden;
                    double clock = BarrelClock(1, L), maxTap = 0;
                    for (int i = 0; i < Standing; i++)
                    {
                        Cycle(clock - 3 * i, out s.Level[i], out s.Tap[i]);
                        s.Inflow[i] = 1;
                        maxTap = Math.Max(maxTap, s.Tap[i]);
                    }
                    double[] landed = { 6, 12 };
                    for (int k = 0; k < 2; k++)
                    {
                        int i = Standing + k;
                        s.Present[i] = Smooth(landed[k] - 3.5, landed[k], L);
                        Cycle(clock - BarrelClock(1, landed[k]), out s.Level[i], out s.Tap[i]);
                        s.Inflow[i] = Smooth(landed[k], landed[k] + 0.6, L);
                        maxTap = Math.Max(maxTap, s.Tap[i]);
                    }
                    s.Trough = maxTap;
                    s.Lake = 0.28 + 0.38 * Smooth(2, 14, L);
                    s.PlateA = 1;
                    break;
                }
            case 2:
                {
                    // Overflow (illustration): every barrel fills to the brim, every tap runs, and
                    // the water pours over the rims and the overflow spouts onto the ground.
                    s.Rain = 1.0;
                    s.Barrels = Full; s.LakeEl = Faint;
                    s.Mill = s.RiverEl = s.Router = s.Link = s.Sea = s.PondEl = Hidden;
                    double end = BarrelClock(1, Steps[1].Seconds), k = Smooth(0, 4, L), maxTap = 0;
                    double[] landed = { 6, 12 };
                    for (int i = 0; i < Positions; i++)
                    {
                        s.Present[i] = 1;
                        double level, tap;
                        Cycle(i < Standing ? end - 3 * i : end - BarrelClock(1, landed[i - Standing]), out level, out tap);
                        s.Level[i] = level + (1 - level) * k;
                        s.Tap[i] = tap + (1 - tap) * k;
                        s.Inflow[i] = 1;
                        maxTap = Math.Max(maxTap, s.Tap[i]);
                    }
                    s.Trough = maxTap;
                    s.Overflow = Smooth(3.5, 6.5, L);
                    s.EtwLost = (long)(25000 * Smooth(5, 16, L));
                    s.Lake = 0.66;
                    s.PlateA = 1;
                    break;
                }
            case 3:
                {
                    // Restart with large barrels. First the barrels, then the lake: the slug fills
                    // it to the crest, the spill runs into the basin, and the mill drains it again.
                    s.Barrels = L < 5.5 ? Full : FadeOut(5.5, 7.5, L);
                    s.LakeEl = FadeIn(5.5, 7.5, L, true);
                    s.Mill = FadeIn(8, 10, L, false);
                    s.RiverEl = s.Router = s.Link = s.Sea = s.PondEl = Hidden;
                    for (int i = 0; i < Standing; i++)
                        s.Level[i] = 0.08 + 0.30 * Smooth(0.5, 10.5, L) - 0.24 * Smooth(11, 14.5, L) + 0.10 * Smooth(15.5, 20, L);
                    Storm(s, L, 6.5, 9.0, 11.0, 15.3);
                    s.Lake = 0.08 + 0.11 * Smooth(7, 8.5, L) + 0.11 * Smooth(9.5, 11, L) + 0.70 * Smooth(12, 15.5, L) - 0.65 * Smooth(16.5, 20, L);
                    s.Spill = Window(15.0, 15.6, 17.4, 18.2, L);
                    s.RingDropped = (long)(400000 * Smooth(15.4, 17.8, L));
                    s.Race = Smooth(8, 10, L);
                    s.Sign1 = Window(10, 11, 16, 17, L);
                    s.PlateA = 1 - Smooth(6.5, 7.5, L);
                    s.PlateB = Smooth(7.5, 8.5, L);
                    break;
                }
            case 4:
                {
                    // Restart with the tier. The taller dam holds the same slug at about half, the
                    // mill passes it on, and the river swells close to its banks.
                    s.Barrels = Faint; s.LakeEl = Full; s.Mill = Full;
                    s.RiverEl = FadeIn(1, 3, L, false); s.Router = FadeIn(1, 3, L, false);
                    s.Link = s.Sea = s.PondEl = Hidden;
                    for (int i = 0; i < Standing; i++)
                        s.Level[i] = 0.10 + 0.26 * Smooth(0, 6, L) - 0.22 * Smooth(6.5, 10, L) + 0.12 * Smooth(11, 18, L);
                    Storm(s, L, 1.0, 3.5, 6.0, 10.3);
                    s.Lake = 0.10 + 0.12 * Smooth(2, 5.5, L) + 0.30 * Smooth(7, 10.5, L) - 0.37 * Smooth(12, 18, L);
                    s.Race = 1; s.RiverFlow = 0.85;
                    s.River = 0.15 + 0.70 * Smooth(9, 12.5, L) - 0.55 * Smooth(13.5, 18, L);
                    s.Sign1 = Window(3, 4, 9, 10, L);
                    s.PlateA = 1;
                    break;
                }
            case 5:
                {
                    // The receiver slows. The second lake fills to its crest, then the sluice tips
                    // the whole lake into the pond, and new water follows it while replay trickles.
                    s.Router = Full;
                    s.Link = FadeIn(0.5, 2.5, L, false); s.Sea = FadeIn(0.5, 2.5, L, false);
                    s.PondEl = FadeIn(9.5, 11.5, L, false);
                    s.Valve = 1 - 0.75 * Smooth(3, 5, L);
                    s.Outlet = 0.6 * s.Valve;
                    s.Gate = Smooth(11.3, 12.3, L);
                    s.Lake2 = L < 11.5 ? 0.15 + 0.85 * Smooth(4, 11, L) : 1 - 0.75 * Smooth(11.5, 14, L);
                    s.SpillPipe = Smooth(11.4, 12.0, L) * (1 - 0.2 * Smooth(14, 15, L));
                    s.Pond = 0.3 * Smooth(11.5, 14, L) + 0.45 * Smooth(14, 20, L);
                    s.ReplayFlow = 0.35 * Smooth(14.5, 15.5, L);
                    s.Spooled = (long)(6000000 * Clamp01((L - 11.5) / 8.5));
                    s.Replayed = (long)(1500000 * Clamp01((L - 14.5) / 5.5));
                    s.Sign1 = Window(2.5, 3.5, 8.5, 9.5, L);
                    s.Sign2 = Window(11.5, 12.5, 17.5, 18.5, L);
                    s.PlateA = Smooth(11, 12, L);
                    break;
                }
            default:
                {
                    // The receiver recovers. Replay drains the pond oldest first while new water
                    // still enters it; when the pond is empty the sluice turns back.
                    s.Router = s.Link = s.Sea = s.PondEl = Full;
                    s.Valve = 0.25 + 0.75 * Smooth(0.5, 3, L);
                    s.Outlet = s.Valve;
                    double drained = Smooth(2, 13.5, L);
                    s.Pond = 0.75 * (1 - drained);
                    s.Gate = 1 - Smooth(13.6, 14.8, L);
                    s.SpillPipe = 0.8 * (1 - Smooth(13.4, 14.2, L));
                    s.ReplayFlow = Smooth(1, 3, L) * (1 - Smooth(13.5, 14.5, L));
                    s.Lake2 = L < 13.5 ? 0.25 + 0.25 * Smooth(1, 4, L) : 0.5 - 0.3 * Smooth(14, 17.5, L);
                    s.Spooled = 6000000 + (long)(100000 * Clamp01(L / 13.5));
                    s.Replayed = 1500000 + (long)(4600000 * drained);
                    s.PlateA = 1;
                    break;
                }
        }
        s.WheelAngle = WheelAngle(t);
        return s;
    }

    // ------------------------------------------------------------------ layout

    // Scene window (inner) 30..1570 x 70..740. Caption 758..842. Footnote 862.
    const float SceneX = 30, SceneY = 70, SceneR = 1570, SceneB = 740;
    const float GutterX = 50, GutterY = 172, GutterH = 14, RainY = 118;
    const float PadY = 450, HillY = 450;
    const float TroughX = 50, TroughY = 466, TroughH = 16;
    const float CliffX = 770, LakeX = 770, LakeBottom = 700, CrestShort = 560, CrestTall = 452;
    const float DamX = 970, DamW = 42, DamFoot = 700, Land = 500;
    const float BasinX = 1018, BasinW = 32, BasinBottom = 640;
    const float RaceX = 1056, RaceR = 1166, RaceBottom = 600, CulvertY = 578;
    const float WheelCx = 1111, WheelCy = 560, WheelR = 40;
    const float RiverX = 1166, RiverR = 1266, BankTop = 500, RiverBottom = 600;
    const float SluiceX = 1266, SluiceW = 26, SluiceWheelY = 440;
    const float Lake2X = 1292, Lake2R = 1402;
    const float WeirX = 1402, WeirW = 24, OutletY = 556, OutletH = 16, OutletR = 1466;
    const float SeaX = 1466, SeaLevel = 600, SeaBottom = 728;
    const float PondX = 1266, PondR = 1436, PondTop = 640, PondBottom = 728;
    const float SpillPipeX = 1274, ReplayPipeX = 1380, PipeW = 10;
    const float CaptionY = 758, CaptionH = 84;
    const float BarrelPitch = 118, BarrelX0 = 50;

    static readonly RectangleF BarrelsRect = new RectangleF(30, 165, 745, 320), LakeRect = new RectangleF(30, 396, 1032, 312),
        MillRect = new RectangleF(944, 486, 226, 122), RiverRect = new RectangleF(1160, 490, 110, 116),
        RouterRect = new RectangleF(1258, 420, 40, 186), LinkRect = new RectangleF(1286, 480, 190, 136),
        SeaRect = new RectangleF(1460, 494, 112, 240), PondRect = new RectangleF(1260, 596, 182, 140),
        CloudRect = new RectangleF(140, 60, 400, 100);

    static float Crest(Step st) { return st.TallDam ? CrestTall : CrestShort; }
    static float BarrelW(Step st) { return st.Tier ? 102 : 92; }
    static float BarrelH(Step st) { return st.Tier ? 244 : 200; }
    static float BarrelX(int i) { return BarrelX0 + i * BarrelPitch; }
    static float RowEnd(Step st) { return BarrelX(Positions - 1) + BarrelW(st) + 12; }

    static string N(long v) { return v.ToString("#,0", CultureInfo.InvariantCulture); }

    // ------------------------------------------------------------------ static layer: the backdrop

    static void DrawStatic(int step)
    {
        var st = Steps[step];
        g.Clear(Slate);
        Modes();

        Text(TitleText, 40, 24, Serif, 26, Ivory, true);

        // The scene window: a steel frame around the landscape.
        SteelPanel(SceneX - 10, SceneY - 10, SceneR - SceneX + 20, SceneB - SceneY + 20, 10);
        using (var inner = Round(SceneX, SceneY, SceneR - SceneX, SceneB - SceneY, 4))
        {
            Fill(inner, SkyLow);
            var old = g.Clip;
            g.SetClip(inner);
            Backdrop();
            g.Clip = old;
            Stroke(inner, A(200, IronDark), 2f);
        }
        Bevel(SceneX - 1, SceneY - 1, SceneR - SceneX + 2, SceneB - SceneY + 2, 5, 90, 2f);

        // Caption plate: the step tag and one sentence.
        Shadow(20, CaptionY, 1560, CaptionH, 8);
        using (var p = Round(20, CaptionY, 1560, CaptionH, 8))
        {
            FillGrad(p, new RectangleF(20, CaptionY, 1560, CaptionH), SlateLight, Slate, 90f);
            Stroke(p, BrassDark, 2f);
        }
        Bevel(21, CaptionY + 1, 1558, CaptionH - 2, 7, 70, 1f);
        string tag = "STEP " + (step + 1) + " OF " + Steps.Length;
        Text(tag, 44, CaptionY + 12, Sans, 14, OptionGold, true);
        if (st.Banner) Text("·   " + BannerText, 44 + Measure(tag, Sans, 14, true).Width + 14, CaptionY + 12, Sans, 14, Warn, true);
        Text(st.Caption, 44, CaptionY + 34, Sans, 28, Ivory);

        Text(Footnote, 44, 862, Sans, 15, Muted);
    }

    // Sky, distant hills, the earth and the shore. No water works: those are elements.
    static void Backdrop()
    {
        FillGradRect(new RectangleF(SceneX, SceneY, SceneR - SceneX, 540), SkyTop, SkyLow, 90f);
        using (var p = new GraphicsPath())
        {
            p.AddBeziers(new[] { new PointF(SceneX, 440), new PointF(300, 350), new PointF(700, 400), new PointF(1000, 370), new PointF(1200, 350), new PointF(1400, 400), new PointF(SceneR, 380) });
            p.AddLine(SceneR, 380, SceneR, 640); p.AddLine(SceneR, 640, SceneX, 640); p.CloseFigure();
            FillGrad(p, new RectangleF(SceneX, 340, SceneR - SceneX, 300), Mix(SkyLow, GrassDark, 0.3), Mix(SkyLow, GrassDark, 0.5), 90f);
        }
        using (var ground = new GraphicsPath())
        {
            ground.AddLines(new[] {
                new PointF(SceneX, HillY), new PointF(DamX + DamW, HillY), new PointF(DamX + DamW, Land), new PointF(SceneR + 10, Land),
                new PointF(SceneR + 10, SceneB + 10), new PointF(SceneX, SceneB + 10) });
            ground.CloseFigure();
            FillGrad(ground, new RectangleF(SceneX, HillY, SceneR - SceneX, SceneB - HillY + 10), EarthLight, EarthDark, 90f);
        }
        GrassLine(SceneX, DamX + DamW, HillY);
        GrassLine(DamX + DamW, SceneR + 10, Land);
        // The trough under the barrels.
        Cavity(TroughX, TroughY, CliffX - TroughX, TroughH);
    }

    static void GrassLine(float x1, float x2, float y)
    {
        if (x2 <= x1) return;
        FillGradRect(new RectangleF(x1, y - 5, x2 - x1, 7), GrassLight, GrassDark, 90f);
    }

    // ------------------------------------------------------------------ dynamic layer: the elements

    static void DrawDynamic(double t)
    {
        var s = Compute(t);
        var st = Steps[s.Step];
        double tm = t * Slow;
        Modes();
        var old = g.Clip;
        using (var inner = Round(SceneX, SceneY, SceneR - SceneX, SceneB - SceneY, 4))
        {
            g.SetClip(inner);
            Motion = 1;
            Layer(CloudRect, 1, () => { Cloud(230, 100, 32, false); Cloud(430, 110, 25, false); });
            Layer(CloudRect, s.CloudDark, () => { Cloud(230, 100, 32, true); Cloud(430, 110, 25, true); });
            Rain(90, RainY, RowEnd(st) - 100, GutterY - RainY + 2, tm, s.Rain);
            Element(s.Barrels, BarrelsRect, () => BarrelWorks(s, st, tm));
            Element(s.LakeEl, LakeRect, () => LakeWorks(s, st, tm));
            Element(s.Mill, MillRect, () => MillWorks(s, tm));
            Element(s.RiverEl, RiverRect, () => RiverWorks(s, tm));
            Element(s.Router, RouterRect, () => RouterWorks(s, tm));
            Element(s.Link, LinkRect, () => LinkWorks(s, tm));
            Element(s.Sea, SeaRect, () => SeaWorks(tm));
            Element(s.PondEl, PondRect, () => PondWorks(s, tm));
            Motion = 1;
            Signs(s, st);
            g.Clip = old;
        }
    }

    static void Element(El e, RectangleF r, Action draw)
    {
        Motion = e.Live;
        Layer(r, e.Vis, draw);
    }

    static void BarrelWorks(State s, Step st, double tm)
    {
        float bw = BarrelW(st), bh = BarrelH(st), top = PadY - bh;
        Gutter(GutterX, GutterY, RowEnd(st) - GutterX, GutterH);
        for (int i = 0; i < Positions; i++)
        {
            float cx = BarrelX(i) + bw / 2;
            Pipe(cx - 3, GutterY + GutterH - 2, 6, top - 14 - (GutterY + GutterH - 2));
            Pad(BarrelX(i), PadY, bw);
        }
        for (int i = 0; i < Positions; i++)
        {
            if (s.Present[i] <= 0.01) continue;
            int k = i;
            float drop = (float)((1 - s.Present[i]) * 90);
            Action draw = () => Barrel(BarrelX(k), PadY - drop, bw, bh, s.Level[k], tm, s.Tap[k], s.Inflow[k], s.Overflow, TroughY + 4, top - 14);
            if (s.Present[i] >= 0.999) draw();
            else Layer(new RectangleF(BarrelX(i) - 24, top - 100, bw + 48, bh + 120), Smooth(0, 0.25, s.Present[i]), draw);
        }
    }

    static void LakeWorks(State s, Step st, double tm)
    {
        float crest = Crest(st);
        Stream(TroughX + 2, TroughY + 4, CliffX - TroughX - 2, TroughH - 7, tm, s.Trough, true, 20);
        // The pit behind the dam, the dam with its cap, and the brass crest rule on the lake side.
        Pit(LakeX, HillY, DamX + DamW - LakeX, LakeBottom - HillY);
        Concrete(DamX, crest, DamW, DamFoot - crest);
        FillGradRect(new RectangleF(DamX - 4, crest - 6, DamW + 8, 7), Mix(ConcreteLight, PureWhite, 0.2), ConcreteDark, 90f);
        Line(LakeX, crest, DamX, crest, A(210, BrassLight), 1.5f);
        // The spill basin: a pit at the dam foot with a grating at the bottom. The spill is lost there.
        Pit(BasinX, Land - 2, BasinW, BasinBottom - Land + 2);
        for (int i = 1; i < 4; i++) Line(BasinX + i * BasinW / 4, BasinBottom - 22, BasinX + i * BasinW / 4, BasinBottom - 6, IronLight, 1.4f);
        float lakeSurface = LakeBottom - (float)(s.Lake * (LakeBottom - crest));
        Fall(CliffX - 3, 8, TroughY + TroughH - 4, lakeSurface, tm, s.Trough);
        Water(LakeX, DamX - LakeX, lakeSurface, LakeBottom, tm, 1.2, 1.0, 255, true, CrestTall - 10);
        if (s.Spill > 0.01)
        {
            // A sheet over the crest, a fall down the face into the basin, foam at the foot.
            int a = (int)(230 * s.Spill);
            FillGradRect(new RectangleF(DamX - 4, crest - 9, DamW + 14, 9), A(a, WaterTop), A(a, WaterMid), 90f);
            Stream(DamX - 4, crest - 9, DamW + 14, 9, tm, s.Spill, true, 24);
            Fall(BasinX + 6, 14, crest - 6, BasinBottom - 24, tm, s.Spill);
            Water(BasinX + 2, BasinW - 4, BasinBottom - 26, BasinBottom - 4, tm, 0.5, 1.5, 230, false);
        }
    }

    static void MillWorks(State s, double tm)
    {
        Cavity(RaceX, Land - 2, RaceR - RaceX, RaceBottom - Land + 2);
        Pipe(DamX - 20, CulvertY, RaceX - (DamX - 20) + 2, 14);
        Water(RaceX, RaceR - RaceX, RaceBottom - 18, RaceBottom, tm, 0.6, 1.0, 255, false);
        Stream(RaceX, RaceBottom - 16, RaceR - RaceX, 8, tm, s.Race, true, 22);
        FillGradRect(new RectangleF(WheelCx - 4, WheelCy, 8, RaceBottom - WheelCy), WoodLight, WoodDark, 0f);
        MillWheel(WheelCx, WheelCy, WheelR, s.WheelAngle);
        FoamBlobs(WheelCx - 18, RaceBottom - 14, 36, tm, 0.5);
    }

    static float RiverSurface(State s) { return RiverBottom - (float)(s.River * (RiverBottom - BankTop)); }

    static void RiverWorks(State s, double tm)
    {
        Cavity(RiverX, Land - 2, RiverR - RiverX, RiverBottom - Land + 2);
        float surface = RiverSurface(s);
        Water(RiverX, RiverR - RiverX, surface, RiverBottom, tm, 1.0, 1.0, 255, false, BankTop);
        Stream(RiverX, surface + 2, RiverR - RiverX, Math.Max(2, Math.Min(9, RiverBottom - surface - 4)), tm, s.RiverFlow * 0.6, true, 14 + 10 * (float)s.RiverFlow);
    }

    // The sluice (the router): the gate is up for the straight path, down to send water to the pond.
    static void RouterWorks(State s, double tm)
    {
        Cavity(SluiceX, Land - 2, SluiceW, RiverBottom - Land + 2);
        FillGradRect(new RectangleF(SluiceX - 3, SluiceWheelY + 10, 5, RiverBottom - SluiceWheelY - 10), IronLight, IronDark, 0f);
        FillGradRect(new RectangleF(SluiceX + SluiceW - 2, SluiceWheelY + 10, 5, RiverBottom - SluiceWheelY - 10), IronLight, IronDark, 0f);
        float gateH = RiverBottom - BankTop + 8;
        float gateTop = BankTop - 8 - (float)((1 - s.Gate) * 58);
        FillGradRect(new RectangleF(SluiceX + 2, gateTop, SluiceW - 4, gateH), IronLight, IronDark, 0f);
        for (int i = 1; i < 6; i++) Line(SluiceX + 5, gateTop + i * 11, SluiceX + SluiceW - 5, gateTop + i * 11, A(90, PureBlack), 1f);
        FillGradRect(new RectangleF(SluiceX + SluiceW / 2 - 2, SluiceWheelY + 10, 4, Math.Max(0, gateTop - SluiceWheelY - 10)), SteelLight, SteelDark, 0f);
        HandWheel(SluiceX + SluiceW / 2, SluiceWheelY, 12, s.Gate * 1.6);
        float surface = RiverSurface(s);
        if (s.Gate < 0.99)
            Stream(SluiceX, surface + 2, SluiceW, Math.Max(2, Math.Min(9, RiverBottom - surface - 4)), tm, s.RiverFlow * 0.6 * (1 - s.Gate), true, 22);
    }

    // The second lake (the link FIFO), the weir, and the outlet (the link) with its valve.
    static void LinkWorks(State s, double tm)
    {
        Cavity(Lake2X, Land - 2, Lake2R - Lake2X, RiverBottom - Land + 2);
        Concrete(WeirX, BankTop - 4, WeirW, RiverBottom - BankTop + 10);
        Pipe(WeirX + 4, OutletY, OutletR - WeirX - 4, OutletH);
        FillGradRect(new RectangleF(WeirX + 14, OutletY - 22, 5, 22), IronLight, IronDark, 0f);
        float surface = RiverBottom - (float)(s.Lake2 * (RiverBottom - BankTop));
        Water(Lake2X, Lake2R - Lake2X, surface, RiverBottom, tm, 1.0, 0.9, 255, true, BankTop);
        Stream(WeirX + WeirW, OutletY + 3, OutletR - WeirX - WeirW, OutletH - 6, tm, s.Outlet, true, 12 + 16 * (float)s.Outlet);
        HandWheel(WeirX + 16, OutletY - 28, 11, (1 - s.Valve) * 2.5);
        Fall(OutletR - 2, 8, OutletY + 4, SeaLevel + 2, tm, s.Outlet);
    }

    static void SeaWorks(double tm)
    {
        Pit(SeaX, Land - 2, SceneR - SeaX + 10, SeaBottom - Land + 2);
        Water(SeaX, SceneR - SeaX, SeaLevel, SeaBottom, tm, 1.6, 0.6);
    }

    // The pond (the disk spool), the spill pipe from the sluice floor, and the replay pipe up from the bottom.
    static void PondWorks(State s, double tm)
    {
        Cavity(PondX, PondTop, PondR - PondX, PondBottom - PondTop);
        Pipe(SpillPipeX, RiverBottom, PipeW, PondTop + 12 - RiverBottom);
        Pipe(ReplayPipeX, RiverBottom - 4, PipeW, PondBottom - 8 - (RiverBottom - 4));
        float surface = PondBottom - 2 - (float)(s.Pond * (PondBottom - PondTop - 4));
        Water(PondX + 3, PondR - PondX - 6, surface, PondBottom - 2, tm, 0.8, 0.7, 215, false, PondTop);
        Stream(SpillPipeX + 2, RiverBottom + 2, PipeW - 4, PondTop + 8 - RiverBottom, tm, s.SpillPipe, false, 16);
        if (s.SpillPipe > 0.01) Fall(SpillPipeX + 2, PipeW - 4, PondTop + 12, surface, tm, s.SpillPipe);
        Stream(ReplayPipeX + 2, RiverBottom - 2, PipeW - 4, PondBottom - 10 - RiverBottom, tm, s.ReplayFlow, false, -16);
    }

    // ------------------------------------------------------------------ signs and the one plate

    static void Signs(State s, Step st)
    {
        string[] none = { "" }, none2 = { "", "" };
        switch (s.Step)
        {
            case 0:
                Sign("ETW buffers", 400, 488, s.Sign1);
                Sign("event channel", 870, 405, s.Sign2);
                PlateLayer(400, 528, s.PlateA, () => Plate(400, 528, new[] { "--etw-buffer-kib 32" }, none, 24));
                break;
            case 1:
                PlateLayer(400, 528, s.PlateA, () => Plate(400, 528, new[] { "--etw-max-buffers 64" }, none, 24));
                break;
            case 2:
                PlateLayer(400, 528, s.PlateA, () => Plate(400, 528, new[] { "etw_lost" }, new[] { "  " + N(s.EtwLost) }, 24, "illustration, not a lab result", true));
                break;
            case 3:
                Sign("parser", WheelCx, 404, s.Sign1);
                PlateLayer(400, 528, s.PlateA, () => Plate(400, 528, new[] { "--etw-buffer-kib 128", "--etw-max-buffers 128" }, none2, 22, "storm: 100,000 queries per second"));
                PlateLayer(870, 362, s.PlateB, () => Plate(870, 362, new[] { "--capacity 131072", "ring_dropped" }, new[] { "", "  " + N(s.RingDropped) }, 22));
                break;
            case 4:
                Sign("send queue", 1200, 392, s.Sign1);
                PlateLayer(400, 528, s.PlateA, () => Plate(400, 528, new[] { "--etw-buffer-kib 128 --etw-min-buffers 12 --etw-max-buffers 128", "--capacity 524288" }, none2, 19));
                break;
            case 5:
                Sign("receiver", 1500, 440, s.Sign1);
                Sign("disk spool", 1120, 640, s.Sign2);
                PlateLayer(1120, 680, s.PlateA, () => Plate(1120, 680, new[] { "spooled" }, new[] { "  " + N(s.Spooled) }, 24));
                break;
            default:
                PlateLayer(1120, 680, s.PlateA, () => Plate(1120, 680, new[] { "replayed" }, new[] { "  " + N(s.Replayed) }, 24));
                break;
        }
    }

    static void PlateLayer(float cx, float y, double alpha, Action draw)
    {
        Layer(new RectangleF(cx - 400, y - 12, 800, 140), alpha, draw);
    }

    // ------------------------------------------------------------------ reports

    public static void Report()
    {
        double s0 = 0;
        for (int i = 0; i < Steps.Length; i++)
        {
            var st = Steps[i];
            double barrelMax = 0, barrelMin = 1, lakeMax = 0, riverMax = 0, pondMax = 0, lake2Max = 0; int barrels = 0;
            State last = null;
            for (double t = s0; t < s0 + st.Seconds; t += 1.0 / Fps)
            {
                var s = Compute(t);
                lakeMax = Math.Max(lakeMax, s.Lake); riverMax = Math.Max(riverMax, s.River); pondMax = Math.Max(pondMax, s.Pond); lake2Max = Math.Max(lake2Max, s.Lake2);
                int n = 0;
                for (int b = 0; b < Positions; b++)
                    if (s.Present[b] >= 0.999) { n++; barrelMax = Math.Max(barrelMax, s.Level[b]); barrelMin = Math.Min(barrelMin, s.Level[b]); }
                barrels = Math.Max(barrels, n);
                last = s;
            }
            Console.WriteLine("Step " + (i + 1) + ": " + st.Seconds + " s, barrels " + barrels + " at " + barrelMin.ToString("0.00") + ".." + barrelMax.ToString("0.00")
                + ", lake " + lakeMax.ToString("0.00") + ", river " + riverMax.ToString("0.00") + ", link FIFO " + lake2Max.ToString("0.00") + ", pond " + pondMax.ToString("0.00")
                + ", etw_lost " + N(last.EtwLost) + ", ring_dropped " + N(last.RingDropped) + ", spooled " + N(last.Spooled) + ", replayed " + N(last.Replayed));
            Console.WriteLine("        " + st.Caption);
            s0 += st.Seconds;
        }
        Console.WriteLine("Loop " + Loop + " s, " + (int)(Loop * Fps) + " frames at " + Fps + " fps");
    }
}
