"""Count one parser-ramp stage while preserving strict-gate failures as diagnostics."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--raw', required=True, type=Path)
p.add_argument('--run-id', required=True)
p.add_argument('--campaign-id', default='parser-ramp-20261030')
p.add_argument('--mode', choices=('authoritative', 'recursive'), required=True)
p.add_argument('--capture-policy', choices=('all_supported', 'client_only'), default='all_supported')
p.add_argument('--client-result', required=True, type=Path)
p.add_argument('--start-marker', required=True, type=Path)
p.add_argument('--end-marker', required=True, type=Path)
p.add_argument('--receiver-ready', type=Path)
p.add_argument('--preflight', type=Path)
p.add_argument('--service-stop', required=True, type=Path)
p.add_argument('--completion', required=True, type=Path)
p.add_argument('--output', required=True, type=Path)
a = p.parse_args()
if a.output.exists():
    raise SystemExit('Refusing to overwrite diagnostic evidence')
zone = 'auth.stress.test' if a.mode == 'authoritative' else 'rec.stress.test'
start = json.loads(a.start_marker.read_text(encoding='utf-8-sig'))
end = json.loads(a.end_marker.read_text(encoding='utf-8-sig'))
summary = json.loads(a.client_result.read_text(encoding='utf-8-sig'))
stop = json.loads(a.service_stop.read_text(encoding='utf-8-sig'))
completion = json.loads(a.completion.read_text(encoding='utf-8-sig'))
if start.get('run') != a.run_id or end.get('run') != a.run_id:
    raise SystemExit('Marker run identity mismatch')
legacy = a.capture_policy == 'all_supported'
if start.get('campaign_id', 'parser-ramp-20261030' if legacy else None) != a.campaign_id or end.get('campaign_id', a.campaign_id if legacy else None) != a.campaign_id:
    raise SystemExit('Marker campaign identity mismatch')
if start.get('capture_policy', 'all_supported' if legacy else None) != a.capture_policy or end.get('capture_policy', 'all_supported' if legacy else None) != a.capture_policy:
    raise SystemExit('Marker capture policy mismatch')
source_filter_verified = a.capture_policy != 'client_only'
if a.capture_policy == 'client_only':
    if a.receiver_ready is None or a.preflight is None:
        raise SystemExit('Client-only diagnostic requires receiver READY and source-filter preflight receipts')
    ready = json.loads(a.receiver_ready.read_text(encoding='utf-8-sig'))
    preflight = json.loads(a.preflight.read_text(encoding='utf-8-sig'))
    expected_filter = {'provider_event_ids': [256, 257], 'level': 4,
                       'keyword_mask': '0x8000000000000000'}
    expected_ops = ['CLIENT_QUERY', 'CLIENT_RESPONSE']
    candidate_hash = preflight.get('expected_binary_sha256')
    if (ready.get('campaign_id') != a.campaign_id or ready.get('capture_policy') != 'client_only'
            or ready.get('expected_operations') != expected_ops
            or ready.get('source_filter_expected') != expected_filter
            or start.get('expected_operations') != expected_ops
            or start.get('source_filter_expected') != expected_filter
            or preflight.get('campaign_id') != a.campaign_id
            or preflight.get('capture_policy') != 'client_only'
            or preflight.get('source_filter') != expected_filter
            or not isinstance(candidate_hash, str) or len(candidate_hash) != 64
            or any(ch not in '0123456789abcdefABCDEF' for ch in candidate_hash)):
        raise SystemExit('Client-only READY/preflight filter contract mismatch')
    if ('expected_operations' in end and end.get('expected_operations') != expected_ops):
        raise SystemExit('End marker operation contract mismatch')
    if ('source_filter_expected' in end and end.get('source_filter_expected') != expected_filter):
        raise SystemExit('End marker source-filter contract mismatch')
    source_filter_verified = True
    end_filter_contract_inherited = ('expected_operations' not in end
                                     or 'source_filter_expected' not in end)
else:
    end_filter_contract_inherited = False
if start.get('raw_path') != end.get('raw_path') or end.get('marker') != f'{a.run_id}-end.json':
    raise SystemExit('Markers do not bind the same stage raw file')
if a.raw.stat().st_size != end.get('offset') or end.get('offset') != end.get('raw_bytes'):
    raise SystemExit('Raw file size differs from final offset')
counts, rcodes = Counter(), Counter()
unknown = malformed = diagnostics = json_rows = other_zone = unrelated = malformed_dns = 0
raw_hash = hashlib.sha256()
prev_hash = None
duplicates = 0
with a.raw.open('rb') as stream:
    stream.seek(start['offset'])
    while stream.tell() < end['offset']:
        line = stream.readline()
        if not line or stream.tell() > end['offset']:
            raise SystemExit('Incomplete line or byte-range overrun')
        raw_hash.update(line)
        try:
            row = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            stripped = line.lstrip()
            if stripped.startswith((b'INFO:', b'WARN:', b'WARNING:', b'ERROR:', b'DEBUG:')):
                diagnostics += 1
            elif stripped:
                malformed += 1
            continue
        if not isinstance(row, dict):
            malformed += 1
            continue
        json_rows += 1
        digest = hashlib.sha256(line.rstrip(b'\r\n')).digest()
        duplicates += int(digest == prev_hash)
        prev_hash = digest
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
        op = tap.get('operation')
        if op not in ('CLIENT_QUERY', 'CLIENT_RESPONSE', 'RESOLVER_QUERY', 'RESOLVER_RESPONSE'):
            unknown += 1
        else:
            counts[op] += 1
        if isinstance(op, str) and op.endswith('_RESPONSE'):
            rcodes[str(dns.get('rcode', '?'))] += 1
        malformed_dns += int(bool(dns.get('malformed-packet')))
if raw_hash.hexdigest() != end.get('raw_sha256'):
    raise SystemExit('Raw SHA-256 differs from final marker')
client = summary.get('counts')
if not isinstance(client, dict):
    raise SystemExit('Client summary lacks counts object')
ops = {key: counts.get(key, 0) for key in
       ('CLIENT_QUERY', 'CLIENT_RESPONSE', 'RESOLVER_QUERY', 'RESOLVER_RESPONSE')}
if a.capture_policy == 'client_only' and (ops['RESOLVER_QUERY'] or ops['RESOLVER_RESPONSE']):
    resolver_filter_failure = True
else:
    resolver_filter_failure = False
upstream_keys = ('upstream_queries_udp', 'upstream_queries_tcp',
                 'upstream_responses_udp', 'upstream_responses_tcp',
                 'upstream_other_udp_dropped', 'upstream_other_tcp_dropped',
                 'upstream_queries', 'upstream_responses', 'upstream_other_dropped')
upstream_start, upstream_end = start.get('upstream_nft_counters'), end.get('upstream_nft_counters')
upstream_deltas = {}
for key in upstream_keys:
    left = upstream_start.get(key) if isinstance(upstream_start, dict) else None
    right = upstream_end.get(key) if isinstance(upstream_end, dict) else None
    upstream_deltas[key] = right - left if (isinstance(left, int) and not isinstance(left, bool)
                                             and isinstance(right, int) and not isinstance(right, bool)) else None
result = {
    'schema_version': 1, 'run_id': a.run_id, 'campaign_id': a.campaign_id,
    'mode': a.mode, 'capture_policy': a.capture_policy,
    'strict_pass': False, 'verdict': 'diagnostic_only_strict_gate_failed',
    'strict_gate_failure': 'Windows SCM stop was not clean; strict reconciler rejected the stage.',
    'raw_bytes': end['raw_bytes'], 'raw_sha256': raw_hash.hexdigest(),
    'source_filter_verified': source_filter_verified,
    'end_filter_contract_inherited_from_start': end_filter_contract_inherited,
    'unexpected_resolver_records_under_client_only_filter': resolver_filter_failure,
    'byte_range': [start['offset'], end['offset']], 'collector_pid': start.get('collector_pid'),
    'collector_stop_exit_code': end.get('collector_stop_exit_code'),
    'collector_final_pid': end.get('collector_final_pid'),
    'collector_stderr_bytes': end.get('errors_bytes'), 'collector_stderr_sha256': end.get('errors_sha256'),
    'json_rows': json_rows, 'operation_counts': ops, 'response_rcodes': dict(rcodes),
    'unknown_selected_zone_operations': unknown, 'malformed_dns_records': malformed_dns,
    'malformed_json_lines': malformed, 'diagnostic_lines': diagnostics,
    'other_test_zone_records': other_zone, 'unrelated_json_records': unrelated,
    'duplicate_identical_consecutive_json_lines': duplicates,
    'client_counts': client,
    'expected_resolver_query_records': 0 if a.capture_policy == 'client_only' else None,
    'expected_resolver_response_records': 0 if a.capture_policy == 'client_only' else None,
    'client_directional_residuals': {
        'sent_minus_CLIENT_QUERY': client['sent'] - ops['CLIENT_QUERY'],
        'completed_minus_CLIENT_RESPONSE': client['completed'] - ops['CLIENT_RESPONSE']},
    'service_stop_receipt': stop, 'completion_receipt': completion,
    'udp_counters_start': start.get('udp_counters'), 'udp_counters_end': end.get('udp_counters'),
    'upstream_nft_counters_start': start.get('upstream_nft_counters'),
    'upstream_nft_counters_end': end.get('upstream_nft_counters'),
    'upstream_nft_packet_deltas_diagnostic_only': upstream_deltas,
    'prometheus_counters': None,
    'attribution_limit': 'Counts describe collector stdout. A service shutdown failure prevents a strict stage pass; counts do not identify a missing record cause.'}
a.output.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
print(json.dumps(result))
