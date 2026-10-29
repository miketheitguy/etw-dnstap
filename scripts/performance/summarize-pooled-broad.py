"""Summarize broad native WPA/Xperf exports between load stages."""

import argparse
import csv
import json
import re
from collections import Counter, defaultdict
from pathlib import Path


def normalize_process(value):
    return re.sub(r'\(\s*(\d+)\)', r'(\1)', value.strip())


def summarize(directory, processes):
    receipt = json.loads((directory / 'receipt.json').read_text(encoding='utf-8-sig'))
    seconds = receipt['end_seconds'] - receipt['start_seconds']
    if seconds <= 0:
        raise ValueError('expected a finite export window')
    samples = defaultdict(lambda: dict(rows=0, weight_ms=0.0, modules_ms=Counter()))
    sample_path = directory / 'evidence_CPU_Usage_(Sampled)_Sampled_CPU_by_process_thread_stack.csv'
    with sample_path.open(encoding='utf-8-sig', newline='') as stream:
        for row in csv.DictReader(stream):
            if row['Process'] not in processes:
                continue
            group = samples[row['Process']]
            weight = float(row['Weight (in view) (ms)'].replace(',', ''))
            group['rows'] += 1
            group['weight_ms'] += weight
            group['modules_ms'][row['Module']] += weight
    for group in samples.values():
        group['modules_ms'] = group['modules_ms'].most_common(12)
    cpu = {}
    with (directory / 'cpu-precise-exclusive.log').open(encoding='utf-8-sig') as stream:
        for line in stream:
            match = re.match(r'^\s*(\d+),\s*(.+\(\s*\d+\))\s*$', line)
            if match and normalize_process(match[2]) in processes:
                process = normalize_process(match[2])
                cpu[process] = dict(cpu_us=int(match[1]),
                    one_core_percent=int(match[1]) / seconds / 10000)
    io = {}
    for kind, name in [
        ('disk', 'evidence_Disk_Usage_Disk_IO_with_initiation_stacks.csv'),
        ('hard_fault', 'evidence_Hard_Faults_Summary_Table_Hard_faults_by_process_file_stack.csv'),
    ]:
        counts = Counter()
        with (directory / name).open(encoding='utf-8-sig', newline='') as stream:
            for row in csv.DictReader(stream):
                if row['Process'] in processes:
                    counts[row['Process']] += int(row['Count'].replace(',', ''))
        io[kind] = {process: counts[process] for process in processes}
    file_io = dict(event_counts_all_processes=Counter(), target_event_counts=defaultdict(Counter),
        target_paths=defaultdict(Counter), target_rw_bytes=Counter())
    file_path = directory / 'file-io-events.log'
    if file_path.exists():
        headers = {}
        in_header = True
        with file_path.open(encoding='utf-8-sig', errors='strict', newline='') as stream:
            for row in csv.reader(stream, skipinitialspace=True):
                if not row:
                    continue
                if row[0] == 'EndHeader':
                    in_header = False
                    continue
                if in_header:
                    if row[0].startswith('FileIo'):
                        headers[row[0]] = row
                    continue
                kind = row[0].strip()
                if not kind.startswith('FileIo') or len(row) < 4:
                    continue
                file_io['event_counts_all_processes'][kind] += 1
                process = normalize_process(row[2])
                if process not in processes:
                    continue
                file_io['target_event_counts'][process][kind] += 1
                header = headers[kind]
                if 'FileName' in header:
                    file_io['target_paths'][process][row[header.index('FileName')]] += 1
                if kind in ('FileIoRead', 'FileIoWrite'):
                    file_io['target_rw_bytes'][process] += int(row[header.index('Size')], 0)
        file_io['target_paths'] = {p: c.most_common(12) for p, c in file_io['target_paths'].items()}
    else:
        file_io = None
    return dict(directory=str(directory), start_s=receipt['start_seconds'],
        end_s=receipt['end_seconds'], seconds=seconds, requested_processes=processes,
        exclusive_cpu=cpu, sampled=samples, io_event_counts=io, file_io=file_io,
        semantics='CPU excludes observed DPC/ISR per recorded Xperf action. Sample weight is statistical CPU evidence. Target disk attribution can omit cached writes completed by System. FileIo includes cached file operations and does not cover TCP or every form of I/O. Zero target hard faults or disk events does not imply no I/O. FileIo path counts include operation-end records as well as requests.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--process', action='append', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = summarize(args.directory, args.process)
    with args.output.open('x', encoding='utf-8') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps({key: value for key, value in result.items() if key != 'sampled'}))


if __name__ == '__main__':
    main()
