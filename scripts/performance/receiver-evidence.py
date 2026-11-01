"""Create bounded receiver snapshots and count the exact JSON records between them."""
import argparse
from collections import Counter
import datetime
import json
import os
from pathlib import Path
import re
import subprocess
import time
import sys
import urllib.request
sys.path.insert(0, str(Path(__file__).resolve().parent))  # lab_config lives next to this script
import lab_config  # noqa: E402

BASE = Path(os.environ.get('DNSTAP_PERF_BASE') or Path(lab_config.get('ubuntu_vm.home')) / 'performance')
LOG = Path(os.environ.get('DNSTAP_PERF_LOG', '/var/log/dnscollector/performance.jsonl'))
parser = argparse.ArgumentParser()
sub = parser.add_subparsers(dest='command', required=True)
mark = sub.add_parser('mark')
mark.add_argument('label')
count = sub.add_parser('count')
count.add_argument('start')
count.add_argument('end')
count.add_argument('--zone', required=True)
args = parser.parse_args()
for label in ([args.label] if args.command == 'mark' else [args.start, args.end]):
    if not re.fullmatch(r'[a-zA-Z0-9_-]{1,80}', label):
        raise SystemExit('Unsafe evidence label')

if args.command == 'mark':
    # The caller drains/stops the sender first. Require a publication plateau,
    # rather than assuming a fixed sleep proves the 1s flush has completed.
    samples = []
    stable = 0
    previous = None
    for _ in range(30):
        time.sleep(1)
        with urllib.request.urlopen('http://127.0.0.1:9165/metrics', timeout=3) as response:
            metrics = response.read().decode()
        offset = LOG.stat().st_size if LOG.exists() else 0
        delivery = sorted(line for line in metrics.splitlines()
                          if line.startswith('dnscollector_exporter_')
                          and any(word in line for word in ('traffic_total', 'policy_dropped_total', 'policy_forwarded_total')))
        current = (offset, delivery)
        stable = stable + 1 if current == previous else 0
        samples.append({'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'offset': offset,
                        'delivery_counters': delivery, 'equal_preceding_samples': stable})
        if stable >= 3:
            break
        previous = current
    else:
        raise SystemExit('Receiver publication did not stabilize; no marker created')
    result = {'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'unix_ns': time.time_ns(), 'offset': LOG.stat().st_size if LOG.exists() else 0,
              'metrics': metrics,
              'nft': json.loads(subprocess.check_output(['nft', '-j', 'list', 'table', 'inet', 'dnstap_performance'])),
              'snmp': Path('/proc/net/snmp').read_text(),
              'netdev': Path('/proc/net/dev').read_text(),
              'diskstats': Path('/proc/diskstats').read_text(),
              'udp_sockets': Path('/proc/net/udp').read_text(),
              'publication_stability_samples': samples,
              'collector_pid': subprocess.check_output(['systemctl', 'show', 'dnscollector', '-p', 'MainPID', '--value']).decode().strip()}
    path = BASE / f'{args.label}.json'
    if path.exists():
        raise SystemExit('Refusing to overwrite marker')
    path.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'path': str(path), 'utc': result['utc'], 'offset': result['offset']}))
else:
    start = json.loads((BASE / f'{args.start}.json').read_text())
    end = json.loads((BASE / f'{args.end}.json').read_text())
    operations, rcodes, types = Counter(), Counter(), Counter()
    malformed = diagnostic_lines = unrelated = all_records = 0
    first = last = None
    with LOG.open('rb') as stream:
        stream.seek(start['offset'])
        while stream.tell() < end['offset']:
            line = stream.readline()
            if not line:
                break
            try:
                row = json.loads(line)
            except (ValueError, UnicodeDecodeError):
                if line.startswith((b'INFO:', b'WARN:', b'WARNING:', b'ERROR:', b'DEBUG:')):
                    diagnostic_lines += 1
                else:
                    malformed += 1
                continue
            all_records += 1
            dns = row.get('dns', {})
            name = dns.get('qname', '').rstrip('.').lower()
            if not name.endswith('.' + args.zone.rstrip('.').lower()):
                unrelated += 1
                continue
            tap = row.get('dnstap', {})
            operations[tap.get('operation', '?')] += 1
            rcodes[dns.get('rcode', '?')] += 1
            types[dns.get('qtype', '?')] += 1
            stamp = tap.get('timestamp-rfc3339ns', tap.get('timestamp', None))
            first = first or stamp
            last = stamp
    result = {'start_marker': args.start, 'end_marker': args.end, 'zone': args.zone,
              'start_offset': start['offset'], 'end_offset': end['offset'],
              'all_json_records': all_records, 'matched_records': sum(operations.values()),
              'malformed_lines': malformed, 'unrelated_records': unrelated,
              'diagnostic_lines': diagnostic_lines,
              'operations': operations, 'rcodes': rcodes, 'qtypes': types,
              'first_record_timestamp': first, 'last_record_timestamp': last,
              'collector_pid_unchanged': start['collector_pid'] == end['collector_pid']}
    out = BASE / f'{args.start}--{args.end}-counts.json'
    out.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))
