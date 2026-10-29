"""Count a gzip JSON archive without retaining records in memory.

Optional client windows select provider timestamps, not receipt timestamps.
Optional monitor samples bound provider-to-durable-output delay by locating the
first sampled stdout size that includes a record's ending byte offset.
"""
import argparse
from bisect import bisect_left
from collections import Counter, defaultdict
import datetime
import gzip
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--archive', required=True, type=Path)
parser.add_argument('--client', action='append', type=Path, default=[])
parser.add_argument('--monitor', type=Path)
parser.add_argument('--start-marker', type=Path)
parser.add_argument('--end-marker', type=Path)
parser.add_argument('--output', required=True, type=Path)
parser.add_argument('--provider-offset-lower-ms', type=float, default=0)
parser.add_argument('--provider-offset-upper-ms', type=float, default=0)
parser.add_argument('--ubuntu-offset-lower-ms', type=float, default=0)
args = parser.parse_args()
start = json.loads(args.start_marker.read_text()) if args.start_marker else None
end = json.loads(args.end_marker.read_text()) if args.end_marker else None
windows = []
for path in args.client:
    data = json.loads(path.read_text(encoding='utf-8-sig'))
    windows.append({'name': path.parent.name, 'start_ms': data['start_unix_ms'],
                    'end_ms': data['sending_end_unix_ms'], 'counts_at_offset_midpoint': Counter(),
                    'counts_definitely_in_window': Counter(), 'counts_possibly_in_window': Counter()})
samples = []
if args.monitor:
    if not start or not end:
        raise SystemExit('Monitor requires start/end marker boundaries for the file epoch')
    for line in args.monitor.open():
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if 'error' not in row and start['utc'] <= row['utc'] <= end['utc']:
            row['_epoch_ms'] = datetime.datetime.fromisoformat(row['utc']).timestamp()*1000
            samples.append(row)
    if any(a['log_size'] > b['log_size'] for a, b in zip(samples, samples[1:])):
        raise SystemExit('Stdout reset inside selected marker epoch')
sizes = [s['log_size'] for s in samples]
operations, response_rcodes, malformed_dns = Counter(), Counter(), 0
answer_ttls = defaultdict(Counter)
response_flags = defaultdict(Counter)
ra_false_client_examples = []
delays = Counter()
diagnostic_lines = malformed_lines = matched = all_json_records = unrelated_records = offset = missing_delay = 0
qtypes = Counter()
first_timestamp = last_timestamp = None
with gzip.open(args.archive, 'rb') as stream:
    for line in stream:
        begin = offset
        offset += len(line)
        if start and begin < start['offset']:
            continue
        if end and begin >= end['offset']:
            break
        try:
            row = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            if line.startswith((b'INFO:', b'WARN:', b'WARNING:', b'ERROR:', b'DEBUG:')):
                diagnostic_lines += 1
            else:
                malformed_lines += 1
            continue
        dns, tap = row.get('dns', {}), row.get('dnstap', {})
        all_json_records += 1
        name = dns.get('qname', '').rstrip('.').lower()
        if not (name.endswith('.auth.stress.test') or name.endswith('.rec.stress.test')):
            unrelated_records += 1
            continue
        operation = tap.get('operation', '?')
        stamp = tap.get('timestamp-rfc3339ns')
        stamp_ms = datetime.datetime.fromisoformat(stamp).timestamp()*1000 if stamp and (windows or samples) else None
        matched += 1
        operations[operation] += 1
        qtypes[dns.get('qtype', '?')] += 1
        malformed_dns += bool(dns.get('malformed-packet'))
        if operation.endswith('_RESPONSE'):
            response_rcodes[dns.get('rcode', '?')] += 1
            flags = dns.get('flags', {})
            for flag in ('qr', 'aa', 'ra', 'rd', 'tc', 'ad', 'cd'):
                response_flags[operation][f'{flag}_missing'] += int(flag not in flags)
                if flag in flags:
                    if not isinstance(flags[flag], bool):
                        response_flags[operation][f'{flag}_non_boolean'] += 1
                    else:
                        response_flags[operation][f'{flag}_{str(flags[flag]).lower()}'] += 1
            if operation == 'CLIENT_RESPONSE' and flags.get('ra') is False:
                response_flags[operation]['ra_false_with_noerror_and_answer'] += int(dns.get('rcode') == 'NOERROR' and dns.get('ancount', 0) > 0)
                if len(ra_false_client_examples) < 10:
                    ra_false_client_examples.append({'timestamp': stamp, 'qname': name,
                        'id': dns.get('id'), 'rcode': dns.get('rcode'), 'ancount': dns.get('ancount'),
                        'flags': flags, 'network': row.get('network', {})})
            for record in dns.get('resource-records', {}).get('an', []):
                answer_ttls[operation][str(record.get('ttl', '?'))] += 1
        first_timestamp = first_timestamp or stamp
        last_timestamp = stamp
        for window in windows:
            if stamp_ms is not None:
                earliest = stamp_ms-args.provider_offset_upper_ms
                latest = stamp_ms-args.provider_offset_lower_ms
                midpoint = (earliest+latest)/2
                if window['start_ms'] <= midpoint < window['end_ms']:
                    window['counts_at_offset_midpoint'][operation] += 1
                if earliest >= window['start_ms'] and latest < window['end_ms']:
                    window['counts_definitely_in_window'][operation] += 1
                if latest >= window['start_ms'] and earliest < window['end_ms']:
                    window['counts_possibly_in_window'][operation] += 1
        if samples and stamp_ms is not None:
            index = bisect_left(sizes, offset)
            if index < len(samples):
                # Histogram upper limits in 100ms increments; includes sampling/flush delay.
                upper_ms = samples[index]['_epoch_ms']-stamp_ms+args.provider_offset_upper_ms-args.ubuntu_offset_lower_ms
                bucket = int(upper_ms//100+1)*100
                delays[bucket] += 1
            else:
                missing_delay += 1

def quantile(histogram, fraction):
    target = sum(histogram.values())*fraction
    cumulative = 0
    for value, count in sorted(histogram.items()):
        cumulative += count
        if cumulative >= target:
            return value

result = {'archive': str(args.archive), 'matched_lab_records': matched,
          'all_json_records': all_json_records, 'unrelated_records': unrelated_records,
          'qtypes': qtypes,
          'operations': operations, 'response_rcodes': response_rcodes,
          'answer_ttls_by_operation': dict(answer_ttls),
          'response_flags_by_operation': dict(response_flags),
          'captured_client_response_ra_false_examples': ra_false_client_examples,
          'malformed_dns_records': malformed_dns, 'malformed_json_lines': malformed_lines,
          'diagnostic_lines': diagnostic_lines, 'first_provider_timestamp': first_timestamp,
          'last_provider_timestamp': last_timestamp, 'client_send_windows': windows,
          'provider_minus_host_clock_bounds_ms': [args.provider_offset_lower_ms, args.provider_offset_upper_ms],
          'ubuntu_minus_host_clock_lower_ms': args.ubuntu_offset_lower_ms,
          'client_window_clock_caveat': 'Provider timestamps use Windows guest UTC, client windows use host UTC; interpret with measured clock bounds.'}
if samples:
    result['durable_output_delay'] = {
        'method': 'Upper bound to first 1s monitor sample whose cumulative stdout byte offset includes the record; includes collector flush and sample delay, plus cross-clock error.',
        'durability_caveat': 'File-size samples bound file visibility, not fsync completion. The complete regular file is preserved in a verified archive.',
        'histogram_upper_ms': dict(sorted(delays.items())),
        'p50_upper_ms': quantile(delays, .5), 'p95_upper_ms': quantile(delays, .95),
        'p99_upper_ms': quantile(delays, .99), 'max_upper_ms': max(delays, default=None),
        'records_without_following_monitor_sample': missing_delay}
args.output.write_text(json.dumps(result, indent=2)+'\n')
print(json.dumps(result))
