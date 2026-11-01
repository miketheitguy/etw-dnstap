"""Summarize measured receiver/upstream evidence for one pair of markers."""
import argparse
import json
import os
from pathlib import Path
import re
import sys
import socket
sys.path.insert(0, str(Path(__file__).resolve().parent))  # lab_config lives next to this script
import lab_config  # noqa: E402

parser = argparse.ArgumentParser()
parser.add_argument('start')
parser.add_argument('end')
parser.add_argument('--directory', type=Path, default=Path(os.environ.get('DNSTAP_PERF_BASE') or Path(lab_config.get('ubuntu_vm.home')) / 'performance'))
args = parser.parse_args()
base = args.directory
start = json.loads((base / f'{args.start}.json').read_text())
end = json.loads((base / f'{args.end}.json').read_text())

def metrics(text):
    result = {}
    for line in text.splitlines():
        if line.startswith('dnscollector_exporter_') and not line.startswith('dnscollector_exporter_build_info'):
            key, value = line.rsplit(' ', 1)
            result[key] = float(value)
    return result

def snmp(text):
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if line.startswith('Udp:'):
            return dict(zip(line.split()[1:], map(int, lines[index+1].split()[1:])))
    return {}

def nft(data):
    result = {}
    for item in data['nftables']:
        rule = item.get('rule', {})
        if rule.get('comment') in ('upstream_queries', 'upstream_responses'):
            for expr in rule['expr']:
                if 'counter' in expr:
                    result[rule['comment']] = expr['counter']['packets']
    return result

def upstream_socket(marker):
    address = socket.inet_aton(lab_config.get('network.receiver_ip'))[::-1].hex().upper() + ':0035'
    rows = [line.split() for line in marker.get('udp_sockets', '').splitlines()[1:]]
    matches = [row for row in rows if len(row) >= 13 and row[1] == address]
    if len(matches) != 1:
        return None
    row = matches[0]
    return {'local_address': row[1], 'inode': row[9], 'drops': int(row[-1]),
            'receive_queue_hex': row[4].split(':')[1]}

a, b = metrics(start['metrics']), metrics(end['metrics'])
udp_a, udp_b = snmp(start['snmp']), snmp(end['snmp'])
nft_a, nft_b = nft(start['nft']), nft(end['nft'])
socket_a, socket_b = upstream_socket(start), upstream_socket(end)
samples = []
for line in (base / 'monitor.jsonl').open():
    row = json.loads(line)
    if 'error' not in row and start['utc'] <= row['utc'] <= end['utc']:
        samples.append(row)
result = {'start_utc': start['utc'], 'end_utc': end['utc'],
          'interval_seconds': (end['unix_ns']-start['unix_ns'])/1e9,
          'collector_pid_unchanged': start['collector_pid'] == end['collector_pid'],
          'worker_counter_deltas': {key: b.get(key, 0)-a.get(key, 0) for key in sorted(a.keys()|b.keys())},
          'udp_counter_deltas': {key: udp_b[key]-udp_a.get(key, 0) for key in udp_b},
          'upstream_packet_deltas': {key: nft_b[key]-nft_a.get(key, 0) for key in nft_b},
          'upstream_socket_start': socket_a, 'upstream_socket_end': socket_b,
          'upstream_socket_drop_delta': socket_b['drops']-socket_a['drops']
              if socket_a and socket_b and socket_a['inode'] == socket_b['inode'] else None,
          'monitor_samples': len(samples)}
if len(samples) >= 2:
    duration = samples[-1]['monotonic'] - samples[0]['monotonic']
    for process in ('collector', 'upstream'):
        valid = [row for row in samples if row[process].get('pid')]
        first, last = valid[0][process], valid[-1][process]
        cpu = (last['utime_ticks']+last['stime_ticks']-first['utime_ticks']-first['stime_ticks'])/samples[0]['clock_ticks']
        peaks = []
        for previous, current in zip(valid, valid[1:]):
            dt = current['monotonic']-previous['monotonic']
            p, c = previous[process], current[process]
            if p['pid'] == c['pid']:
                ticks = c['utime_ticks']+c['stime_ticks']-p['utime_ticks']-p['stime_ticks']
                peaks.append(100*ticks/samples[0]['clock_ticks']/dt)
        result[process] = {'pid_unchanged': first['pid'] == last['pid'],
                           'cpu_seconds': cpu, 'mean_cpu_percent_one_core': 100*cpu/duration,
                           'peak_sample_cpu_percent_one_core': max(peaks, default=0),
                           'peak_rss_bytes': max(row[process]['rss_bytes'] for row in valid)}
    result['minimum_disk_free_bytes'] = min(row['disk_free'] for row in samples)
    result['stdout_bytes_written'] = end['offset']-start['offset']
out = base / f'{args.start}--{args.end}-resources.json'
out.write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result))
