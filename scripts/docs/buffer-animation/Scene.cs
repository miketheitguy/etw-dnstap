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
// The picture: rain from dns.exe falls into a roof gutter that feeds a row of rain
// barrels (the ETW buffers). The brass taps (the flush) run into a ground trough that
// pours into a lake behind a dam (the event channel). A culvert turns a mill wheel (the
// parser). The river (the send queue) passes a sluice (the router) into a second lake
// (the link FIFO) and over an outlet into the sea (the TCP receiver). Under the second
// lake lies the pond (the encrypted disk spool).

using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Text;
using System.Globalization;

public static partial class Watershed
{
    public const int Width = 1600, Height = 900, Fps = 8;

    // ------------------------------------------------------------------ storyboard

    enum Focus { None, EtwLost, RingDropped, Spooled, Replayed }

    sealed class Step
    {
        public string Caption, BarrelPlate, CrestPlate;
        public double Seconds;
        public bool Tier, TallDam, SpoolLabel, Banner;
        public Focus Counter;
    }

    static readonly Step[] Steps = {
        new Step { Seconds = 12, SpoolLabel = true, BarrelPlate = "--etw-buffer-kib 32",
            Caption = "Light rain fills the ETW barrels, and the taps pass it on, lake by lake, to the receiver." },
        new Step { Seconds = 12, BarrelPlate = "--etw-max-buffers 64",
            Caption = "When rain outpaces the taps, ETW sets down more barrels, up to the maximum count." },
        new Step { Seconds = 12, Counter = Focus.EtwLost, Banner = true,
            Caption = "With every barrel full at the maximum, new rain is lost, and etw_lost counts it." },
        new Step { Seconds = 14, Tier = true, CrestPlate = "--capacity 131072", Counter = Focus.RingDropped,
            Caption = "Large barrels pass the whole storm on, the lake tops the dam, and ring_dropped counts the spill." },
        new Step { Seconds = 14, Tier = true, TallDam = true, CrestPlate = "--capacity 524288",
            BarrelPlate = "--etw-buffer-kib 128\n--etw-min-buffers 12 --etw-max-buffers 128",
            Caption = "A taller dam holds the same storm, so nothing spills, and the mill passes it on." },
        new Step { Seconds = 14, Tier = true, TallDam = true, SpoolLabel = true, Counter = Focus.Spooled,
            Caption = "When the receiver slows, the last lake fills, then spills whole into the disk spool." },
        new Step { Seconds = 14, Tier = true, TallDam = true, SpoolLabel = true, Counter = Focus.Replayed,
            Caption = "When the receiver recovers, replay drains the pond oldest first, before new water passes." },
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

    const int Positions = 7, Standing = 5;

    sealed class State
    {
        public int Step; public double T, L;
        public double Rain, Overflow, Trough, Lake, Spill, WheelAngle, Race;
        public double River, RiverFlow, Lake2, Outlet, Valve, Gate, SpillPipe, ReplayFlow, Pond;
        public double[] Level = new double[Positions], Tap = new double[Positions], Present = new double[Positions];
        public long EtwLost, RingDropped, Spooled, Replayed;
    }

    static double Bump(double phase, double width) { phase = Frac(phase); return phase < width ? Math.Sin(Math.PI * phase / width) : 0; }
    static double Window(double a, double b, double c, double d, double v) { return Smooth(a, b, v) * (1 - Smooth(c, d, v)); }

    static State Compute(double t)
    {
        var s = new State();
        double u;
        int step = StepAt(t, out u);
        var st = Steps[step];
        s.Step = step; s.T = t; s.L = t - StepStart(step);
        double L = s.L;

        // Wheel angle: continuous across steps. Each step has a constant speed in radians per second.
        double[] speed = { 0.3, 0.45, 0.5, 0.7, 0.7, 0.7, 0.7 };
        double angle = 0;
        for (int i = 0; i < step; i++) angle += speed[i] * Steps[i].Seconds;
        s.WheelAngle = angle + speed[step] * L;

        for (int i = 0; i < Standing; i++) s.Present[i] = 1;
        s.Lake2 = 0.15; s.Outlet = 0.6; s.Valve = 1.0; s.River = 0.12; s.RiverFlow = 0.4; s.Race = 0.5;

        switch (step)
        {
            case 0:
                {
                    // Calm: two barrels fill and drain in turn, the others stay near empty.
                    s.Rain = 0.3;
                    double maxTap = 0;
                    for (int i = 0; i < Standing; i++)
                    {
                        if (i == 0 || i == 2)
                        {
                            double phase = Frac((L + (i == 2 ? 3 : 0)) / 6) * 6;
                            s.Level[i] = 0.15 + 0.55 * (Smooth(0, 4.4, phase) - Smooth(4.7, 5.8, phase));
                            s.Tap[i] = Window(4.4, 4.9, 5.6, 6.0, phase);
                        }
                        else s.Level[i] = 0.10 + 0.02 * Math.Sin(L * 0.5 + i);
                        maxTap = Math.Max(maxTap, s.Tap[i]);
                    }
                    s.Trough = 0.25 + 0.75 * maxTap;
                    s.Lake = 0.10 + 0.04 * (0.5 + 0.5 * Math.Sin(2 * Math.PI * (L - 1.2) / 3));
                    break;
                }
            case 1:
                {
                    // A burst: every barrel fills, and two more barrels are set down on their pads.
                    s.Rain = 1.0;
                    double ramp = Smooth(0, 4, L);
                    double maxTap = 0;
                    for (int i = 0; i < Positions; i++)
                    {
                        double landed = i == 5 ? 6 : 9.5;
                        s.Present[i] = i < Standing ? 1 : i == 5 ? Smooth(3, 6, L) : Smooth(6.5, 9.5, L);
                        double cyc = Frac((L + i * 0.9) / 5);
                        double tri = cyc < 0.68 ? Smooth(0, 0.68, cyc) : 1 - Smooth(0.72, 0.98, cyc);
                        double lv = 0.55 + 0.42 * tri;
                        double tap = Window(0.64, 0.72, 0.90, 0.98, cyc);
                        double k = i < Standing ? ramp : Smooth(landed, landed + 2.5, L);
                        s.Level[i] = i < Standing ? 0.15 + (lv - 0.15) * k : lv * k;
                        s.Tap[i] = tap * k;
                        maxTap = Math.Max(maxTap, s.Tap[i]);
                    }
                    s.Trough = 0.4 + 0.6 * maxTap;
                    s.Lake = 0.10 + 0.56 * Smooth(2, 10, L);
                    s.Race = 0.7; s.River = 0.16; s.RiverFlow = 0.6;
                    break;
                }
            case 2:
                {
                    // Overflow (illustration): every barrel is full, every tap runs, and the spouts spill.
                    s.Rain = 1.0;
                    for (int i = 0; i < Positions; i++)
                    {
                        s.Present[i] = 1;
                        s.Level[i] = 0.97 + 0.02 * Math.Sin(L * 0.4 + i);
                        s.Tap[i] = 1;
                    }
                    s.Trough = 1;
                    s.Overflow = Smooth(0.5, 3.5, L);
                    s.EtwLost = (long)(25000 * Smooth(1.5, 11.5, L));
                    s.Lake = 0.69 + 0.01 * Math.Sin(L * 0.5);
                    s.Race = 0.8; s.River = 0.18; s.RiverFlow = 0.6;
                    break;
                }
            default:
                {
                    // The 100,000 queries per second storm with large barrels. The taps deliver in an
                    // uneven rhythm: two small beats, then one large slug. The picture does not show
                    // where the water waits, and the barrels stay low.
                    s.Rain = 0.8;
                    double period = 7, phi = Frac(L / period);
                    int slug = (int)Math.Floor(L / period);
                    double big = Window(0.26, 0.34, 0.56, 0.64, phi);
                    double small = 0.35 * Math.Max(Bump(phi - 0.02, 0.11), Bump(phi - 0.15, 0.11));
                    for (int i = 0; i < Positions; i++)
                    {
                        s.Present[i] = i < Standing ? 1 : i == 5 ? (step == 3 ? Smooth(2, 5, L) : 1) : 0;
                        s.Level[i] = 0.12 + 0.14 * (0.5 + 0.5 * Math.Sin(2 * Math.PI * (phi + i * 0.15)));
                        s.Tap[i] = Math.Max(big, i < 2 ? small : 0) * s.Present[i];
                    }
                    s.Trough = Math.Max(big, small);
                    s.Race = 0.9; s.RiverFlow = 0.85;
                    if (step == 3)
                    {
                        // The lake fills to the crest at the slug, spills, then falls before the next slug.
                        s.Lake = 0.30 + 0.70 * Window(0.28, 0.56, 0.70, 1.0, phi);
                        s.Spill = Window(0.53, 0.58, 0.72, 0.78, phi);
                        double done = slug * 200000 + 200000 * Smooth(0.55, 0.76, phi);
                        s.RingDropped = (long)Math.Min(400000, done);
                        s.River = 0.14 + 0.14 * Window(0.45, 0.75, 0.85, 1.0, phi);
                    }
                    else
                    {
                        // The taller dam: the same slug raises the lake to about half, and the river swells.
                        s.Lake = 0.10 + 0.42 * Window(0.28, 0.56, 0.68, 1.0, phi);
                        s.River = 0.15 + 0.70 * Window(0.40, 0.68, 0.78, 1.0, phi);
                    }
                    if (step == 5)
                    {
                        // The sea mouth narrows. The second lake fills, then spills whole into the pond.
                        s.Valve = 1 - 0.75 * Smooth(0.5, 2.5, L);
                        s.Outlet = 0.6 * s.Valve;
                        s.Gate = Smooth(6.3, 7.0, L);
                        if (L < 6.5) s.Lake2 = 0.15 + 0.85 * Smooth(1.5, 6.5, L);
                        else if (L < 8.5)
                        {
                            double k = Smooth(6.5, 8.5, L);
                            s.Lake2 = 1 - 0.75 * k; s.Pond = 0.3 * k;
                            s.SpillPipe = Smooth(6.4, 6.9, L);
                        }
                        else
                        {
                            s.SpillPipe = 0.8;
                            double k = (L - 8.5) / 5.5;
                            s.Pond = 0.3 + 0.45 * (1 - (1 - k) * (1 - k));
                            s.Lake2 = 0.25 + 0.02 * Math.Sin(L * 0.5);
                            s.ReplayFlow = 0.35 * Smooth(8.5, 9.5, L);
                        }
                        s.Spooled = (long)(6000000 * Clamp01((L - 6.5) / 7.3));
                        s.Replayed = (long)(1500000 * Clamp01((L - 8.5) / 5.3));
                    }
                    if (step == 6)
                    {
                        // The sea mouth opens. Replay drains the pond oldest first; new water still enters the pond.
                        s.Valve = 0.25 + 0.75 * Smooth(0.5, 2.5, L);
                        s.Outlet = 1.0 * s.Valve;
                        double drained = Smooth(1.5, 11, L);
                        s.Pond = 0.75 * (1 - drained);
                        s.Gate = 1 - Smooth(10.8, 11.6, L);
                        s.SpillPipe = 0.8 * (1 - Smooth(10.8, 11.3, L));
                        s.ReplayFlow = Smooth(0.5, 2.5, L) * (1 - Smooth(10.8, 11.5, L));
                        s.Lake2 = L < 11 ? 0.25 + 0.25 * Smooth(0.5, 3, L) : 0.5 - 0.3 * Smooth(11, 14, L);
                        s.Spooled = 6000000 + (long)(100000 * Clamp01(L / 11));
                        s.Replayed = 1500000 + (long)(4600000 * drained);
                    }
                    break;
                }
        }
        return s;
    }

    // ------------------------------------------------------------------ layout

    // Scene window (inner) 30..1570 x 70..740. Caption 758..842. Footnote 862.
    const float SceneX = 30, SceneY = 70, SceneR = 1570, SceneB = 740;
    const float GutterX = 60, GutterY = 200, GutterH = 14, RainY = 132;
    const float PadY = 390, HillY = 400;
    const float TroughX = 60, TroughY = 404, TroughH = 16;
    const float CliffX = 660, LakeX = 660, LakeBottom = 660, CrestShort = 540, CrestTall = 380;
    const float DamX = 900, DamW = 44, DamFoot = 660, Land = 480;
    const float BasinX = 948, BasinW = 36, BasinY = 476, BasinH = 24;
    const float RaceX = 990, RaceR = 1110, RaceBottom = 580, CulvertY = 560;
    const float WheelCx = 1050, WheelCy = 540, WheelR = 40;
    const float RiverX = 1110, RiverR = 1290, BankTop = 480, RiverBottom = 580;
    const float SluiceX = 1290, SluiceW = 26, SluiceWheelY = 420;
    const float Lake2X = 1316, Lake2R = 1396;
    const float WeirX = 1396, WeirW = 24, OutletY = 540, OutletH = 16, OutletR = 1450;
    const float SeaX = 1450, SeaLevel = 600, SeaBottom = 740;
    const float PondX = 1316, PondR = 1420, PondTop = 630, PondBottom = 730;
    const float SpillPipeX = 1298, ReplayPipeX = 1394, PipeW = 10;
    const float CaptionY = 758, CaptionH = 84;

    static float Crest(Step st) { return st.TallDam ? CrestTall : CrestShort; }
    static float BarrelW(Step st) { return st.Tier ? 64 : 56; }
    static float BarrelH(Step st) { return st.Tier ? 160 : 120; }
    static float BarrelPitch(Step st) { return st.Tier ? 86 : 78; }
    static float BarrelX0(Step st) { return st.Tier ? 60 : 70; }
    static float BarrelX(Step st, int i) { return BarrelX0(st) + i * BarrelPitch(st); }
    static float RowEnd(Step st) { return BarrelX(st, Positions - 1) + BarrelW(st) + 12; }

    static string N(long v) { return v.ToString("#,0", CultureInfo.InvariantCulture); }

    // ------------------------------------------------------------------ static layer

    static void DrawStatic(int step)
    {
        var st = Steps[step];
        g.Clear(Slate);
        g.SmoothingMode = SmoothingMode.AntiAlias;
        g.TextRenderingHint = TextRenderingHint.AntiAliasGridFit;
        g.PixelOffsetMode = PixelOffsetMode.HighQuality;

        Text(TitleText, 40, 20, Serif, 30, Ivory, true);

        // The scene window: a steel frame around the landscape.
        SteelPanel(SceneX - 10, SceneY - 10, SceneR - SceneX + 20, SceneB - SceneY + 20, 10);
        using (var inner = Round(SceneX, SceneY, SceneR - SceneX, SceneB - SceneY, 4))
        {
            Fill(inner, SkyLow);
            var old = g.Clip;
            g.SetClip(inner);
            Landscape(st);
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

    static void Landscape(Step st)
    {
        float crest = Crest(st);
        float bw = BarrelW(st), bh = BarrelH(st), rowEnd = RowEnd(st);

        // Sky and distant hills.
        FillGradRect(new RectangleF(SceneX, SceneY, SceneR - SceneX, 540), SkyTop, SkyLow, 90f);
        using (var p = new GraphicsPath())
        {
            p.AddBeziers(new[] { new PointF(SceneX, 420), new PointF(300, 330), new PointF(700, 380), new PointF(1000, 350), new PointF(1200, 330), new PointF(1400, 380), new PointF(SceneR, 360) });
            p.AddLine(SceneR, 360, SceneR, 620); p.AddLine(SceneR, 620, SceneX, 620); p.CloseFigure();
            FillGrad(p, new RectangleF(SceneX, 320, SceneR - SceneX, 300), Mix(SkyLow, GrassDark, 0.3), Mix(SkyLow, GrassDark, 0.5), 90f);
        }

        // Earth: the hill with the barrels, the lake cut, the land, the sea cliff.
        using (var ground = new GraphicsPath())
        {
            ground.AddLines(new[] {
                new PointF(SceneX, HillY), new PointF(CliffX, HillY), new PointF(CliffX, LakeBottom), new PointF(DamX + DamW, LakeBottom),
                new PointF(DamX + DamW, Land), new PointF(SeaX, Land), new PointF(SeaX, SeaBottom), new PointF(SceneR, SeaBottom),
                new PointF(SceneR, SceneB + 10), new PointF(SceneX, SceneB + 10) });
            ground.CloseFigure();
            FillGrad(ground, new RectangleF(SceneX, HillY, SceneR - SceneX, SceneB - HillY + 10), EarthLight, EarthDark, 90f);
            Speckle(ground, new RectangleF(SceneX, HillY, SceneR - SceneX, SceneB - HillY), 31, 1600, 30);
        }
        GrassLine(SceneX, CliffX, HillY);
        GrassLine(DamX + DamW, BasinX - 2, Land);
        GrassLine(BasinX + BasinW + 2, RaceX, Land);
        GrassLine(WeirX + WeirW, SeaX, Land);
        // The cliff into the lake, and the lake floor.
        FillGradRect(new RectangleF(CliffX - 8, HillY, 8, LakeBottom - HillY), StoneDark, StoneLight, 0f);
        FillGradRect(new RectangleF(CliffX - 8, LakeBottom - 3, DamX - CliffX + 8, 7), StoneLight, StoneDark, 90f);
        FillGradRect(new RectangleF(SeaX, SeaBottom - 10, SceneR - SeaX, 10), EarthDark, StoneDark, 90f);

        // Channels cut into the land.
        Cavity(TroughX, TroughY, CliffX - TroughX, TroughH, 44);
        Cavity(BasinX, BasinY, BasinW, BasinH, 45);
        Cavity(RaceX, Land - 2, RaceR - RaceX, RaceBottom - Land + 2, 47);
        Cavity(RiverX, Land - 2, RiverR - RiverX, RiverBottom - Land + 2, 41);
        Cavity(SluiceX, Land - 2, SluiceW, RiverBottom - Land + 2, 42);
        Cavity(Lake2X, Land - 2, Lake2R - Lake2X, RiverBottom - Land + 2, 43);
        for (int i = 1; i < 4; i++) Line(BasinX + i * BasinW / 4, BasinY + 3, BasinX + i * BasinW / 4, BasinY + BasinH - 3, IronLight, 1.4f);

        // The gutter above the barrels, a downspout at every pad, and the pads.
        Gutter(GutterX, GutterY, rowEnd - GutterX, GutterH);
        for (int i = 0; i < Positions; i++)
        {
            float cx = BarrelX(st, i) + bw / 2;
            float lidTop = PadY - bh;
            Pipe(cx - 3, GutterY + GutterH - 2, 6, lidTop + 8 - (GutterY + GutterH - 2));
            Pad(BarrelX(st, i), PadY, bw);
        }

        // The dam: concrete with a cap, and a brass rule along the crest on the lake side.
        Concrete(DamX, crest, DamW, DamFoot - crest, 51);
        FillGradRect(new RectangleF(DamX - 4, crest - 6, DamW + 8, 7), Mix(ConcreteLight, PureWhite, 0.2), ConcreteDark, 90f);
        Line(LakeX, crest, DamX, crest, A(210, BrassLight), 1.5f);
        // Culvert from the lake through the dam to the mill race.
        Pipe(DamX - 20, CulvertY, RaceX - (DamX - 20) + 2, 14);
        // Mill post.
        FillGradRect(new RectangleF(WheelCx - 4, WheelCy, 8, RaceBottom - WheelCy), WoodLight, WoodDark, 0f);
        // Sluice posts (the router).
        FillGradRect(new RectangleF(SluiceX - 3, SluiceWheelY + 10, 5, RiverBottom - SluiceWheelY - 10), IronLight, IronDark, 0f);
        FillGradRect(new RectangleF(SluiceX + SluiceW - 2, SluiceWheelY + 10, 5, RiverBottom - SluiceWheelY - 10), IronLight, IronDark, 0f);
        // Weir at the end of the second lake and the outlet pipe (the link) to the sea.
        Concrete(WeirX, BankTop - 4, WeirW, RiverBottom - BankTop + 10, 57);
        Pipe(WeirX + 4, OutletY, OutletR - WeirX - 4, OutletH);
        FillGradRect(new RectangleF(WeirX + 14, OutletY - 22, 5, 22), IronLight, IronDark, 0f);
        // The pond (the disk spool) and its two pipes.
        Cavity(PondX, PondTop, PondR - PondX, PondBottom - PondTop, 61);
        Pipe(SpillPipeX, RiverBottom, PipeW, PondTop - RiverBottom + 4);
        Pipe(ReplayPipeX, RiverBottom - 4, PipeW, PondTop - RiverBottom + 10);

        // Clouds.
        Cloud(255, 86, 50, st.Tier || Steps[0] != st);
        Cloud(455, 102, 34, st.Tier || Steps[0] != st);

        // Labels: one to three words each.
        Label(705, 80, "rain from dns.exe", st.Tier ? "100,000 queries per second" : null);
        Label(300, 432, "ETW buffers");
        Label(805, crest - 48, "event channel");
        Label(WheelCx, 436, "parser");
        Label(1200, 436, "send queue");
        if (st.SpoolLabel) Label(1160, 616, "disk spool");
        Label(1510, 494, "receiver");

        // The one option plate of the step.
        if (st.BarrelPlate != null)
        {
            var lines = st.BarrelPlate.Split('\n');
            var blank = new string[lines.Length];
            for (int i = 0; i < blank.Length; i++) blank[i] = "";
            Plate(300, 478, lines, blank, lines.Length > 1 ? 20 : 24);
        }
        if (st.CrestPlate != null) Plate(805, crest - 98, new[] { st.CrestPlate }, new[] { "" }, 24);
    }

    // A channel cut into the earth: a dark cavity with stone walls and a stone floor.
    static void Cavity(float x, float y, float w, float h, int seed)
    {
        using (var p = Round(x, y, w, h, 3))
        {
            FillGrad(p, new RectangleF(x, y, w, h), C(52, 46, 40), C(26, 24, 22), 90f);
            Speckle(p, new RectangleF(x, y, w, h), seed, (int)(w * h / 60), 30);
        }
        FillGradRect(new RectangleF(x - 4, y, 4, h), StoneDark, StoneLight, 0f);
        FillGradRect(new RectangleF(x + w, y, 4, h), StoneLight, StoneDark, 0f);
        FillGradRect(new RectangleF(x - 4, y + h - 3, w + 8, 6), StoneLight, StoneDark, 90f);
    }

    static void GrassLine(float x1, float x2, float y)
    {
        if (x2 <= x1) return;
        FillGradRect(new RectangleF(x1, y - 5, x2 - x1, 7), GrassLight, GrassDark, 90f);
    }

    // ------------------------------------------------------------------ dynamic layer

    static void DrawDynamic(double t)
    {
        var s = Compute(t);
        var st = Steps[s.Step];
        g.SmoothingMode = SmoothingMode.AntiAlias;
        g.TextRenderingHint = TextRenderingHint.AntiAliasGridFit;
        g.PixelOffsetMode = PixelOffsetMode.HighQuality;
        var old = g.Clip;
        using (var inner = Round(SceneX, SceneY, SceneR - SceneX, SceneB - SceneY, 4))
        {
            g.SetClip(inner);
            Scenery(s, st);
            g.Clip = old;
        }
    }

    static void Scenery(State s, Step st)
    {
        double t = s.T;
        float crest = Crest(st);
        float bw = BarrelW(st), bh = BarrelH(st);

        // Rain into the gutter.
        Rain(100, RainY, Math.Min(560, RowEnd(st) - 10) - 100, GutterY - RainY + 2, t, s.Rain);

        // The barrels.
        for (int i = 0; i < Positions; i++)
        {
            if (s.Present[i] <= 0.01) continue;
            if (s.Present[i] >= 0.999) Barrel(BarrelX(st, i), PadY, bw, bh, s.Level[i], t, s.Tap[i], s.Overflow, TroughY + 4);
            else BarrelArriving(BarrelX(st, i), PadY, bw, bh, s.Level[i], t, s.Present[i]);
        }

        // The trough collects the taps and pours over the cliff into the lake.
        Stream(TroughX + 2, TroughY + 4, CliffX - TroughX - 2, TroughH - 7, t, s.Trough, true, 20);
        float lakeSurface = LakeBottom - (float)(s.Lake * (LakeBottom - crest));
        Fall(CliffX - 3, 8, TroughY + TroughH - 4, lakeSurface, t, s.Trough);

        // The lake behind the dam, and the spill over the crest into the basin.
        Water(LakeX, DamX - LakeX, lakeSurface, LakeBottom, t, 1.2, 1.0, 255, true, CrestTall - 10);
        if (s.Spill > 0.01)
        {
            // A sheet over the crest, a dark fall down the face into the basin, foam at the foot.
            int a = (int)(230 * s.Spill);
            FillGradRect(new RectangleF(DamX - 4, crest - 9, DamW + 14, 9), A(a, WaterTop), A(a, WaterMid), 90f);
            Stream(DamX - 4, crest - 9, DamW + 14, 9, t, s.Spill, true, 24);
            FillGradRect(new RectangleF(DamX + DamW, crest - 6, 16, BasinY + BasinH - 8 - crest + 6), A(a, WaterMid), A(a, WaterDeep), 0f);
            Fall(DamX + DamW + 2, 12, crest - 6, BasinY + BasinH - 8, t, s.Spill);
            Water(BasinX + 2, BasinW - 4, BasinY + 5, BasinY + BasinH - 2, t, 0.5, 1.5, 230, false);
        }

        // The mill race and the wheel (the parser).
        Water(RaceX, RaceR - RaceX, RaceBottom - 18, RaceBottom, t, 0.6, 1.0, 255, false);
        Stream(RaceX, RaceBottom - 16, RaceR - RaceX, 8, t, s.Race, true, 22);
        MillWheel(WheelCx, WheelCy, WheelR, s.WheelAngle);
        FoamBlobs(WheelCx - 18, RaceBottom - 14, 36, t, 0.5);

        // The river (the send queue).
        float riverSurface = RiverBottom - (float)(s.River * (RiverBottom - BankTop));
        Water(RiverX, RiverR - RiverX, riverSurface, RiverBottom, t, 1.0, 1.0, 255, false, BankTop);
        Stream(RiverX, riverSurface + 2, RiverR - RiverX, Math.Max(2, Math.Min(9, RiverBottom - riverSurface - 4)), t, s.RiverFlow * 0.6, true, 14 + 10 * (float)s.RiverFlow);

        // The sluice (the router): the gate is up for the straight path, down to send water to the pond.
        float gateH = RiverBottom - BankTop + 8;
        float gateTop = BankTop - 8 - (float)((1 - s.Gate) * 58);
        FillGradRect(new RectangleF(SluiceX + 2, gateTop, SluiceW - 4, gateH), IronLight, IronDark, 0f);
        for (int i = 1; i < 6; i++) Line(SluiceX + 5, gateTop + i * 11, SluiceX + SluiceW - 5, gateTop + i * 11, A(90, PureBlack), 1f);
        FillGradRect(new RectangleF(SluiceX + SluiceW / 2 - 2, SluiceWheelY + 10, 4, Math.Max(0, gateTop - SluiceWheelY - 10)), SteelLight, SteelDark, 0f);
        HandWheel(SluiceX + SluiceW / 2, SluiceWheelY, 12, s.Gate * 1.6);
        if (s.Gate < 0.99)
            Stream(SluiceX, riverSurface + 2, SluiceW, Math.Max(2, Math.Min(9, RiverBottom - riverSurface - 4)), t, s.RiverFlow * 0.6 * (1 - s.Gate), true, 22);
        Stream(SpillPipeX + 2, RiverBottom + 2, PipeW - 4, PondTop - RiverBottom, t, s.SpillPipe, false, 16);

        // The second lake (the link FIFO) and the replay pipe up from the pond.
        float lake2Surface = RiverBottom - (float)(s.Lake2 * (RiverBottom - BankTop));
        Water(Lake2X, Lake2R - Lake2X, lake2Surface, RiverBottom, t, 1.0, 0.9, 255, true, BankTop);
        Stream(ReplayPipeX + 2, RiverBottom - 2, PipeW - 4, PondTop - RiverBottom + 6, t, s.ReplayFlow, false, -16);

        // The outlet (the link): flow in the pipe, the valve wheel, and the fall into the sea.
        Stream(WeirX + WeirW, OutletY + 3, OutletR - WeirX - WeirW, OutletH - 6, t, s.Outlet, true, 12 + 16 * (float)s.Outlet);
        HandWheel(WeirX + 16, OutletY - 28, 11, (1 - s.Valve) * 2.5);
        Fall(OutletR - 2, 8, OutletY + 4, SeaLevel + 2, t, s.Outlet);

        // The sea (the receiver).
        Water(SeaX, SceneR - SeaX, SeaLevel, SeaBottom, t, 1.6, 0.6);

        // The pond (the disk spool).
        float pondSurface = PondBottom - 2 - (float)(s.Pond * (PondBottom - PondTop - 4));
        Water(PondX + 3, PondR - PondX - 6, pondSurface, PondBottom - 2, t, 0.8, 0.7, 215, false, PondTop);
        if (s.SpillPipe > 0.01) Fall(SpillPipeX + 3, 6, PondTop + 4, pondSurface, t, s.SpillPipe);

        // The one counter plate of the step.
        switch (st.Counter)
        {
            case Focus.EtwLost: Plate(300, 478, new[] { "etw_lost" }, new[] { "  " + N(s.EtwLost) }, 24, "illustration, not a lab result"); break;
            case Focus.RingDropped: Plate(1000, 672, new[] { "ring_dropped" }, new[] { "  " + N(s.RingDropped) }, 24); break;
            case Focus.Spooled: Plate(1160, 662, new[] { "spooled" }, new[] { "  " + N(s.Spooled) }, 24); break;
            case Focus.Replayed: Plate(1160, 662, new[] { "replayed" }, new[] { "  " + N(s.Replayed) }, 24); break;
        }
    }

    // ------------------------------------------------------------------ reports

    public static void Report()
    {
        double s0 = 0;
        for (int i = 0; i < Steps.Length; i++)
        {
            var st = Steps[i];
            double lakeMax = 0, riverMax = 0, pondMax = 0, lake2Max = 0; int barrels = 0;
            State last = null;
            for (double t = s0; t < s0 + st.Seconds; t += 1.0 / Fps)
            {
                var s = Compute(t);
                lakeMax = Math.Max(lakeMax, s.Lake); riverMax = Math.Max(riverMax, s.River); pondMax = Math.Max(pondMax, s.Pond); lake2Max = Math.Max(lake2Max, s.Lake2);
                int n = 0; foreach (double p in s.Present) if (p >= 0.999) n++;
                barrels = Math.Max(barrels, n);
                last = s;
            }
            Console.WriteLine("Step " + (i + 1) + ": " + st.Seconds + " s, barrels " + barrels + ", lake " + lakeMax.ToString("0.00") + ", river " + riverMax.ToString("0.00") + ", link FIFO " + lake2Max.ToString("0.00") + ", pond " + pondMax.ToString("0.00")
                + ", etw_lost " + N(last.EtwLost) + ", ring_dropped " + N(last.RingDropped) + ", spooled " + N(last.Spooled) + ", replayed " + N(last.Replayed));
            Console.WriteLine("        " + st.Caption);
            s0 += st.Seconds;
        }
        Console.WriteLine("Loop " + Loop + " s, " + (int)(Loop * Fps) + " frames at " + Fps + " fps");
    }
}
