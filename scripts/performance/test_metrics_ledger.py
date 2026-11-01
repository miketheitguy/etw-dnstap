"""Exercise service ledger validation and the actual analysis command paths.

Run with ``python -I scripts/performance/test_metrics_ledger.py`` (or ``python -m unittest``
from this directory).  The analysis scripts keep their hyphenated command names.  Tests for the archived
analyzers live in archive/test_archived_analyzers.py.
"""
import copy
import json
from pathlib import Path
import subprocess
import shutil
import sys
import unittest
import uuid

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))  # `python -I` does not put the script directory on sys.path

from metrics_ledger import ledger_summary, normalize_metrics  # noqa: E402


def fixture(schema=2, loss=None):
    # Independent, explicitly enumerated service sample, not validator constants.
    value = dict(schema=schema, final=True, received=100, ring_dropped=0,
                 callback_errors=0, dequeued=100, queue_backlog=0, encoded=100,
                 ignored=0, rejected=0, output_limit_dropped=0,
                 output_failed_events=0, output_errors=0, etw_events_lost=0,
                 etw_realtime_buffers_lost=0, etw_stats_valid=True, etw_stats_errors=0,
                 parse_ns=1000, output_ns=1000, epoch_ms=1000,
                 run_id="fixture-123", pid=123, session="fixture")
    if schema == 2:
        value.update(intake_dropped=0, pool_exhausted=0, receiver_disconnected=0)
    if loss:
        value[loss] = 10
        value.update(dequeued=90, encoded=90)
        if schema == 2:
            value["intake_dropped"] = 10
    return value


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")


def run_script(script, *args):
    return subprocess.run([sys.executable, str(SCRIPTS / script), *map(str, args)],
                          capture_output=True, text=True)


class LedgerTests(unittest.TestCase):
    def test_schema1_and_schema2_zero_and_loss_ledgers(self):
        for schema, loss in ((1, None), (1, "ring_dropped"), (2, None),
                             (2, "pool_exhausted"), (2, "receiver_disconnected")):
            with self.subTest(schema=schema, loss=loss):
                raw = fixture(schema, loss)
                before = copy.deepcopy(raw)
                normalized = normalize_metrics(raw)
                self.assertEqual(raw, before)
                self.assertEqual(normalized["intake_dropped"], 10 if loss else 0)
                self.assertEqual(ledger_summary(normalized)["capture_residual"], 0)

    def test_invalid_fields_rejected(self):
        for key in ("received", "ring_dropped", "callback_errors", "dequeued",
                    "queue_backlog", "encoded", "ignored", "rejected",
                    "output_limit_dropped", "output_failed_events", "output_errors",
                    "etw_events_lost", "etw_realtime_buffers_lost", "etw_stats_errors", "intake_dropped",
                    "pool_exhausted", "receiver_disconnected"):
            for invalid in (-1, True, False, 0.0, "0", None):
                with self.subTest(key=key, invalid=invalid):
                    value = fixture()
                    value[key] = invalid
                    with self.assertRaises(ValueError):
                        normalize_metrics(value)
            value = fixture()
            del value[key]
            with self.subTest(missing=key), self.assertRaises(ValueError):
                normalize_metrics(value)

    def test_unknown_missing_and_boolean_schema_rejected(self):
        for schema in (None, 0, 3, True, "2"):
            value = fixture()
            value["schema"] = schema
            with self.subTest(schema=schema), self.assertRaises(ValueError):
                normalize_metrics(value)
        value = fixture()
        del value["schema"]
        with self.assertRaises(ValueError):
            normalize_metrics(value)

    def test_final_and_stats_flags_are_explicit_booleans(self):
        for key in ("final", "etw_stats_valid"):
            for invalid in (None, 1, "true"):
                value = fixture()
                value[key] = invalid
                with self.subTest(key=key, invalid=invalid), self.assertRaises(ValueError):
                    normalize_metrics(value)
        value = fixture()
        value["final"] = False
        with self.assertRaises(ValueError):
            normalize_metrics(value)
        self.assertFalse(normalize_metrics(value, require_final=False)["final"])

    def test_all_final_residuals_rejected(self):
        for key in ("intake_dropped", "pool_exhausted", "received", "encoded"):
            value = fixture()
            value[key] += 1
            with self.subTest(key=key), self.assertRaises(ValueError):
                normalize_metrics(value)
            with self.assertRaises(ValueError):
                normalize_metrics(value, require_final=False)
            value["final"] = False
            normalize_metrics(value, require_final=False)


class ConsumerTests(unittest.TestCase):
    def setUp(self):
        # Inherit the workspace ACL; Windows tempfile's private ACL prevents
        # the restricted execution token from accessing its own child files.
        self.temp_root = (SCRIPTS.parents[1] / "target/metrics-ledger-tests").resolve()
        self.base = self.temp_root / uuid.uuid4().hex
        self.base.mkdir(parents=True)
        self.addCleanup(self.cleanup)

    def cleanup(self):
        if self.base.resolve().parent != self.temp_root:
            raise ValueError("Test cleanup escaped the temporary test root")
        shutil.rmtree(self.base)

    def windows(self, sample):
        capture = self.base / "capture"
        write_json(capture / "metadata.json", dict(service_pid=1, dns_pid=2,
                                                   logical_processors=2))
        (capture / "counters.csv").write_text(
            '"(PDH-CSV 4.0) (0)"\n"01/01/2026 00:00:01.000"\n')
        write_json(self.base / "service.jsonl", sample)
        return run_script("analyze-windows.py", capture, "--metrics",
                          self.base / "service.jsonl", "--output", self.base / "analysis.json")

    def test_windows_accepts_valid_schema2_loss_ledgers(self):
        for loss in ("pool_exhausted", "receiver_disconnected"):
            with self.subTest(loss=loss):
                result = self.windows(fixture(2, loss))
                self.assertEqual(result.returncode, 0, result.stderr)
                ledger = json.loads((self.base / "analysis.json").read_text())["ledger"]
                self.assertEqual(ledger["capture_residual"], 0)
                self.assertEqual(ledger["parser_residual"], 0)
                self.assertEqual(ledger["intake_dropped"], 10)

    def test_windows_rejects_capture_and_parser_residuals(self):
        for key in ("received", "encoded"):
            sample = fixture()
            sample[key] += 1
            with self.subTest(key=key):
                self.assertNotEqual(self.windows(sample).returncode, 0)


if __name__ == "__main__":
    unittest.main()
