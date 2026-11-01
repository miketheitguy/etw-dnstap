"""Exercise the archived analyzers: send window, receiver results, phases and ring-fix CPU.

Run with ``python -I scripts/performance/archive/test_archived_analyzers.py``.  The
analyzers keep their hyphenated command names and are not part of the CI test set.
"""
import json
from pathlib import Path
import subprocess
import shutil
import sys
import unittest
import uuid

SCRIPTS = Path(__file__).resolve().parent
PERF = SCRIPTS.parent
sys.path.insert(0, str(PERF))  # `python -I` does not put the script directory on sys.path


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


class ArchivedAnalyzerTests(unittest.TestCase):
    def setUp(self):
        # Inherit the workspace ACL; Windows tempfile's private ACL prevents
        # the restricted execution token from accessing its own child files.
        self.temp_root = (SCRIPTS.parents[2] / "target/archived-analyzer-tests").resolve()
        self.base = self.temp_root / uuid.uuid4().hex
        self.base.mkdir(parents=True)
        self.addCleanup(self.cleanup)

    def cleanup(self):
        if self.base.resolve().parent != self.temp_root:
            raise ValueError("Test cleanup escaped the temporary test root")
        shutil.rmtree(self.base)

    def test_receiver_disk_analysis_keeps_os_and_data_volume_distinct(self):
        monitor = self.base / 'monitor.jsonl'
        samples = []
        for tick in range(5):
            process = dict(pid=123, utime_ticks=tick * 100, stime_ticks=0, rss_bytes=100)
            samples.append(dict(utc=f'1970-01-01T00:00:0{tick}+00:00',
                clock_ticks=100, collector=process, upstream=process,
                log_size=tick * 1000, disk_free=100000,
                diskstats=f'8 0 sda 0 0 {tick*2} 0 0 0 {tick*4} 0 0 {tick*10} {tick*10}\n'
                          f'8 16 sdb 0 0 {tick*8} 0 0 0 {tick*16} 0 0 {tick*100} {tick*200}\n',
                snmp='Udp: InDatagrams RcvbufErrors\nUdp: 0 0\n'))
        monitor.write_text('\n'.join(json.dumps(row) for row in samples) + '\n')
        client = self.base / 'client.json'
        write_json(client, dict(start_unix_ms=1000, sending_end_unix_ms=3000))
        output = self.base / 'resources.json'
        result = run_script('analyze-send-window.py', '--monitor', monitor,
                            '--client', client, '--receiver-disk', 'sdb', '--output', output)
        self.assertEqual(result.returncode, 0, result.stderr)
        value = json.loads(output.read_text())
        self.assertEqual(value['receiver_disk_device'], 'sdb')
        self.assertEqual(value['guest_disk_sda']['busy_percent_estimated'], 1)
        self.assertEqual(value['guest_disk_sdb']['busy_percent_estimated'], 10)
        self.assertEqual(value['receiver_disk']['write_bytes_estimated'], 16384)

    def receiver(self, sample, deployment=None):
        base = self.base
        rows = sample["encoded"]
        write_json(base / "client/run/summary.json", dict(
            start_unix_ms=0, sending_end_unix_ms=1000,
            options=dict(mode="authoritative", qps=50, threads=1, inflight=1),
            names_loaded=50, counts=dict(sent=50, completed=50, successful=50,
                timeouts=0, rcode=[0, 0, 0], ra_missing=0)))
        write_json(base / "ubuntu/run-archive-counts.json", dict(
            all_json_records=rows, unrelated_records=0, matched_lab_records=rows,
            malformed_json_lines=0, malformed_dns_records=0, operations={},
            answer_ttls_by_operation={}, response_flags_by_operation={},
            captured_client_response_ra_false_examples=[]))
        workers = {}
        for worker in ("tap", "console"):
            for counter in ("ingress", "egress", "discarded"):
                workers[f'dnscollector_exporter_worker_{counter}_traffic_total{{worker="{worker}"}}'] = 0 if counter == "discarded" else rows
        write_json(base / "ubuntu/run-start--run-end-resources.json", dict(
            upstream_packet_deltas=dict(upstream_queries=0, upstream_responses=0),
            worker_counter_deltas=workers, udp_counter_deltas=dict(RcvbufErrors=0),
            start_utc="start", end_utc="end"))
        resource = dict(mean_cpu_percent_one_core_estimated=1,
                        cpu_seconds_estimated=1, peak_sample_rss_bytes=100)
        write_json(base / "ubuntu/run-send-window.json", dict(
            collector=resource, upstream=resource,
            guest_disk_sda=dict(busy_percent_estimated=0)))
        write_json(base / "archive/run-verification.json", {})
        write_json(base / "windows/run.123.jsonl", sample)
        if deployment is not None:
            write_json(base / "windows/run-Start.json", deployment)
        return run_script("summarize-receiver-results.py", "--directory", base,
                          "--runs", "run", "--output", base / "receiver.json")

    def test_receiver_preserves_deployment_object_and_singleton_array(self):
        metadata = dict(binary_sha256="reviewed-hash", requested_capacity=8192,
                        actual_command="service --dnstap", capacity_override=False)
        for value in (metadata, [metadata]):
            result = self.receiver(fixture(), value)
            self.assertEqual(result.returncode, 0, result.stderr)
            row = json.loads((self.base / "receiver.json").read_text())["runs"][0]
            self.assertEqual(row["service_binary_sha256"], "reviewed-hash")
            self.assertEqual(row["service_deployment"], metadata)
        for value in ([], [metadata, metadata], [1], "invalid"):
            result = self.receiver(fixture(), value)
            self.assertNotEqual(result.returncode, 0)

    def test_receiver_actual_zero_loss_predicate(self):
        for schema, loss in ((1, None), (1, "ring_dropped"), (2, None),
                             (2, "pool_exhausted"), (2, "receiver_disconnected")):
            with self.subTest(schema=schema, loss=loss):
                result = self.receiver(fixture(schema, loss))
                self.assertEqual(result.returncode, 0, result.stderr)
                row = json.loads((self.base / "receiver.json").read_text())["runs"][0]
                self.assertTrue(row["event_denominator_domain_closure_verified"])
                self.assertTrue(row["receiver_exact_delivery_verified"])
                self.assertEqual(row["capture_pipeline_zero_loss_verified"], loss is None)
                self.assertEqual(row["callback_ledger_residual"], 0)

    def test_receiver_rejects_unclosed_or_incomplete_evidence(self):
        for key, value in (("received", 101), ("dequeued", 99),
                           ("intake_dropped", 1), ("schema", 3), ("final", False)):
            sample = fixture()
            sample[key] = value
            with self.subTest(key=key):
                self.assertNotEqual(self.receiver(sample).returncode, 0)
        sample = fixture()
        del sample["pool_exhausted"]
        self.assertNotEqual(self.receiver(sample).returncode, 0)

    def test_receiver_reports_transient_query_errors_and_requires_final_stats(self):
        for valid in (True, False):
            with self.subTest(etw_stats_valid=valid):
                sample = dict(fixture(), etw_stats_errors=2, etw_stats_valid=valid)
                result = self.receiver(sample)
                self.assertEqual(result.returncode, 0, result.stderr)
                row = json.loads((self.base / "receiver.json").read_text())["runs"][0]
                self.assertEqual(row["etw_stats_errors"], 2)
                self.assertEqual(row["etw_stats_valid"], valid)
                self.assertEqual(row["capture_pipeline_zero_loss_verified"], valid)
        for invalid in (-1, True, "0", None):
            with self.subTest(etw_stats_errors=invalid):
                self.assertNotEqual(self.receiver(dict(fixture(), etw_stats_errors=invalid)).returncode, 0)
        sample = fixture()
        del sample["etw_stats_errors"]
        self.assertNotEqual(self.receiver(sample).returncode, 0)

    def phases(self, samples):
        write_json(self.base / "client.json", dict(start_unix_ms=1000,
                                                   sending_end_unix_ms=2000))
        (self.base / "wpr-start-utc.txt").write_text("1970-01-01T00:00:01Z")
        (self.base / "wpr-stop-end-utc.txt").write_text("1970-01-01T00:00:02Z")
        (self.base / "phases.jsonl").write_text("\n".join(map(json.dumps, samples)))
        return run_script("analyze-metrics-phases.py", self.base,
                          self.base / "client.json", self.base / "phases.jsonl")

    def test_phase_live_nonclosure_and_intake_percent(self):
        previous = fixture()
        current = fixture()
        previous.update(final=False, received=1, dequeued=0, encoded=0,
                        intake_dropped=0, pool_exhausted=1)
        current.update(final=False, epoch_ms=2000, intake_dropped=10, pool_exhausted=10)
        result = self.phases([previous, current])
        self.assertEqual(result.returncode, 0, result.stderr)
        group = json.loads(result.stdout)["groups"]["wpr_and_rundown"]
        self.assertEqual(group["ring_drop_percent_of_received"], 0)
        self.assertAlmostEqual(group["intake_drop_percent_of_received"], 1000 / 99)

    def test_phase_counter_reset_and_identity_change_rejected(self):
        previous = fixture()
        for key, value in (("pool_exhausted", 0), ("schema", 1),
                           ("pid", 456), ("run_id", "different")):
            first = dict(previous, final=False, pool_exhausted=1)
            second = dict(previous, final=False, epoch_ms=2000)
            second[key] = value
            with self.subTest(key=key):
                self.assertNotEqual(self.phases([first, second]).returncode, 0)

    def test_phase_explicit_final_residual_rejected(self):
        previous = dict(fixture(), final=False)
        current = dict(fixture(), epoch_ms=2000, received=101)
        self.assertNotEqual(self.phases([previous, current]).returncode, 0)

    def test_cpu_actual_consumer_preserves_ring_and_exposes_intake(self):
        process = dict(cpu_raw_time_weighted_percent=dict(mean=10, valid_seconds=30, n=30),
                       cpu_seconds_integrated=3, private_bytes_delta=0,
                       counters={"private bytes": dict(max=1048576)})
        analysis = dict(service_final=fixture(2, "pool_exhausted"),
                        metadata=dict(service_sha256="fixture", service_command="fixture",
                                      logical_processors=2),
                        processes=dict(service=process, dns=process),
                        system_time_weighted={"processor(_total)\\% processor time": dict(mean=10)})
        write_json(self.base / "windows/run/analysis.json", analysis)
        write_json(self.base / "client/run/summary.json", dict(
            measured_send_duration_ns=30000000000, counts=dict(sent=50, successful=50),
            options=dict(mode="authoritative")))
        result = run_script("analyze-ring-fix-cpu.py", self.base)
        self.assertEqual(result.returncode, 0, result.stderr)
        row = json.loads(result.stdout)["runs"][0]
        self.assertEqual(row["ring_drop_percent"], 0)
        self.assertEqual(row["intake_drop_percent"], 10)
        analysis["service_final"]["schema"] = 3
        write_json(self.base / "windows/run/analysis.json", analysis)
        self.assertNotEqual(run_script("analyze-ring-fix-cpu.py", self.base).returncode, 0)


if __name__ == "__main__":
    unittest.main()
