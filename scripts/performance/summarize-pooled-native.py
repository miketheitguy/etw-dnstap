"""Index completed native comparison summaries without reopening ETLs."""

import argparse
import csv
import datetime
import json
from pathlib import Path
from perf_common import normalize_process as process_name, read_json as read


def named_threads(campaign, run, view):
    directory = campaign / 'wpa' / run / view
    summary = read(directory / 'scheduling-summary.json')
    process_cpu, thread_cpu = {}, {}
    with (directory / 'cpu-precise-exclusive.log').open(encoding='utf-8-sig') as stream:
        for row in csv.reader(stream):
            if len(row) not in (2, 3) or not row[0].strip().isdigit():
                continue
            process = process_name(row[1])
            if len(row) == 2:
                process_cpu[process] = int(row[0])
            elif row[2].strip().isdigit():
                thread_cpu[(process, int(row[2]))] = int(row[0])
    lifetimes = {}
    with (campaign / 'wpa' / run / 'inspect/process-thread-lifetimes.log').open(
            encoding='utf-8-sig') as stream:
        for row in csv.reader(stream):
            if len(row) < 6 or row[2].strip() != 'Thread' or not row[5].strip().isdigit():
                continue
            key = (process_name(row[4]), int(row[5]))
            lifetimes.setdefault(key, []).append(dict(
                start_us=int(row[0]) if row[0].strip().isdigit() else row[0].strip(),
                end_us=int(row[1]) if row[1].strip().isdigit() else row[1].strip()))
    selected = []
    fields = ('process', 'tid', 'names', 'rows', 'cpu_ms', 'positive_ready_us',
              'positive_wait_us', 'interior_positive_ready_us',
              'interior_positive_wait_us', 'ready_clipped_us', 'wait_clipped_us',
              'ready_before_view', 'wait_before_view', 'prev_states',
              'prev_wait_reasons', 'new_stack_rows', 'ready_stack_rows')
    for thread in summary['threads']:
        if not set(thread['names']).intersection(('dnstap-parser', 'etw-consumer')):
            continue
        key = (thread['process'], thread['tid'])
        selected.append(dict(**{key: thread[key] for key in fields},
            exclusive_cpu_us=thread_cpu[key], lifetimes=lifetimes[key]))
    if len(selected) != 2:
        raise ValueError(f'{run}/{view}: expected one parser and one consumer')
    return dict(source_summary=str(directory / 'scheduling-summary.json'),
        source_cpu=str(directory / 'cpu-precise-exclusive.log'),
        source_lifetimes=str(campaign / 'wpa' / run / 'inspect/process-thread-lifetimes.log'),
        start_s=summary['start_s'], end_s=summary['end_s'],
        exclusive_process_cpu_us=process_cpu, named_threads=selected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--campaign', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    stages = []
    for run in ('pooled-auth-1000-300s-2', 'pooled-rec-1000-300s-2',
                'pooled-auth-48000-300s-1', 'pooled-rec-28000-300s-1'):
        directory = args.campaign / 'wpa' / run
        broad = directory / 'broad/summary-v2.json'
        if not broad.exists():
            broad = directory / 'broad/summary.json'
        broad_summary = read(broad)
        if 'io_details' not in broad_summary:
            raise ValueError(f'{run}: use an explicitly time-filtered I/O summary')
        stage = read(args.campaign / 'stages' / run / 'stage-summary.json')
        gate = read(directory / 'trace-gate-v1.json')
        if not gate['trace_gate_pass']:
            raise ValueError(f'{run}: trace gate failed')
        stages.append(dict(run_id=run, source_stage=str(args.campaign / 'stages' / run / 'stage-summary.json'),
            source_trace_gate=str(directory / 'trace-gate-v1.json'),
            achieved_qps=stage['achieved_qps'], all_dns_validity_pass=stage['all_dns_validity_pass'],
            client_ra_missing=stage['client_ra_missing'],
            pdh_service_cpu=stage['service_cpu_one_core_percent'],
            full_sending_service_private_bytes=stage['service_private_bytes'],
            full_sending_service_working_set=stage['service_working_set'],
            source_broad=str(broad), broad=broad_summary,
            matched=named_threads(args.campaign, run, 'matched')))
    loss_run = 'pooled-rec-32000-60s-1'
    result = dict(schema=1, derivative=True,
        generated_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
        raw_etls_scanned=False, stages=stages,
        recursive_limit=dict(run_id=loss_run,
            source_drop_intervals=str(args.campaign / 'stages' / loss_run / 'drop-timeline.json'),
            early=named_threads(args.campaign, loss_run, 'early-loss'),
            late=named_threads(args.campaign, loss_run, 'late-loss')),
        semantics='Derived from completed native exports and small summaries. Matched views use service-only scheduling/sample filters; Xperf CPU includes all processes. Broad views use sending seconds 5–295. PDH CPU/private bytes cover full sending with their recorded coverage. Native CPU excludes observed DPC/ISR only where explicitly named exclusive. Raw Ready/Wait observations can cross boundaries; interior and clipped measures remain separate, and final tails can be absent. No forced wall-time sum, lock, timer, driver, or source-function cause is inferred. Trace fidelity and DNS/application/receiver/lifecycle gates are separate.')
    with args.output.open('x', encoding='utf-8') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps(dict(output=str(args.output), stages=len(stages), loss_views=2)))


if __name__ == '__main__':
    main()
