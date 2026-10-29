"""Build a derivative WPR gate index from existing small inspection artifacts."""

import argparse
import csv
import datetime
import json
import re
from decimal import Decimal
from pathlib import Path


def duration(value):
    fields = value.split(':')
    result = Decimal(fields[-1])
    for field, multiplier in zip(reversed(fields[:-1]), (60, 3600, 86400)):
        result += int(field) * multiplier
    return float(result)


def utc(value):
    date, time = value.split(':', 1)
    return date.replace('/', '-') + 'T' + time + 'Z'


def field(text, expression):
    match = re.search(expression, text, re.MULTILINE)
    if not match:
        raise ValueError(f'missing required trace field: {expression}')
    return match[1]


def summarize(campaign):
    audit = campaign / 'build/management-overlap-audit-v1.json'
    perturbed = set()
    if audit.exists():
        for call in json.loads(audit.read_text(encoding='utf-8-sig'))['calls']:
            perturbed.update(call['receipt_inside_sending_windows'])
    gates = []
    for receipt_path in sorted((campaign / 'wpa').glob('*/inspect/receipt.json')):
        inspect = receipt_path.parent
        run = inspect.parent.name
        receipt = json.loads(receipt_path.read_text(encoding='utf-8-sig'))
        text = (inspect / 'trace-timespan.log').read_text(encoding='utf-8-sig')
        start = utc(field(text, r'^Start time \(UTC\)\s*:\s*(\S+)'))
        retained = duration(field(text, r'^End time \(UTC\).*?\(\+\s*([0-9:.]+)'))
        first = utc(field(text, r'^First reliable event\s*:\s*(\S+)'))
        last = utc(field(text, r'^Last event\s*:\s*(\S+)'))
        last_offset = duration(field(text, r'^Last event.*?\(\+\s*([0-9:.]+)'))
        lost_buffers = int(field(text, r'^Total # Lost Buffers\s*:\s*(\d+)'))
        lost_events = int(field(text, r'^Total # Lost Events\s*:\s*(\d+)'))
        intervals = []
        with (inspect / 'cswitch-full-coverage.log').open(encoding='utf-8-sig') as stream:
            for row in csv.reader(stream):
                if len(row) >= 3 and row[0].strip().isdigit() and row[1].strip().isdigit():
                    intervals.append((int(row[0]), int(row[1])))
        if not intervals:
            raise ValueError(f'{run}: no scheduling coverage rows')
        gaps = [(a[1], b[0]) for a, b in zip(intervals, intervals[1:]) if a[1] != b[0]]
        client_path = campaign / 'client' / run / 'summary.json'
        client = json.loads(client_path.read_text(encoding='utf-8-sig'))
        epoch = datetime.datetime.fromisoformat(start.replace('Z', '+00:00')).timestamp()
        client_start = client['start_unix_ms'] / 1000 - epoch
        client_end = client['sending_end_unix_ms'] / 1000 - epoch
        contained = (not gaps and intervals[0][0] / 1e6 <= client_start
                     and client_end <= min(retained, intervals[-1][1] / 1e6))
        results = []
        for result_path in sorted(inspect.glob('*-result.json')):
            result = json.loads(result_path.read_text(encoding='utf-8-sig'))
            results.append(dict(path=str(result_path), exit_code=result['exit_code']))
        passed = (lost_events == 0 and lost_buffers == 0 and not gaps
                  and contained and len(results) == 4
                  and all(result['exit_code'] == 0 for result in results))
        gates.append(dict(run_id=run, canonical_etl=receipt['trace'],
            canonical_etl_bytes=receipt['trace_bytes'], canonical_sha256=receipt['trace_sha256'],
            trace_start_utc=start, retained_duration_s=retained,
            first_reliable_event_utc=first, actual_last_event_utc=last,
            actual_last_event_offset_s=last_offset, lost_events=lost_events, lost_buffers=lost_buffers,
            cswitch_coverage=dict(start_s=intervals[0][0]/1e6, end_s=intervals[-1][1]/1e6,
                intervals=len(intervals), continuous=not gaps, gaps_us=gaps),
            client_trace_start_s=client_start, client_trace_end_s=client_end,
            client_interval_contained=contained, trace_gate_pass=passed,
            in_load_management_enumeration=run in perturbed,
            source_receipt=str(receipt_path), source_client=str(client_path),
            source_timespan=str(inspect/'trace-timespan.log'),
            source_coverage=str(inspect/'cswitch-full-coverage.log'),
            source_event_stacks=str(inspect/'trace-events-stacks.log'), tool_results=results))
    return dict(schema=1, derivative=True, generated_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
        campaign=str(campaign), raw_etls_scanned=False,
        method='Parse existing inspection receipts, trace headers, full-file one-second CSwitch utilization coverage, and client UTC windows. Hashes are copied from completed inspection receipts; no ETL rehash or export occurs. UTC alignment inherits the stage clock bounds. A passing WPR trace gate does not imply passing application, output, DNS, or receiver ledgers. Exact production application symbols are unavailable; selected windows do not establish whole-stage absence of locks.',
        gates=gates)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--campaign', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = summarize(args.campaign)
    # This index is explicitly derivative and can be refreshed; raw receipts stay intact.
    temporary = args.output.with_suffix(args.output.suffix + '.tmp')
    temporary.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    temporary.replace(args.output)
    for gate in result['gates']:
        receipt = args.campaign / 'wpa' / gate['run_id'] / 'trace-gate-v1.json'
        pending = receipt.with_suffix('.json.tmp')
        pending.write_text(json.dumps(dict(schema=1, derivative=True,
            scope='WPR trace loss/coverage only; application, DNS, output and receiver gates are separate.',
            **gate), indent=2) + '\n', encoding='utf-8')
        pending.replace(receipt)
    print(json.dumps(dict(output=str(args.output), runs=len(result['gates']),
        failing_trace_gates=[g['run_id'] for g in result['gates'] if not g['trace_gate_pass']])))


if __name__ == '__main__':
    main()
