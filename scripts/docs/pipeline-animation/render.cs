// Renders the frames of the ETW-to-TCP pipeline animation with GDI+.
// Output: one raw file, "frames.bin", that make_animation.py turns into a GIF.
//
// File layout (little endian): "PIPEFRM1", width u32, height u32, frame count u32,
// 256 palette entries (R, G, B), then one byte per pixel for every frame.
//
// The animation is a pure function of the time t, so every frame is repeatable.
// The stages match docs/design/data-path.md. Stage 6 shows a receiver outage:
// frames go to the disk spool and are replayed after the reconnect. All times
// are slowed: they are not to scale.

using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Drawing.Text;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

public static class PipelineAnimation
{
    const int Width = 1000, Height = 720;
    const int Fps = 8;
    const double Loop = 36.0;     // seconds: six stages of six seconds each
    const double Period = 6.0;    // one ETW delivery cycle, slowed
    const int Cycles = 6;
    const int PerCycle = 5;       // events in each cycle
    const int TransparentIndex = 255;

    static readonly Color Bg = Color.FromArgb(11, 18, 32), Panel = Color.FromArgb(22, 36, 59),
        White = Color.FromArgb(237, 244, 255), Muted = Color.FromArgb(163, 181, 208),
        Blue = Color.FromArgb(106, 159, 255), Green = Color.FromArgb(66, 221, 175),
        Red = Color.FromArgb(255, 113, 128), Amber = Color.FromArgb(255, 202, 105),
        Line = Color.FromArgb(69, 86, 113), Purple = Color.FromArgb(193, 160, 255);

    // Five boxes in a row. The spine is the horizontal line that events travel on.
    const float BoxW = 140, BoxGap = 55, BoxY = 205, BoxH = 280, Spine = 330;
    static float Left(int i) { return 40 + i * (BoxW + BoxGap); }
    static float Right(int i) { return Left(i) + BoxW; }

    static readonly string[] StageNames = { "Produce", "Intake", "Buffer", "Parse", "Send", "Spool" };
    static readonly string[] StageTitle = {
        "The DNS Server writes events 256, 257, and 258 to an ETW session.",
        "The ETW callback checks each event and copies it into a pool slot.",
        "A bounded channel holds the records. A full channel drops the newest event.",
        "The parser decodes each record in one pass and encodes it as dnstap.",
        "The sender thread writes Frame Streams over TCP. TLS is on by default.",
        "While the receiver is down, frames go to the disk spool." };
    static readonly string[] StageSub = {
        "ETW keeps the events in session buffers. It delivers them about once per second.",
        "It checks the provider, event ID, and process ID. It does not wait or allocate.",
        "After each delivered ETW buffer, the buffer callback wakes the parser once.",
        "It packs the frames into 64 KiB blocks and queues them for the sender thread.",
        "The sender owns the receiver link and the spool. The parser never waits on the network.",
        "After the reconnect, the backlog is sent first, oldest first. No frame is sent twice." };

    static Graphics g;
    static Color[] palette;
    static readonly Dictionary<int, byte> lookup = new Dictionary<int, byte>();

    // ---------------------------------------------------------------- timeline

    sealed class Ev
    {
        public int Cycle, J, Slot;
        public bool Foreign;
        public double Born, Deliver, CopiedAt, Enqueued, Pull, Released, End;
        public double FlushDepart;   // when the frame leaves the sink buffer
        public double ReceivedAt;    // when the frame reaches the receiver
        public bool Spooled;         // the frame waited in the disk spool
        public int SpoolCell;
        public double SpoolAt;       // when the frame lands in the spool
        public double ReplayDepart;  // when the frame leaves the spool
        public readonly List<KeyValuePair<double, PointF>> Path = new List<KeyValuePair<double, PointF>>();
        public readonly List<KeyValuePair<double, PointF>> OutPath = new List<KeyValuePair<double, PointF>>();

        public void Key(double t, float x, float y) { Path.Add(new KeyValuePair<double, PointF>(t, new PointF(x, y))); }

        public static PointF Along(List<KeyValuePair<double, PointF>> path, double t)
        {
            if (t <= path[0].Key) return path[0].Value;
            for (int i = 1; i < path.Count; i++)
            {
                if (t <= path[i].Key)
                {
                    double span = path[i].Key - path[i - 1].Key;
                    double u = span <= 0 ? 1 : (t - path[i - 1].Key) / span;
                    PointF a = path[i - 1].Value, b = path[i].Value;
                    return new PointF((float)(a.X + (b.X - a.X) * u), (float)(a.Y + (b.Y - a.Y) * u));
                }
            }
            return path[path.Count - 1].Value;
        }
    }

    static readonly List<Ev> events = new List<Ev>();
    static readonly List<double> wakeStarts = new List<double>();

    const double WakeTravel = 0.9;
    static PointF EtwCell(int j) { return new PointF(Left(0) + 18 + j * 26, BoxY + 176); }
    static PointF SlotCell(int slot)
    {
        int col = slot % 6, row = slot / 6;
        return new PointF(Left(2) + 17.5f + col * 21, BoxY + 166.5f + row * 21);
    }
    static float ReceiverTop { get { return BoxY + 205; } }

    // The receiver outage in stage 6, and the replay pace after the reconnect.
    const double DownAt = 30.3, UpAt = 33.0, ReplayStep = 0.25;
    const int SpoolCells = 5;
    static PointF SpoolCellPt(int i) { return new PointF(Left(4) + 22 + i * 14, BoxY + 164); }
    static bool ReceiverDown(double t) { return t >= DownAt && t < UpAt; }

    static void BuildTimeline()
    {
        for (int c = 0; c < Cycles; c++)
        {
            double b = c * Period;
            bool foreignCycle = c == 1;
            var pooled = new List<Ev>();
            double lastCopied = 0;
            for (int j = 0; j < PerCycle; j++)
            {
                var e = new Ev { Cycle = c, J = j, Foreign = foreignCycle && j == 2 };
                e.Born = b + 0.2 + 0.65 * j;
                e.Deliver = b + 3.5 + 0.35 * j;
                double td = e.Deliver;
                e.CopiedAt = td + 1.35;
                PointF cell = EtwCell(j);
                e.Key(e.Born, Left(0) + 38, Spine);
                e.Key(e.Born + 0.4, cell.X, cell.Y);
                e.Key(td, cell.X, cell.Y);
                e.Key(td + 0.4, Right(0), Spine);
                e.Key(td + 0.75, Left(1), Spine);
                if (e.Foreign)
                {
                    e.Key(td + 1.35, Left(1) + 114, Spine);
                    e.Key(td + 1.8, Left(1) + 114, Spine + 44);
                    e.End = td + 1.8;
                }
                else
                {
                    e.Key(td + 1.45, Left(1) + 132, Spine);
                    e.Key(td + 1.55, Right(1), Spine);
                    e.Key(td + 1.9, Left(2), Spine);
                    pooled.Add(e);
                    lastCopied = Math.Max(lastCopied, td + 1.45);
                }
                events.Add(e);
            }
            double wakeStart = lastCopied;
            wakeStarts.Add(wakeStart);
            double wakeArrive = wakeStart + WakeTravel;
            int k = 0;
            double lastArrival = 0;
            foreach (var e in pooled)
            {
                e.Slot = (c * PerCycle + k) % 24;
                PointF cell = SlotCell(e.Slot);
                e.Enqueued = e.Deliver + 2.2;
                e.Key(e.Enqueued, cell.X, cell.Y);
                e.Pull = wakeArrive + 0.9 * k;
                e.Key(e.Pull, cell.X, cell.Y);
                e.Key(e.Pull + 0.3, Right(2), Spine);
                e.Key(e.Pull + 0.65, Left(3), Spine);
                e.Key(e.Pull + 0.8, Left(3) + 36, Spine);
                e.Key(e.Pull + 1.1, Left(3) + 36, Spine);
                e.Key(e.Pull + 1.2, Left(3) + 104, Spine);
                e.Key(e.Pull + 1.5, Left(3) + 104, Spine);
                e.Released = e.Pull + 1.5;
                e.Key(e.Pull + 1.6, Right(3), Spine);
                e.Key(e.Pull + 1.95, Left(4), Spine);
                e.Key(e.Pull + 2.1, Left(4) + 36, Spine);
                e.End = e.Pull + 2.1;
                lastArrival = e.End;
                k++;
            }
            double flush = lastArrival + 0.4;
            int f = 0;
            foreach (var e in pooled)
            {
                e.FlushDepart = flush + 0.15 * f;
                f++;
            }
        }
        RouteFrames();
    }

    // The sender writes a frame to the link, or to the spool while the receiver
    // is down or a backlog is still being replayed (so frames stay in order).
    static void RouteFrames()
    {
        var sent = new List<Ev>();
        foreach (var e in events) if (!e.Foreign) sent.Add(e);
        sent.Sort((a, b) => a.FlushDepart.CompareTo(b.FlushDepart));
        double backlogUntil = -1, nextReplay = UpAt + 0.4;
        int cell = 0;
        PointF block = new PointF(Left(4) + 36, Spine);
        PointF link = new PointF(Left(4) + 104, Spine);
        PointF receiver = new PointF(Left(4) + 122, ReceiverTop + 4);
        foreach (var e in sent)
        {
            double d = e.FlushDepart;
            e.OutPath.Add(new KeyValuePair<double, PointF>(d, block));
            if (!ReceiverDown(d) && d >= backlogUntil)
            {
                e.OutPath.Add(new KeyValuePair<double, PointF>(d + 0.3, link));
                e.OutPath.Add(new KeyValuePair<double, PointF>(d + 0.65, receiver));
                e.ReceivedAt = d + 0.65;
                continue;
            }
            e.Spooled = true;
            e.SpoolCell = cell++ % SpoolCells;
            PointF slot = SpoolCellPt(e.SpoolCell);
            e.OutPath.Add(new KeyValuePair<double, PointF>(d + 0.15, new PointF(block.X, Spine + 20)));
            e.OutPath.Add(new KeyValuePair<double, PointF>(d + 0.5, slot));
            e.SpoolAt = d + 0.5;
            e.ReplayDepart = Math.Max(nextReplay, e.SpoolAt + 0.3);
            nextReplay = e.ReplayDepart + ReplayStep;
            backlogUntil = e.ReplayDepart;
            e.OutPath.Add(new KeyValuePair<double, PointF>(e.ReplayDepart, slot));
            e.OutPath.Add(new KeyValuePair<double, PointF>(e.ReplayDepart + 0.35, link));
            e.OutPath.Add(new KeyValuePair<double, PointF>(e.ReplayDepart + 0.85, receiver));
            e.ReceivedAt = e.ReplayDepart + 0.85;
        }
    }

    // --------------------------------------------------------------- primitives

    static Font F(float size, bool bold)
    {
        return new Font("Segoe UI", size, bold ? FontStyle.Bold : FontStyle.Regular, GraphicsUnit.Pixel);
    }

    static void Text(string s, float x, float y, float size, Color color, bool bold = false)
    {
        using (var f = F(size, bold)) using (var b = new SolidBrush(color))
            g.DrawString(s, f, b, x, y, StringFormat.GenericTypographic);
    }

    static void Center(string s, float cx, float y, float size, Color color, bool bold = false)
    {
        using (var f = F(size, bold)) using (var b = new SolidBrush(color)) using (var sf = new StringFormat())
        {
            sf.Alignment = StringAlignment.Center;
            g.DrawString(s, f, b, cx, y, sf);
        }
    }

    static void Box(float x, float y, float w, float h, Color fill, Color stroke, float r = 14, float strokeWidth = 1.5f)
    {
        using (var p = new GraphicsPath())
        {
            p.AddArc(x, y, r * 2, r * 2, 180, 90);
            p.AddArc(x + w - 2 * r, y, 2 * r, 2 * r, 270, 90);
            p.AddArc(x + w - 2 * r, y + h - 2 * r, 2 * r, 2 * r, 0, 90);
            p.AddArc(x, y + h - 2 * r, 2 * r, 2 * r, 90, 90);
            p.CloseFigure();
            using (var b = new SolidBrush(fill)) g.FillPath(b, p);
            if (stroke.A > 0) using (var pen = new Pen(stroke, strokeWidth)) g.DrawPath(pen, p);
        }
    }

    static Color Mix(Color a, Color b, double u)
    {
        return Color.FromArgb((int)(a.R + (b.R - a.R) * u), (int)(a.G + (b.G - a.G) * u), (int)(a.B + (b.B - a.B) * u));
    }

    static void Rule(float x1, float y1, float x2, float y2, Color color, float width = 3, bool arrow = false)
    {
        using (var p = new Pen(color, width))
        {
            if (arrow) p.CustomEndCap = new AdjustableArrowCap(4, 4);
            g.DrawLine(p, x1, y1, x2, y2);
        }
    }

    static void Square(PointF p, float size, Color fill, Color outline)
    {
        using (var b = new SolidBrush(fill)) g.FillRectangle(b, p.X - size / 2, p.Y - size / 2, size, size);
        if (outline.A > 0) using (var pen = new Pen(outline, 1.5f)) g.DrawRectangle(pen, p.X - size / 2, p.Y - size / 2, size, size);
    }

    static void Triangle(PointF p, float size, Color fill)
    {
        using (var b = new SolidBrush(fill))
            g.FillPolygon(b, new[] { new PointF(p.X + size, p.Y), new PointF(p.X - size * 0.8f, p.Y - size * 0.9f), new PointF(p.X - size * 0.8f, p.Y + size * 0.9f) });
    }

    static void Cross(PointF p, float r, Color color)
    {
        using (var pen = new Pen(color, 3))
        {
            g.DrawLine(pen, p.X - r, p.Y - r, p.X + r, p.Y + r);
            g.DrawLine(pen, p.X - r, p.Y + r, p.X + r, p.Y - r);
        }
    }

    static void Chip(float x, float y, float w, float h, string label, bool lit, Color color, double fill = 0)
    {
        Box(x, y, w, h, Bg, lit ? color : Line, 8, lit ? 2f : 1.5f);
        if (fill > 0)
        {
            using (var b = new SolidBrush(Mix(Panel, color, 0.55)))
                g.FillRectangle(b, x + 3, y + h - 8, (float)((w - 6) * Math.Min(1, fill)), 5);
        }
        Center(label, x + w / 2, y + (h - 15) / 2 - (fill > 0 ? 2 : 0), 13, lit ? color : Muted, true);
    }

    // ------------------------------------------------------------------- scene

    static int Count(Func<Ev, bool> test)
    {
        int n = 0;
        foreach (var e in events) if (test(e)) n++;
        return n;
    }

    static void Header(int stage)
    {
        Text("ETW TO DNSTAP", 40, 26, 42, White, true);
        Text("DNS events travel from ETW to a TCP receiver. A disk spool covers outages.", 42, 82, 22, Muted);
        for (int i = 0; i < 6; i++)
        {
            float x = 40 + i * 155;
            bool active = i == stage;
            Box(x, 128, 143, 48, active ? Panel : Bg, active ? Blue : Line, 10);
            Text((i + 1).ToString("00"), x + 14, 140, 19, active ? Blue : Muted, true);
            Text(StageNames[i], x + 53, 139, 20, active ? White : Muted, active);
        }
    }

    static void NodeFrame(int i, string title, string status, Color statusColor, bool active)
    {
        Box(Left(i), BoxY, BoxW, BoxH, Panel, active ? Blue : Line, 14, active ? 2.5f : 1.5f);
        Text(title, Left(i) + 14, BoxY + 13, 22, White, true);
        Text(status, Left(i) + 14, BoxY + 44, 14, statusColor, true);
    }

    static void Boxes(int stage, double t)
    {
        for (int i = 0; i < 4; i++)
        {
            Color c = i == 0 ? Blue : (i == 3 ? Green : Purple);
            Rule(Right(i), Spine, Left(i + 1) - 1, Spine, c, 3, true);
        }
        NodeFrame(0, "DNS Server", "ETW producer", Blue, stage == 0);
        NodeFrame(1, "Intake", "ETW callback", Blue, stage == 1);
        NodeFrame(2, "Buffer", "pool + channel", Purple, stage == 2);
        NodeFrame(3, "Parser", "decode + encode", Green, stage == 3);
        NodeFrame(4, "Sender", "dnstap-sender", Green, stage >= 4);
        for (int i = 0; i < 4; i++) Rule(Left(i) + 6, Spine, Right(i), Spine, Line, 3);
        Rule(Left(4) + 6, Spine, Left(4) + 104, Spine, Line, 3);

        // 1. DNS Server
        float x0 = Left(0);
        Text("Events 256-258", x0 + 14, BoxY + 68, 14, Blue, true);
        Text("DNS Server provider", x0 + 14, BoxY + 86, 13, Muted);
        bool born = false;
        foreach (var e in events) if (t >= e.Born && t < e.Born + 0.4) born = true;
        Chip(x0 + 8, Spine - 16, 60, 32, "dns.exe", born, Blue);
        Text("ETW session buffers", x0 + 14, BoxY + 146, 13, Muted);
        for (int j = 0; j < PerCycle; j++)
        {
            PointF c = EtwCell(j);
            bool full = false;
            foreach (var e in events)
                if (e.J == j && t >= e.Born + 0.4 && t < e.Deliver) full = true;
            Box(c.X - 10, c.Y - 8, 20, 16, full ? Mix(Panel, Blue, 0.45) : Bg, full ? Blue : Line, 4, 1.5f);
        }
        double inCycle = t - Math.Floor(t / Period) * Period;
        double timer = inCycle < 3.5 ? inCycle / 3.5 : 1;
        Text("Flush timer", x0 + 14, BoxY + 200, 12, Muted);
        Box(x0 + 14, BoxY + 220, 112, 8, Bg, Line, 3, 1.2f);
        using (var b = new SolidBrush(Blue)) g.FillRectangle(b, x0 + 15, BoxY + 221, (float)(110 * timer), 6);
        Text("etw_lost", x0 + 14, BoxY + 246, 13, Muted);
        Text("0", x0 + 76, BoxY + 246, 14, Green, true);

        // 2. Intake
        x0 = Left(1);
        Text("Checks, then copies.", x0 + 14, BoxY + 68, 12, Muted);
        Text("No wait. No allocation.", x0 + 14, BoxY + 86, 12, Muted);
        string[] checks = { "Provider", "Event ID", "PID" };
        float[] cx = { x0 + 26, x0 + 70, x0 + 114 };
        for (int k = 0; k < 3; k++)
        {
            bool ok = false, bad = false;
            foreach (var e in events)
            {
                if (t >= e.Deliver + 0.75 && t < e.Deliver + 1.55)
                {
                    PointF p = Ev.Along(e.Path, t);
                    if (p.X >= cx[k] - 2 && p.X <= Right(1))
                    {
                        if (e.Foreign && k == 2) bad = true; else ok = true;
                    }
                }
                if (e.Foreign && k == 2 && t >= e.Deliver + 1.35 && t < e.Deliver + 1.8) bad = true;
            }
            Color col = bad ? Red : (ok ? Green : Line);
            using (var b = new SolidBrush(bad || ok ? Mix(Panel, col, 0.4) : Bg)) g.FillEllipse(b, cx[k] - 11, Spine - 11, 22, 22);
            using (var pen = new Pen(col, 2)) g.DrawEllipse(pen, cx[k] - 11, Spine - 11, 22, 22);
            if (bad) Cross(new PointF(cx[k], Spine), 4, Red);
            else if (ok)
                using (var pen = new Pen(Green, 2.5f))
                    g.DrawLines(pen, new[] { new PointF(cx[k] - 5, Spine), new PointF(cx[k] - 1, Spine + 4), new PointF(cx[k] + 5, Spine - 4) });
            Center(checks[k], cx[k], Spine + 17, 11, bad ? Red : Muted, false);
        }
        Text("Copy into slot", x0 + 14, BoxY + 160, 14, Purple, true);
        Text("received", x0 + 14, BoxY + 196, 13, Muted);
        Text(Count(e => !e.Foreign && t >= e.CopiedAt).ToString(), x0 + 76, BoxY + 196, 14, White, true);
        Text("foreign_pid_rejected", x0 + 14, BoxY + 232, 11, Muted);
        Text(Count(e => e.Foreign && t >= e.CopiedAt).ToString(), x0 + 14, BoxY + 247, 14, Red, true);

        // 3. Buffer
        x0 = Left(2);
        Text("24,576 events", x0 + 14, BoxY + 68, 13, Muted);
        Text("2 KiB slots", x0 + 14, BoxY + 86, 13, Muted);
        Text("Full: drop newest", x0 + 14, BoxY + 142, 12, Red);
        var state = new int[24];  // 0 free, 1 queued, 2 leased by the parser
        foreach (var e in events)
        {
            if (e.Foreign) continue;
            if (t >= e.Enqueued && t < e.Pull) state[e.Slot] = 1;
            else if (t >= e.Pull && t < e.Released) state[e.Slot] = 2;
        }
        for (int s = 0; s < 24; s++)
        {
            PointF c = SlotCell(s);
            Color fill = state[s] == 1 ? Purple : Bg;
            Color stroke = state[s] == 0 ? Line : Purple;
            Box(c.X - 8.5f, c.Y - 8.5f, 17, 17, fill, stroke, 3, 1.5f);
        }
        int queued = Count(e => !e.Foreign && t >= e.Enqueued && t < e.Pull);
        Text("queued", x0 + 14, BoxY + 255, 13, Muted);
        Text(queued.ToString(), x0 + 70, BoxY + 255, 14, White, true);

        // 4. Parser
        x0 = Left(3);
        Text("Compiled schema plan", x0 + 14, BoxY + 68, 12, Muted);
        Text("reads each field once", x0 + 14, BoxY + 86, 12, Muted);
        bool decoding = false, encoding = false;
        foreach (var e in events)
        {
            if (e.Foreign) continue;
            if (t >= e.Pull + 0.8 && t < e.Pull + 1.1) decoding = true;
            if (t >= e.Pull + 1.2 && t < e.Pull + 1.5) encoding = true;
        }
        Chip(x0 + 8, Spine - 16, 56, 32, "Decode", decoding, Green);
        Chip(x0 + 76, Spine - 16, 56, 32, "Encode", encoding, Green);
        Text("Then release slot", x0 + 14, BoxY + 160, 14, Purple, true);
        Text("CLIENT_QUERY", x0 + 14, BoxY + 184, 13, Green, true);
        Text("CLIENT_RESPONSE", x0 + 14, BoxY + 201, 13, Green, true);
        Text("into 64 KiB blocks", x0 + 14, BoxY + 224, 12, Muted);
        Text("encoded", x0 + 14, BoxY + 250, 13, Muted);
        Text(Count(e => !e.Foreign && t >= e.Released).ToString(), x0 + 70, BoxY + 250, 14, White, true);

        // 5. Sender thread: block queue, receiver link, and disk spool
        x0 = Left(4);
        bool down = ReceiverDown(t);
        Text("spooled", x0 + 14, BoxY + 68, 13, Muted);
        Text(Count(e => e.Spooled && t >= e.SpoolAt).ToString(), x0 + 86, BoxY + 68, 14, Amber, true);
        Text("replayed", x0 + 14, BoxY + 86, 13, Muted);
        Text(Count(e => e.Spooled && t >= e.ReplayDepart).ToString(), x0 + 86, BoxY + 86, 14, White, true);
        int buffered = Count(e => !e.Foreign && t >= e.End && t < e.FlushDepart);
        bool sending = Count(e => !e.Foreign && t < e.ReceivedAt
            && t >= (e.Spooled ? e.ReplayDepart : e.FlushDepart)) > 0;
        Chip(x0 + 8, Spine - 16, 56, 32, "64 KiB", buffered > 0, Green, buffered / 5.0);
        Chip(x0 + 76, Spine - 16, 56, 32, "TCP", sending || down, down ? Red : Green);
        if (down) Cross(new PointF(x0 + 104, Spine + 26), 5, Red);

        bool spooling = Count(e => e.Spooled && ((t >= e.FlushDepart && t < e.SpoolAt + 0.15)
            || (t >= e.ReplayDepart - 0.05 && t < e.ReplayDepart + 0.35))) > 0;
        Box(x0 + 8, BoxY + 150, 78, 48, Bg, spooling ? Amber : Line, 8, spooling ? 2f : 1.5f);
        for (int i = 0; i < SpoolCells; i++)
        {
            PointF c = SpoolCellPt(i);
            bool held = Count(e => e.Spooled && e.SpoolCell == i && t >= e.SpoolAt && t < e.ReplayDepart) > 0;
            Box(c.X - 5, c.Y - 5, 10, 10, held ? Amber : Bg, held ? Amber : Line, 2, 1.2f);
        }
        Text("Disk spool", x0 + 15, BoxY + 176, 12, Amber, true);

        int got = Count(e => !e.Foreign && t >= e.ReceivedAt);
        bool receiving = Count(e => !e.Foreign && t >= e.ReceivedAt - 0.05 && t < e.ReceivedAt + 0.3) > 0;
        Color edge = down ? Red : (receiving ? Green : Line);
        Box(x0 + 8, ReceiverTop, 124, 56, Bg, edge, 8, down || receiving ? 2f : 1.5f);
        Center("dnstap receiver", x0 + 70, ReceiverTop + 10, 13, White, true);
        Center(down ? "down" : "frames " + got, x0 + 70, ReceiverTop + 32, 13,
            down ? Red : (receiving ? Green : Muted), down);
    }

    static void ThreadStrip(double t)
    {
        Text("dns.exe and ETW", Left(0), 490, 12, Muted);
        Text("etw-consumer thread", Left(1), 490, 12, Blue);
        Text("shared memory", Left(2), 490, 12, Purple);
        Text("dnstap-parser thread", Left(3), 490, 12, Green);
        Text("dnstap-sender thread", Left(4), 490, 12, Green);
        using (var p = new SolidBrush(Line)) g.FillRectangle(p, Left(0), 509, BoxW, 3);
        using (var p = new SolidBrush(Blue)) g.FillRectangle(p, Left(1), 509, BoxW, 3);
        using (var p = new SolidBrush(Purple)) g.FillRectangle(p, Left(2), 509, BoxW, 3);
        using (var p = new SolidBrush(Green)) g.FillRectangle(p, Left(3), 509, BoxW, 3);
        using (var p = new SolidBrush(Green)) g.FillRectangle(p, Left(4), 509, BoxW, 3);

        // The buffer callback on the consumer thread wakes the parser.
        float wx1 = Left(1) + 128, wx2 = Left(3) + 128, wy = 545;
        var path = new[] { new PointF(wx1, 485), new PointF(wx1, wy), new PointF(wx2, wy), new PointF(wx2, 487) };
        using (var pen = new Pen(Mix(Bg, Amber, 0.45), 2))
        {
            pen.CustomEndCap = new AdjustableArrowCap(4, 4);
            g.DrawLines(pen, path);
        }
        Center("Buffer callback wakes the parser: once for each delivered ETW buffer", (wx1 + wx2) / 2, wy + 6, 13, Amber);
        foreach (double start in wakeStarts)
        {
            double u = (t - start) / WakeTravel;
            if (u < 0 || u > 1) continue;
            double down = wy - 485, across = wx2 - wx1, len = down * 2 + across, d = u * len;
            float x, y;
            if (d < down) { x = wx1; y = 485 + (float)d; }
            else if (d < down + across) { x = wx1 + (float)(d - down); y = wy; }
            else { x = wx2; y = wy - (float)(d - down - across); }
            using (var b = new SolidBrush(Amber)) g.FillEllipse(b, x - 5, y - 5, 10, 10);
            if (u > 0.93) using (var pen = new Pen(Amber, 2)) g.DrawEllipse(pen, wx2 - 12, 487 - 12, 24, 24);
        }
    }

    static void Tokens(double t)
    {
        foreach (var e in events)
        {
            if (t < e.Born) continue;
            if (!e.Foreign && t >= e.FlushDepart && t <= e.ReceivedAt)
            {
                // A frame that waits in the spool is drawn as a filled spool cell.
                if (!(e.Spooled && t >= e.SpoolAt && t < e.ReplayDepart))
                    Triangle(Ev.Along(e.OutPath, t), 7, Green);
                continue;
            }
            if (t > e.End) continue;
            PointF p = Ev.Along(e.Path, t);
            if (e.Foreign && t >= e.CopiedAt) Square(p, 10, Red, Color.Transparent);
            else if (t < e.CopiedAt) Square(p, t >= e.Born + 0.4 && t < e.Deliver ? 9 : 10, Blue, Color.Transparent);
            else if (t < e.Released) Square(p, 12, Purple, White);
            else Triangle(p, 7, Green);
        }
    }

    static void Caption(int stage, double t)
    {
        Box(40, 617, 920, 70, Panel, Color.Transparent, 12);
        Text(StageTitle[stage], 60, 627, 22, White, true);
        Text(StageSub[stage], 60, 660, 17, Muted);
        Text("Events slowed. Timing and counter values are illustrative, not to scale.", 40, 697, 13, Muted);
        Text("src/etw, src/pipeline, src/sender, src/spool", 700, 697, 13, Muted);
        using (var b = new SolidBrush(Blue)) g.FillRectangle(b, 0, 717, (float)(Width * t / Loop), 3);
    }

    static void Legend()
    {
        const float y = 586;
        Square(new PointF(45, y + 11), 10, Blue, Color.Transparent);
        Text("ETW event", 60, y, 16, Muted);
        Square(new PointF(198, y + 11), 12, Purple, White);
        Text("Pooled record", 214, y, 16, Muted);
        Triangle(new PointF(380, y + 11), 7, Green);
        Text("dnstap frame", 394, y, 16, Muted);
        using (var b = new SolidBrush(Amber)) g.FillEllipse(b, 548, y + 6, 10, 10);
        Text("Wake signal", 566, y, 16, Muted);
        Cross(new PointF(722, y + 11), 5, Red);
        Text("Rejected or dropped", 738, y, 16, Muted);
    }

    static void Frame(double t)
    {
        int stage = Math.Min(5, (int)(t / 6.0));
        g.Clear(Bg);
        g.SmoothingMode = SmoothingMode.AntiAlias;
        g.TextRenderingHint = TextRenderingHint.AntiAliasGridFit;
        Header(stage);
        ThreadStrip(t);
        Boxes(stage, t);
        Tokens(t);
        Legend();
        Caption(stage, t);
    }

    // ------------------------------------------------------------ GIF palette

    static void BuildPalette()
    {
        var colors = new List<Color>();
        Color[] bases = { Bg, Panel, White, Muted, Blue, Green, Red, Amber, Line, Purple };
        foreach (var c in bases) colors.Add(c);
        foreach (var c in bases)
            foreach (var back in new[] { Bg, Panel })
                for (int v = 1; v <= 12; v++)
                    colors.Add(Color.FromArgb(back.R + (c.R - back.R) * v / 13, back.G + (c.G - back.G) * v / 13, back.B + (c.B - back.B) * v / 13));
        while (colors.Count < 255) colors.Add(Bg);
        colors.Add(Color.FromArgb(255, 0, 255));   // index 255: transparent in the GIF
        palette = colors.ToArray();
    }

    static byte Nearest(int r, int gr, int b)
    {
        int key = r << 16 | gr << 8 | b;
        byte value;
        if (lookup.TryGetValue(key, out value)) return value;
        int min = int.MaxValue;
        value = 0;
        for (int c = 0; c < TransparentIndex; c++)
        {
            int dr = r - palette[c].R, dg = gr - palette[c].G, db = b - palette[c].B;
            int dist = dr * dr + dg * dg + db * db;
            if (dist < min) { min = dist; value = (byte)c; }
        }
        lookup[key] = value;
        return value;
    }

    static byte[] Indexed(Bitmap image)
    {
        var rect = new Rectangle(0, 0, image.Width, image.Height);
        var data = image.LockBits(rect, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        byte[] rgb = new byte[data.Stride * data.Height];
        Marshal.Copy(data.Scan0, rgb, 0, rgb.Length);
        image.UnlockBits(data);
        byte[] index = new byte[image.Width * image.Height];
        for (int y = 0; y < image.Height; y++)
            for (int x = 0; x < image.Width; x++)
            {
                int k = y * data.Stride + x * 4;
                index[y * image.Width + x] = Nearest(rgb[k + 2], rgb[k + 1], rgb[k]);
            }
        return index;
    }

    public static void Render(string outDir)
    {
        Directory.CreateDirectory(outDir);
        BuildTimeline();
        BuildPalette();
        int frames = (int)(Loop * Fps);
        int[] previews = { 12, 60, 92, 124, 152, 200, 210, 232, 246, 254, 262, 272, 280 };
        string previewDir = Path.Combine(outDir, "previews");
        Directory.CreateDirectory(previewDir);
        using (var image = new Bitmap(Width, Height, PixelFormat.Format32bppArgb))
        using (g = Graphics.FromImage(image))
        using (var stream = new FileStream(Path.Combine(outDir, "frames.bin"), FileMode.Create))
        using (var writer = new BinaryWriter(stream))
        {
            writer.Write(Encoding.ASCII.GetBytes("PIPEFRM1"));
            writer.Write((uint)Width);
            writer.Write((uint)Height);
            writer.Write((uint)frames);
            foreach (var c in palette) { writer.Write(c.R); writer.Write(c.G); writer.Write(c.B); }
            for (int i = 0; i < frames; i++)
            {
                Frame(i / (double)Fps);
                writer.Write(Indexed(image));
                if (Array.IndexOf(previews, i) >= 0)
                    image.Save(Path.Combine(previewDir, "frame-" + i.ToString("D3") + ".png"), ImageFormat.Png);
                if (i % 40 == 0) Console.WriteLine("Rendered " + i + " / " + frames);
            }
        }
        Console.WriteLine("Wrote " + Path.Combine(outDir, "frames.bin"));
    }
}
