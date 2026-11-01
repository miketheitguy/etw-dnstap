"""Join client, Windows forwarder, kernel, and verified receiver evidence."""
import argparse
import json
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))  # metrics_ledger lives in the parent directory
from metrics_ledger import ledger_summary, normalize_metrics

parser = argparse.ArgumentParser()
parser.add_argument('--directory', type=Path, default=Path(__file__).resolve().parents[3] / 'target/performance', help='evidence directory (default: <repo>/target/performance)')
parser.add_argument('--runs', nargs='+', default=['auth-1', 'rec-1', 'rec-2'])
parser.add_argument('--output', type=Path)
parser.add_argument('--service-metrics-directory', type=Path)
args = parser.parse_args()
base = args.directory
results = []
for run in args.runs:
    client = json.loads((base/'client'/run/'summary.json').read_text(encoding='utf-8-sig'))
    archive = json.loads((base/'ubuntu'/f'{run}-archive-counts.json').read_text())
    marker = json.loads((base/'ubuntu'/f'{run}-start--{run}-end-resources.json').read_text())
    resources = json.loads((base/'ubuntu'/f'{run}-send-window.json').read_text())
    verification = json.loads((base/'archive'/f'{run}-verification.json').read_text())
    metric_files = list((args.service_metrics_directory or base/'windows').glob(f'{run}.*.jsonl'))
    if len(metric_files) != 1:
        raise RuntimeError(f'{run}: expected one Windows service metric file')
    raw_final = [json.loads(line) for line in metric_files[0].read_text(encoding='utf-8-sig').splitlines() if line.strip()][-1]
    final = normalize_metrics(raw_final)
    ledger = ledger_summary(final)
    upstream = marker['upstream_packet_deltas']
    queries, responses = upstream.get('upstream_queries', 0), upstream.get('upstream_responses', 0)
    expected = 2*client['counts']['sent']+queries+responses
    send_seconds = (client['sending_end_unix_ms']-client['start_unix_ms'])/1000
    encoded = final['encoded']
    rows = archive['all_json_records']
    deployment_path = metric_files[0].parent/f'{run}-Start.json'
    deployment = json.loads(deployment_path.read_text(encoding='utf-8-sig')) if deployment_path.exists() else {}
    if isinstance(deployment, list):
        if len(deployment) != 1 or not isinstance(deployment[0], dict):
            raise ValueError(f'{run}: deployment must be one object or a singleton object array')
        deployment = deployment[0]
    if not isinstance(deployment, dict):
        raise ValueError(f'{run}: deployment metadata is not an object')
    preparation_path = base/'ubuntu'/f'{run}-ready.json'
    preparation = json.loads(preparation_path.read_text(encoding='utf-8-sig')) if preparation_path.exists() else {}
    if not isinstance(preparation, dict):
        raise ValueError(f'{run}: receiver preparation metadata is not an object')
    domain_closure = (expected == final['received']+final['etw_events_lost']
                      and final.get('etw_stats_valid') is True
                      and final.get('callback_errors', 0) == 0
                      and archive['unrelated_records'] == 0
                      and archive['matched_lab_records'] == rows
                      and final.get('ignored', 0) == 0 and final.get('rejected', 0) == 0)
    worker_deltas = marker['worker_counter_deltas']
    receiver_exact = (encoded == rows and all(
        worker_deltas.get(f'dnscollector_exporter_worker_{counter}_traffic_total{{worker="{worker}"}}') == encoded
        for worker in ('tap', 'console') for counter in ('ingress', 'egress'))
        and all(worker_deltas.get(f'dnscollector_exporter_worker_discarded_traffic_total{{worker="{worker}"}}') == 0
                for worker in ('tap', 'console')))
    zero_error_fields = ('intake_dropped', 'ring_dropped', 'pool_exhausted', 'receiver_disconnected',
                         'callback_errors', 'queue_backlog', 'ignored', 'rejected',
                         'output_limit_dropped', 'output_failed_events', 'output_errors',
                         'etw_events_lost', 'etw_realtime_buffers_lost')
    result = {'run': run, 'client_send_duration_seconds': send_seconds,
              'mode': client['options']['mode'],
              'configured_global_qps': client['options'].get('qps'),
              'client_threads': client['options']['threads'],
              'client_inflight_per_thread': client['options']['inflight'],
              'names_loaded': client['names_loaded'],
              'service_binary_sha256': deployment.get('binary_sha256'),
              'service_deployment': deployment,
              'receiver_preparation': preparation,
              'receiver_preparation_file': f'ubuntu/{run}-ready.json' if preparation_path.exists() else None,
              'service_capacity': deployment.get('requested_capacity'),
              'client_sent': client['counts']['sent'],
              'client_completed': client['counts']['completed'],
              'client_successful': client['counts']['successful'],
              'client_timeouts': client['counts']['timeouts'],
              'client_servfail': client['counts']['rcode'][2],
              'client_ra_missing': client['counts']['ra_missing'],
              'client_qps': client['counts']['sent']/send_seconds,
              'upstream_queries_marker_interval': queries,
              'upstream_responses_marker_interval': responses,
              'upstream_queries_per_client_sent': queries/client['counts']['sent'],
              'udp_receive_buffer_errors_marker_interval': marker['udp_counter_deltas']['RcvbufErrors'],
              'event_denominator': expected if domain_closure else None,
              'event_denominator_candidate': expected,
              'event_denominator_domain_closure_verified': domain_closure,
              'receiver_exact_delivery_verified': receiver_exact,
              'capture_pipeline_zero_loss_verified': domain_closure and receiver_exact
                  and all(final.get(key) == 0 for key in zero_error_fields)
                  and archive['malformed_json_lines'] == 0 and archive['malformed_dns_records'] == 0,
              'all_client_queries_successful': client['counts']['successful'] == client['counts']['sent'],
              'denominator_formula': '2 * client_sent + actual_upstream_queries + actual_upstream_responses; accepted only if domain_closure_verified is true',
              'etw_events_lost': final['etw_events_lost'],
              'etw_stats_valid': final['etw_stats_valid'],
              'etw_stats_errors': final['etw_stats_errors'],
              'service_final_metrics': raw_final,
              'service_ledger': ledger,
              'service_callback_received': final['received'],
              'service_ring_dropped': final['ring_dropped'],
              'service_intake_dropped': final['intake_dropped'],
              'service_pool_exhausted': final['pool_exhausted'],
              'service_receiver_disconnected': final['receiver_disconnected'],
              'service_encoded': encoded, 'durable_json_records': rows,
              'encoded_minus_durable_records': encoded-rows,
              'receiver_worker_counter_deltas': marker['worker_counter_deltas'],
              'udp_counter_deltas_marker_interval': marker['udp_counter_deltas'],
              'captured_unrelated_records': archive['unrelated_records'],
              'etw_ledger_residual': expected-final['received']-final['etw_events_lost'],
              'callback_ledger_residual': ledger['capture_residual'],
              'parser_ledger_residual': ledger['parser_residual'],
              'captured_operations': archive['operations'],
              'captured_response_rcodes': archive.get('response_rcodes'),
              'captured_qtypes': archive.get('qtypes'),
              'upstream_socket_drop_delta_marker_interval': marker.get('upstream_socket_drop_delta'),
              'captured_positive_answer_ttls_by_operation': archive['answer_ttls_by_operation'],
              'captured_response_flags_by_operation': archive['response_flags_by_operation'],
              'captured_client_response_ra_false_examples': archive['captured_client_response_ra_false_examples'],
              'captured_dns_malformed': archive['malformed_dns_records'],
              'captured_json_malformed': archive['malformed_json_lines'],
              'receiver_retention_fraction_of_event_denominator': rows/expected if domain_closure else None,
              'collector_cpu_percent_one_core_estimated': resources['collector']['mean_cpu_percent_one_core_estimated'],
              'collector_cpu_seconds_estimated': resources['collector']['cpu_seconds_estimated'],
              'collector_cpu_microseconds_per_encoded_record_estimated': resources['collector']['cpu_seconds_estimated']*1e6/encoded if encoded else None,
              'collector_peak_rss_bytes': resources['collector']['peak_sample_rss_bytes'],
              'upstream_cpu_percent_one_core_estimated': resources['upstream']['mean_cpu_percent_one_core_estimated'],
              'upstream_cpu_seconds_estimated': resources['upstream']['cpu_seconds_estimated'],
              'upstream_peak_rss_bytes': resources['upstream']['peak_sample_rss_bytes'],
              'guest_disk_busy_percent_estimated': resources.get('receiver_disk', resources['guest_disk_sda'])['busy_percent_estimated'],
              'receiver_disk_device': resources.get('receiver_disk_device', 'sda'),
              'ubuntu_send_window_resources': resources,
              'archive_verification': verification,
              'counter_interval_start': marker['start_utc'], 'counter_interval_end': marker['end_utc'],
              'resource_interval_start_unix_ms': client['start_unix_ms'],
              'resource_interval_end_unix_ms': client['sending_end_unix_ms']}
    results.append(result)
output = {'runs': results, 'caveats': [
    'Resources interpolate 1s Ubuntu samples at client send boundaries. Counts use wider start/end markers including final drain.',
    'Recursive cases repeat a 5000-name working set; TTL0 does not imply every client lookup reaches the upstream. Actual upstream packet counts supply the event denominator.',
    'Captured RA-flag counts must be interpreted with each run\'s measured ETW/ring loss. They do not estimate uncaptured client responses.',
    'Measured collector JSON stdout writes a regular file; the original console/journal destination is restored after testing.',
    'Runs are single observations with 1s resource sampling and potentially unequal encoded work; raw CPU and per-record normalizations do not establish a causal performance ranking.'
]}
if 'rec-1' in args.runs:
    output['caveats'].append('rec-1 has a 212992-byte upstream receive socket and 8086 kernel receive-buffer drops; rec-2 uses an 8MiB socket and no new kernel drops.')
path = args.output or base/'ubuntu'/'receiver-results.json'
path.write_text(json.dumps(output, indent=2)+'\n')
print(path)
