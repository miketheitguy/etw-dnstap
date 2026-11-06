// Scene.cs - "Buffers as a watershed": the storyboard, the water model and the picture.
//
// Nine steps. Each step is one run of the same storm. Steps 1 to 4 are one run at the
// defaults, steps 5, 6 and 9 start after a service restart, steps 7 and 8 continue the
// step 6 run. The water, the needles and the counters are a function of the time t, so
// every build gives the same GIF.
//
// Every number on screen is an illustration (see the footnote). Option names, defaults
// and tier values are exact and come from docs/guide/tuning.md, docs/guide/configuration.md,
// src/pipeline/config.rs, src/etw/session.rs, src/etw/intake.rs, src/sender.rs and
// src/sender/router.rs. The storyboard is docs/design/buffer-watershed.md.

using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Text;
using System.Globalization;

public static partial class Watershed
{
    public const int Width = 1600, Height = 900, Fps = 6;

    // ------------------------------------------------------------------ storyboard

    sealed class Step
    {
        public string Title, Caption, Settings, Storm;
        public double Seconds;
        public bool Restart, TierTanks, TallDam, NoSpool, Banner;
        public int MaxBuffers, Capacity;
    }

    const string Defaults = "--etw-buffer-kib 32 --etw-min-buffers 12 --etw-max-buffers 64 --etw-flush-seconds 1 --capacity 131072 --spool-max-mib 4096";
    const string TierLow = "--etw-buffer-kib 128 --etw-min-buffers 12 --etw-max-buffers 128 --etw-flush-seconds 1 --capacity 131072 --spool-max-mib 4096";
    const string Tier = "--etw-buffer-kib 128 --etw-min-buffers 12 --etw-max-buffers 128 --etw-flush-seconds 1 --capacity 524288 --spool-max-mib 4096";
    const string TierNoSpool = "--etw-buffer-kib 128 --etw-min-buffers 12 --etw-max-buffers 128 --etw-flush-seconds 1 --capacity 524288 --no-spool";

    static readonly Step[] Steps = {
        new Step { Title = "The whole river", Seconds = 10, Settings = Defaults, Storm = "light rain", MaxBuffers = 64, Capacity = 131072,
            Caption = "Rain from dns.exe passes through each buffer in turn, and every lake stays low at a calm rate." },
        new Step { Title = "The ETW tanks", Seconds = 11, Settings = Defaults, Storm = "light rain", MaxBuffers = 64, Capacity = 131072,
            Caption = "ETW holds events in fixed tanks and opens the gate when a tank is full or the timer ends." },
        new Step { Title = "A burst digs tanks", Seconds = 10, Settings = Defaults, Storm = "heavy rain", MaxBuffers = 64, Capacity = 131072,
            Caption = "When the tanks fill faster than the gate opens, ETW adds tanks up to the maximum count." },
        new Step { Title = "ETW overflow (illustration)", Seconds = 10, Settings = Defaults, Storm = "heavy rain", MaxBuffers = 64, Capacity = 131072, Banner = true,
            Caption = "With every tank full at the maximum, ETW discards new events and counts them in etw_lost." },
        new Step { Title = "The dam spills", Seconds = 11, Settings = TierLow, Storm = "100,000 queries per second", MaxBuffers = 128, Capacity = 131072, Restart = true, TierTanks = true,
            Caption = "Large tanks pass the storm on, so the full burst reaches the lake and the newest events spill over." },
        new Step { Title = "The taller dam", Seconds = 10, Settings = Tier, Storm = "100,000 queries per second", MaxBuffers = 128, Capacity = 524288, Restart = true, TierTanks = true, TallDam = true,
            Caption = "A taller dam holds the same burst, so no event spills, and the mill passes it downstream." },
        new Step { Title = "A slow sea", Seconds = 12, Settings = Tier, Storm = "100,000 queries per second", MaxBuffers = 128, Capacity = 524288, TierTanks = true, TallDam = true,
            Caption = "When the receiver slows, the full second lake spills into the disk pond, and new water follows it." },
        new Step { Title = "Replay in order", Seconds = 12, Settings = Tier, Storm = "100,000 queries per second", MaxBuffers = 128, Capacity = 524288, TierTanks = true, TallDam = true,
            Caption = "When the receiver recovers, replay empties the pond oldest first before new water flows past it." },
        new Step { Title = "No pond", Seconds = 10, Settings = TierNoSpool, Storm = "100,000 queries per second", MaxBuffers = 128, Capacity = 524288, Restart = true, TierTanks = true, TallDam = true, NoSpool = true,
            Caption = "Without a spool, a full second lake stops the sluice, and the mill drops new blocks into queue_dropped." },
    };

    const string TitleText = "Buffers as a watershed";
    const string Footnote = "Illustration: counts, sizes and rates are not to scale. Step 4 is an illustration, not a lab result. A setting change takes a service restart. Option names and values are exact.";

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

    sealed class State
    {
        public int Step; public double U, T, Local;
        public double Rain, GateOpen, Chute, WheelAngle, Spill, Lake, River, RiverFlow, Lake2, Outlet, Valve, Pond, ReplayFlow, SpillPipe, LostDrops, RiverDrop;
        public int Dug, Allocated, InUse;
        public double[] Tank = new double[28];
        public bool SluiceToPond, SluiceClosed;
        public string Event = ""; public Color EventColor = LampOff; public bool EventOn;
        public long Received, Encoded, Delivered, EtwLost, RingDropped, QueueDropped, Spooled, Replayed, ChannelPeak, SendPeak, SpoolBytes, SpoolSegments, SpoolAge, DisconnectedDropped;
        public double ChannelDepth, BytesPerSec;
        public bool Connected = true, Saturated, SpoolAvailable = true;
        public double InsetLevel, InsetGate, InsetClock, InsetFloat;
    }

    static double Bump(double phase, double width) { phase = Frac(phase); return phase < width ? Math.Sin(Math.PI * phase / width) : 0; }

    static State Compute(double t)
    {
        var s = new State();
        double u;
        int step = StepAt(t, out u);
        var st = Steps[step];
        s.Step = step; s.U = u; s.T = t; s.Local = t - StepStart(step);
        double L = s.Local;
        int solid = 12;
        int ghosts = st.TierTanks ? 16 : 10;

        // Wheel angle: continuous across steps. Each step has a constant speed.
        double[] speed = { 0.9, 0.9, 1.6, 1.6, 2.6, 2.6, 2.6, 2.6, 2.6 };
        double angle = 0;
        for (int i = 0; i < step; i++) angle += speed[i] * Steps[i].Seconds;
        s.WheelAngle = angle + speed[step] * L;

        s.SpoolAvailable = !st.NoSpool;
        s.Allocated = 12; s.InUse = 2; s.Dug = 0;
        s.Lake2 = 0.15; s.Outlet = 0.6; s.Valve = 1.0; s.River = 0.12; s.RiverFlow = 0.5;

        switch (step)
        {
            case 0:
            case 1:
                s.Rain = 0.3;
                s.Tank[0] = 0.15 + 0.5 * Frac(L); s.Tank[1] = 0.15 + 0.5 * Frac(L + 0.5);
                s.GateOpen = Math.Max(Bump(L, 0.3), Bump(L + 0.5, 0.3));
                s.Chute = s.GateOpen;
                s.Lake = 0.03 + 0.045 * (1 - Frac(L - 0.25));
                s.ChannelDepth = s.Lake * st.Capacity;
                s.ChannelPeak = 10000;
                s.SendPeak = 3; s.River = 0.1; s.RiverFlow = 0.4;
                s.Received = (long)(600000 * (step + u)); s.Encoded = s.Received; s.Delivered = (long)(1200 * (step + u));
                s.BytesPerSec = 1.0;
                if (step == 1)
                {
                    // Inset: first half, the float trigger; second half, the timer trigger.
                    if (L < 5.5)
                    {
                        double p = L / 5.5;
                        s.InsetLevel = p < 0.75 ? p / 0.75 : 1 - (p - 0.75) / 0.25;
                        s.InsetGate = p < 0.72 ? 0 : p < 0.8 ? (p - 0.72) / 0.08 : 1 - Smooth(0.9, 1.0, p);
                        s.InsetClock = p * 0.6;
                    }
                    else
                    {
                        double p = (L - 5.5) / 5.5;
                        s.InsetLevel = p < 0.75 ? 0.45 * p / 0.75 : 0.45 * (1 - (p - 0.75) / 0.25);
                        s.InsetGate = p < 0.72 ? 0 : p < 0.8 ? (p - 0.72) / 0.08 : 1 - Smooth(0.9, 1.0, p);
                        s.InsetClock = 0.6 + p;
                    }
                    s.InsetFloat = s.InsetLevel;
                }
                break;
            case 2:
                {
                    s.Rain = 1.0;
                    double fill = Smooth(0, 0.7, u);
                    s.Allocated = (int)Math.Round(12 + 52 * Smooth(0.1, 0.75, u));
                    s.Dug = (int)Math.Round((s.Allocated - 12) / 52.0 * ghosts);
                    for (int i = 0; i < solid + s.Dug; i++)
                        s.Tank[i] = Clamp01(0.2 + 0.75 * fill + 0.12 * Math.Sin(2 * Math.PI * (L * 0.5) + i * 0.9) - (i >= solid ? 0.1 : 0));
                    s.InUse = Math.Min(60, (int)Math.Round(2 + 58 * fill));
                    s.GateOpen = Bump(L * 2, 0.4);
                    s.Chute = s.GateOpen;
                    s.Lake = 0.05 + 0.64 * Smooth(0.05, 0.6, u) + 0.02 * Math.Sin(L * 2);
                    s.ChannelDepth = s.Lake * st.Capacity;
                    s.ChannelPeak = (long)Math.Max(10000, s.Lake * st.Capacity);
                    s.SendPeak = 20; s.River = 0.15 + 0.08 * Bump(L * 0.5, 0.6); s.RiverFlow = 0.7;
                    s.Received = 1200000 + (long)(1800000 * u); s.Encoded = s.Received; s.Delivered = 2400 + (long)(3600 * u);
                    s.BytesPerSec = 5;
                    if (s.Allocated >= 58) SetEvent(s, "Event ID 21: the ETW buffers are near the maximum", LampAmber);
                    break;
                }
            case 3:
                s.Rain = 1.0;
                s.Allocated = 64; s.Dug = ghosts; s.InUse = 64;
                for (int i = 0; i < solid + ghosts; i++) s.Tank[i] = 0.97 + 0.03 * Math.Sin(L * 3 + i);
                s.GateOpen = Bump(L * 2, 0.4);
                s.Chute = s.GateOpen;
                s.LostDrops = Smooth(0.05, 0.3, u);
                s.EtwLost = (long)(25000 * Smooth(0.1, 0.95, u));
                s.Lake = 0.69 + 0.02 * Math.Sin(L * 2);
                s.ChannelDepth = s.Lake * st.Capacity; s.ChannelPeak = 90000;
                s.SendPeak = 20; s.River = 0.15 + 0.08 * Bump(L * 0.5, 0.6); s.RiverFlow = 0.7;
                s.Received = 3000000 + (long)(2000000 * u); s.Encoded = s.Received - s.EtwLost; s.Delivered = 6000 + (long)(3000 * u);
                s.BytesPerSec = 5;
                if (u > 0.15) SetEvent(s, "Event ID 10: data loss in ETW", LampRed);
                break;
            case 4:
            case 5:
            case 6:
            case 7:
            case 8:
                {
                    s.Rain = 0.8;
                    bool spills = step == 4;
                    double period = 3.0;
                    double phi = Frac(L / period);
                    int slug = (int)Math.Floor(L / period);
                    s.Allocated = (int)Math.Round(12 + 22 * Smooth(0, 0.4, step == 4 ? u : 1));
                    if (step > 4) s.Allocated = 34;
                    s.Dug = (int)Math.Round((s.Allocated - 12) / 116.0 * ghosts);
                    s.InUse = (int)Math.Round(4 + 10 * Bump(phi, 0.25));
                    for (int i = 0; i < 4; i++) s.Tank[i] = Clamp01(0.12 + 0.3 * Bump(phi + i * 0.07, 0.3));
                    // The gate: two small beats, then the big slug. The pause is not explained in the picture.
                    double big = Bump(phi, 0.3);
                    double small = 0.35 * Math.Max(Bump(phi - 0.55, 0.12), Bump(phi - 0.75, 0.12));
                    s.GateOpen = Math.Max(big, small);
                    s.Chute = s.GateOpen;
                    if (spills)
                    {
                        // The lake fills to the crest at the slug, spills, then falls again before the next slug.
                        s.Lake = phi < 0.12 ? 0.25 + 0.75 * Smooth(0, 0.12, phi) : phi < 0.42 ? 1.0 : 1.0 - 0.75 * Smooth(0.42, 0.95, phi);
                        s.Spill = phi >= 0.1 && phi < 0.42 ? Math.Min(1, Smooth(0.1, 0.16, phi) * (1 - Smooth(0.36, 0.42, phi))) : 0;
                        double spillSpan = 0.32, perStep = Steps[step].Seconds / period * spillSpan;
                        double done = slug * spillSpan + Clamp01((phi - 0.1) / spillSpan) * spillSpan;
                        s.RingDropped = (long)(400000 * Math.Min(1, done / perStep));
                        s.ChannelDepth = s.Lake * st.Capacity;
                        s.ChannelPeak = slug > 0 || phi > 0.12 ? st.Capacity : (long)s.ChannelDepth;
                        s.River = 0.12 + 0.12 * Bump(phi - 0.15, 0.6); s.RiverFlow = 0.8; s.SendPeak = 60;
                        if (phi > 0.08 && s.Spill < 0.5 && slug == 0) SetEvent(s, "Event ID 20: the event channel is near full", LampAmber);
                        else if (s.RingDropped > 0) SetEvent(s, "Event ID 11: data loss at capacity: the event channel was full", LampRed);
                        s.Received = (long)(12000000 * u); s.Encoded = s.Received - s.RingDropped; s.Delivered = (long)(2000 * u);
                        s.BytesPerSec = 20;
                    }
                    else
                    {
                        // The taller dam: the same slug raises the lake to about half.
                        s.Lake = phi < 0.14 ? 0.1 + 0.38 * Smooth(0, 0.14, phi) : phi < 0.3 ? 0.48 : 0.48 - 0.38 * Smooth(0.3, 0.95, phi);
                        s.ChannelDepth = s.Lake * st.Capacity;
                        s.ChannelPeak = 250000;
                        // The river swells behind the slug, close to its banks, then drains.
                        double riverSwell = phi < 0.22 ? 0.15 + 0.71 * Smooth(0.04, 0.22, phi) : 0.86 - 0.71 * Smooth(0.25, 0.9, phi);
                        s.River = riverSwell; s.RiverFlow = 0.9; s.SendPeak = 220;
                        s.Received = step == 5 ? (long)(12000000 * u) : 12000000 + (long)(6000000 * (step - 6)) + (long)(6000000 * u);
                        s.Delivered = (long)(2000 * (step == 5 ? u : 1 + 0.5 * (step - 5) + 0.5 * u));
                        s.Encoded = s.Received;
                        s.BytesPerSec = 20;
                        if (step == 5)
                        {
                            if (u > 0.2 && u < 0.6) SetEvent(s, "Event ID 22: the send queue is near full", LampAmber);
                        }
                        if (step == 6)
                        {
                            // The sea mouth narrows. The second lake fills, then spills whole into the pond.
                            s.Valve = 1 - 0.75 * Smooth(0.02, 0.1, u);
                            s.Outlet = 0.6 * s.Valve;
                            double fillTo = 0.35, tip = 0.42;
                            if (u < fillTo) { s.Lake2 = 0.15 + 0.85 * Smooth(0.08, fillTo, u); s.River = Math.Max(s.River, 0.2); }
                            else if (u < tip) { double k = (u - fillTo) / (tip - fillTo); s.Lake2 = 1 - k; s.Pond = 0.3 * k; s.SpillPipe = 1; s.SluiceToPond = true; }
                            else
                            {
                                s.SluiceToPond = true; s.SpillPipe = 0.8;
                                s.Pond = 0.3 + 0.45 * (u - tip) / (1 - tip);
                                s.Lake2 = 0.25 + 0.05 * Math.Sin(L * 1.5);
                                s.ReplayFlow = 0.35;
                            }
                            s.SendPeak = 30;
                            double sp = Clamp01((u - fillTo) / (1 - fillTo));
                            s.Spooled = (long)(6000000 * sp); s.Replayed = (long)(1500000 * Clamp01((u - tip) / (1 - tip)));
                            s.SpoolBytes = (long)((s.Spooled - s.Replayed) * 103L);
                            s.SpoolSegments = (long)Math.Ceiling(s.SpoolBytes / (64.0 * 1024 * 1024));
                            s.SpoolAge = (long)(30000 * sp);
                            s.Saturated = u > 0.45;
                            s.BytesPerSec = 5;
                            if (u > 0.6) SetEvent(s, "Event ID 23: dnstap link is saturated", LampAmber);
                        }
                        if (step == 7)
                        {
                            // The sea mouth opens. Replay drains the pond oldest first; new water still enters the pond.
                            s.Valve = 0.25 + 0.75 * Smooth(0.02, 0.1, u);
                            double drained = Smooth(0.08, 0.8, u);
                            s.Pond = 0.75 * (1 - drained);
                            bool backlog = u < 0.8;
                            s.SluiceToPond = backlog; s.SpillPipe = backlog ? 0.8 : 0;
                            s.ReplayFlow = backlog ? 1.0 : 0;
                            s.Lake2 = backlog ? 0.5 + 0.05 * Math.Sin(L * 1.5) : 0.5 - 0.3 * Smooth(0.8, 1.0, u);
                            s.Outlet = 1.0 * s.Valve;
                            s.SendPeak = 30;
                            s.Spooled = 6000000 + (long)(100000 * Clamp01(u / 0.8));
                            s.Replayed = 1500000 + (long)(4600000 * Clamp01(u / 0.8));
                            s.SpoolBytes = Math.Max(0, (s.Spooled - s.Replayed) * 103L);
                            s.SpoolSegments = (long)Math.Ceiling(s.SpoolBytes / (64.0 * 1024 * 1024));
                            s.SpoolAge = (long)(30000 * (1 - Clamp01(u / 0.8)));
                            s.Saturated = u < 0.3;
                            s.BytesPerSec = backlog ? 20 + 60 * Smooth(0.08, 0.2, u) : 20;
                            if (u > 0.3) SetEvent(s, "dnstap link is no longer saturated", LampGreen);
                        }
                        if (step == 8)
                        {
                            // No pond: the full second lake stops the sluice; the river fills and the mill drops blocks.
                            s.Valve = 1 - 0.75 * Smooth(0.02, 0.1, u);
                            s.Outlet = 0.6 * s.Valve;
                            s.Lake2 = 0.15 + 0.85 * Smooth(0.08, 0.35, u);
                            s.SluiceClosed = u >= 0.35;
                            double riverFill = Smooth(0.35, 0.55, u);
                            s.River = Math.Max(s.River, riverFill);
                            if (u >= 0.35) { s.River = Math.Max(0.86, s.River); s.RiverFlow = 0.2; }
                            s.RiverDrop = Smooth(0.55, 0.62, u);
                            s.QueueDropped = (long)(4500000 * Clamp01((u - 0.55) / 0.45));
                            s.SendPeak = u >= 0.5 ? 256 : 220;
                            s.Received = (long)(6000000 * u); s.Encoded = s.Received; s.Delivered = (long)(1000 * u);
                            s.BytesPerSec = 5;
                            if (u > 0.6) SetEvent(s, "Event ID 13: data loss on the network path", LampRed);
                            else if (u > 0.45) SetEvent(s, "Event ID 22: the send queue is near full", LampAmber);
                        }
                    }
                    break;
                }
        }
        return s;
    }

    static void SetEvent(State s, string text, Color c) { s.Event = text; s.EventColor = c; s.EventOn = true; }

    // ------------------------------------------------------------------ layout

    // Scene window (inner) 28..1572 x 86..648. Panel 664..776. Caption 786..850. Footnote 860.
    const float SceneX = 28, SceneY = 86, SceneR = 1572, SceneB = 648;
    const float HillY = 322;                       // ground under the tanks
    const float TankX0 = 70; const int SolidTanks = 12;
    const float TroughY = 300, GateX = 436, GateW = 30, ChuteEnd = 596;
    const float LakeX = 580, LakeBottom = 520, CrestShort = 470, CrestTall = 320;
    const float DamX = 790, DamW = 40, DamFoot = 556;
    const float Land = 500;                        // the ground right of the dam
    const float BasinX = 836, BasinW = 36, BasinDepth = 16;   // spill basin
    const float WheelCx = 922, WheelCy = 506, WheelR = 32, PitX = 886, PitR = 958, PitBottom = 570;
    const float RiverX = 958, RiverR = 1110, BankTop = 500, RiverBottom = 554;
    const float SluiceX = 1110, SluiceW = 30;
    const float Lake2X = 1140, Lake2R = 1330;
    const float WeirX = 1330, WeirW = 24, OutletY = 518, OutletH = 16, OutletR = 1436;
    const float SeaX = 1436, SeaLevel = 540, SeaBottom = 648;
    const float PondX = 1110, PondR = 1330, PondTop = 596, PondBottom = 646;
    const float PanelY = 664, PanelH = 130, CaptionY = 804, CaptionH = 64;

    static float Crest(Step st) { return st.TallDam ? CrestTall : CrestShort; }
    static float TankH(Step st) { return st.TierTanks ? 72 : 50; }
    static float TankW(Step st) { return st.TierTanks ? 18 : 24; }
    static float TankPitch(Step st) { return st.TierTanks ? 22 : 29; }
    static float SolidRowY(Step st) { return st.TierTanks ? 140 : 158; }
    static float GhostRowY(Step st) { return st.TierTanks ? 220 : 216; }
    static int Ghosts(Step st) { return st.TierTanks ? 16 : 10; }

    static string N(long v) { return v.ToString("#,0", CultureInfo.InvariantCulture); }

    // ------------------------------------------------------------------ static layer

    static void DrawStatic(int step)
    {
        var st = Steps[step];
        g.Clear(Slate);
        g.SmoothingMode = SmoothingMode.AntiAlias;
        g.TextRenderingHint = TextRenderingHint.AntiAliasGridFit;
        g.PixelOffsetMode = PixelOffsetMode.HighQuality;

        // Header: title and the settings plate.
        Text(TitleText, 40, 16, Serif, 38, Ivory, true);
        string settings = (st.Restart ? "after a restart:  " : "settings:  ") + st.Settings;
        float sw = Measure(settings, Mono, 12).Width;
        BrassPlateBox(1570 - sw - 44, 26, sw + 44, 26);
        Engrave(settings, 1570 - sw - 22, 32, Mono, 12);

        // The scene window: a steel frame around the landscape.
        SteelPanel(20, 78, 1560, 578, 10, 11, false);
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

        // Instrument panel.
        SteelPanel(20, PanelY, 1560, PanelH, 10, 23);
        PanelStatic(st);

        // Caption plate.
        Shadow(20, CaptionY, 1560, CaptionH, 8);
        using (var p = Round(20, CaptionY, 1560, CaptionH, 8))
        {
            FillGrad(p, new RectangleF(20, CaptionY, 1560, CaptionH), SlateLight, Slate, 90f);
            Stroke(p, BrassDark, 2f);
        }
        Bevel(21, CaptionY + 1, 1558, CaptionH - 2, 7, 70, 1f);
        string indicator = "STEP " + (step + 1) + " OF " + Steps.Length + "   ·   " + st.Title.ToUpperInvariant();
        Text(indicator, 44, CaptionY + 8, Sans, 13, OptionGold, true);
        Text(st.Caption, 44, CaptionY + 27, Sans, 25, Ivory);

        // Footnote.
        Text(Footnote, 44, 878, Sans, 14, Muted);
    }

    static void Landscape(Step st)
    {
        float crest = Crest(st);
        // Sky.
        FillGradRect(new RectangleF(SceneX, SceneY, SceneR - SceneX, 440), SkyTop, SkyLow, 90f);
        // Distant hills.
        using (var p = new GraphicsPath())
        {
            p.AddBeziers(new[] { new PointF(SceneX, 330), new PointF(300, 250), new PointF(700, 300), new PointF(1000, 280), new PointF(1200, 260), new PointF(1400, 300), new PointF(SceneR, 290) });
            p.AddLine(SceneR, 290, SceneR, 560); p.AddLine(SceneR, 560, SceneX, 560); p.CloseFigure();
            FillGrad(p, new RectangleF(SceneX, 250, SceneR - SceneX, 310), Mix(SkyLow, GrassDark, 0.35), Mix(SkyLow, GrassDark, 0.55), 90f);
        }

        // Earth below the ground profile: the hill, the lake cut, the land, the sea cliff.
        using (var ground = new GraphicsPath())
        {
            ground.AddLines(new[] {
                new PointF(SceneX, HillY), new PointF(LakeX, HillY), new PointF(LakeX, LakeBottom), new PointF(DamX + DamW, LakeBottom),
                new PointF(DamX + DamW, Land), new PointF(SeaX, Land), new PointF(SeaX, SeaBottom), new PointF(SceneR, SeaBottom),
                new PointF(SceneR, SceneB + 10), new PointF(SceneX, SceneB + 10) });
            ground.CloseFigure();
            FillGrad(ground, new RectangleF(SceneX, HillY, SceneR - SceneX, SceneB - HillY + 10), EarthLight, EarthDark, 90f);
            Speckle(ground, new RectangleF(SceneX, HillY, SceneR - SceneX, SceneB - HillY), 31, 2600, 40);
        }
        GrassLine(SceneX, LakeX, HillY);
        GrassLine(DamX + DamW, BasinX - 2, Land);
        GrassLine(BasinX + BasinW + 2, PitX, Land);
        GrassLine(PitR, RiverX, Land);
        GrassLine(SluiceX + SluiceW, Lake2X, Land);
        GrassLine(Lake2R + WeirW, SeaX, Land);
        // Cliff face into the lake, and the lake floor.
        FillGradRect(new RectangleF(LakeX - 7, HillY, 7, LakeBottom - HillY), StoneDark, StoneLight, 0f);
        StoneRow(LakeX - 4, LakeBottom - 2, DamX - LakeX + 4, 7, 37);
        // Channels cut into the land: spill basin, wheel pit, river, sluice slot, second lake.
        Cavity(BasinX, Land - 2, BasinW, BasinDepth + 2, 45);
        Cavity(PitX, Land - 2, PitR - PitX, PitBottom - Land + 2, 47);
        Cavity(RiverX, Land - 2, RiverR - RiverX, RiverBottom - Land + 2, 41);
        Cavity(SluiceX, Land - 2, SluiceW, RiverBottom - Land + 2, 42);
        Cavity(Lake2X, Land - 2, Lake2R - Lake2X, RiverBottom - Land + 2, 43);
        // Spill basin grate.
        for (int i = 1; i < 6; i++) Line(BasinX + i * BasinW / 6, Land + 2, BasinX + i * BasinW / 6, Land + BasinDepth - 2, IronLight, 1.2f);
        // Sea bed.
        FillGradRect(new RectangleF(SeaX, SeaBottom - 12, SceneR - SeaX, 12), EarthDark, StoneDark, 90f);

        // The trough under the tanks, the gate posts and the chute to the lake (etw-consumer).
        Pipe(60, TroughY, GateX - 60 + 4, 14, false);
        FillGradRect(new RectangleF(GateX - 4, TroughY - 46, 5, 60), IronLight, IronDark, 0f);
        FillGradRect(new RectangleF(GateX + GateW - 1, TroughY - 46, 5, 60), IronLight, IronDark, 0f);
        Pipe(GateX + GateW, TroughY, ChuteEnd - GateX - GateW, 14, true);

        // The dam: concrete with a stone crest and a spillway lip that leads over the crest to the right.
        Concrete(DamX, crest, DamW, DamFoot - crest, 51);
        StoneRow(DamX - 3, crest - 8, DamW + 6, 9, 53);
        FillGradRect(new RectangleF(DamX + DamW - 6, crest - 3, 20, 6), SteelLight, SteelDark, 90f);
        // Crest marker: a brass rule along the lake side of the crest.
        Line(LakeX, crest, DamX, crest, A(200, BrassLight), 1.5f);
        // Culvert from the lake floor under the dam to the wheel pit.
        Pipe(DamX - 26, LakeBottom - 2, 14, DamFoot + 4 - (LakeBottom - 2), true);
        Pipe(DamX - 26, DamFoot - 10, PitX - (DamX - 26) + 4, 14, false);
        // Mill house post.
        FillGradRect(new RectangleF(WheelCx - 4, WheelCy, 8, PitBottom - WheelCy), WoodLight, WoodDark, 0f);

        // Sluice posts (dnstap-sender).
        FillGradRect(new RectangleF(SluiceX - 3, BankTop - 56, 5, RiverBottom - BankTop + 56), IronLight, IronDark, 0f);
        FillGradRect(new RectangleF(SluiceX + SluiceW - 2, BankTop - 56, 5, RiverBottom - BankTop + 56), IronLight, IronDark, 0f);
        // Weir at the end of the second lake, and the outlet pipe (dnstap-link) through the land to the sea.
        Concrete(WeirX, BankTop - 4, WeirW, RiverBottom - BankTop + 14, 57);
        Pipe(WeirX + 4, OutletY, OutletR - WeirX - 4, OutletH, true);
        FillGradRect(new RectangleF(1392, OutletY - 24, 6, 24), IronLight, IronDark, 0f);

        // Pond (disk spool) or dry ground.
        if (!st.NoSpool)
        {
            Cavity(PondX, PondTop, PondR - PondX, PondBottom - PondTop, 61);
            StoneRow(PondX - 6, PondTop - 7, PondR - PondX + 12, 8, 62);
            Pipe(SluiceX + 8, RiverBottom, 14, PondTop - RiverBottom + 4, true);
            Pipe(Lake2R - 44, RiverBottom - 6, 14, PondTop - RiverBottom + 10, true);
        }
        else
        {
            StoneRow(PondX - 6, PondTop - 7, PondR - PondX + 12, 8, 62);
            FillGradRect(new RectangleF(PondX, PondTop, PondR - PondX, PondBottom - PondTop), Mix(EarthLight, StoneLight, 0.3), EarthLight, 90f);
            BrassPlate((PondX + PondR) / 2, PondTop + 14, "--no-spool: dry ground, no disk spool", 12, Mono);
        }

        // Name plates.
        BrassPlate(215, HillY + 12, "ETW buffers  ·  one tank = --etw-buffer-kib " + (st.TierTanks ? "128" : "32"), 13, Mono);
        BrassPlate(290, HillY + 40, "solid: --etw-min-buffers 12  ·  ghost sockets: up to --etw-max-buffers " + st.MaxBuffers + "  ·  not to scale", 12, Mono);
        BrassPlate(700, 246, "flush gate  ·  --etw-flush-seconds 1", 13, Mono);
        BrassPlate(700, 272, "etw-consumer: copies each event into the channel, never waits", 12, Sans);
        BrassPlate(694, crest - 26, "crest: --capacity " + st.Capacity, 11, Mono);
        BrassPlate(685, LakeBottom + 24, "event channel (bounded, newest event dropped when full)", 12, Sans);
        BrassPlate(846, Land - 60, "dnstap-parser", 11, Mono);
        BrassPlate(1006, Land - 54, "send queue  ·  256 blocks", 11, Mono);
        BrassPlate(SluiceX + SluiceW / 2, BankTop - 122, "dnstap-sender (router sluice)", 12, Mono);
        BrassPlate(1250, Land - 30, "link FIFO  ·  256 blocks", 11, Mono);
        BrassPlate(1395, OutletY - 82, "dnstap-link", 12, Mono);
        BrassPlate((SeaX + SceneR) / 2, SeaLevel - 34, "TCP receiver (sea)", 13, Sans);

        // Storm plate right of the cloud.
        BrassPlate(520, 104, "rain = dns.exe events  ·  " + st.Storm, 13, Sans);
        if (st.Banner)
        {
            float bw = Measure("Illustration, not a lab result", Sans, 16, true).Width + 60;
            float bx = 870 - bw / 2;
            Shadow(bx, 92, bw, 34, 6);
            using (var p = Round(bx, 92, bw, 34, 6)) { FillGrad(p, new RectangleF(bx, 92, bw, 34), C(170, 50, 40), C(110, 28, 22), 90f); Stroke(p, BrassMid, 2f); }
            TextCenter("Illustration, not a lab result", 870, 100, Sans, 16, Ivory, true);
        }
    }

    // A channel cut into the earth: a dark cavity with stone walls and a stone floor.
    static void Cavity(float x, float y, float w, float h, int seed)
    {
        using (var p = Round(x, y, w, h, 3))
        {
            FillGrad(p, new RectangleF(x, y, w, h), C(52, 46, 40), C(26, 24, 22), 90f);
            Speckle(p, new RectangleF(x, y, w, h), seed, (int)(w * h / 40), 40);
        }
        FillGradRect(new RectangleF(x - 4, y, 4, h), StoneDark, StoneLight, 0f);
        FillGradRect(new RectangleF(x + w, y, 4, h), StoneLight, StoneDark, 0f);
        StoneRow(x - 4, y + h - 3, w + 8, 6, seed + 1);
    }

    static void GrassLine(float x1, float x2, float y)
    {
        if (x2 <= x1) return;
        FillGradRect(new RectangleF(x1, y - 5, x2 - x1, 7), GrassLight, GrassDark, 90f);
    }

    // The instrument panel: six gauges, six odometers, three lamps. Faces are static, needles move.
    static readonly string[] GaugeNames = { "ETW Buffers In Use", "% ETW Buffers Used", "Channel Depth", "Send Queue Peak Depth", "Spool Bytes", "Bytes Sent/sec" };
    static readonly string[] OdoNames = { "Events Received", "Events Encoded", "ETW Buffers Delivered", "ETW Buffers Allocated", "Channel Peak Depth", "Spool Segments" };
    static readonly string[] LampNames = { "Receiver Connected", "Link Saturated", "Spool Available" };
    const float GaugeR = 38, GaugeY = PanelY + 46, GaugeX0 = 86, GaugePitch = 140;
    const float OdoX0 = 870, OdoColW = 270, OdoRowH = 36, OdoY0 = PanelY + 14, OdoLabelW = 134, OdoBoxW = 120;
    const float LampX = 1432, LampY0 = PanelY + 24, LampPitch = 32;

    static void PanelStatic(Step st)
    {
        for (int i = 0; i < 6; i++)
        {
            float cx = GaugeX0 + i * GaugePitch;
            GaugeFace(cx, GaugeY, GaugeR, null);
            using (var p = Round(cx - 56, GaugeY + GaugeR + 4, 112, 15, 3)) { Fill(p, C(26, 26, 30)); Stroke(p, A(160, BrassDark), 1f); }
            BrassPlate(cx, PanelY + 106, GaugeNames[i], 11, Sans, false, 6);
        }
        for (int i = 0; i < 6; i++)
        {
            float x = OdoX0 + (i / 3) * OdoColW, y = OdoY0 + (i % 3) * OdoRowH;
            BrassPlateBox(x, y + 3, OdoLabelW, 24);
            Engrave(OdoNames[i], x + 10, y + 8, Sans, 10.5f);
            OdometerBox(x + OdoLabelW + 6, y, OdoBoxW, 30);
        }
        for (int i = 0; i < 3; i++)
        {
            float y = LampY0 + i * LampPitch;
            BrassPlateBox(LampX + 16, y - 12, 124, 24);
            Engrave(LampNames[i], LampX + 28, y - 7, Sans, 11);
        }
        // Illustrative values tag.
        BrassPlate(1500, PanelY + 106, "illustrative values", 10, Sans, false, 8);
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
        PanelDynamic(s, st);
    }

    static void Scenery(State s, Step st)
    {
        double t = s.T;
        float crest = Crest(st);
        float th = TankH(st), tw = TankW(st), pitch = TankPitch(st);
        float rowA = SolidRowY(st), rowB = GhostRowY(st);
        int ghosts = Ghosts(st);

        // Cloud and rain. The rain falls on the tank bank.
        Cloud(245, 118, 30, s.Rain > 0.5);
        Rain(TankX0 - 6, 138, SolidTanks * pitch + 6, rowA - 140, t, s.Rain);
        // Lost drops beside the tanks (step 4 only).
        if (s.LostDrops > 0)
        {
            LostDrops(SceneX + 6, TankX0 - 14 - SceneX, 140, HillY - 4, t, s.LostDrops);
            LostDrops(TankX0 + SolidTanks * pitch + 2, GateX - 8 - (TankX0 + SolidTanks * pitch + 2), 140, HillY - 4, t, s.LostDrops);
        }

        // Tanks: the solid row and the ghost row.
        for (int i = 0; i < SolidTanks + ghosts; i++)
        {
            bool solidRow = i < SolidTanks;
            int col = solidRow ? i : i - SolidTanks;
            float x = TankX0 + col * pitch, y = solidRow ? rowA : rowB;
            bool dug = solidRow || col < s.Dug;
            TankFrame(x, y, tw, th, dug);
            if (dug)
            {
                TankWater(x, y, tw, th, s.Tank[i], t);
                Glass(x, y, tw, th);
            }
        }
        // Drips from the tanks into the trough: a short stream on the trough when the gate is open.
        Stream(66, TroughY + 3, GateX - 66, 8, t, 0.4 + 0.6 * s.GateOpen, true, 50);

        // The flush gate: plate lifts between the posts; the hand-wheel turns with it.
        float lift = (float)(s.GateOpen * 18);
        FillGradRect(new RectangleF(GateX, TroughY - 10 - lift, GateW, 26), IronLight, IronDark, 0f);
        for (int i = 1; i < 4; i++) Line(GateX + 3, TroughY - 10 - lift + i * 6, GateX + GateW - 3, TroughY - 10 - lift + i * 6, A(90, PureBlack), 1f);
        FillGradRect(new RectangleF(GateX + GateW / 2 - 2, TroughY - 46, 4, 36 - lift), SteelLight, SteelDark, 0f);
        HandWheel(GateX + GateW / 2, TroughY - 50, 12, s.GateOpen * 2.5);
        // Water through the chute (etw-consumer) and the fall into the lake.
        Stream(GateX + GateW, TroughY + 3, ChuteEnd - GateX - GateW, 8, t, s.Chute, true, 90);
        float lakeSurface = LakeBottom - (float)(s.Lake * (LakeBottom - crest));
        Fall(ChuteEnd - 4, 8, TroughY + 10, lakeSurface, t, s.Chute);

        // The lake behind the dam.
        Water(LakeX, DamX - LakeX, lakeSurface, LakeBottom, t, 1.4, 0.8);
        // Spill over the crest: a sheet down the dam face into the basin.
        if (s.Spill > 0.01)
        {
            Stream(DamX + 6, crest - 5, DamW + 10, 5, t, s.Spill, true, 80);
            Fall(DamX + DamW + 8, 10, crest - 3, Land + BasinDepth - 6, t, s.Spill);
            Water(BasinX + 2, BasinW - 4, Land + 4, Land + BasinDepth - 2, t, 0.6, 2, 230, false);
        }
        // Culvert flow arrows and the wheel pit.
        Water(PitX, PitR - PitX, PitBottom - 14, PitBottom, t, 0.8, 1.6, 255, false);
        MillWheel(WheelCx, WheelCy, WheelR, s.WheelAngle);
        // Splash at the wheel foot.
        FoamBlobs(WheelCx - 20, PitBottom - 12, 40, t, 0.6);

        // The river (send queue).
        float riverSurface = RiverBottom - (float)(s.River * (RiverBottom - BankTop));
        Water(RiverX, RiverR - RiverX, riverSurface, RiverBottom, t, 1.2, 1.0, 255, false);
        Stream(RiverX, riverSurface + 2, RiverR - RiverX, Math.Max(2, Math.Min(10, RiverBottom - riverSurface - 4)), t, s.RiverFlow * 0.6, true, 50 + 60 * (float)s.RiverFlow);
        // Blocks that fall over the river bank at the mill end (step 9).
        if (s.RiverDrop > 0.01)
        {
            for (int i = 0; i < 4; i++)
            {
                double ph = Frac(t * 0.9 + i * 0.25);
                float bx = RiverX + 12 + i * 16 - (float)(ph * 24), by = BankTop - 12 + (float)(ph * ph * 100);
                Shadow(bx, by, 10, 8, 2, 2, 3, 50);
                FillGradRect(new RectangleF(bx, by, 10, 8), WaterTop, WaterDeep, 45f);
            }
            FoamBlobs(RiverX - 20, RiverBottom + 34, 60, t, s.RiverDrop);
        }

        // The sluice (dnstap-sender): the plate across the river mouth, the hand-wheel above.
        float sluiceLift = s.SluiceClosed ? 0 : (float)(RiverBottom - BankTop) * 0.8f;
        FillGradRect(new RectangleF(SluiceX + 2, BankTop - 6 - sluiceLift, SluiceW - 4, RiverBottom - BankTop + 6), IronLight, IronDark, 0f);
        for (int i = 1; i < 6; i++) Line(SluiceX + 5, BankTop - 6 - sluiceLift + i * 9, SluiceX + SluiceW - 5, BankTop - 6 - sluiceLift + i * 9, A(90, PureBlack), 1f);
        FillGradRect(new RectangleF(SluiceX + SluiceW / 2 - 2, BankTop - 56, 4, 50 - sluiceLift), SteelLight, SteelDark, 0f);
        HandWheel(SluiceX + SluiceW / 2, BankTop - 60, 13, s.SluiceClosed ? 0.4 : (s.SluiceToPond ? 1.2 : 0));
        // Where the sluice sends the water: straight on, or down the spill pipe into the pond.
        if (!s.SluiceClosed && !s.SluiceToPond)
            Stream(SluiceX, riverSurface + 2, SluiceW, Math.Max(2, Math.Min(10, RiverBottom - riverSurface - 4)), t, s.RiverFlow * 0.6, true, 80);
        if (s.SpillPipe > 0.01 && !st.NoSpool)
            Stream(SluiceX + 11, RiverBottom + 4, 8, PondTop - RiverBottom, t, s.SpillPipe, false, 70);
        // A small direction plate on the sluice.
        string dir = s.SluiceClosed ? "closed" : s.SluiceToPond ? "to the pond" : "to the link FIFO";
        BrassPlate(SluiceX + SluiceW / 2, BankTop - 96, dir, 11, Sans, false, 6);

        // The second lake (link FIFO).
        float lake2Surface = RiverBottom - (float)(s.Lake2 * (RiverBottom - BankTop));
        Water(Lake2X, Lake2R - Lake2X, lake2Surface, RiverBottom, t, 1.2, 0.9);
        // Replay pipe flow from the pond head up into the second lake.
        if (s.ReplayFlow > 0.01 && !st.NoSpool)
            Stream(Lake2R - 37, RiverBottom - 4, 8, PondTop - RiverBottom + 8, t, s.ReplayFlow, false, 70);
        // The outlet (dnstap-link): flow through the pipe, a valve wheel, and the fall into the sea.
        Stream(WeirX + WeirW, OutletY + 3, OutletR - WeirX - WeirW, OutletH - 6, t, s.Outlet, true, 40 + 80 * (float)s.Outlet);
        HandWheel(1395, OutletY - 30, 12, (1 - s.Valve) * 3);
        Fall(OutletR - 2, 8, OutletY + 4, SeaLevel + 2, t, s.Outlet);

        // The sea.
        Water(SeaX, SceneR - SeaX, SeaLevel, SeaBottom, t, 2.2, 0.6);

        // The pond (disk spool).
        if (!st.NoSpool)
        {
            float pondSurface = PondBottom - 2 - (float)(s.Pond * (PondBottom - PondTop - 4));
            Water(PondX + 3, PondR - PondX - 6, pondSurface, PondBottom - 2, t, 1.0, 0.7, 205);
            if (s.SpillPipe > 0.01) Fall(SluiceX + 11, 8, PondTop + 4, pondSurface, t, s.SpillPipe);
            // The name plate is drawn after the water so that the pond never covers it.
            BrassPlate((PondX + PondR) / 2 + 14, PondBottom - 28, "disk spool (pond)  ·  --spool-max-mib 4096", 11, Mono);
        }

        // Loss counters by their items.
        Counter(150, HillY + 72, "etw_lost", s.EtwLost, s.EtwLost > 0);
        Counter(150, HillY + 98, "etw_buffers_lost", 0L, false);
        Counter(770, DamFoot + 24, "ring_dropped", s.RingDropped, s.RingDropped > 0);
        Counter(975, Land - 88, "queue_dropped", s.QueueDropped, s.QueueDropped > 0);
        Counter(SluiceX + SluiceW / 2, BankTop - 148, "disconnected_dropped", s.DisconnectedDropped, false);
        if (!st.NoSpool)
        {
            Counter(980, PitBottom + 7, "spooled", s.Spooled, false);
            Counter(980, PitBottom + 30, "replayed", s.Replayed, false);
            Counter(980, PitBottom + 53, "spool_dropped", 0, false);
        }

        // Event log tag at the top right of the scene.
        EventTag(s);

        // Step 2: the detail inset of one tank and the two gate triggers.
        if (s.Step == 1) Inset(s);
    }

    

    // A small loss counter: a brass name plate with an odometer window beside it.
    static void Counter(float cx, float y, string name, long value, bool alarm)
    {
        float nameW = Measure(name, Mono, 11).Width + 24;
        float boxW = 84;
        float total = nameW + 6 + boxW;
        float x = cx - total / 2;
        BrassPlateBox(x, y, nameW, 19);
        Engrave(name, x + 12, y + 3, Mono, 11);
        OdometerBox(x + nameW + 6, y - 1, boxW, 21);
        OdometerDigits(x + nameW + 6, y - 1, boxW, 21, N(value), 12);
        if (alarm) Lamp(x + nameW + 6 + boxW + 10, y + 9, 4, LampRed, true);
    }

    static void EventTag(State s)
    {
        float x = 1130, y = 94, w = 436, h = 32;
        BrassPlateBox(x, y, w, h);
        Lamp(x + 20, y + h / 2, 6, s.EventOn ? s.EventColor : LampOff, s.EventOn);
        Engrave("event log:", x + 36, y + 9, Sans, 12, true);
        string text = s.EventOn ? s.Event : "no event";
        float px = 12;
        while (Measure(text, Sans, px).Width > w - 120 && px > 9) px -= 0.5f;
        Engrave(text, x + 112, y + 9 + (12 - px) / 2, Sans, px);
    }

    // Step 2: one tank in detail, with the float trigger and the timer trigger of the flush gate.
    static void Inset(State s)
    {
        float x = 1140, y = 136, w = 420, h = 210;
        SteelPanel(x, y, w, h, 8, 71);
        using (var p = Round(x + 8, y + 8, w - 16, h - 16, 5))
        {
            FillGrad(p, new RectangleF(x, y, w, h), C(60, 66, 76), C(36, 40, 48), 90f);
            Stroke(p, A(200, IronDark), 1f);
        }
        TextCenter("detail: one ETW tank and the two gate triggers", x + w / 2, y + 12, Sans, 13, Ivory, true);
        float tx = x + 34, ty = y + 40, tw = 56, th = 118;
        float gx = x + 128, py = ty + th - 18;
        // Outlet pipe from the tank foot, through the gate, on to the channel.
        Pipe(tx + tw, py, gx - tx - tw, 10, false);
        Pipe(gx + 20, py, 70, 10, false);
        Stream(gx + 20, py + 2, 70, 6, s.T, s.InsetGate, true, 60);
        TankFrame(tx, ty, tw, th, true);
        TankWater(tx, ty, tw, th, s.InsetLevel, s.T);
        // The float: a brass ball on the water, a rod up through the lid to the lever.
        float surface = ty + 2 + (th - 4) * (float)(1 - s.InsetFloat);
        float fx = tx + tw / 2;
        FillGradRect(new RectangleF(fx - 1.5f, ty - 10, 3, surface - ty + 8), SteelLight, SteelDark, 0f);
        Sphere(fx, surface - 2, 6, BrassLight, BrassDark);
        Glass(tx, ty, tw, th);
        BrassPlate(fx, ty + th + 10, "32 KiB", 12, Mono);
        // The gate: posts, plate and stem. The lever from the float lifts the stem when the tank is full.
        FillGradRect(new RectangleF(gx - 4, py - 44, 4, 56), IronLight, IronDark, 0f);
        FillGradRect(new RectangleF(gx + 20, py - 44, 4, 56), IronLight, IronDark, 0f);
        float lift = (float)(s.InsetGate * 22);
        FillGradRect(new RectangleF(gx, py - 12 - lift, 20, 24), IronLight, IronDark, 0f);
        FillGradRect(new RectangleF(gx + 8, py - 44, 4, 32 - lift), SteelLight, SteelDark, 0f);
        Line(fx, ty - 10, gx + 10, py - 44 - lift * 0.3f, IronMid, 2.5f);
        Sphere(fx, ty - 10, 3, SteelLight, IronDark);
        Text("float: full tank", gx + 30, py - 48, Sans, 11, Ivory);
        Text("to the channel", gx + 28, py + 16, Sans, 11, Ivory);
        // The clock.
        float cx = x + 325, cy = y + 104, cr = 38;
        GaugeFace(cx, cy, cr, null);
        for (int i = 0; i < 12; i++)
        {
            double a = i * Math.PI / 6;
            Line(cx + (float)(Math.Sin(a) * 23), cy - (float)(Math.Cos(a) * 23), cx + (float)(Math.Sin(a) * 27), cy - (float)(Math.Cos(a) * 27), Ink, 1.5f);
        }
        double hand = s.InsetClock * 2 * Math.PI;
        Line(cx, cy, cx + (float)(Math.Sin(hand) * 23), cy - (float)(Math.Cos(hand) * 23), NeedleRed, 2.2f);
        Sphere(cx, cy, 3.5f, SteelLight, IronDark);
        GaugeGlass(cx, cy, cr);
        TextCenter("timer: the gate opens once a second", cx, y + 42, Sans, 11, Ivory);
        BrassPlate(cx, cy + 46, "--etw-flush-seconds 1", 11, Mono);
    }

    // ------------------------------------------------------------------ panel: needles, digits, lamps

    static void PanelDynamic(State s, Step st)
    {
        double[] values = {
            s.InUse / (double)st.MaxBuffers,
            s.InUse / (double)st.MaxBuffers,
            Clamp01(s.ChannelDepth / st.Capacity),
            s.SendPeak / 256.0,
            s.SpoolBytes / (4096.0 * 1024 * 1024),
            s.BytesPerSec / 100.0 };
        string[] readings = {
            s.InUse + " of " + st.MaxBuffers,
            Math.Round(100.0 * s.InUse / st.MaxBuffers) + " %",
            N((long)s.ChannelDepth) + " of " + N(st.Capacity),
            N(s.SendPeak) + " blocks",
            N(s.SpoolBytes / (1024 * 1024)) + " MiB",
            N((long)s.BytesPerSec) + " MB/s" };
        for (int i = 0; i < 6; i++)
        {
            float cx = GaugeX0 + i * GaugePitch;
            GaugeNeedle(cx, GaugeY, GaugeR, values[i]);
            GaugeGlass(cx, GaugeY, GaugeR);
            TextCenter(readings[i], cx, GaugeY + GaugeR + 6, Mono, 11, Ivory, true);
        }
        string[] odo = { N(s.Received), N(s.Encoded), N(s.Delivered), s.Allocated + " of " + st.MaxBuffers, N(s.ChannelPeak), N(s.SpoolSegments) };
        for (int i = 0; i < 6; i++)
        {
            float x = OdoX0 + (i / 3) * OdoColW + OdoLabelW + 6, y = OdoY0 + (i % 3) * OdoRowH;
            OdometerDigits(x, y, OdoBoxW, 30, odo[i], 15);
        }
        bool[] on = { s.Connected, s.Saturated, s.SpoolAvailable };
        Color[] colors = { LampGreen, LampAmber, LampGreen };
        for (int i = 0; i < 3; i++) Lamp(LampX, LampY0 + i * LampPitch, 7, colors[i], on[i]);
    }

    // ------------------------------------------------------------------ reports

    public static void Report()
    {
        double s0 = 0;
        for (int i = 0; i < Steps.Length; i++)
        {
            var st = Steps[i];
            double lakeMax = 0, riverMax = 0, pondMax = 0, lake2Max = 0;
            State last = null;
            for (double t = s0; t < s0 + st.Seconds; t += 1.0 / Fps)
            {
                var s = Compute(t);
                lakeMax = Math.Max(lakeMax, s.Lake); riverMax = Math.Max(riverMax, s.River); pondMax = Math.Max(pondMax, s.Pond); lake2Max = Math.Max(lake2Max, s.Lake2);
                last = s;
            }
            Console.WriteLine("Step " + (i + 1) + " " + st.Title + ": " + st.Seconds + " s, lake " + lakeMax.ToString("0.00") + ", river " + riverMax.ToString("0.00") + ", link FIFO " + lake2Max.ToString("0.00") + ", pond " + pondMax.ToString("0.00")
                + ", received " + N(last.Received) + ", etw_lost " + N(last.EtwLost) + ", ring_dropped " + N(last.RingDropped) + ", queue_dropped " + N(last.QueueDropped) + ", spooled " + N(last.Spooled) + ", replayed " + N(last.Replayed));
            s0 += st.Seconds;
        }
        Console.WriteLine("Loop " + Loop + " s, " + (int)(Loop * Fps) + " frames at " + Fps + " fps");
    }
}
