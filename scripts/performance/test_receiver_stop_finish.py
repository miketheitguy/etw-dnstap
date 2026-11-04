"""Check the receiver STOP/FINISH capture parser and the verdict logic."""

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))  # `python -I` does not add the script directory

import receiver_stop_finish as RSF  # noqa: E402

SERVICE, RECEIVER = "172.27.215.163", "172.27.215.60"


def ip_packet(src: str, dst: str, sport: int, dport: int, payload: bytes, padding: bytes = b"") -> bytes:
    tcp = bytearray(20)
    tcp[0:2], tcp[2:4] = sport.to_bytes(2, "big"), dport.to_bytes(2, "big")
    tcp[12] = 5 << 4
    total = 20 + 20 + len(payload)
    ip = bytearray(20)
    ip[0], ip[2:4], ip[8], ip[9] = 0x45, total.to_bytes(2, "big"), 64, 6
    ip[12:16] = bytes(int(x) for x in src.split("."))
    ip[16:20] = bytes(int(x) for x in dst.split("."))
    return bytes(ip + tcp) + payload + padding


def dump(ts: float, src: str, sport: int, dst: str, dport: int, flags: str, payload: bytes,
         padding: bytes = b"") -> str:
    packet = ip_packet(src, dst, sport, dport, payload, padding)
    lines = [f"{ts:.6f} IP {src}.{sport} > {dst}.{dport}: Flags [{flags}], seq 1:{1 + len(payload)}, "
             f"ack 1, win 502, length {len(payload)}"]
    for offset in range(0, len(packet), 16):
        words = packet[offset:offset + 16].hex()
        spaced = " ".join(words[i:i + 4] for i in range(0, len(words), 4))
        lines.append(f"\t0x{offset:04x}:  {spaced}")
    return "\n".join(lines)


def stop_from_service(ts: float, port: int = 49673) -> str:
    return dump(ts, SERVICE, port, RECEIVER, 6000, "P.", RSF.STOP_FRAME)


def finish_from_receiver(ts: float, port: int = 49673) -> str:
    return dump(ts, RECEIVER, 6000, SERVICE, port, "P.", RSF.FINISH_FRAME)


class ParseTests(unittest.TestCase):
    def test_frames_are_the_frame_streams_control_bytes(self):
        self.assertEqual(RSF.STOP_FRAME.hex(), "000000000000000400000003")
        self.assertEqual(RSF.FINISH_FRAME.hex(), "000000000000000400000005")

    def test_payload_is_decoded_and_padding_dropped(self):
        text = dump(1.5, SERVICE, 49673, RECEIVER, 6000, "P.", RSF.STOP_FRAME, padding=b"\x00\x00\x00\x00")
        (packet,) = RSF.parse_capture(text)
        self.assertEqual(packet.payload, RSF.STOP_FRAME)
        self.assertEqual((packet.src_port, packet.dst_port, packet.length), (49673, 6000, 12))

    def test_flag_only_packet_has_empty_payload(self):
        (packet,) = RSF.parse_capture(dump(2.0, SERVICE, 49673, RECEIVER, 6000, "F.", b""))
        self.assertEqual(packet.payload, b"")
        self.assertIn("F", packet.flags)

    def test_noise_and_interface_prefix_are_ignored(self):
        text = ("listening on any, link-type LINUX_SLL2\n"
                + stop_from_service(3.0).replace("3.000000 IP", "3.000000 ens3 In  IP") + "\n"
                + "2 packets captured\n")
        (packet,) = RSF.parse_capture(text)
        self.assertEqual(packet.payload, RSF.STOP_FRAME)

    def test_filter_is_limited_to_tiny_payloads_and_fin_rst(self):
        self.assertIn("tcp port 6000", RSF.BPF_FILTER)
        self.assertIn("tcp-fin|tcp-rst", RSF.BPF_FILTER)
        self.assertIn("<= 16", RSF.BPF_FILTER)

    def test_capture_command_never_writes_a_file(self):
        command = RSF.remote_capture_command()
        self.assertTrue(command.startswith("sudo -n timeout --signal=INT "))
        self.assertNotIn(" -w ", command)
        self.assertNotIn(">", command.split("'")[0])  # no shell redirect before the filter
        self.assertIn("-l", RSF.TCPDUMP_ARGS)


class EvaluateTests(unittest.TestCase):
    def verdict(self, *blocks: str) -> dict:
        return RSF.evaluate(RSF.parse_capture("\n".join(blocks)), SERVICE)

    def test_stop_then_finish_is_confirmed(self):
        result = self.verdict(stop_from_service(10.0), finish_from_receiver(10.001),
                              dump(10.002, SERVICE, 49673, RECEIVER, 6000, "F.", b""))
        self.assertEqual(result["receiver_stop_finish"], RSF.CONFIRMED)
        self.assertTrue(result["service_fin_seen"])
        self.assertEqual(result["service_port"], 49673)

    def test_stop_without_finish_is_not_seen(self):
        result = self.verdict(stop_from_service(10.0), dump(10.1, SERVICE, 49673, RECEIVER, 6000, "R.", b""))
        self.assertEqual(result["receiver_stop_finish"], RSF.NOT_SEEN)
        self.assertTrue(result["stop_seen"])
        self.assertFalse(result["finish_seen"])
        self.assertTrue(result["service_rst_seen"])

    def test_finish_on_another_connection_is_not_seen(self):
        result = self.verdict(stop_from_service(10.0, 49673), finish_from_receiver(10.1, 49999))
        self.assertEqual(result["receiver_stop_finish"], RSF.NOT_SEEN)

    def test_finish_before_stop_is_not_seen(self):
        result = self.verdict(finish_from_receiver(9.0), stop_from_service(10.0))
        self.assertEqual(result["receiver_stop_finish"], RSF.NOT_SEEN)

    def test_empty_capture_is_not_seen(self):
        self.assertEqual(self.verdict("")["receiver_stop_finish"], RSF.NOT_SEEN)

    def test_other_sender_is_not_the_service(self):
        other = dump(10.0, "10.0.0.9", 40000, RECEIVER, 6000, "P.", RSF.STOP_FRAME)
        self.assertEqual(self.verdict(other, finish_from_receiver(10.1, 40000))["receiver_stop_finish"],
                         RSF.NOT_SEEN)


class StageTests(unittest.TestCase):
    def stage(self, tmp: str, capture: str | None, **receipt) -> Path:
        stage = Path(tmp)
        folder = stage / RSF.CAPTURE_DIR
        folder.mkdir()
        body = {"listening_confirmed": True, "service_ip": SERVICE, "capture_file": RSF.CAPTURE_NAME}
        body.update(receipt)
        (folder / RSF.RECEIPT_NAME).write_text(json.dumps(body), encoding="utf-8")
        if capture is not None:
            (folder / RSF.CAPTURE_NAME).write_text(capture, encoding="utf-8")
        return stage

    def test_old_evidence_without_the_field_is_not_checked(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(RSF.stage_label(Path(tmp)), RSF.NOT_CHECKED)

    def test_confirmed_stage(self):
        with tempfile.TemporaryDirectory() as tmp:
            stage = self.stage(tmp, stop_from_service(1.0) + "\n" + finish_from_receiver(1.1))
            self.assertEqual(RSF.stage_label(stage), RSF.CONFIRMED)

    def test_listening_not_confirmed_is_not_checked(self):
        with tempfile.TemporaryDirectory() as tmp:
            stage = self.stage(tmp, stop_from_service(1.0), listening_confirmed=False)
            self.assertEqual(RSF.stage_label(stage), RSF.NOT_CHECKED)

    def test_missing_capture_file_is_not_checked(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(RSF.stage_label(self.stage(tmp, None)), RSF.NOT_CHECKED)

    def test_listening_capture_without_frames_is_not_seen(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(RSF.stage_label(self.stage(tmp, "listening on any\n")), RSF.NOT_SEEN)


def load_script(filename: str):
    spec = importlib.util.spec_from_file_location(filename.replace("-", "_").removesuffix(".py"),
                                                  Path(__file__).resolve().parent / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ReportTests(unittest.TestCase):
    """The parser-ramp table must read receiver_stop_finish, and tolerate rows without it."""

    @staticmethod
    def row(**extra) -> dict:
        row = {
            "run_id": "r1", "mode": "authoritative", "requested_qps": 1000, "achieved_qps": 999.5,
            "requested_duration_seconds": 60, "measured_send_duration_seconds": 60.0, "sent": 10,
            "completed": 10, "responses_received": 10, "successful": 10, "dns_delivery_deficit": 0,
            "timeouts": 0, "client_query_records": 10, "client_response_records": 10,
            "resolver_query_records": 0, "resolver_response_records": 0,
            "sent_minus_client_query_records": 0, "completed_minus_client_response_records": 0,
            "actual_upstream_queries": 0, "actual_upstream_responses": 0,
            "upstream_evidence_available": False, "upstream_evidence_status": "unavailable",
            "canonical_verdict": "clean", "canonical_clean": True, "canonical_strict_pass": True,
            "rate_qualification": "rate ok", "protocol_status": "protocol ok", "flags": {},
            "lifecycle_status": "clean SCM stop", "evidence_class": "canonical reconciliation ledger",
        }
        row.update(extra)
        return row

    def test_row_with_the_field_shows_it(self):
        module = load_script("build-parser-ramp-report.py")
        for label in RSF.VERDICTS:
            with self.subTest(label=label):
                self.assertIn(f"receiver STOP/FINISH {label}",
                              module.table_row(self.row(receiver_stop_finish=label)))

    def test_old_row_without_the_field_reads_not_checked(self):
        module = load_script("build-parser-ramp-report.py")
        line = module.table_row(self.row())
        self.assertIn("receiver STOP/FINISH not-checked", line)
        self.assertNotIn("FAILED", line)

    def test_old_diagnostic_event_note_is_kept_as_historical(self):
        module = load_script("build-parser-ramp-report.py")
        line = module.table_row(self.row(service_error_event="STOP/FINISH event 10054 at X (historical service event, temporal correlation only)"))
        self.assertIn("historical service event", line)


if __name__ == "__main__":
    unittest.main()
