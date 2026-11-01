"""Record receiver, upstream, kernel, and disk evidence once per second.

Started by setup-ubuntu-workload.sh as a root systemd unit from the root-owned lab library
directory.  Configuration is by environment variable: DNSTAP_PERF_BASE (required, evidence
directory), DNSTAP_PERF_LOG, DNSTAP_PERF_MIN_FREE, DNSTAP_PERF_RAW_LIMIT and
DNSTAP_PERF_UPSTREAM_UNIT.  Appends one JSON line per second for up to two hours.
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import urllib.request

BASE = Path(os.environ['DNSTAP_PERF_BASE'])  # required: no baked-in home directory
LOG = Path(os.environ.get('DNSTAP_PERF_LOG', '/var/log/dnscollector/performance.jsonl'))
MIN_FREE = int(os.environ.get('DNSTAP_PERF_MIN_FREE', 3 * 1024**3))
RAW_LIMIT = int(os.environ.get('DNSTAP_PERF_RAW_LIMIT', 12 * 1024**3))
UPSTREAM = os.environ.get('DNSTAP_PERF_UPSTREAM_UNIT', 'dnstap-performance-upstream')

def process(unit: str) -> dict:
    pid = int(subprocess.check_output(['systemctl', 'show', unit, '-p', 'MainPID', '--value']))
    if not pid:
        return {'pid': 0}
    stat = Path(f'/proc/{pid}/stat').read_text().split()
    return {'pid': pid, 'utime_ticks': int(stat[13]), 'stime_ticks': int(stat[14]),
            'rss_bytes': int(stat[23]) * os.sysconf('SC_PAGE_SIZE'),
            'io': Path(f'/proc/{pid}/io').read_text()}

argparse.ArgumentParser(description=__doc__.splitlines()[0]).parse_args()
with (BASE / 'monitor.jsonl').open('a', buffering=1) as out:
    for _ in range(7200):
        started = time.monotonic()
        try:
            with urllib.request.urlopen('http://127.0.0.1:9165/metrics', timeout=2) as response:
                metrics = response.read().decode()
            sample = {'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                      'monotonic': time.monotonic(), 'clock_ticks': os.sysconf('SC_CLK_TCK'),
                      'collector': process('dnscollector'),
                      'upstream': process(UPSTREAM),
                      'proc_stat': Path('/proc/stat').read_text(),
                      'snmp': Path('/proc/net/snmp').read_text(),
                      'netdev': Path('/proc/net/dev').read_text(),
                      'diskstats': Path('/proc/diskstats').read_text(),
                      'udp_sockets': Path('/proc/net/udp').read_text(),
                      'memory': Path('/proc/meminfo').read_text(),
                      'log_size': LOG.stat().st_size if LOG.exists() else 0,
                      'disk_free': shutil.disk_usage(LOG.parent).free,
                      'archive_disk_free': shutil.disk_usage(BASE).free,
                      'raw_path': str(LOG),
                      'metrics': metrics}
            out.write(json.dumps(sample) + '\n')
            if min(sample['disk_free'], sample['archive_disk_free']) < MIN_FREE or sample['log_size'] > RAW_LIMIT:
                subprocess.run(['systemctl', 'stop', 'dnscollector'], check=True)
                raise RuntimeError(f'Receiver evidence disk limit reached; collector stopped; min_free={MIN_FREE}, raw_limit={RAW_LIMIT}')
        except Exception as exc:
            out.write(json.dumps({'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'error': str(exc)}) + '\n')
            if isinstance(exc, RuntimeError):
                raise
        time.sleep(max(0, 1 - (time.monotonic() - started)))
