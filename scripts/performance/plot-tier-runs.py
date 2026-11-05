"""Render lab-run graphs: one PNG per run and one summary PNG per QPS tier.

Input is a folder per run, ``<runs-root>/<run-id>/``, with ``run.json`` and a
one-row-per-second ``timeseries.csv`` (see the README for the columns).  Output
is ``<run-id>.png`` for every run and ``tier-<qps>.png`` for every tier.

Loss percent is lost events divided by events offered: the events the service received
(the sum of ``events_received_per_s``, which includes events dropped later) plus the events
ETW lost before delivery (``etw_lost``).
``etw_buffers_lost`` counts buffers, not events, so it is drawn in the run
graph but left out of every percentage.  ``spooled`` is diverted, not lost, and
is also left out.  Loss columns are cumulative totals; the graphs show the
per-second increment.  Empty cells are missing samples: a gap in a cumulative
column puts its increment on the next sample that has a value.
"""
from __future__ import annotations

import argparse
import csv
import json
import math
import os
import sys
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
os.environ.setdefault("MPLCONFIGDIR", str(ROOT / "target/performance/matplotlib-cache"))
import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

NAN = float("nan")

# Okabe-Ito.  One color per series, the same in every chart.
C_DNS, C_SVC = "#D55E00", "#000000"
C_STAGE = {"etw": "#0072B2", "parser": "#009E73", "sender": "#E69F00", "link": "#CC79A7"}
STAGE_LABEL = {"etw": "ETW consumer", "parser": "Parser", "sender": "Sender", "link": "Link"}
STAGE_COL = {k: f"cpu_{k}_pct" for k in C_STAGE}

# Failure points: key -> (label, event columns, color).  Order is pipeline order.
POINTS = {
    "etw": ("ETW session", ["etw_lost"], "#0072B2"),
    "channel": ("Event channel", ["ring_dropped"], "#56B4E9"),
    "pool": ("Buffer pool", ["pool_exhausted"], "#E69F00"),
    "decode": ("Decode", ["rejected"], "#CC79A7"),
    "network": ("Network", ["queue_dropped", "disconnected_dropped"], "#D55E00"),
    "spool": ("Spool", ["spool_dropped"], "#009E73"),
}
C_SPOOLED, C_ETW_BUF, C_GUIDE, C_EVENTS = "#7F7F7F", "#0072B2", "#999999", "#999999"
LOSS_COLS = ["etw_lost", "etw_buffers_lost", "ring_dropped", "pool_exhausted", "rejected",
             "queue_dropped", "disconnected_dropped", "spool_dropped", "spooled"]
COLUMNS = ["time_s", "qps_sent", "events_received_per_s", "dns_cpu_pct", "service_cpu_pct",
           *STAGE_COL.values(), *LOSS_COLS, "channel_depth", "channel_capacity",
           "send_queue_depth", "send_queue_capacity", "etw_buffers_allocated",
           "etw_buffers_in_use", "etw_buffers_max", "rx_mbps"]
WARN_CHANNEL_SEND, WARN_ETW = 80, 90

PLOT_PARAMS = {
    "font.family": "DejaVu Sans", "font.size": 11, "axes.titlesize": 13,
    "axes.spines.top": False, "axes.spines.right": False, "axes.grid": True,
    "grid.color": "#E6E6E6", "grid.linewidth": 0.8, "figure.facecolor": "white",
    "axes.facecolor": "white", "axes.axisbelow": True, "legend.frameon": False, "legend.fontsize": 10,
}
PNG_META = {"Software": None}
DPI = 100


@dataclass
class Run:
    """One run: metadata plus column arrays (NaN for empty cells)."""

    meta: dict
    cols: dict[str, np.ndarray] = field(default_factory=dict)

    @property
    def id(self) -> str:
        return str(self.meta["id"])

    @property
    def n(self) -> int:
        return len(self.cols["time_s"])


def _num(text: str | None) -> float:
    text = (text or "").strip()
    if not text:
        return NAN
    try:
        value = float(text)
    except ValueError:
        return NAN
    return value if math.isfinite(value) else NAN


def load_run(folder: Path) -> Run:
    """Read ``run.json`` and ``timeseries.csv``.

    Unknown columns are ignored; absent columns read as all-missing."""
    meta = json.loads((folder / "run.json").read_text(encoding="utf-8"))
    with (folder / "timeseries.csv").open(newline="", encoding="utf-8-sig") as stream:
        rows = list(csv.DictReader(stream))
    if not rows:
        raise ValueError(f"{folder.name}: timeseries.csv has no rows")
    cols = {name: np.array([_num(r.get(name)) for r in rows]) for name in COLUMNS}
    if np.isnan(cols["time_s"]).any():
        raise ValueError(f"{folder.name}: time_s has an empty or invalid cell")
    return Run(meta, cols)


def increments(cumulative: np.ndarray) -> np.ndarray:
    """Per-sample increments of a cumulative counter that starts at 0.

    A missing sample stays NaN; its increment lands on the next present sample.
    A drop (counter reset) counts as 0."""
    out = np.full(cumulative.shape, NAN)
    previous = 0.0
    for i, value in enumerate(cumulative):
        if math.isnan(value):
            continue
        out[i] = max(value - previous, 0.0)
        previous = value
    return out


def total(cumulative: np.ndarray) -> float:
    """Run total of a cumulative counter: its last present value (0 if none)."""
    present = cumulative[~np.isnan(cumulative)]
    return float(present[-1]) if present.size else 0.0


def pct(value: np.ndarray, capacity: np.ndarray) -> np.ndarray:
    """Percent of capacity; NaN where the capacity is missing or zero."""
    with np.errstate(divide="ignore", invalid="ignore"):
        return np.where(capacity > 0, 100.0 * value / capacity, NAN)


def _nanstat(fn, values: np.ndarray) -> float:
    values = values[~np.isnan(values)]
    return float(fn(values)) if values.size else NAN


def summarize(run: Run) -> dict:
    """Numbers shared by the run graph title and the tier summary."""
    c = run.cols
    point_totals = {k: sum(total(c[col]) for col in cols) for k, (_, cols, _) in POINTS.items()}
    sent = float(np.nansum(c["events_received_per_s"])) + total(c["etw_lost"])
    lost = sum(point_totals.values())

    def percent(events: float) -> float:
        return 100.0 * events / sent if sent > 0 else 0.0

    start = float(run.meta.get("load_start_s", c["time_s"][0]))
    end = float(run.meta.get("load_end_s", c["time_s"][-1]))
    window = (c["time_s"] >= start) & (c["time_s"] <= end)
    if not window.any():
        window = np.ones(run.n, dtype=bool)
    return {
        "id": run.id,
        "sent": sent,
        "lost": lost,
        "loss_pct": percent(lost),
        "point_totals": point_totals,
        "point_pct": {k: percent(v) for k, v in point_totals.items()},
        "etw_buffers_lost": total(c["etw_buffers_lost"]),
        "spooled": total(c["spooled"]),
        "dns_mean": _nanstat(np.mean, c["dns_cpu_pct"][window]),
        "dns_max": _nanstat(np.max, c["dns_cpu_pct"][window]),
        "service_mean": _nanstat(np.mean, c["service_cpu_pct"][window]),
        "stage_mean": {k: _nanstat(np.mean, c[col][window]) for k, col in STAGE_COL.items()},
    }


def _settings_text(meta: dict) -> str:
    s = meta.get("settings", {})
    capacity = s.get("capacity", "?")
    if isinstance(capacity, (int, float)):
        capacity = f"{capacity:,}"
    return (f"ETW buffers {s.get('etw_buffer_kib', '?')} KiB, "
            f"{s.get('etw_min_buffers', '?')} to {s.get('etw_max_buffers', '?')}; "
            f"channel capacity {capacity}")


def _shade(ax, run: Run) -> None:
    start, end = run.meta.get("load_start_s"), run.meta.get("load_end_s")
    if start is not None and end is not None:
        ax.axvspan(start, end, color="#F1F1F1", zorder=0, lw=0)


def render_run(run: Run, out: Path) -> dict:
    """Write the three-panel PNG for one run and return its summary."""
    info = summarize(run)
    c, t = run.cols, run.cols["time_s"]
    with plt.rc_context(PLOT_PARAMS):
        fig, (cpu, loss, fill) = plt.subplots(
            3, 1, figsize=(16, 12), sharex=True, gridspec_kw={"height_ratios": [1.2, 1, 1]})
        fig.subplots_adjust(top=0.88, bottom=0.07, left=0.07, right=0.90, hspace=0.12)
        qps = int(run.meta.get("tier_qps", 0))
        fig.suptitle(
            f"{run.id}: {qps:,} QPS, {run.meta.get('busy_threads', '?')} busy threads, "
            f"{run.meta.get('vcpus', '?')} vCPUs", x=0.07, y=0.975, ha="left",
            fontsize=16, weight="bold")
        fig.text(0.07, 0.935, f"{_settings_text(run.meta)}  |  "
                 f"total loss {info['loss_pct']:.3f}% of {info['sent']:,.0f} events offered",
                 fontsize=11.5, color="#333333")
        for ax in (cpu, loss, fill):
            _shade(ax, run)

        # Panel 1: CPU
        cpu.plot(t, c["dns_cpu_pct"], color=C_DNS, lw=1.8, label="dns.exe")
        cpu.plot(t, c["service_cpu_pct"], color=C_SVC, lw=1.8, label="EtwDnstap total")
        for key, col in STAGE_COL.items():
            cpu.plot(t, c[col], color=C_STAGE[key], lw=1.2, label=STAGE_LABEL[key])
        cpu.set_ylabel("CPU (% of one logical CPU)")
        cpu.set_ylim(bottom=0)
        events = cpu.twinx()
        events.spines["right"].set_visible(True)
        events.grid(False)
        events.plot(t, c["events_received_per_s"], color=C_EVENTS, lw=0.8,
                    label="Events received/s (right axis)")
        events.set_ylabel("Events received per second")
        events.set_ylim(bottom=0)
        h1, l1 = cpu.get_legend_handles_labels()
        h2, l2 = events.get_legend_handles_labels()
        cpu.legend(h1 + h2, l1 + l2, ncol=4, loc="lower left", bbox_to_anchor=(0, 1.0),
                   borderaxespad=0.2)

        # Panel 2: loss per second
        series = []
        for _, (label, cols, color) in POINTS.items():
            for col in cols:
                tot = total(c[col])
                if tot > 0:
                    series.append((f"{label}: {col} ({tot:,.0f})", increments(c[col]),
                                   color, "-"))
        if info["etw_buffers_lost"] > 0:
            series.append((f"ETW session: etw_buffers_lost ({info['etw_buffers_lost']:,.0f} "
                           "buffers)", increments(c["etw_buffers_lost"]), C_ETW_BUF, ":"))
        if info["spooled"] > 0:
            series.append((f"Spooled, diverted not lost ({info['spooled']:,.0f})",
                           increments(c["spooled"]), C_SPOOLED, "--"))
        for label, y, color, style in series:
            loss.plot(t, y, color=color, ls=style, lw=1.6, label=label)
        loss.set_ylabel("Events per second")
        loss.set_title("Loss per second at each failure point", loc="left", fontsize=12)
        if series:
            loss.set_ylim(bottom=0)
            loss.set_ylim(top=loss.get_ylim()[1] * 1.3)
            loss.legend(loc="upper left", ncol=2)
        else:
            loss.set_ylim(0, 1)
            loss.text(0.5, 0.5, "No loss", transform=loss.transAxes, ha="center",
                      va="center", fontsize=22, color="#555555")

        # Panel 3: fill
        fill.plot(t, pct(c["channel_depth"], c["channel_capacity"]),
                  color=POINTS["channel"][2], lw=1.6, label="Event channel depth")
        fill.plot(t, pct(c["send_queue_depth"], c["send_queue_capacity"]),
                  color=POINTS["network"][2], lw=1.6, label="Send queue depth")
        fill.plot(t, pct(c["etw_buffers_in_use"], c["etw_buffers_max"]),
                  color=C_ETW_BUF, lw=1.6, label="ETW buffers in use (% of max)")
        fill.plot(t, pct(c["etw_buffers_allocated"], c["etw_buffers_max"]),
                  color=C_ETW_BUF, lw=1.4, ls="--", label="ETW buffers allocated (% of max)")
        for level, text, align in (
                (WARN_CHANNEL_SEND, "80% channel and\nsend queue warning", "top"),
                (WARN_ETW, "90% ETW buffers\nwarning", "bottom")):
            fill.axhline(level, color=C_GUIDE, lw=1.0, ls=(0, (1, 3)))
            fill.text(1.005, level, text, transform=fill.get_yaxis_transform(),
                      fontsize=8.5, color="#555555", va=align)
        fill.set_ylim(0, 125)
        fill.set_yticks([0, 20, 40, 60, 80, 100])
        fill.set_ylabel("Fill (% of capacity)")
        fill.set_xlabel("Time (s)")
        fill.legend(loc="upper left", ncol=2)
        fill.set_xlim(t.min(), t.max())

        out.parent.mkdir(parents=True, exist_ok=True)
        fig.savefig(out, dpi=DPI, metadata=PNG_META)
        plt.close(fig)
    return info


def render_tier(qps: int, entries: list[tuple[dict, dict]], out: Path) -> None:
    """Write the summary PNG for one tier.  ``entries`` is [(summary, run meta)]."""
    entries = sorted(entries, key=lambda e: e[0]["id"])
    labels = [f"{i['id']}\n{m.get('busy_threads', '?')} busy" for i, m in entries]
    x = np.arange(len(entries))

    def nz(v: float) -> float:
        return 0.0 if math.isnan(v) else v

    with plt.rc_context(PLOT_PARAMS):
        fig, (a, b, d) = plt.subplots(3, 1, figsize=(16, 12))
        fig.subplots_adjust(top=0.92, bottom=0.08, left=0.07, right=0.97, hspace=0.45)
        fig.suptitle(f"{qps:,} QPS tier: {len(entries)} runs", x=0.07, y=0.975,
                     ha="left", fontsize=16, weight="bold")
        w = 0.38
        a.bar(x - w / 2, [nz(i["dns_mean"]) for i, _ in entries], w, color=C_DNS,
              alpha=0.55, label="dns.exe mean")
        a.bar(x + w / 2, [nz(i["dns_max"]) for i, _ in entries], w, color=C_DNS,
              label="dns.exe max")
        a.set_ylabel("CPU (% of one logical CPU)")
        a.set_title("dns.exe CPU during the load window", loc="left", fontsize=12)
        a.set_ylim(top=a.get_ylim()[1] * 1.2)
        a.legend(loc="upper left")

        bottom = np.zeros(len(entries))
        for key in C_STAGE:
            vals = np.array([nz(i["stage_mean"][key]) for i, _ in entries])
            b.bar(x, vals, 0.6, bottom=bottom, color=C_STAGE[key], label=STAGE_LABEL[key])
            bottom += vals
        b.plot(x, [nz(i["service_mean"]) for i, _ in entries], ls="none", marker="_",
               ms=34, mew=2.5, color=C_SVC, label="EtwDnstap total (measured)")
        b.set_ylabel("CPU (% of one logical CPU)")
        b.set_title("EtwDnstap mean CPU by pipeline stage, load window", loc="left",
                    fontsize=12)
        b.set_ylim(top=b.get_ylim()[1] * 1.2)
        b.legend(loc="upper left", ncol=5)

        bottom = np.zeros(len(entries))
        for key, (label, _, color) in POINTS.items():
            vals = np.array([i["point_pct"][key] for i, _ in entries])
            d.bar(x, vals, 0.6, bottom=bottom, color=color, label=label)
            bottom += vals
        for xi, (i, _) in zip(x, entries):
            d.text(xi, i["loss_pct"], f"{i['loss_pct']:.3f}%", ha="center", va="bottom",
                   fontsize=10)
        d.set_ylabel("Lost events (% of offered)")
        d.set_ylim(0, max(1.0, float(bottom.max()) * 1.3))
        d.set_title("Loss by failure point", loc="left", fontsize=12)
        d.legend(loc="upper left", ncol=6)
        for ax in (a, b, d):
            ax.set_xticks(x, labels)
            ax.grid(axis="x", visible=False)
        out.parent.mkdir(parents=True, exist_ok=True)
        fig.savefig(out, dpi=DPI, metadata=PNG_META)
        plt.close(fig)


def render_all(runs_root: Path, out_dir: Path, tier: int | None = None) -> list[Path]:
    """Render every run under ``runs_root``; return the PNG paths written."""
    folders = sorted(p for p in runs_root.iterdir() if (p / "run.json").is_file())
    if not folders:
        raise SystemExit(f"no run folders (run.json) under {runs_root}")
    tiers: dict[int, list[tuple[dict, dict]]] = defaultdict(list)
    written: list[Path] = []
    for folder in folders:
        run = load_run(folder)
        qps = int(run.meta["tier_qps"])
        if tier is not None and qps != tier:
            continue
        path = out_dir / f"{run.id}.png"
        tiers[qps].append((render_run(run, path), run.meta))
        written.append(path)
    for qps, entries in sorted(tiers.items()):
        path = out_dir / f"tier-{qps}.png"
        render_tier(qps, entries, path)
        written.append(path)
    if not written:
        raise SystemExit(f"no runs in tier {tier}")
    return written


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--runs", type=Path, required=True, help="folder of run folders")
    parser.add_argument("--out", type=Path, required=True, help="output folder for PNGs")
    parser.add_argument("--tier", type=int, help="render only this tier_qps")
    args = parser.parse_args(argv)
    for path in render_all(args.runs, args.out, args.tier):
        print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
