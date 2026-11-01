"""Render matched native WPR CPU and client QPS series as static SVGs."""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import xml.etree.ElementTree as ET
from perf_common import sha256_file as digest

ROOT = Path(__file__).resolve().parents[2]
os.environ.setdefault("MPLCONFIGDIR", str(ROOT / "target/performance/matplotlib-cache"))
import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.ticker import StrMethodFormatter

RUNS = {
    "pooled-auth-1000-300s-2": ("Authoritative baseline", 1000, "PASS"),
    "pooled-rec-1000-300s-2": ("Recursive baseline", 1000, "FAIL: one RA=false response"),
    "pooled-auth-48000-300s-1": ("Authoritative high-rate confirmation", 48000, "PASS"),
    "pooled-rec-28000-300s-1": ("Recursive high-rate confirmation", 28000, "PASS"),
}


def render(run_id, source, output, previews):
    with source.open(newline="", encoding="utf-8-sig") as stream:
        rows = list(csv.DictReader(stream))
    values = {key: [float(row[key]) for row in rows] for key in (
        "elapsed_s", "interval_start_s", "interval_end_s",
        "service_cpu_percent_one_core", "dns_cpu_percent_one_core", "achieved_qps",
        "requested_qps")}
    assert len(rows) == 290, (run_id, len(rows))
    assert values["interval_start_s"] == list(range(5, 295))
    assert values["interval_end_s"] == list(range(6, 296))
    title, requested, status = RUNS[run_id]
    assert all(value == requested for value in values["requested_qps"])
    assert all(float(row["interval_start_s"]) <= float(row["elapsed_s"]) <=
               float(row["interval_end_s"]) for row in rows)
    assert all(value >= 0 for key, series in values.items() for value in series)
    fig, (cpu, qps) = plt.subplots(2, 1, figsize=(12, 7.2), sharex=True,
                                   gridspec_kw={"height_ratios": [1.2, 1]})
    fig.subplots_adjust(top=.82, bottom=.14, left=.10, right=.97, hspace=.18)
    fig.suptitle(title, x=.10, y=.97, ha="left", fontsize=17, weight="bold")
    fig.text(.10, .915, f"{run_id}  |  requested {requested:,} QPS  |  2 vCPUs / 4 GiB",
             fontsize=10.5)
    fig.text(.10, .875, f"Strict stage: {status}  |  zero DNS delivery loss and capture loss",
             fontsize=10, color="#333333")
    x = values["elapsed_s"]
    cpu.plot(x, values["service_cpu_percent_one_core"], label="etw-dnstap service",
             color="#0072B2", linewidth=1.3)
    cpu.plot(x, values["dns_cpu_percent_one_core"], label="Windows DNS", color="#D55E00",
             linestyle="--", linewidth=1.3)
    cpu.set_ylabel("CPU (% of one core)")
    cpu.set_ylim(0, max(10, max(values["dns_cpu_percent_one_core"] +
                               values["service_cpu_percent_one_core"]) * 1.15))
    cpu.legend(loc="upper right", ncols=2, fontsize=9, framealpha=.95)
    qps.plot(x, values["achieved_qps"], color="#009E73", label="Achieved client QPS",
             linewidth=1.3)
    qps.axhline(requested, color="#555555", linestyle=":", linewidth=1.6,
                label="Requested rate ceiling")
    qps.set_ylim(0, max(requested, max(values["achieved_qps"])) * 1.20)
    qps.set_ylabel("DNS queries / second")
    qps.set_xlabel("Elapsed sending time (seconds)")
    qps.yaxis.set_major_formatter(StrMethodFormatter("{x:,.0f}"))
    qps.legend(loc="upper right", ncols=2, fontsize=9, framealpha=.95)
    for axis in (cpu, qps):
        axis.set_xlim(5, 295)
        axis.set_xticks([5, 50, 100, 150, 200, 250, 295])
        axis.grid(True, color="#D9DDE1", linewidth=.6)
        axis.set_axisbelow(True)
        axis.spines[["top", "right"]].set_visible(False)
    fig.text(.10, .035,
             "Sending seconds 5–295; one-second bins. WPR scheduled CPU excludes observed DPC/ISR.\n"
             "Approximate clock alignment. QPS uses counter boundary estimates; no smoothing.",
             fontsize=9, color="#444444")
    svg = output / f"{run_id}.svg"
    fig.savefig(svg, metadata={"Date": None, "Creator": f"Matplotlib {matplotlib.__version__}"})
    fig.savefig(previews / f"{run_id}.png", dpi=120)
    plt.close(fig)
    tree = ET.parse(svg)
    for element in tree.iter():
        assert element.tag.rsplit("}", 1)[-1] not in {"script", "image", "foreignObject"}
        for attribute, value in element.attrib.items():
            if attribute.rsplit("}", 1)[-1] == "href":
                assert value.startswith("#"), value
    assert any(element.tag.endswith("}text") for element in tree.iter())
    return {"run_id": run_id, "csv": f"data/{source.name}", "csv_sha256": digest(source),
            "svg": svg.name, "svg_sha256": digest(svg), "rows": len(rows)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "docs/assets/performance")
    parser.add_argument("--previews", type=Path,
                        default=ROOT / "target/performance/pooled-graph-previews")
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text(encoding="utf-8-sig"))
    args.output.mkdir(parents=True, exist_ok=True)
    args.previews.mkdir(parents=True, exist_ok=True)
    data_output = args.output / "data"
    data_output.mkdir(exist_ok=True)
    plt.rcParams.update({"svg.fonttype": "none", "svg.hashsalt": "pooled-wpr-v1",
                         "font.family": "DejaVu Sans", "font.size": 10,
                         "path.simplify": False})
    results = []
    entries = {entry["run_id"]: entry for entry in manifest["runs"]}
    assert set(entries) == set(RUNS), "Require all four validated primary runs"
    for run_id in RUNS:
        entry = entries[run_id]
        source = ROOT / Path(entry["series_csv"])
        assert digest(source).upper() == entry["series_sha256"].upper()
        assert entry["strict_pass"] == (RUNS[run_id][2] == "PASS")
        results.append(render(run_id, source, args.output, args.previews))
        destination = data_output / f"{run_id}.csv"
        shutil.copyfile(source, destination)
        results[-1]["csv"] = f"data/{destination.name}"
    shutil.copyfile(args.manifest, data_output / "source-manifest.json")
    provenance = {"renderer": "scripts/performance/plot-pooled-wpr.py",
                  "renderer_sha256": digest(Path(__file__)),
                  "matplotlib_version": matplotlib.__version__,
                  "python_version": sys.version, "source_manifest_sha256": digest(args.manifest),
                  "source_manifest": "data/source-manifest.json", "plots": results}
    (args.output / "plot-provenance.json").write_text(
        json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(provenance, indent=2))


if __name__ == "__main__":
    main()
