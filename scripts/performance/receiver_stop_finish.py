"""Receiver-side proof that a requested stop reached STOP and FINISH.

Since the slow-receiver change the service exits with code 0 and logs nothing when the
Frame Streams STOP/FINISH exchange does not complete, so the SCM exit code and the
``output_errors`` counter no longer prove it.  The lab proves it on the receiver instead:
a ``tcpdump`` limited to tiny TCP payloads (1 to 16 bytes) and FIN/RST packets on the
receiver port runs around the stop and prints to stdout.  No pcap file and no bulk stream
data is ever written on the receiver.  This module holds the capture command, the parser
for that output and the verdict logic shared by the stage summary and the report builders.

The verdict is one of ``confirmed``, ``not-seen`` and ``not-checked``.  Evidence folders
from before this check have no capture receipt and read as ``not-checked``, never as a
failure.

Command line (offline, on host evidence)::

    python receiver_stop_finish.py <stage-dir>
    python receiver_stop_finish.py --print-command   # the command the stage script runs over ssh
"""
from __future__ import annotations

import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

CONFIRMED = "confirmed"
NOT_SEEN = "not-seen"
NOT_CHECKED = "not-checked"
VERDICTS = (CONFIRMED, NOT_SEEN, NOT_CHECKED)

CAPTURE_PORT = 6000
CAPTURE_DIR = "receiver-stop"
RECEIPT_NAME = "receiver-stop-capture.json"
CAPTURE_NAME = "capture.txt"
MAX_PAYLOAD = 16

# Frame Streams control frame: 4-byte zero escape, 4-byte length (4), 4-byte type.
STOP_FRAME = bytes.fromhex("00000000" "00000004" "00000003")
FINISH_FRAME = bytes.fromhex("00000000" "00000004" "00000005")

_PAYLOAD_LEN = "(ip[2:2] - ((ip[0] & 0xf) << 2) - ((tcp[12] & 0xf0) >> 2))"
BPF_FILTER = (f"ip and tcp port {CAPTURE_PORT} and ((tcp[tcpflags] & (tcp-fin|tcp-rst) != 0) "
              f"or (({_PAYLOAD_LEN} > 0) and ({_PAYLOAD_LEN} <= {MAX_PAYLOAD})))")
# Runs on the receiver.  `timeout` bounds the capture if the ssh session dies.  -s 128
# keeps only headers and the tiny payload, -l line-buffers stdout, -x prints the packet
# from the IP header in hex.  No -w: nothing is written to a file on the receiver.
TCPDUMP_ARGS = ("tcpdump", "-i", "any", "-l", "-nn", "-tt", "-x", "-s", "128", BPF_FILTER)
CAPTURE_TIMEOUT_SECONDS = 120


def remote_capture_command() -> str:
    """Return the shell command line to run over ssh on the receiver."""
    quoted = " ".join(f"'{part}'" if " " in part else part for part in TCPDUMP_ARGS)
    return f"sudo -n timeout --signal=INT {CAPTURE_TIMEOUT_SECONDS} {quoted}"


@dataclass(frozen=True)
class Packet:
    timestamp: float
    src_ip: str
    src_port: int
    dst_ip: str
    dst_port: int
    flags: str
    length: int
    payload: bytes | None


_HEADER = re.compile(
    r"^(?P<ts>\d+(?:\.\d+)?)\s+(?:\S+\s+(?:In|Out)\s+)?IP\s+"
    r"(?P<sip>\d+\.\d+\.\d+\.\d+)\.(?P<sport>\d+)\s+>\s+"
    r"(?P<dip>\d+\.\d+\.\d+\.\d+)\.(?P<dport>\d+):\s+Flags\s+\[(?P<flags>[^\]]*)\].*?"
    r"\blength\s+(?P<len>\d+)")
_HEX_LINE = re.compile(r"^\s+0x[0-9a-fA-F]+:\s+(?P<hex>(?:[0-9a-fA-F]{2,4}\s?)+)")


def _tcp_payload(packet: bytes) -> bytes | None:
    """Return the TCP payload of an IPv4 packet, or None when the hex is too short."""
    if len(packet) < 20:
        return None
    ip_header = (packet[0] & 0x0F) * 4
    total = int.from_bytes(packet[2:4], "big")
    if len(packet) < ip_header + 20:
        return None
    tcp_header = (packet[ip_header + 12] >> 4) * 4
    start = ip_header + tcp_header
    end = min(total, len(packet))  # drop link-layer padding
    return packet[start:end] if start <= end else None


def parse_capture(text: str) -> list[Packet]:
    """Parse ``tcpdump -nn -tt -x`` output into packets.  Unknown lines are ignored."""
    packets: list[Packet] = []
    header: re.Match[str] | None = None
    hex_parts: list[str] = []

    def flush() -> None:
        if header is None:
            return
        raw = bytes.fromhex("".join(hex_parts)) if hex_parts else b""
        payload = _tcp_payload(raw) if raw else None
        packets.append(Packet(float(header["ts"]), header["sip"], int(header["sport"]),
                              header["dip"], int(header["dport"]), header["flags"],
                              int(header["len"]), payload))

    for line in text.splitlines():
        match = _HEADER.match(line)
        if match:
            flush()
            header, hex_parts = match, []
            continue
        hexed = _HEX_LINE.match(line)
        if hexed and header is not None:
            hex_parts.append(re.sub(r"\s+", "", hexed["hex"]))
    flush()
    return packets


def evaluate(packets: list[Packet], service_ip: str, port: int = CAPTURE_PORT) -> dict[str, Any]:
    """Decide whether the service connection saw STOP and then FINISH.

    STOP is the control frame sent by ``service_ip`` to the receiver port.  FINISH is
    the control frame sent back from the receiver port to the same service port, at or
    after the STOP packet.
    """
    ordered = sorted(packets, key=lambda p: p.timestamp)
    stops = [p for p in ordered if p.src_ip == service_ip and p.dst_port == port
             and p.payload == STOP_FRAME]
    stop = stops[0] if stops else None
    finish = None
    for candidate in stops:
        reply = next((p for p in ordered
                      if p.src_port == port and p.dst_ip == service_ip
                      and p.dst_port == candidate.src_port and p.payload == FINISH_FRAME
                      and p.timestamp >= candidate.timestamp), None)
        if reply is not None:
            stop, finish = candidate, reply
            break
    service_flags = [p.flags for p in ordered if p.src_ip == service_ip and p.dst_port == port]
    return {
        "receiver_stop_finish": CONFIRMED if stop and finish else NOT_SEEN,
        "stop_seen": stop is not None,
        "finish_seen": finish is not None,
        "service_port": stop.src_port if stop else None,
        "service_fin_seen": any("F" in f for f in service_flags),
        "service_rst_seen": any("R" in f for f in service_flags),
        "packets_parsed": len(packets),
    }


def verdict_for_stage(stage: Path) -> dict[str, Any]:
    """Return the receiver STOP/FINISH verdict for one stage directory.

    A stage without a capture receipt (all evidence from before this check) is
    ``not-checked``.  A receipt that did not confirm the capture was listening, or whose
    capture file is missing, is also ``not-checked``, with the reason recorded.
    """
    folder = stage / CAPTURE_DIR
    receipt_path = folder / RECEIPT_NAME
    if not receipt_path.is_file():
        return {"receiver_stop_finish": NOT_CHECKED, "reason": "no receiver stop capture receipt"}
    receipt = json.loads(receipt_path.read_text(encoding="utf-8-sig"))
    capture_path = folder / str(receipt.get("capture_file") or CAPTURE_NAME)
    if receipt.get("listening_confirmed") is not True:
        return {"receiver_stop_finish": NOT_CHECKED, "reason": "capture was not confirmed listening"}
    if not capture_path.is_file():
        return {"receiver_stop_finish": NOT_CHECKED, "reason": "capture output is missing"}
    service_ip = receipt.get("service_ip")
    if not isinstance(service_ip, str) or not service_ip:
        return {"receiver_stop_finish": NOT_CHECKED, "reason": "receipt has no service IP"}
    packets = parse_capture(capture_path.read_text(encoding="utf-8", errors="replace"))
    return evaluate(packets, service_ip)


def stage_label(stage: Path) -> str:
    """Return just the verdict string for a stage directory."""
    return verdict_for_stage(stage)["receiver_stop_finish"]


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if args == ["--print-command"]:
        print(remote_capture_command())
        return 0
    if len(args) != 1:
        print("usage: receiver_stop_finish.py <stage-dir>", file=sys.stderr)
        return 2
    result = verdict_for_stage(Path(args[0]))
    print(json.dumps(result, indent=2))
    return 0 if result["receiver_stop_finish"] == CONFIRMED else 1


if __name__ == "__main__":
    raise SystemExit(main())
