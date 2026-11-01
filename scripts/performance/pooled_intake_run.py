"""Run the real private Rust callback benchmark; do not model the production pool."""

import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from perf_common import sha256_file as sha256


def validate(payload, capacity, mode):
    """Reject incomplete evidence rather than treating missing fields as zero."""
    if payload["capacity"] != capacity or payload["mode"] != mode:
        raise ValueError("benchmark configuration differs from requested matrix")
    for key in ("offered", "accepted", "dequeued", "intake_dropped", "ring_dropped",
                "pool_exhausted", "receiver_disconnected", "callback_errors", "final_backlog"):
        if not isinstance(payload[key], int) or isinstance(payload[key], bool) or payload[key] < 0:
            raise ValueError(f"invalid count: {key}")
    if payload["intake_dropped"] != sum(payload[key] for key in
            ("ring_dropped", "pool_exhausted", "receiver_disconnected")):
        raise ValueError("drop components do not reconcile")
    if payload["offered"] != payload["accepted"] + payload["intake_dropped"] + payload["callback_errors"]:
        raise ValueError("offered ledger does not reconcile")
    if payload["accepted"] != payload["dequeued"] + payload["final_backlog"]:
        raise ValueError("accepted ledger does not reconcile")
    if payload["final_backlog"] or payload["recycle_verified"] is not True:
        raise ValueError("final ownership/recycle verification failed")
    if payload["elapsed_ns"] <= 0:
        raise ValueError("missing positive benchmark duration")
    for fixture in ("success", "channel_full", "pool_exhausted", "disconnected"):
        counters = payload["allocator_fixtures"][fixture]
        for key in ("allocations", "deallocations", "reallocations"):
            if counters[key] != 0:
                raise ValueError(f"warmed callback allocator call: {fixture}/{key}")
    return payload


class ProcessEvidence:
    """Hold the child handle through exit so final CPU accounting is durable."""

    class Counters(ctypes.Structure):
        _fields_ = [("cb", wintypes.DWORD), ("page_faults", wintypes.DWORD)] + [
            (key, ctypes.c_size_t) for key in ("peak_working_set", "working_set",
            "quota_peak_paged", "quota_paged", "quota_peak_nonpaged", "quota_nonpaged",
            "pagefile", "peak_pagefile", "private")]

    def __init__(self, pid):
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.psapi = ctypes.WinDLL("psapi", use_last_error=True)
        self.kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        self.kernel.OpenProcess.restype = wintypes.HANDLE
        self.kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        self.kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
        self.psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD]
        self.handle = self.kernel.OpenProcess(0x0400 | 0x0010, False, pid)
        if not self.handle:
            raise ctypes.WinError(ctypes.get_last_error())
        self.private_peak = 0
        self.working_peak = 0
        self.samples = 0

    def sample(self):
        counters = self.Counters()
        counters.cb = ctypes.sizeof(counters)
        if self.psapi.GetProcessMemoryInfo(self.handle, ctypes.byref(counters), counters.cb):
            self.private_peak = max(self.private_peak, counters.private)
            self.working_peak = max(self.working_peak, counters.peak_working_set)
            self.samples += 1

    def finish(self):
        clocks = [wintypes.FILETIME() for _ in range(4)]
        try:
            if not self.kernel.GetProcessTimes(self.handle, *(ctypes.byref(value) for value in clocks)):
                raise ctypes.WinError(ctypes.get_last_error())
            ticks = [(value.dwHighDateTime << 32) | value.dwLowDateTime for value in clocks]
            return dict(process_kernel_cpu_seconds=ticks[2] / 1e7,
                        process_user_cpu_seconds=ticks[3] / 1e7,
                        sampled_private_peak_bytes=self.private_peak,
                        process_peak_working_set_bytes=self.working_peak,
                        memory_samples=self.samples)
        finally:
            self.kernel.CloseHandle(self.handle)


def run(args):
    if sys.platform != "win32":
        raise RuntimeError("process resource evidence requires Windows")
    args.output_dir.mkdir(parents=True, exist_ok=False)
    sources = {str(path): sha256(path) for pattern in ("src/**/*.rs", "Cargo.toml", "Cargo.lock")
               for path in Path.cwd().glob(pattern)}
    provenance = dict(test_exe=str(args.test_exe.resolve()), binary_sha256=sha256(args.test_exe),
                      source_sha256=sources, toolchain=subprocess.check_output(
                          ["rustc", "-Vv"], text=True), runner_sha256=sha256(Path(__file__)))
    (args.output_dir / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")
    results = []
    for capacity in (1024, 8192):
        for mode in ("steady", "bursty"):
            for repetition in range(1, args.repetitions + 1):
                label = f"{capacity}-{mode}-{repetition}"
                command = [str(args.test_exe.resolve()), "synthetic_pooled_intake_benchmark",
                           "--ignored", "--nocapture", "--test-threads=1"]
                settings = dict(ETW_BENCH_CAPACITY=str(capacity), ETW_BENCH_EVENTS=str(args.events),
                                ETW_BENCH_MODE=mode)
                started = time.time_ns()
                (args.output_dir / f"{label}.command.json").write_text(json.dumps(dict(
                    command=command, environment=settings, start_unix_ns=started), indent=2) + "\n")
                clock = time.perf_counter_ns()
                # Files avoid a bounded pipe filling while the sampler waits for process exit.
                with (args.output_dir / f"{label}.stdout.txt").open("w") as stdout, \
                     (args.output_dir / f"{label}.stderr.txt").open("w") as stderr:
                    child = subprocess.Popen(command, env=os.environ | settings, stdout=stdout, stderr=stderr)
                    evidence = ProcessEvidence(child.pid)
                    while child.poll() is None:
                        evidence.sample()
                        if time.perf_counter_ns() - clock > args.timeout * 1_000_000_000:
                            child.kill()
                            child.wait()
                            evidence.finish()
                            raise TimeoutError(f"bounded benchmark deadline: {label}")
                        time.sleep(0.02)
                    resources = evidence.finish()
                elapsed = time.perf_counter_ns() - clock
                output = (args.output_dir / f"{label}.stdout.txt").read_text()
                lines = [line.split("ETW_BENCH_JSON=", 1)[1] for line in output.splitlines()
                         if "ETW_BENCH_JSON=" in line]
                if child.returncode != 0 or len(lines) != 1:
                    raise RuntimeError(f"benchmark failed or missing unique JSON: {label}; inspect raw logs")
                payload = validate(json.loads(lines[0]), capacity, mode)
                if payload["offered"] != args.events:
                    raise ValueError("offered count differs from requested event count")
                record = dict(label=label, command=command, environment=settings,
                              start_unix_ns=started, process_wall_ns=elapsed,
                              benchmark=payload, resources=resources)
                cpu = resources["process_kernel_cpu_seconds"] + resources["process_user_cpu_seconds"]
                record["process_mean_cpu_one_core_percent"] = cpu / (elapsed / 1e9) * 100
                record["offered_per_second"] = payload["offered"] / (payload["elapsed_ns"] / 1e9)
                record["dequeued_per_second"] = payload["dequeued"] / (payload["elapsed_ns"] / 1e9)
                (args.output_dir / f"{label}.json").write_text(json.dumps(record, indent=2) + "\n")
                results.append(record)
                print(label, payload["dequeued"], payload["intake_dropped"], flush=True)
    (args.output_dir / "results.json").write_text(json.dumps(dict(
        scope="Synthetic real callback/pool/channel only; excludes native ETW/TDH/encoding/export. "
              "Process resources include startup/warmup/fixtures; callback allocation counts cover "
              "warmed Rust global allocator calls, not all Windows/native heap activity.",
        provenance=provenance, runs=results), indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-exe", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--events", type=int, default=200000)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--timeout", type=int, default=120)
    args = parser.parse_args()
    if args.events <= 0 or args.repetitions <= 0 or args.timeout <= 0:
        parser.error("events, repetitions and timeout must be positive")
    run(args)


if __name__ == "__main__":
    main()
