"""Stream-count one parser-ramp receiver stdout capture and reconcile its domains."""
import argparse
from collections import Counter
import datetime
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--raw', required=True, type=Path,
                    help='Stage-isolated journal -o cat capture, preserved unchanged')
parser.add_argument('--run-id', required=True)
parser.add_argument('--campaign-id', default='parser-ramp-20261030')
parser.add_argument('--mode', choices=('authoritative', 'recursive'), required=True)
parser.add_argument('--capture-policy', choices=('all_supported', 'client_only'), default='all_supported')
parser.add_argument('--qps', required=True, type=int)
parser.add_argument('--duration', required=True, type=int)
parser.add_argument('--names-file', required=True, type=Path)
parser.add_argument('--client-result', required=True, type=Path)
parser.add_argument('--invocation', required=True, type=Path)
parser.add_argument('--preflight', required=True, type=Path)
parser.add_argument('--receiver-ready', required=True, type=Path)
parser.add_argument('--start-marker', required=True, type=Path)
parser.add_argument('--end-marker', required=True, type=Path)
parser.add_argument('--client-exit', required=True, type=Path)
parser.add_argument('--completion', required=True, type=Path)
parser.add_argument('--service-stop', required=True, type=Path)
parser.add_argument('--errors', required=True, type=Path)
parser.add_argument('--archive-manifest', required=True, type=Path)
parser.add_argument('--transfer-proof', required=True, type=Path)
parser.add_argument('--output', required=True, type=Path)
args = parser.parse_args()

if not args.raw.is_file():
    raise SystemExit('Raw capture does not exist')
if args.output.exists():
    raise SystemExit('Refusing to overwrite reconciliation evidence')

zone = 'auth.stress.test' if args.mode == 'authoritative' else 'rec.stress.test'
ready = json.loads(args.receiver_ready.read_text(encoding='utf-8-sig'))
start = json.loads(args.start_marker.read_text(encoding='utf-8-sig'))
end = json.loads(args.end_marker.read_text(encoding='utf-8-sig'))
invocation = json.loads(args.invocation.read_text(encoding='utf-8-sig'))
preflight = json.loads(args.preflight.read_text(encoding='utf-8-sig'))
client_exit = json.loads(args.client_exit.read_text(encoding='utf-8-sig'))
completion = json.loads(args.completion.read_text(encoding='utf-8-sig'))
service_stop = json.loads(args.service_stop.read_text(encoding='utf-8-sig'))
archive_manifest = json.loads(args.archive_manifest.read_text(encoding='utf-8-sig'))
transfer_proof = json.loads(args.transfer_proof.read_text(encoding='utf-8-sig'))
for field, expected in (('run', args.run_id), ('mode', args.mode),
                        ('configured_global_qps', args.qps), ('duration_seconds', args.duration)):
    if ready.get(field) != expected:
        raise SystemExit(f'Receiver READY {field} does not match requested stage')
if ready.get('campaign_id', 'parser-ramp-20261030') != args.campaign_id:
    raise SystemExit('Receiver READY campaign ID does not match requested campaign')
if ready.get('capture_policy', 'all_supported') != args.capture_policy:
    raise SystemExit('Receiver READY capture policy does not match requested policy')
legacy = args.capture_policy == 'all_supported'
if start.get('campaign_id', args.campaign_id if legacy else None) != args.campaign_id:
    raise SystemExit('Start marker campaign ID mismatch')
if end.get('campaign_id', args.campaign_id if legacy else None) != args.campaign_id:
    raise SystemExit('End marker campaign ID mismatch')
if start.get('capture_policy', 'all_supported' if legacy else None) != args.capture_policy:
    raise SystemExit('Start marker capture policy mismatch')
if end.get('capture_policy', 'all_supported' if legacy else None) != args.capture_policy:
    raise SystemExit('End marker capture policy mismatch')
expected_operations = (['CLIENT_QUERY', 'CLIENT_RESPONSE'] if args.capture_policy == 'client_only'
                       else ['CLIENT_QUERY', 'CLIENT_RESPONSE', 'RESOLVER_QUERY', 'RESOLVER_RESPONSE'])
if ready.get('expected_operations', expected_operations) != expected_operations:
    raise SystemExit('Receiver READY expected-operation list does not match capture policy')
if args.capture_policy == 'client_only':
    expected_source_filter = {'provider_event_ids': [256, 257], 'level': 4,
                              'keyword_mask': '0x8000000000000000'}
    if (ready.get('source_filter_expected') != expected_source_filter
            or start.get('expected_operations') != expected_operations
            or start.get('source_filter_expected') != expected_source_filter):
        raise SystemExit('Receiver READY/start marker do not carry the expected client-only source-filter contract')
    if ('expected_operations' in end and end.get('expected_operations') != expected_operations):
        raise SystemExit('End marker expected-operation list does not match the client-only contract')
    if ('source_filter_expected' in end and end.get('source_filter_expected') != expected_source_filter):
        raise SystemExit('End marker source-filter contract mismatch')
    expected_binary_hash = preflight.get('expected_binary_sha256')
    if (preflight.get('campaign_id') != args.campaign_id
            or preflight.get('capture_policy') != 'client_only'
            or preflight.get('source_filter') != expected_source_filter
            or not isinstance(expected_binary_hash, str)
            or len(expected_binary_hash) != 64
            or any(ch not in '0123456789abcdefABCDEF' for ch in expected_binary_hash)):
        raise SystemExit('Preflight does not confirm the requested client-only source filter')
    end_filter_contract_inherited = ('expected_operations' not in end
                                     or 'source_filter_expected' not in end)
else:
    end_filter_contract_inherited = False
if ready.get('receiver_ready') is not True or ready.get('start_marker_ready') is not True:
    raise SystemExit('Receiver READY/start marker gate is false')
if start.get('run') != args.run_id or start.get('offset') != 0:
    raise SystemExit('Start marker run identity or fresh-file offset mismatch')
if end.get('run') != args.run_id or end.get('marker') != f'{args.run_id}-end.json':
    raise SystemExit('End marker run identity mismatch')
if (start.get('raw_path') != end.get('raw_path')
        or Path(end.get('raw_path', '')).name != f'{args.run_id}.jsonl'):
    raise SystemExit('Receiver markers do not point to this isolated raw capture')
if ready.get('raw_path') != start.get('raw_path'):
    raise SystemExit('Receiver READY and start marker raw paths differ')
if start.get('collector_pid') != end.get('collector_pid'):
    raise SystemExit('Collector PID changed during the stage')
if end.get('collector_final_pid') != 0 or end.get('collector_stop_exit_code') != 0:
    raise SystemExit('Receiver stop was not clean after sender drain')
if (ready.get('collector_config_sha256') != start.get('collector_config_sha256')
        or start.get('collector_config_sha256') != end.get('collector_config_sha256')
        or ready.get('collector_binary_sha256') != start.get('collector_binary_sha256')
        or start.get('collector_binary_sha256') != end.get('collector_binary_sha256')):
    raise SystemExit('Collector config or binary identity changed during the stage')
if end.get('offset') != end.get('raw_bytes') or args.raw.stat().st_size != end.get('raw_bytes'):
    raise SystemExit('Raw size does not match final stable marker offset')
if (archive_manifest.get('run') != args.run_id
        or archive_manifest.get('raw_sha256') != end.get('raw_sha256')
        or archive_manifest.get('raw_bytes') != end.get('raw_bytes')
        or archive_manifest.get('roundtrip_sha256') != end.get('raw_sha256')
        or archive_manifest.get('roundtrip_bytes') != end.get('raw_bytes')):
    raise SystemExit('Guest archive manifest does not match the final source marker')
if (transfer_proof.get('run') != args.run_id or transfer_proof.get('gzip_integrity_verified') is not True
        or transfer_proof.get('source_hashes_match') is not True
        or transfer_proof.get('raw_sha256') != end.get('raw_sha256')
        or transfer_proof.get('raw_bytes') != end.get('raw_bytes')
        or transfer_proof.get('archive_sha256') != archive_manifest.get('compressed_sha256')):
    raise SystemExit('Host compressed-transfer round-trip receipt does not match guest source')
if not args.errors.is_file():
    raise SystemExit('Collector stderr evidence file is missing')
errors_hash = hashlib.file_digest(args.errors.open('rb'), 'sha256').hexdigest()
if args.errors.stat().st_size != end.get('errors_bytes') or errors_hash != end.get('errors_sha256'):
    raise SystemExit('Collector stderr size/hash differs from final marker')
stable_samples = end.get('publication_stability_samples')
if not isinstance(stable_samples, list) or len(stable_samples) < 4 or any(
        sample.get('offset') != end.get('offset') or sample.get('collector_pid') != end.get('collector_final_pid')
        or sample.get('tcp_6000_established_peers') for sample in stable_samples[-4:]):
    raise SystemExit('End marker lacks four stable publication samples with no TCP peer')
pre_stop_samples = end.get('pre_stop_publication_samples')
if not isinstance(pre_stop_samples, list) or len(pre_stop_samples) < 4 or any(
        sample.get('offset') != pre_stop_samples[-1].get('offset')
        or sample.get('collector_pid') != start.get('collector_pid')
        or sample.get('tcp_6000_established_peers') for sample in pre_stop_samples[-4:]):
    raise SystemExit('Receiver was not stable and disconnected before graceful stop')
if (client_exit.get('exit_code') != 0 or completion.get('failure') not in (None, '')
        or service_stop.get('State') != 'Stopped' or service_stop.get('ExitCode') != 0
        or service_stop.get('ServiceSpecificExitCode') != 0):
    raise SystemExit('Client exit or stage completion receipt indicates failure')
if completion.get('run') != args.run_id or completion.get('mode') != args.mode:
    raise SystemExit('Stage completion run identity mismatch')

client = json.loads(args.client_result.read_text(encoding='utf-8-sig'))
if not isinstance(client, dict) or client.get('schema_version') != 1:
    raise SystemExit('Unsupported or malformed dns-stress summary schema')
counts = client.get('counts')
if not isinstance(counts, dict):
    raise SystemExit('Client summary has no counts object')
required = ('sent', 'completed', 'successful', 'timeouts', 'send_errors',
            'receive_errors', 'malformed', 'mismatched', 'unsolicited',
            'late', 'duplicate', 'aa_missing', 'ra_missing', 'rd_mismatch',
            'truncated', 'noerror_without_answers')
for key in required:
    value = counts.get(key)
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise SystemExit(f'Client count {key!r} is missing or invalid')
outstanding = client.get('outstanding_final')
if isinstance(outstanding, bool) or not isinstance(outstanding, int) or outstanding < 0:
    raise SystemExit('Client outstanding_final is missing or invalid')
rcodes = counts.get('rcode')
if (not isinstance(rcodes, list) or len(rcodes) != 16
        or any(isinstance(value, bool) or not isinstance(value, int) or value < 0 for value in rcodes)):
    raise SystemExit('Client rcode counters are missing or invalid')
options = client.get('options')
expected_server = f"{preflight.get('ip')}:53"
if (not isinstance(options, dict) or options.get('mode') != args.mode
        or options.get('qps') != args.qps or options.get('duration') != args.duration
        or options.get('server') != expected_server):
    raise SystemExit('Client summary mode/qps/duration/server do not match the stage')
argv = invocation.get('arguments')
if not isinstance(argv, list) or not isinstance(invocation.get('command'), str):
    raise SystemExit('Windows stage invocation is missing its command/arguments')
def flag_value(flag):
    try:
        return argv[argv.index(flag) + 1]
    except (ValueError, IndexError):
        return None
if (flag_value('--mode') != args.mode or flag_value('--qps') != str(args.qps)
        or flag_value('--duration') != str(args.duration)
        or flag_value('--server') != expected_server):
    raise SystemExit('Windows invocation mode/qps/duration/server do not match stage')
def normalized_path(value):
    return Path(str(value).strip('"')).resolve()
expected_client_dir = (args.invocation.parent / 'client').resolve()
if (args.client_result.name != 'summary.json'
        or args.client_result.parent.resolve() != expected_client_dir
        or normalized_path(options.get('output_dir', '')) != expected_client_dir
        or normalized_path(flag_value('--output-dir') or '') != expected_client_dir):
    raise SystemExit('Client summary/output directory is not bound to this exact stage invocation')
if (not invocation['command'].lower().endswith('dns-stress.exe')
        or args.invocation.parent.name != args.run_id):
    raise SystemExit('Unexpected load generator executable or invocation path')
if invocation.get('exe_sha256', '').upper() != '7A5800CFC4252196E4C6162F7D2CCF81217FE95A339D7CFBB9CD84063F4B6960':
    raise SystemExit('Load generator binary hash differs from the preserved client identity')
names_digest = hashlib.file_digest(args.names_file.open('rb'), 'sha256').hexdigest().upper()
if invocation.get('names_sha256', '').upper() != names_digest:
    raise SystemExit('Windows invocation name-list hash does not match the selected workload file')
if Path(options.get('names', '')).exists():
    summary_names_digest = hashlib.file_digest(Path(options['names']).open('rb'), 'sha256').hexdigest().upper()
    if summary_names_digest != names_digest:
        raise SystemExit('Client summary name-list path/hash differs from selected workload file')
if client_exit.get('exit_code') != 0 or completion.get('failure') not in (None, ''):
    raise SystemExit('Client exit or stage completion receipt indicates failure')
if completion.get('run') != args.run_id or completion.get('mode') != args.mode:
    raise SystemExit('Stage completion run identity mismatch')
operations = Counter()
rcode = Counter()
malformed_lines = diagnostic_lines = json_rows = other_zone = unrelated = 0
malformed_dns = unknown_operations = 0
duplicate_lines = 0
previous_line_hash = None
first_timestamp = last_timestamp = None
raw_hash = hashlib.sha256()
raw_bytes = 0
start_offset, end_offset = start['offset'], end['offset']
if (isinstance(start_offset, bool) or isinstance(end_offset, bool)
        or not isinstance(start_offset, int) or not isinstance(end_offset, int)
        or not 0 <= start_offset <= end_offset):
    raise SystemExit('Invalid receiver byte offsets')

with args.raw.open('rb') as stream:
    stream.seek(start_offset)
    for line in stream:
        line_start = stream.tell() - len(line)
        if line_start >= end_offset:
            break
        if stream.tell() > end_offset:
            raise SystemExit('Stage end marker cuts through a JSONL record')
        raw_hash.update(line)
        raw_bytes += len(line)
        try:
            row = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            stripped = line.lstrip()
            if stripped.startswith((b'INFO:', b'WARN:', b'WARNING:', b'ERROR:', b'DEBUG:')):
                diagnostic_lines += 1
            elif stripped:
                malformed_lines += 1
            continue
        if not isinstance(row, dict):
            malformed_lines += 1
            continue
        json_rows += 1
        raw_line = line.rstrip(b'\r\n')
        digest = hashlib.sha256(raw_line).digest()
        if digest == previous_line_hash:
            duplicate_lines += 1
        previous_line_hash = digest
        dns = row.get('dns') if isinstance(row.get('dns'), dict) else {}
        qname = dns.get('qname', '')
        if not isinstance(qname, str) or not qname:
            unrelated += 1
            continue
        name = qname.rstrip('.').lower()
        if not name.endswith('.' + zone):
            if name.endswith('.auth.stress.test') or name.endswith('.rec.stress.test'):
                other_zone += 1
            else:
                unrelated += 1
            continue
        tap = row.get('dnstap') if isinstance(row.get('dnstap'), dict) else {}
        op = tap.get('operation', '?')
        if not isinstance(op, str):
            op = '?'
        if op not in ('CLIENT_QUERY', 'CLIENT_RESPONSE', 'RESOLVER_QUERY', 'RESOLVER_RESPONSE'):
            unknown_operations += 1
        else:
            operations[op] += 1
        if op.endswith('_RESPONSE'):
            rcode[str(dns.get('rcode', '?'))] += 1
        malformed_dns += int(bool(dns.get('malformed-packet')))
        stamp = tap.get('timestamp-rfc3339ns', tap.get('timestamp'))
        first_timestamp = first_timestamp or stamp
        last_timestamp = stamp

raw_stat = args.raw.stat()
if not raw_bytes or (raw_bytes and not line.endswith(b'\n')):
    raise SystemExit('Stage output is empty or ends with an incomplete JSONL record')
if raw_bytes != end_offset - start_offset or raw_hash.hexdigest() != end.get('raw_sha256'):
    raise SystemExit('Streamed stage byte count/hash differs from the final receiver marker')
if raw_stat.st_size != end_offset:
    raise SystemExit('Raw file extends beyond its final receiver marker')
unexpected_resolver = (args.mode == 'authoritative'
                       and (operations.get('RESOLVER_QUERY', 0) or operations.get('RESOLVER_RESPONSE', 0)))
resolver_records = operations.get('RESOLVER_QUERY', 0) + operations.get('RESOLVER_RESPONSE', 0)
unexpected_filtered_resolver = args.capture_policy == 'client_only' and resolver_records != 0
capture_content_clean = not (unknown_operations or malformed_lines or malformed_dns
                              or other_zone or unrelated or unexpected_resolver
                              or unexpected_filtered_resolver)
result = {
    'run_id': args.run_id,
    'campaign_id': args.campaign_id,
    'mode': args.mode,
    'capture_policy': args.capture_policy,
    'source_filter_verified': args.capture_policy == 'all_supported' or preflight.get('source_filter') == expected_source_filter,
    'source_filter_contract': (expected_source_filter if args.capture_policy == 'client_only' else None),
    'end_filter_contract_inherited_from_start': end_filter_contract_inherited,
    'preflight_expected_service_binary_sha256': (preflight.get('expected_binary_sha256')
                                                 if args.capture_policy == 'client_only' else None),
    'zone': zone,
    'captured_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'raw_capture': str(args.raw),
    'raw_bytes': raw_bytes,
    'raw_sha256': raw_hash.hexdigest(),
    'raw_mtime_ns': raw_stat.st_mtime_ns,
    'byte_range': [start_offset, end_offset],
    'receiver_pid_unchanged': start['collector_pid'] == end['collector_pid'],
    'receiver_stopped_after_final_drain': end.get('collector_final_pid') == 0,
    'stable_publication_confirmed': True,
    'guest_archive_manifest': str(args.archive_manifest),
    'host_transfer_proof': str(args.transfer_proof),
    'collector_stderr_bytes': args.errors.stat().st_size,
    'collector_stderr_sha256': errors_hash,
    'json_rows': json_rows,
    'operation_counts': {key: operations.get(key, 0) for key in
                         ('CLIENT_QUERY', 'CLIENT_RESPONSE', 'RESOLVER_QUERY', 'RESOLVER_RESPONSE')},
    'response_rcodes': dict(sorted(rcode.items())),
    'known_operation_records': sum(operations.values()),
    'selected_zone_unknown_operation_records': unknown_operations,
    'selected_zone_records': sum(operations.values()) + unknown_operations,
    'capture_content_clean': capture_content_clean,
    'unexpected_resolver_records_in_authoritative_mode': bool(unexpected_resolver),
    'unexpected_resolver_records_under_client_only_filter': bool(unexpected_filtered_resolver),
    'malformed_dns_records': malformed_dns,
    'malformed_journal_lines': malformed_lines,
    'diagnostic_journal_lines': diagnostic_lines,
    'other_test_zone_records': other_zone,
    'unrelated_json_records': unrelated,
    'unknown_operations_in_selected_zone': unknown_operations,
    'duplicate_identical_consecutive_json_lines': duplicate_lines,
    'first_provider_timestamp': first_timestamp,
    'last_provider_timestamp': last_timestamp,
    'prometheus_receiver_counters': None,
    'prometheus_receiver_counters_note': 'No metrics listener was present in live preflight; do not infer ingress/egress/drop counters from stdout counts.',
    'loss_attribution': 'Receiver record counts establish what was emitted to stdout. Missing records cannot be assigned specifically to ETW, ring admission, TCP, or collector without independent counters.'
}
result['client_result'] = str(args.client_result)
result['client_options'] = options
result['client_counts'] = {key: counts[key] for key in required}
result['client_rcode_counts'] = rcodes
result['client_outstanding_final'] = outstanding
result['client_sent'] = counts['sent']
result['client_completed_matched_responses'] = counts['completed']
result['client_successful'] = counts['successful']
anomalies = ('timeouts', 'send_errors', 'receive_errors', 'malformed', 'mismatched',
             'unsolicited', 'late', 'duplicate', 'aa_missing', 'ra_missing',
             'rd_mismatch', 'truncated', 'noerror_without_answers')
bad_client = {key: counts[key] for key in anomalies if counts[key]}
if rcodes[2]:
    bad_client['servfail_rcode'] = rcodes[2]
result['client_anomalies'] = bad_client
result['client_dns_path_clean_for_record_reconciliation'] = (
    not bad_client and counts['sent'] == counts['completed'] == counts['successful']
    and outstanding == 0)
result['client_query_response_count_residuals'] = {
    'client_sent_minus_CLIENT_QUERY': counts['sent'] - operations.get('CLIENT_QUERY', 0),
    'client_completed_minus_CLIENT_RESPONSE': counts['completed'] - operations.get('CLIENT_RESPONSE', 0)}
result['client_event_count_residual_interpretation'] = (
    'Count differences are capture-attributable only when client DNS path is clean; otherwise report ambiguity.')

def counter_delta(before, after, key):
    if not isinstance(before, dict) or not isinstance(after, dict):
        return None
    left, right = before.get(key), after.get(key)
    if (isinstance(left, bool) or isinstance(right, bool)
            or not isinstance(left, int) or not isinstance(right, int)):
        return None
    return right - left

udp_start, udp_end = start.get('udp_counters'), end.get('udp_counters')
result['udp_counter_deltas'] = {
    key: counter_delta(udp_start, udp_end, key)
    for key in ('InDatagrams', 'NoPorts', 'InErrors', 'OutDatagrams', 'RcvbufErrors', 'SndbufErrors')}
result['udp_counters_monotonic'] = all(value is None or value >= 0
                                       for value in result['udp_counter_deltas'].values())
result['udp_loss_counters_zero'] = (result['udp_counter_deltas'].get('InErrors') == 0
                                    and result['udp_counter_deltas'].get('RcvbufErrors') == 0)

def upstream_packet_delta_set():
    keys = ('upstream_queries_udp', 'upstream_queries_tcp',
            'upstream_responses_udp', 'upstream_responses_tcp',
            'upstream_other_udp_dropped', 'upstream_other_tcp_dropped',
            'upstream_queries', 'upstream_responses', 'upstream_other_dropped')
    return {key: counter_delta(start.get('upstream_nft_counters'),
                               end.get('upstream_nft_counters'), key)
            for key in keys}

upstream_packet_deltas = upstream_packet_delta_set()
result['upstream_nft_packet_deltas'] = upstream_packet_deltas
if args.capture_policy == 'client_only':
    upstream_q = counter_delta(start.get('upstream_nft_counters'), end.get('upstream_nft_counters'), 'upstream_queries')
    upstream_r = counter_delta(start.get('upstream_nft_counters'), end.get('upstream_nft_counters'), 'upstream_responses')
    result['upstream_evidence_available'] = upstream_q is not None and upstream_r is not None
    result['upstream_expected_by_workload'] = (
        'The client-only ETW filter expects zero resolver dnstap records. Upstream packet counters are retained '
        'as independent workload diagnostics and are not compared to resolver operation counts.')
    if args.mode == 'recursive':
        if (upstream_q is None or upstream_r is None
                or any(value is None for value in upstream_packet_deltas.values())):
            raise SystemExit('Recursive stage lacks actual upstream query/response packet counter snapshots')
        if any(value < 0 for value in upstream_packet_deltas.values()):
            raise SystemExit('Upstream nft packet counter decreased during the stage')
else:
    if args.mode == 'authoritative':
        upstream_q = counter_delta(start.get('upstream_nft_counters'), end.get('upstream_nft_counters'), 'upstream_queries')
        upstream_r = counter_delta(start.get('upstream_nft_counters'), end.get('upstream_nft_counters'), 'upstream_responses')
        result['upstream_evidence_available'] = upstream_q is not None and upstream_r is not None
        result['upstream_expected_by_workload'] = 'No recursive upstream fixture is configured in authoritative mode; resolver operations are expected absent, while packet counters remain unmeasured when nft evidence is unavailable.'
    else:
        upstream_q = counter_delta(start.get('upstream_nft_counters'), end.get('upstream_nft_counters'), 'upstream_queries')
        upstream_r = counter_delta(start.get('upstream_nft_counters'), end.get('upstream_nft_counters'), 'upstream_responses')
        if (upstream_q is None or upstream_r is None
                or any(value is None for value in upstream_packet_deltas.values())):
            raise SystemExit('Recursive stage lacks actual upstream query/response packet counter snapshots')
        if any(value < 0 for value in upstream_packet_deltas.values()):
            raise SystemExit('Upstream nft packet counter decreased during the stage')
if ((upstream_q is not None and upstream_q < 0)
        or (upstream_r is not None and upstream_r < 0)):
    raise SystemExit('Upstream counter decreased during the stage')
result['actual_upstream_queries'] = upstream_q
result['actual_upstream_responses'] = upstream_r
result['upstream_query_count_residual'] = (None if args.capture_policy == 'client_only' else
    (upstream_q - operations.get('RESOLVER_QUERY', 0) if upstream_q is not None else None))
result['upstream_response_count_residual'] = (None if args.capture_policy == 'client_only' else
    (upstream_r - operations.get('RESOLVER_RESPONSE', 0) if upstream_r is not None else None))
result['expected_client_query_count'] = counts['sent']
result['expected_client_response_count'] = counts['completed']
result['operation_directional_residuals'] = {
    'client_query': counts['sent'] - operations.get('CLIENT_QUERY', 0),
    'client_response': counts['completed'] - operations.get('CLIENT_RESPONSE', 0),
    'resolver_query': (None if args.capture_policy == 'client_only' else result['upstream_query_count_residual']),
    'resolver_response': (None if args.capture_policy == 'client_only' else result['upstream_response_count_residual'])}
result['expected_resolver_query_records'] = (0 if args.capture_policy == 'client_only' else None)
result['expected_resolver_response_records'] = (0 if args.capture_policy == 'client_only' else None)
result['receiver_record_reconciliation_clean'] = (
    result['client_dns_path_clean_for_record_reconciliation'] and capture_content_clean
    and result['udp_counters_monotonic'] and result['udp_loss_counters_zero']
    and all(value == 0 for value in result['operation_directional_residuals'].values()
            if value is not None)
    and (args.mode == 'authoritative' or (
        upstream_q is not None and upstream_r is not None
        and upstream_packet_deltas.get('upstream_other_dropped') == 0)))
result['client_measured_send_duration_ns'] = client.get('measured_send_duration_ns')
result['client_elapsed_with_drain_ns'] = client.get('elapsed_with_drain_ns')

args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
print(json.dumps(result))
