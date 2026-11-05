"""Test plot-tier-runs.py against the synthetic fixture in testdata/plot-tier-runs.

Run with ``python -I scripts/performance/test_plot_tier_runs.py`` (needs matplotlib,
see requirements.txt).  The fixture has two runs of the 10,000 QPS tier:
``t10k-busy-r1`` loses events in several stages and has missing cells,
``t10k-idle-r1`` loses nothing.
"""
import importlib.util
import math
from pathlib import Path
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parent
FIXTURE = SCRIPTS / "testdata" / "plot-tier-runs"
sys.path.insert(0, str(SCRIPTS))  # `python -I` does not put the script directory on sys.path

spec = importlib.util.spec_from_file_location("plot_tier_runs", SCRIPTS / "plot-tier-runs.py")
ptr = importlib.util.module_from_spec(spec)
sys.modules["plot_tier_runs"] = ptr
spec.loader.exec_module(ptr)

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


class IncrementTests(unittest.TestCase):
    def test_increments_and_gaps(self):
        nan = math.nan
        got = ptr.increments(ptr.np.array([0, 5, nan, 9, 9, 3]))
        self.assertEqual(got[0], 0)
        self.assertEqual(got[1], 5)
        self.assertTrue(math.isnan(got[2]))
        self.assertEqual(got[3], 4)  # a gap puts its increment on the next sample
        self.assertEqual(got[4], 0)
        self.assertEqual(got[5], 0)  # counter reset counts as 0, not negative

    def test_total_uses_last_present_value(self):
        self.assertEqual(ptr.total(ptr.np.array([1, 7, math.nan])), 7)
        self.assertEqual(ptr.total(ptr.np.array([math.nan, math.nan])), 0)


class FixtureTests(unittest.TestCase):
    def setUp(self):
        self.busy = ptr.load_run(FIXTURE / "t10k-busy-r1")
        self.idle = ptr.load_run(FIXTURE / "t10k-idle-r1")

    def test_missing_cells_load_as_nan(self):
        self.assertEqual(self.busy.n, 30)
        self.assertTrue(math.isnan(self.busy.cols["dns_cpu_pct"][12]))
        self.assertTrue(math.isnan(self.busy.cols["etw_lost"][12]))
        self.assertTrue(math.isnan(self.busy.cols["etw_lost"][13]))
        self.assertFalse(math.isnan(self.busy.cols["etw_lost"][14]))

    def test_busy_summary(self):
        info = ptr.summarize(self.busy)
        self.assertEqual(info["sent"], 18950)  # 18,790 received plus 160 lost in ETW
        # Fixture totals: etw 8 s x 20, ring 8 s x 10, network 7 s x 5.
        self.assertEqual(info["point_totals"]["etw"], 160)
        self.assertEqual(info["point_totals"]["channel"], 80)
        self.assertEqual(info["point_totals"]["network"], 35)
        self.assertEqual(info["lost"], 275)
        self.assertAlmostEqual(info["loss_pct"], 100 * 275 / 18950)
        self.assertEqual(info["etw_buffers_lost"], 8)  # buffers, not in the percent
        self.assertEqual(info["spooled"], 210)  # diverted, not in the percent
        self.assertAlmostEqual(info["point_pct"]["etw"], 100 * 160 / 18950)

    def test_busy_cpu_means_use_load_window_and_skip_missing(self):
        info = ptr.summarize(self.busy)
        self.assertEqual(info["dns_max"], 140)
        self.assertAlmostEqual(info["service_mean"], 72)
        self.assertAlmostEqual(info["stage_mean"]["parser"], 30)

    def test_per_second_increments(self):
        inc = ptr.increments(self.busy.cols["etw_lost"])
        self.assertEqual(inc[10], 20)
        self.assertTrue(math.isnan(inc[12]))
        self.assertEqual(inc[14], 60)  # t=12 and t=13 missing: three seconds land on t=14
        self.assertEqual(ptr.np.nansum(inc), 160)

    def test_idle_summary_has_no_loss(self):
        info = ptr.summarize(self.idle)
        self.assertEqual(info["sent"], 20000)
        self.assertEqual(info["lost"], 0)
        self.assertEqual(info["loss_pct"], 0)

    def test_zero_sent_does_not_divide_by_zero(self):
        self.idle.cols["events_received_per_s"][:] = 0
        self.assertEqual(ptr.summarize(self.idle)["loss_pct"], 0)


class RenderTests(unittest.TestCase):
    def test_render_all_writes_run_and_tier_pngs(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "png"
            written = ptr.render_all(FIXTURE, out)
            names = sorted(p.name for p in written)
            self.assertEqual(names, ["t10k-busy-r1.png", "t10k-idle-r1.png", "tier-10000.png"])
            for path in written:
                data = path.read_bytes()
                self.assertTrue(data.startswith(PNG_MAGIC), path.name)
                self.assertGreater(len(data), 10_000, path.name)

    def test_run_marked_out_of_tier_summary_gets_only_its_own_graph(self):
        with tempfile.TemporaryDirectory() as tmp:
            runs = Path(tmp) / "runs"
            for name in ("t10k-busy-r1", "t10k-idle-r1"):
                (runs / name).mkdir(parents=True)
                for f in ("run.json", "timeseries.csv"):
                    (runs / name / f).write_bytes((FIXTURE / name / f).read_bytes())
            meta_path = runs / "t10k-busy-r1" / "run.json"
            meta = ptr.json.loads(meta_path.read_text(encoding="utf-8"))
            meta["tier_summary"] = False
            meta_path.write_text(ptr.json.dumps(meta), encoding="utf-8")
            seen = []
            real = ptr.render_tier
            ptr.render_tier = lambda qps, entries, out: seen.extend(i["id"] for i, _ in entries)
            try:
                written = ptr.render_all(runs, Path(tmp) / "png")
            finally:
                ptr.render_tier = real
            self.assertIn("t10k-busy-r1.png", [p.name for p in written])
            self.assertEqual(seen, ["t10k-idle-r1"])

    def test_tier_filter(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(SystemExit):
                ptr.render_all(FIXTURE, Path(tmp), tier=99)

    def test_output_is_deterministic(self):
        with tempfile.TemporaryDirectory() as tmp:
            first = [p.read_bytes() for p in ptr.render_all(FIXTURE, Path(tmp) / "a")]
            second = [p.read_bytes() for p in ptr.render_all(FIXTURE, Path(tmp) / "b")]
            self.assertEqual(first, second)


if __name__ == "__main__":
    unittest.main()
