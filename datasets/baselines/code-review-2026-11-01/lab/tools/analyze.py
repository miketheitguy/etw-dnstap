"""Summarize the Phase 2 lab A/B runs. Usage: analyze.py <evidence-dir> <out-dir>"""
import csv, json, statistics as st, sys, glob, os, re

ev, out = sys.argv[1], sys.argv[2]
BYTES_PER_FRAME = 103.01
runs = []
for path in sorted(glob.glob(os.path.join(ev, 'r[0-9]*-*', 'run.json'))):
    r = json.load(open(path, encoding='utf-8-sig'))
    pre, post = r['pre'], r['post']

    def thr(snap):
        return {t['tid']: t for t in snap['threads']}

    tp, tq = thr(pre), thr(post)
    groups = {'dnstap-parser': 0, 'etw-consumer': 0, 'other': 0}
    for tid, t in tq.items():
        d = t['cycles'] - (tp[tid]['cycles'] if tid in tp else 0)
        g = t['name'] if t['name'] in ('dnstap-parser', 'etw-consumer') else 'other'
        groups[g] += d
    c1 = post['counters']
    c0 = pre['counters']
    recv = c1['events received'] - c0['events received']
    sent = c1['frames sent'] - c0['frames sent']
    dropped = c1['events dropped'] - c0['events dropped']
    rx = r['rx_post']['bytes_received'][0] - r['rx_pre']['bytes_received'][0]
    frames_rx = rx / BYTES_PER_FRAME
    cpu = post['cpu_seconds'] - pre['cpu_seconds']
    cycles = sum(groups.values())
    stopped = ' '.join(e for e in r['stop']['events'] if 'capture stopped' in e)
    final = {k: int(re.search(k + r'=(\d+)', stopped).group(1)) for k in
             ['received', 'encoded', 'ring_dropped', 'pool_exhausted_regular', 'pool_exhausted_jumbo', 'etw_lost']}
    polls = r['polls']
    row = {
        'run': r['label'], 'image': r['image'], 'sha8': r['image_sha256'][:8],
        'installed_sha8_verified': r['begin']['installed_sha256'][:8],
        'qps_requested': r['qps_requested'], 'qps_achieved': round(r['client']['qps_achieved'], 2),
        'client_timeouts': r['client']['timeouts'], 'client_send_errors': r['client']['send_errors'],
        'client_p50_us': round(r['client']['p50_ns'] / 1000), 'client_p99_us': round(r['client']['p99_ns'] / 1000),
        'events_received': recv, 'frames_sent_counter': sent, 'events_dropped': dropped,
        'final_received': final['received'], 'final_encoded': final['encoded'], 'final_ring_dropped': final['ring_dropped'],
        'final_pool_exhausted': final['pool_exhausted_regular'] + final['pool_exhausted_jumbo'], 'final_etw_lost': final['etw_lost'],
        'receiver_bytes_delta': rx, 'frames_delivered_rx': round(frames_rx),
        'channel_peak_depth': int(c1['channel peak depth']),
        'service_cpu_s': round(cpu, 4),
        'cycles_parser_G': round(groups['dnstap-parser'] / 1e9, 4),
        'cycles_consumer_G': round(groups['etw-consumer'] / 1e9, 4),
        'cycles_other_G': round(groups['other'] / 1e9, 4),
        'cycles_total_G': round(cycles / 1e9, 4),
        'parser_cycles_per_event': round(groups['dnstap-parser'] / recv, 1),
        'consumer_cycles_per_event': round(groups['etw-consumer'] / recv, 1),
        'total_cycles_per_event': round(cycles / recv, 1),
        'frames_per_cpu_s': round(frames_rx / cpu, 0),
        'frames_per_gcycle': round(frames_rx / (cycles / 1e9), 0),
        'private_pre_mib': round(pre['private_bytes'] / 2**20, 2),
        'private_post_mib': round(post['private_bytes'] / 2**20, 2),
        'private_poll_mean_mib': round(st.mean(p['private_bytes'] for p in polls) / 2**20, 2),
        'private_poll_max_mib': round(max(p['private_bytes'] for p in polls) / 2**20, 2),
        'working_set_post_mib': round(post['working_set'] / 2**20, 2),
        'peak_working_set_post_mib': round(post['peak_working_set'] / 2**20, 2),
        'dns_cpu_s': round(post['dns_cpu_seconds'] - pre['dns_cpu_seconds'], 2),
        'guest_cpu_pct_mean': round(st.mean(p['total_cpu_pct'] for p in polls), 2),
        'host_cpu_pct_mean': round(st.mean(p['host_total_cpu_pct'] for p in polls), 2),
        'host_builders_max': max(r['host_build_processes_at_start'], r['host_build_processes_max_during'], r['host_build_processes_at_end']),
        'stop_state': r['stop']['state'], 'stop_exit_code': r['stop']['exit_code'],
        'rx_connections': r['rx_post']['connections'],
    }
    runs.append(row)

os.makedirs(out, exist_ok=True)
with open(os.path.join(out, 'runs.csv'), 'w', newline='') as f:
    w = csv.DictWriter(f, fieldnames=list(runs[0].keys()))
    w.writeheader()
    w.writerows(runs)

metrics = ['qps_achieved', 'frames_delivered_rx', 'events_dropped', 'service_cpu_s', 'cycles_parser_G', 'cycles_consumer_G',
           'cycles_other_G', 'cycles_total_G', 'parser_cycles_per_event', 'consumer_cycles_per_event', 'total_cycles_per_event',
           'frames_per_cpu_s', 'frames_per_gcycle', 'private_pre_mib', 'private_post_mib', 'private_poll_mean_mib', 'private_poll_max_mib', 'working_set_post_mib', 'peak_working_set_post_mib', 'dns_cpu_s',
           'guest_cpu_pct_mean', 'host_cpu_pct_mean', 'channel_peak_depth']
rows = []
for m in metrics:
    a = [r[m] for r in runs if r['image'] == 'base']
    b = [r[m] for r in runs if r['image'] == 'phase2']
    ma, mb = st.median(a), st.median(b)
    chg = (mb - ma) / ma * 100 if ma else float('nan')
    spread_a = (max(a) - min(a)) / ma * 100 if ma else float('nan')
    spread_b = (max(b) - min(b)) / mb * 100 if mb else float('nan')
    overlap = not (max(b) < min(a) or min(b) > max(a))
    pairs = [(p - q) / q * 100 for q, p in zip(a, b)] if all(a) else []
    rows.append({'metric': m, 'base_values': ' '.join(map(str, a)), 'phase2_values': ' '.join(map(str, b)),
                 'base_median': round(ma, 4), 'phase2_median': round(mb, 4), 'change_pct': round(chg, 2),
                 'base_range_pct_of_median': round(spread_a, 2), 'phase2_range_pct_of_median': round(spread_b, 2),
                 'ranges_overlap': overlap, 'paired_change_pct': ' '.join('%+.1f' % x for x in pairs)})
with open(os.path.join(out, 'summary.csv'), 'w', newline='') as f:
    w = csv.DictWriter(f, fieldnames=list(rows[0].keys()))
    w.writeheader()
    w.writerows(rows)
for r in rows:
    print('%-26s base %12s [%s]  phase2 %12s [%s]  chg %+7.2f%%  ranges %s/%s%%  overlap=%s  pairs %s' % (
        r['metric'], r['base_median'], r['base_values'], r['phase2_median'], r['phase2_values'], r['change_pct'],
        r['base_range_pct_of_median'], r['phase2_range_pct_of_median'], r['ranges_overlap'], r['paired_change_pct']))
