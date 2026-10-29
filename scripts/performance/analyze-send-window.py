"""Interpolate cumulative Ubuntu samples at the client's sending boundaries.

Clock alignment is assumed; the report states the sampling and clock limitation.
No benchmark, marker, or service configuration is changed by this analysis.
"""
import argparse
import datetime
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--monitor', required=True, type=Path)
parser.add_argument('--client', required=True, type=Path)
parser.add_argument('--output', required=True, type=Path)
parser.add_argument('--receiver-disk', default='sda')
args = parser.parse_args()
client = json.loads(args.client.read_text(encoding='utf-8-sig'))
samples = []
for line in args.monitor.read_text().splitlines():
    try:
        row = json.loads(line)
    except ValueError:
        continue
    if 'error' not in row:
        row['_epoch'] = datetime.datetime.fromisoformat(row['utc']).timestamp()
        samples.append(row)
start = client['start_unix_ms']/1000
end = client['sending_end_unix_ms']/1000
duration = (client['sending_end_unix_ms']-client['start_unix_ms'])/1000

def bracket(when):
    before = max((s for s in samples if s['_epoch'] <= when), key=lambda s: s['_epoch'])
    after = min((s for s in samples if s['_epoch'] >= when), key=lambda s: s['_epoch'])
    width = after['_epoch']-before['_epoch']
    return before, after, (when-before['_epoch'])/width if width else 0

def interpolate(bracketed, getter):
    before, after, proportion = bracketed
    return getter(before)+(getter(after)-getter(before))*proportion

first, last = bracket(start), bracket(end)
inside = [s for s in samples if start <= s['_epoch'] <= end]
result = {'run': args.client.parent.name,
          'start_unix_ms': client['start_unix_ms'],
          'sending_end_unix_ms': client['sending_end_unix_ms'],
          'send_duration_seconds': duration, 'samples_inside_send_window': len(inside),
          'method': 'Linear interpolation of cumulative 1s Ubuntu samples at client UTC send boundaries; clocks assumed aligned. CPU and byte totals are estimates, not exact boundary samples.',
          'boundary_bracket_seconds': [b[1]['_epoch']-b[0]['_epoch'] for b in (first, last)],
          'stdout_bytes_estimated': interpolate(last, lambda s: s['log_size'])-interpolate(first, lambda s: s['log_size']),
          'minimum_disk_free_bytes': min(s['disk_free'] for s in inside)}
for process in ('collector', 'upstream'):
    relevant = [first[0], *inside, last[1]]
    pids = {s[process]['pid'] for s in relevant}
    cpu = lambda s: (s[process]['utime_ticks']+s[process]['stime_ticks'])/s['clock_ticks']
    seconds = interpolate(last, cpu)-interpolate(first, cpu) if len(pids)==1 else None
    peaks = []
    for previous, current in zip(relevant, relevant[1:]):
        dt = current['_epoch']-previous['_epoch']
        if dt > 0 and previous[process]['pid'] == current[process]['pid']:
            peaks.append(100*(cpu(current)-cpu(previous))/dt)
    result[process] = {'pids': sorted(pids), 'cpu_seconds_estimated': seconds,
                       'mean_cpu_percent_one_core_estimated': 100*seconds/duration if seconds is not None else None,
                       'peak_sample_cpu_percent_one_core': max(peaks, default=None),
                       'peak_sample_rss_bytes': max(s[process].get('rss_bytes', 0) for s in relevant)}
result['stdout_bytes_per_second_estimated'] = result['stdout_bytes_estimated']/duration
result['projected_stdout_bytes_300_seconds_at_observed_rate'] = 300*result['stdout_bytes_per_second_estimated']

def disk(row, name):
    for line in row['diskstats'].splitlines():
        fields = line.split()
        if fields[2] == name:
            return list(map(int, fields[3:]))
    raise ValueError(f'Missing guest disk {name}')

for device in dict.fromkeys(('sda', args.receiver_disk)):
    read_bytes = lambda s: disk(s, device)[2]*512
    write_bytes = lambda s: disk(s, device)[6]*512
    busy_ms = lambda s: disk(s, device)[9]
    weighted_ms = lambda s: disk(s, device)[10]
    result[f'guest_disk_{device}'] = {
        'read_bytes_estimated': interpolate(last, read_bytes)-interpolate(first, read_bytes),
        'write_bytes_estimated': interpolate(last, write_bytes)-interpolate(first, write_bytes),
        'busy_percent_estimated': (interpolate(last, busy_ms)-interpolate(first, busy_ms))/duration/10,
        'mean_weighted_queue_estimated': (interpolate(last, weighted_ms)-interpolate(first, weighted_ms))/duration/1000,
        'max_sample_io_in_flight': max(disk(s, device)[8] for s in inside),
        'scope': f'All guest activity on {device}; sampled virtual block-device counters.'}
result['receiver_disk_device'] = args.receiver_disk
result['receiver_disk'] = result[f'guest_disk_{args.receiver_disk}']

def udp(row):
    lines = row['snmp'].splitlines()
    for index, line in enumerate(lines):
        if line.startswith('Udp:'):
            return dict(zip(line.split()[1:], map(int, lines[index+1].split()[1:])))
    return {}

before_udp, after_udp = udp(first[0]), udp(last[1])
result['udp_counter_deltas_enclosing_sample_window'] = {key: after_udp[key]-before_udp.get(key, 0) for key in after_udp}
result['udp_enclosing_window_seconds'] = last[1]['_epoch']-first[0]['_epoch']
args.output.write_text(json.dumps(result, indent=2)+'\n')
print(json.dumps(result))
