"""Summarize raw pooled WPA scheduling and stack exports without lock inference."""

import argparse
import csv
import json
import math
from collections import Counter, defaultdict
from pathlib import Path


def number(value):
    result = float(value.replace(',', ''))
    if not math.isfinite(result):
        raise ValueError('nonfinite exported number')
    return result


def distribution(values):
    ordered = sorted(values)
    return dict(count=len(ordered), sum=sum(ordered), max=max(ordered, default=None),
                **{f'p{p}': ordered[math.ceil(len(ordered) * p / 100) - 1]
                   if ordered else None for p in (50, 95, 99)})


def top(counter, limit=12):
    return [dict(value=value, weight=weight) for value, weight in counter.most_common(limit)]


def read_rows(path):
    with path.open(encoding='utf-8-sig', newline='') as stream:
        reader = csv.reader(stream)
        header = next(reader)
        for line, row in enumerate(reader, 2):
            if len(row) != len(header):
                raise ValueError(f'{path}:{line}: incomplete CSV row')
            yield header, row


def summarize(directory, start, end, processes):
    groups = defaultdict(lambda: dict(rows=0, names=set(), cpu_ms=0.0,
        positive_ready_us=[], positive_wait_us=[], interior_positive_wait_us=[],
        interior_positive_ready_us=[], switchins=[], ready_clipped_us=0.0,
        wait_clipped_us=0.0, ready_before_view=0, wait_before_view=0,
        prev_states=Counter(), prev_wait_reasons=Counter(), readying_cpu_ms=Counter(),
        readying_wait_us=Counter(), wait_stacks_us=Counter(), ready_stacks_us=Counter(),
        new_stack_rows=0, ready_stack_rows=0, symbols_disabled_rows=0))
    path = directory / 'evidence_CPU_Usage_(Precise)_Scheduling_and_readying_stacks.csv'
    for header, row in read_rows(path):
        if header[:7] != ['New Process', 'New Thread Id', 'Thread Name',
                'New Thread Stack', 'Readying Process', 'Readying Thread Id', 'Ready Thread Stack']:
            raise ValueError('unexpected scheduling profile')
        ix = lambda name: header.index(name)
        if row[0] not in processes:
            continue
        if number(row[ix('Count')]) != 1 or not row[1]:
            raise ValueError('expected raw scheduling leaf rows, not collapsed process groups')
        group = groups[(row[0], int(row[1]))]
        group['rows'] += 1
        if row[2]:
            group['names'].add(row[2])
        cpu = number(row[ix('CPU Usage (in view) (ms)')])
        ready = number(row[ix('Ready (µs)')])
        wait = number(row[ix('Waits (µs)')])
        switchin = number(row[ix('Switch-In Time (s)')])
        readytime = number(row[ix('Ready Time (s)')])
        switchout = number(row[ix('Last Switch-Out Time (s)')])
        group['cpu_ms'] += cpu
        group['switchins'].append(switchin)
        for key, value in (('positive_ready_us', ready), ('positive_wait_us', wait)):
            if value > 0:
                group[key].append(value)
        if start <= switchout <= readytime <= switchin <= end:
            if ready > 0:
                group['interior_positive_ready_us'].append(ready)
            if wait > 0:
                group['interior_positive_wait_us'].append(wait)
        group['ready_clipped_us'] += max(0, min(end, switchin) - max(start, readytime)) * 1e6
        group['wait_clipped_us'] += max(0, min(end, readytime) - max(start, switchout)) * 1e6
        group['ready_before_view'] += readytime < start
        group['wait_before_view'] += switchout < start
        group['prev_states'][row[ix('New Prev State')]] += 1
        group['prev_wait_reasons'][row[ix('New Prev Wait Reason')]] += 1
        readying = f'{row[4]} / TID {row[5]}'
        group['readying_cpu_ms'][readying] += cpu
        group['readying_wait_us'][readying] += wait
        group['wait_stacks_us'][row[3]] += wait
        group['ready_stacks_us'][row[6]] += ready
        group['new_stack_rows'] += row[3] not in ('', '[Root]')
        group['ready_stack_rows'] += row[6] not in ('', '[Root]')
        group['symbols_disabled_rows'] += '<Symbols disabled>' in row[3] + row[6]

    threads = []
    for (process, tid), group in groups.items():
        times = sorted(group.pop('switchins'))
        group['switchin_gap_us'] = distribution([(b - a) * 1e6 for a, b in zip(times, times[1:])])
        group['switchin_first_s'] = times[0]
        group['switchin_last_s'] = times[-1]
        group['names'] = sorted(group['names'])
        for key in ['positive_ready_us', 'positive_wait_us', 'interior_positive_wait_us',
                    'interior_positive_ready_us']:
            group[key] = distribution(group[key])
        for key in ['readying_cpu_ms', 'readying_wait_us', 'wait_stacks_us', 'ready_stacks_us']:
            group[key] = top(group[key])
        group['mean_cpu_one_core_percent'] = group['cpu_ms'] / ((end - start) * 10)
        threads.append(dict(process=process, tid=tid, **group))

    sampled = defaultdict(lambda: dict(rows=0, count=0, weight_ms=0.0,
        modules_ms=Counter(), stacks_ms=Counter(), stack_rows=0, symbols_disabled_rows=0))
    path = directory / 'evidence_CPU_Usage_(Sampled)_Sampled_CPU_by_process_thread_stack.csv'
    for header, row in read_rows(path):
        ix = lambda name: header.index(name)
        if row[0] not in processes:
            continue
        group = sampled[row[0]]
        weight = number(row[ix('Weight (in view) (ms)')])
        group['rows'] += 1
        group['count'] += number(row[ix('Count')])
        group['weight_ms'] += weight
        group['modules_ms'][row[ix('Module')]] += weight
        group['stacks_ms'][row[ix('Stack')]] += weight
        group['stack_rows'] += row[ix('Stack')] not in ('', '[Root]')
        group['symbols_disabled_rows'] += '<Symbols disabled>' in row[ix('Stack')]
    samples = []
    for process, group in sampled.items():
        for key in ['modules_ms', 'stacks_ms']:
            group[key] = top(group[key])
        samples.append(dict(process=process, **group))

    return dict(directory=str(directory), start_s=start, end_s=end,
        requested_processes=processes,
        missing_scheduling_processes=sorted(set(processes) - {g['process'] for g in threads}),
        semantics='CPU is scheduled CPU Usage(in view); use the matching Xperf exclusive report to exclude observed DPC/ISR. Ready/Wait distributions describe positive intervals associated with in-view switch-ins, not locks. Interior distributions exclude boundary-crossing intervals. Clipped occupancy sums still omit final tails without an in-view switch-in. Switch-in gaps include running plus nonrunning time. All sums aggregate thread time; no function attribution is inferred from source or dispatcher wait reasons. Sampled CPU is sampling evidence, not exact elapsed CPU.',
        threads=sorted(threads, key=lambda g: g['cpu_ms'], reverse=True), sampled=samples)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--start-s', type=float, required=True)
    parser.add_argument('--end-s', type=float, required=True)
    parser.add_argument('--process', action='append', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not math.isfinite(args.start_s) or not math.isfinite(args.end_s) or not 0 <= args.start_s < args.end_s:
        parser.error('require finite 0 <= start-s < end-s')
    report = summarize(args.directory, args.start_s, args.end_s, args.process)
    with args.output.open('x', encoding='utf-8') as stream:
        json.dump(report, stream, indent=2)
        stream.write('\n')
    print(json.dumps(dict(output=str(args.output), threads=len(report['threads']),
        missing_scheduling_processes=report['missing_scheduling_processes'])))


if __name__ == '__main__':
    main()
