"""Aggregate WPA leaf context-switch rows without treating Ready/Wait as locks."""

import argparse
import csv
import json
from collections import Counter, defaultdict
from decimal import Decimal
from math import floor
from pathlib import Path


def decimal(value):
    return Decimal(value.replace(',', ''))


def fresh_group():
    return dict(rows=0, names=set(), scheduled_ms=Decimal(0), ready_us=Decimal(0),
                ready_max_us=Decimal(0), waits_us=Decimal(0), waits_max_us=Decimal(0),
                waits_count=0, switch_in_min_s=None, switch_in_max_s=None,
                ready_before_view=0, switch_out_before_view=0,
                positive_ready_histogram_us=Counter(), positive_wait_histogram_us=Counter(),
                positive_ready_samples_us=[], positive_wait_samples_us=[],
                prev_state_counts=Counter(), prev_wait_reason_counts=Counter(),
                seconds=defaultdict(lambda: dict(rows=0, scheduled_ms=Decimal(0),
                    ready_us=Decimal(0), waits_us=Decimal(0), waits_count=0)))


def aggregate(source, start, end, pid=None):
    groups = defaultdict(fresh_group)
    with source.open(encoding='utf-8-sig', newline='') as stream:
        reader = csv.reader(stream)
        header = next(reader)
        if len(header) != 15 or header[:3] != ['New Process', 'New Thread Id', 'Thread Name']:
            raise ValueError('input must use the accompanying 15-column CPU precise profile')
        if header[3] != 'CPU Usage (in view) (ms)' or header[4] != 'Ready (µs)' or header[6] != 'Waits (µs)':
            raise ValueError(f'unexpected exported units: {header[3:8]}')
        for line_number, row in enumerate(reader, 2):
            if not row:
                continue
            if len(row) != len(header) or decimal(row[9]) != 1:
                raise ValueError(f'expected a leaf CSwitch row at line {line_number}')
            if pid is not None and not row[0].endswith(f' ({pid})'):
                continue
            group = groups[(row[0], int(row[1]))]
            group['rows'] += 1
            if row[2]:
                group['names'].add(row[2])
            scheduled, ready, ready_max, waits, waits_max = map(decimal, row[3:8])
            wait_count = int(decimal(row[8]))
            switch_in, ready_time, switch_out = map(decimal, row[10:13])
            group['scheduled_ms'] += scheduled
            group['ready_us'] += ready
            group['ready_max_us'] = max(group['ready_max_us'], ready_max)
            group['waits_us'] += waits
            group['waits_max_us'] = max(group['waits_max_us'], waits_max)
            group['waits_count'] += wait_count
            if ready > 0:
                group['positive_ready_samples_us'].append(ready)
            if waits > 0:
                group['positive_wait_samples_us'].append(waits)
            for value, histogram in ((ready, group['positive_ready_histogram_us']),
                                     (waits, group['positive_wait_histogram_us'])):
                if value > 0:
                    bound = next((upper for upper in (1, 10, 100, 1000, 3000, 10000, 100000, 1000000, 10000000) if value <= upper), 'above_10000000')
                    histogram[bound] += 1
            group['switch_in_min_s'] = switch_in if group['switch_in_min_s'] is None else min(group['switch_in_min_s'], switch_in)
            group['switch_in_max_s'] = switch_in if group['switch_in_max_s'] is None else max(group['switch_in_max_s'], switch_in)
            group['ready_before_view'] += int(ready_time < start)
            group['switch_out_before_view'] += int(switch_out < start)
            group['prev_state_counts'][row[13]] += 1
            group['prev_wait_reason_counts'][row[14]] += 1
            second = floor(switch_in - start)
            bucket = group['seconds'][second]
            bucket['rows'] += 1
            bucket['scheduled_ms'] += scheduled
            bucket['ready_us'] += ready
            bucket['waits_us'] += waits
            bucket['waits_count'] += wait_count
    result = []
    for (process, tid), group in groups.items():
        group['names'] = sorted(group['names'])
        group['scheduled_thread_s'] = group['scheduled_ms'] / 1000
        group['ready_thread_s'] = group['ready_us'] / 1000000
        group['wait_thread_s'] = group['waits_us'] / 1000000
        for key in ('positive_ready_histogram_us', 'positive_wait_histogram_us'):
            group[key] = [dict(upper_bound_us=bound, count=count) for bound, count in group[key].items()]
        for kind in ('ready', 'wait'):
            samples = sorted(group.pop(f'positive_{kind}_samples_us'))
            group[f'positive_{kind}_quantiles_us'] = {
                f'p{percent}': samples[(len(samples) * percent + 99) // 100 - 1] if samples else None
                for percent in (50, 95, 99)
            }
        group['seconds'] = [dict(second_offset=second, **values) for second, values in sorted(group['seconds'].items())]
        result.append(dict(process=process, tid=tid, **group))
    return dict(source=str(source), original_columns=header, view_start_s=start,
                view_end_s=end, view_wall_s=end-start, selected_pid=pid,
                semantics='CPU is scheduled CPU Usage(in view), not asserted exclusive of interrupts. Ready/Wait sums are associated with exported switch-ins; boundary-crossing durations can extend outside the view and tails without in-view switch-in can be absent. Per-second buckets attribute whole rows by switch-in time, not clipped per-second occupancy. Aggregate durations are thread-seconds, not wall seconds. Prev wait reason is observed dispatcher metadata, not proof of a lock.',
                threads=sorted(result, key=lambda group: group['scheduled_thread_s'], reverse=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--start-s', type=Decimal, required=True)
    parser.add_argument('--end-s', type=Decimal, required=True)
    parser.add_argument('--pid', type=int)
    args = parser.parse_args()
    if not args.start_s.is_finite() or not args.end_s.is_finite() or args.start_s < 0 or args.end_s <= args.start_s:
        parser.error('require finite 0 <= start-s < end-s')
    result = aggregate(args.input, args.start_s, args.end_s, args.pid)
    with args.output.open('x', encoding='utf-8') as stream:
        json.dump(result, stream, indent=2, default=str)
        stream.write('\n')
    for group in result['threads']:
        if 'etw-dnstap' in group['process']:
            print(json.dumps({key: value for key, value in group.items() if key != 'seconds'}, default=str))


if __name__ == '__main__':
    main()
