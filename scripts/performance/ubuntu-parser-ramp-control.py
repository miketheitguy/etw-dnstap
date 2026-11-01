"""Prepare and mark one isolated Ubuntu receiver stage over its existing SSH access."""
import argparse
import datetime
import hashlib
import gzip
import json
import os
from pathlib import Path
import pwd
import re
import shutil
import subprocess
import time
import sys
from parser_ramp_receipts import finish_marker_contract
sys.path.insert(0, str(Path(__file__).resolve().parent))  # lab_config lives next to this script
import lab_config  # noqa: E402

HOST = lab_config.get('ubuntu_vm.hostname')
DEFAULT_CAMPAIGN_ID = 'parser-ramp-20261030'
CAMPAIGN_ID = DEFAULT_CAMPAIGN_ID
CONFIG = Path('/etc/dnscollector/config.yml')
BINARY = Path('/usr/local/bin/dnscollector')
UNIT = 'dnscollector'
RAW_MOUNT = Path('/mnt/dnstap-pooled-vm-20261029-0942')
ARCHIVE_BASE = Path(f'/home/dnstaplab/{DEFAULT_CAMPAIGN_ID}')
DROPIN = Path(f'/etc/systemd/system/dnscollector.service.d/{DEFAULT_CAMPAIGN_ID}.conf')
RAW_UUID = '3046058a-a6b1-4e74-97f5-a63905494a1f'
RAW_WWN = '0x600224805c7ffa68166d3a03b164a313'
ROOT_UUID = 'a9c87573-c170-44a0-8c14-09fcc03a56a9'
EXPECTED_CONFIG_SHA256 = '5d71afe6f46d8f81802ae592a97c8be2582891b52c5624405c6ef64120065935'
EXPECTED_BINARY_SHA256 = 'fea6c2f23d27d51e281de2f84aa56a014067e2cce1ccd53200ed894fae05e406'
FLOOR = 3 * 1024**3
SAFETY = 1024**3
GIB = 1024**3


def run(argv, *, check=True, text=True):
    result = subprocess.run(argv, check=check, capture_output=True, text=text)
    return result.stdout


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(8 * 1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def final_byte_is_newline(path):
    with Path(path).open('rb') as stream:
        stream.seek(0, os.SEEK_END)
        if stream.tell() == 0:
            return True
        stream.seek(-1, os.SEEK_END)
        return stream.read(1) == b'\n'


def fs(path):
    tree = json.loads(run(['findmnt', '-J', '-o', 'SOURCE,TARGET,UUID,FSTYPE,OPTIONS,MAJ:MIN',
                           '--target', str(path)]))['filesystems']
    if len(tree) != 1:
        raise RuntimeError(f'Expected one filesystem for {path}')
    item = tree[0]
    stat = os.statvfs(path)
    return {'source': item['source'], 'mountpoint': item['target'], 'uuid': item['uuid'],
            'fstype': item['fstype'], 'options': item['options'], 'major_minor': item['maj:min'],
            'available_bytes': stat.f_bavail * stat.f_frsize}


def systemd_state():
    return run(['systemctl', 'show', UNIT, '-p', 'MainPID', '-p', 'ExecStart', '-p', 'User',
                '-p', 'StandardOutput', '-p', 'StandardError', '--no-pager'])


def pid():
    return int(run(['systemctl', 'show', UNIT, '-p', 'MainPID', '--value']).strip())


def established_peers():
    result = []
    for family in ('tcp', 'tcp6'):
        path = Path('/proc/net') / family
        for line in path.read_text().splitlines()[1:]:
            fields = line.split()
            if len(fields) < 4 or fields[3] != '01':
                continue
            local_port = int(fields[1].rsplit(':', 1)[1], 16)
            if local_port != 6000:
                continue
            result.append({'family': family, 'local': fields[1], 'remote': fields[2], 'state': fields[3]})
    return result


def listener_present():
    for line in Path('/proc/net/tcp').read_text().splitlines()[1:]:
        fields = line.split()
        if len(fields) >= 4 and fields[1].rsplit(':', 1)[1].upper() == '1770' and fields[3] == '0A':
            return True
    return False


def udp_state():
    lines = Path('/proc/net/snmp').read_text().splitlines()
    result = {}
    for index, line in enumerate(lines[:-1]):
        if line.startswith('Udp:') and lines[index + 1].startswith('Udp:'):
            result = dict(zip(line.split()[1:], map(int, lines[index + 1].split()[1:])))
    return result


def nft_counters():
    if run(['nft', 'list', 'table', 'inet', 'dnstap_performance'], check=False).strip() == '':
        return None
    data = json.loads(run(['nft', '-j', 'list', 'table', 'inet', 'dnstap_performance']))
    result = {}
    for item in data.get('nftables', []):
        rule = item.get('rule', {})
        label = rule.get('comment')
        if isinstance(label, str) and (label.startswith('upstream_') or label in ('upstream_queries', 'upstream_responses')):
            for expression in rule.get('expr', []):
                if isinstance(expression, dict) and isinstance(expression.get('counter'), dict):
                    result[label] = int(expression['counter']['packets'])
    if result:
        query_keys = ('upstream_queries_udp', 'upstream_queries_tcp')
        response_keys = ('upstream_responses_udp', 'upstream_responses_tcp')
        drop_keys = ('upstream_other_udp_dropped', 'upstream_other_tcp_dropped')
        expected = set(query_keys + response_keys + drop_keys)
        if not expected.issubset(result):
            raise RuntimeError('Upstream nft table lacks a required transport/dropped counter')
        result['upstream_queries'] = sum(result[key] for key in query_keys)
        result['upstream_responses'] = sum(result[key] for key in response_keys)
        result['upstream_other_dropped'] = sum(result[key] for key in drop_keys)
    return result


def marker_snapshot(raw, *, stable_samples=None):
    mount = fs(RAW_MOUNT)
    root = fs(ARCHIVE_BASE)
    stat = raw.stat() if raw.exists() else None
    return {'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'unix_ns': time.time_ns(), 'offset': raw.stat().st_size if raw.exists() else 0,
            'raw_bytes': stat.st_size if stat else 0, 'raw_mtime_ns': stat.st_mtime_ns if stat else None,
            'collector_pid': pid(), 'collector_state': systemd_state(),
            'raw_volume': mount, 'archive_volume': root, 'prometheus_counters': None,
            'prometheus_note': 'No existing listener on 127.0.0.1:9165; config preserved unchanged.',
            'upstream_nft_counters': nft_counters(), 'udp_counters': udp_state(),
            'tcp_6000_established_peers': established_peers(),
            'publication_stability_samples': stable_samples}


def safe_run_id(value):
    if not re.fullmatch(r'[A-Za-z0-9_-]{1,64}', value):
        raise SystemExit('Unsafe run ID')


def configure_campaign(args):
    global ARCHIVE_BASE, CAMPAIGN_ID, DROPIN
    value = args.campaign_id
    if not re.fullmatch(r'[A-Za-z0-9_-]{1,64}', value):
        raise SystemExit('Unsafe campaign ID')
    CAMPAIGN_ID = value
    ARCHIVE_BASE = Path('/home/dnstaplab') / value
    DROPIN = Path('/etc/systemd/system/dnscollector.service.d') / f'{value}.conf'


def share_with_lab_account(*paths):
    account = pwd.getpwnam('dnstaplab')
    for value in paths:
        path = Path(value)
        if path.exists():
            os.chown(path, account.pw_uid, account.pw_gid)
            os.chmod(path, 0o700 if path.is_dir() else 0o600)


def metadata(run_id):
    if run(['hostname']).strip() != HOST:
        raise RuntimeError('Unexpected Ubuntu guest hostname')
    if sha256(CONFIG) != EXPECTED_CONFIG_SHA256:
        raise RuntimeError('Collector configuration hash differs from the verified baseline')
    if sha256(BINARY) != EXPECTED_BINARY_SHA256:
        raise RuntimeError('Collector binary hash differs from the verified v3.2.3 baseline')
    raw_fs, archive_fs = fs(RAW_MOUNT), fs(ARCHIVE_BASE)
    if (raw_fs['uuid'] != RAW_UUID or raw_fs['major_minor'] != '8:16'
            or raw_fs['fstype'] != 'ext4' or RAW_WWN not in run(['lsblk', '-dn', '-o', 'WWN', raw_fs['source']]).strip()):
        raise RuntimeError('Raw data volume identity does not match the retained VHDX receipt')
    if archive_fs['uuid'] != ROOT_UUID or raw_fs['uuid'] == archive_fs['uuid']:
        raise RuntimeError('Archive volume identity mismatch or raw/archive volumes are not separate')
    required_mount = ('nosuid', 'nodev', 'noexec', 'discard', 'commit=30')
    if any(option not in raw_fs['options'].split(',') for option in required_mount):
        raise RuntimeError('Raw data volume mount options changed')
    return {'hostname': HOST, 'collector_config_path': str(CONFIG),
            'collector_config_sha256': sha256(CONFIG), 'collector_binary_path': str(BINARY),
            'collector_binary_sha256': sha256(BINARY), 'raw_volume': raw_fs,
            'archive_volume': archive_fs, 'collector_state': systemd_state(),
            'collector_pid': pid(), 'prometheus_listener_present':
            subprocess.run(['curl', '-fsS', '--max-time', '1', 'http://127.0.0.1:9165/metrics'],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False).returncode == 0,
            'run_id': run_id}


def prepare(args):
    configure_campaign(args)
    safe_run_id(args.run_id)
    if args.qps <= 0 or args.duration not in (60, 300):
        raise SystemExit('QPS and duration outside the bounded stage protocol')
    if args.raw_budget_bytes <= 0 or args.raw_budget_bytes > 48 * GIB:
        raise SystemExit('Raw budget must be positive and at most 48 GiB')
    if args.forecast_raw_bytes <= 0 or args.forecast_raw_bytes > args.raw_budget_bytes:
        raise SystemExit('Raw forecast must be positive and within its explicit budget')
    run_base = ARCHIVE_BASE / args.run_id
    if run_base.exists():
        if run_base.is_symlink() or not run_base.is_dir() or any(run_base.iterdir()):
            raise RuntimeError('Stage evidence directory exists and is not empty; preserve it')
    else:
        run_base.mkdir(mode=0o700, parents=True, exist_ok=False)
    identity = metadata(args.run_id)
    raw = RAW_MOUNT / f'{args.run_id}.jsonl'
    errors = run_base / 'collector-errors.log'
    if raw.exists() or raw.is_symlink() or errors.exists():
        raise RuntimeError('Stage evidence destination already exists')
    raw_need = args.forecast_raw_bytes + FLOOR + SAFETY
    archive_reserve = max((args.forecast_raw_bytes + 3) // 4, GIB)
    archive_need = archive_reserve + FLOOR + SAFETY
    if identity['raw_volume']['available_bytes'] < raw_need:
        raise RuntimeError('Raw volume forecast plus 3 GiB floor and 1 GiB safety does not fit')
    if identity['archive_volume']['available_bytes'] < archive_need:
        raise RuntimeError('Archive volume compression reserve plus 3 GiB floor and 1 GiB safety does not fit')
    if DROPIN.exists():
        previous_state = ARCHIVE_BASE / 'control-state.json'
        if not previous_state.exists():
            raise RuntimeError('Unowned parser-ramp drop-in already exists')
        state = json.loads(previous_state.read_text())
        previous = state.get('last_run')
        if not previous or not (ARCHIVE_BASE / previous / 'end.json').exists():
            raise RuntimeError('Prior parser-ramp stage has no end marker')
        saved = DROPIN.read_text()
        if hashlib.sha256(saved.encode()).hexdigest() != state.get('dropin_sha256'):
            raise RuntimeError('Task-owned drop-in changed since the prior stage')
        previous_dir = ARCHIVE_BASE / previous
        previous_ready = json.loads((previous_dir / 'ready.json').read_text())
        previous_end = json.loads((previous_dir / 'end.json').read_text())
        previous_manifest = json.loads((previous_dir / 'archive-manifest.json').read_text())
        previous_raw = Path(previous_ready['raw_path'])
        if previous_raw.exists():
            if (previous_raw.is_symlink() or not previous_raw.is_file()
                    or previous_raw.stat().st_size != previous_end.get('raw_bytes')
                    or sha256(previous_raw) != previous_end.get('raw_sha256')):
                raise RuntimeError('Prior stopped raw stage changed after its immutable end boundary')
        else:
            removal_path = previous_dir / 'raw-removal.json'
            if not removal_path.exists():
                raise RuntimeError('Prior raw file is absent without a verified-removal receipt')
            removal = json.loads(removal_path.read_text())
            archive_path = Path(previous_manifest['archive_path'])
            if (removal.get('host_verification_hashes_match') is not True
                    or removal.get('raw_sha256') != previous_end.get('raw_sha256')
                    or removal.get('raw_bytes') != previous_end.get('raw_bytes')
                    or removal.get('compressed_sha256') != previous_manifest.get('compressed_sha256')
                    or sha256(archive_path) != previous_manifest.get('compressed_sha256')):
                raise RuntimeError('Prior raw removal receipt or preserved compressed archive is inconsistent')
        shutil.copy2(DROPIN, run_base / 'dropin-before.conf')
    else:
        saved = None
        if (ARCHIVE_BASE / 'control-state.json').exists():
            raise RuntimeError('Parser-ramp state expects a missing drop-in')
    if established_peers():
        raise RuntimeError('Receiver still has a TCP :6000 peer; Windows sender must be stopped before preparation')
    (run_base / 'collector-unit-before.txt').write_text(run(['systemctl', 'cat', UNIT]))
    (run_base / 'collector-properties-before.txt').write_text(identity['collector_state'])
    (run_base / 'collector-config.sha256').write_text(identity['collector_config_sha256'] + '  ' + str(CONFIG) + '\n')
    (run_base / 'collector-binary.sha256').write_text(identity['collector_binary_sha256'] + '  ' + str(BINARY) + '\n')
    content = f'[Service]\nStandardOutput=append:{raw}\nStandardError=append:{errors}\n'
    DROPIN.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
    DROPIN.write_text(content)
    subprocess.run(['systemctl', 'stop', UNIT], check=True)
    subprocess.run(['systemctl', 'daemon-reload'], check=True)
    subprocess.run(['systemctl', 'start', UNIT], check=True)
    for _ in range(30):
        if pid() > 0 and raw.exists() and listener_present():
            break
        time.sleep(0.2)
    else:
        raise RuntimeError('Collector did not become ready on its isolated stdout file')
    if sha256(CONFIG) != EXPECTED_CONFIG_SHA256:
        raise RuntimeError('Collector configuration changed during preparation')
    expected_operations = (['CLIENT_QUERY', 'CLIENT_RESPONSE'] if args.capture_policy == 'client_only'
                           else ['CLIENT_QUERY', 'CLIENT_RESPONSE', 'RESOLVER_QUERY', 'RESOLVER_RESPONSE'])
    source_filter_expected = ({'provider_event_ids': [256, 257], 'level': 4,
                               'keyword_mask': '0x8000000000000000'}
                              if args.capture_policy == 'client_only' else None)
    ready = {'run': args.run_id, 'campaign_id': CAMPAIGN_ID, 'mode': args.mode,
             'capture_policy': args.capture_policy, 'expected_operations': expected_operations,
             'source_filter_expected': source_filter_expected,
             'configured_global_qps': args.qps,
             'duration_seconds': args.duration, 'receiver_ready': True, 'start_marker_ready': True,
             'ready_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
             'marker': f'{args.run_id}-start.json', 'stable_publication': True,
             'storage_protocol': 'separate_raw_data_disk_and_root_archives',
             'archive_double_footprint_fits': False,
             'forecast_raw_bytes': args.forecast_raw_bytes, 'raw_budget_bytes': args.raw_budget_bytes,
             'archive_compression_reserve_bytes': archive_reserve,
             'min_free_bytes': FLOOR, 'safety_bytes': SAFETY,
             'raw_volume': {**identity['raw_volume'], 'required_bytes': raw_need,
                            'available_bytes': fs(RAW_MOUNT)['available_bytes']},
             'archive_volume': {**identity['archive_volume'], 'required_bytes': archive_need,
                                'available_bytes': fs(ARCHIVE_BASE)['available_bytes']},
             'raw_path': str(raw), 'errors_path': str(errors),
             'raw_start_offset': 0, 'prometheus_counters': None,
             'prometheus_note': 'No exporter or listener on 127.0.0.1:9165; no counters claimed.',
             'collector_config_sha256': sha256(CONFIG), 'collector_binary_sha256': sha256(BINARY),
             'collector_pid': pid(), 'collector_config_unchanged': True,
             'retained_data_disk_uuid': RAW_UUID, 'retained_data_disk_wwn': RAW_WWN,
             'windows_sender_must_be_stopped_before_prepare': True}
    start = marker_snapshot(raw)
    start.update({'run': args.run_id, 'campaign_id': CAMPAIGN_ID,
                  'capture_policy': args.capture_policy,
                  'expected_operations': expected_operations,
                  'source_filter_expected': source_filter_expected,
                  'marker': f'{args.run_id}-start.json', 'offset': 0,
                  'raw_path': str(raw), 'collector_config_sha256': sha256(CONFIG),
                  'collector_binary_sha256': sha256(BINARY)})
    (run_base / 'start.json').write_text(json.dumps(start, indent=2) + '\n')
    (run_base / 'ready.json').write_text(json.dumps(ready, indent=2) + '\n')
    state = {'campaign_id': CAMPAIGN_ID, 'last_run': args.run_id,
             'dropin_sha256': hashlib.sha256(content.encode()).hexdigest(),
             'dropin_path': str(DROPIN), 'baseline_config_sha256': EXPECTED_CONFIG_SHA256,
             'baseline_binary_sha256': EXPECTED_BINARY_SHA256}
    (ARCHIVE_BASE / 'control-state.json').write_text(json.dumps(state, indent=2) + '\n')
    share_with_lab_account(ARCHIVE_BASE, run_base, run_base / 'ready.json', run_base / 'start.json',
                           run_base / 'collector-unit-before.txt', run_base / 'collector-properties-before.txt',
                           run_base / 'collector-config.sha256', run_base / 'collector-binary.sha256',
                           ARCHIVE_BASE / 'control-state.json')
    print(json.dumps(ready))


def finish(args):
    configure_campaign(args)
    safe_run_id(args.run_id)
    if not args.sender_stopped:
        raise SystemExit('Require Windows STOP/FINISH drain confirmation')
    run_base = ARCHIVE_BASE / args.run_id
    ready = json.loads((run_base / 'ready.json').read_text())
    # The stage must have started; a missing or corrupt start marker stops finish.
    json.loads((run_base / 'start.json').read_text())
    raw = Path(ready['raw_path'])
    if raw.is_symlink() or not raw.is_file():
        raise RuntimeError('Stage raw output file is missing or unsafe')
    pre_stop_stable = []
    previous = None
    same = 0
    for index in range(45):
        sample = {'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                  'offset': raw.stat().st_size, 'collector_pid': pid(),
                  'tcp_6000_established_peers': established_peers()}
        pre_stop_stable.append(sample)
        current = (sample['offset'], sample['collector_pid'], tuple(map(str, sample['tcp_6000_established_peers'])))
        same = same + 1 if current == previous else 0
        if same >= 3 and not sample['tcp_6000_established_peers']:
            break
        previous = current
        time.sleep(1)
    else:
        raise RuntimeError('Receiver output did not plateau or a TCP peer remains before collector stop')
    collector_pid_before_stop = pid()
    stopped = subprocess.run(['systemctl', 'stop', UNIT], capture_output=True, text=True, check=False)
    if stopped.returncode != 0:
        raise RuntimeError(f'Collector stop failed: {stopped.stderr.strip()}')
    final_stable = []
    previous = None
    same = 0
    for index in range(45):
        sample = {'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                  'offset': raw.stat().st_size, 'collector_pid': pid(),
                  'tcp_6000_established_peers': established_peers()}
        final_stable.append(sample)
        current = (sample['offset'], sample['collector_pid'], tuple(map(str, sample['tcp_6000_established_peers'])))
        same = same + 1 if current == previous else 0
        if same >= 3 and sample['collector_pid'] == 0 and not sample['tcp_6000_established_peers']:
            break
        previous = current
        time.sleep(1)
    else:
        raise RuntimeError('Stopped collector output did not reach a final stable offset')
    final_state = run(['systemctl', 'is-active', UNIT], check=False).strip()
    if final_state != 'inactive' or pid() != 0:
        raise RuntimeError('Collector is not stopped after final drain')
    end = marker_snapshot(raw, stable_samples=final_stable)
    end['collector_pid'] = collector_pid_before_stop
    before_hash = raw.stat()
    raw_sha = sha256(raw)
    after_hash = raw.stat()
    if (before_hash.st_size, before_hash.st_mtime_ns) != (after_hash.st_size, after_hash.st_mtime_ns):
        raise RuntimeError('Raw output changed while its post-drain digest was computed')
    errors_path = Path(ready['errors_path'])
    errors_stat = errors_path.stat() if errors_path.exists() else None
    end.update({'run': args.run_id, **finish_marker_contract(ready),
                'marker': f'{args.run_id}-end.json',
                'raw_path': str(raw), 'collector_config_sha256': sha256(CONFIG),
                'collector_binary_sha256': sha256(BINARY), 'raw_bytes': after_hash.st_size,
                'raw_mtime_ns': after_hash.st_mtime_ns, 'raw_sha256': raw_sha,
                'collector_pid_before_stop': collector_pid_before_stop,
                'collector_final_pid': 0, 'collector_stop_exit_code': stopped.returncode,
                'collector_stop_state': final_state, 'pre_stop_publication_samples': pre_stop_stable,
                'errors_path': str(errors_path), 'errors_bytes': errors_stat.st_size if errors_stat else 0,
                'errors_sha256': sha256(errors_path) if errors_stat else None,
                'final_line_terminated': final_byte_is_newline(raw)})
    if not end['final_line_terminated']:
        raise RuntimeError('Raw stdout ends with an incomplete JSONL line')
    end_path = run_base / 'end.json'
    if end_path.exists():
        raise RuntimeError('End marker already exists')
    end_path.write_text(json.dumps(end, indent=2) + '\n')
    share_with_lab_account(run_base, end_path, errors_path)
    print(json.dumps({'run': args.run_id, 'end_marker': str(end_path), 'end_offset': end['offset'],
                      'raw_path': str(raw), 'stable_publication': True,
                      'raw_bytes': end['raw_bytes'], 'raw_sha256': end['raw_sha256'],
                      'collector_pid': end['collector_pid'], 'collector_final_pid': end['collector_final_pid'],
                      'upstream_nft_counters': end['upstream_nft_counters'],
                      'udp_counters': end['udp_counters'],
                      'prometheus_counters': None}))


def monitor(args):
    configure_campaign(args)
    safe_run_id(args.run_id)
    run_base = ARCHIVE_BASE / args.run_id
    ready = json.loads((run_base / 'ready.json').read_text())
    start_marker = json.loads((run_base / 'start.json').read_text())
    raw = Path(ready['raw_path'])
    path = run_base / 'resources.jsonl'
    if path.exists():
        raise RuntimeError('Resource monitor evidence already exists')
    initial_pid = ready['collector_pid']
    deadline = time.monotonic() + args.seconds
    with path.open('x', buffering=1) as stream:
        sample_no = 0
        while time.monotonic() < deadline:
            started = time.monotonic()
            sample_no += 1
            raw_free = fs(RAW_MOUNT)['available_bytes']
            archive_free = fs(run_base)['available_bytes']
            current_pid = pid()
            state = run(['systemctl', 'is-active', UNIT], check=False).strip()
            raw_bytes = raw.stat().st_size if raw.exists() else 0
            udp = udp_state()
            nft = nft_counters() if ready['mode'] == 'recursive' else None
            sample = {'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                      'sample': sample_no, 'raw_bytes': raw_bytes,
                      'raw_free_bytes': raw_free, 'archive_free_bytes': archive_free,
                      'collector_pid': current_pid, 'collector_active': state == 'active',
                      'tcp_6000_established_peers': established_peers(),
                      'udp_counters': udp, 'upstream_nft_counters': nft}
            sample['breach'] = (raw_bytes > ready['raw_budget_bytes']
                                or raw_free < ready['min_free_bytes'] + ready['safety_bytes']
                                or archive_free < ready['archive_compression_reserve_bytes']
                                + ready['min_free_bytes'] + ready['safety_bytes'])
            if state == 'active' and current_pid != initial_pid:
                sample['breach'] = True
                sample['reason'] = 'collector PID changed during stage'
            if state not in ('active', 'inactive'):
                sample['breach'] = True
                sample['reason'] = f'unexpected collector state: {state}'
            if ready['mode'] == 'recursive':
                baseline = start_marker.get('udp_counters', {})
                if (not isinstance(nft, dict)
                        or any(key not in nft for key in ('upstream_queries', 'upstream_responses', 'upstream_other_dropped'))):
                    sample['breach'] = True
                    sample['reason'] = 'required upstream query/response/drop counters became unavailable'
                elif nft['upstream_other_dropped'] > 0:
                    sample['breach'] = True
                    sample['reason'] = 'out-of-scope DNS packets were dropped by the task firewall'
                if (udp.get('InErrors', baseline.get('InErrors', 0)) > baseline.get('InErrors', 0)
                        or udp.get('RcvbufErrors', baseline.get('RcvbufErrors', 0)) > baseline.get('RcvbufErrors', 0)):
                    sample['breach'] = True
                    sample['reason'] = 'guest UDP error or receive-buffer-drop counter increased'
            stream.write(json.dumps(sample) + '\n')
            if sample['breach']:
                print(json.dumps(sample), flush=True)
                raise RuntimeError('Parser-ramp Ubuntu storage/collector guard breached; notify Windows coordinator to stop')
            if sample_no == 1 or sample_no % 15 == 0:
                print(json.dumps(sample), flush=True)
            if state == 'inactive' and current_pid == 0:
                break
            time.sleep(max(0, 1 - (time.monotonic() - started)))
    print(json.dumps({'run': args.run_id, 'monitor': str(path), 'samples': sample_no,
                      'monitor_exit_utc': datetime.datetime.now(datetime.timezone.utc).isoformat()}))
    share_with_lab_account(run_base, path)


def archive(args):
    configure_campaign(args)
    safe_run_id(args.run_id)
    run_base = ARCHIVE_BASE / args.run_id
    ready = json.loads((run_base / 'ready.json').read_text())
    start = json.loads((run_base / 'start.json').read_text())
    end = json.loads((run_base / 'end.json').read_text())
    raw = Path(ready['raw_path'])
    archive_path = run_base / 'receiver-raw.jsonl.gz'
    manifest_path = run_base / 'archive-manifest.json'
    if archive_path.exists() or manifest_path.exists():
        raise RuntimeError('Refusing to overwrite a prior stage archive')
    if run(['systemctl', 'is-active', UNIT], check=False).strip() != 'inactive' or pid() != 0:
        raise RuntimeError('Collector must be stopped before archiving')
    if established_peers():
        raise RuntimeError('TCP peer remains while archiving')
    before = raw.stat()
    if before.st_size != end['raw_bytes'] or sha256(raw) != end['raw_sha256']:
        raise RuntimeError('Raw source differs from its immutable end marker')
    digest = hashlib.sha256()
    count = 0
    with raw.open('rb') as source, archive_path.open('xb') as out:
        with gzip.GzipFile(filename=archive_path.name, fileobj=out, mode='wb', compresslevel=1, mtime=0) as compressed:
            for block in iter(lambda: source.read(8 * 1024 * 1024), b''):
                digest.update(block)
                count += len(block)
                compressed.write(block)
    after = raw.stat()
    if (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
        raise RuntimeError('Raw file changed during archive compression')
    if count != end['raw_bytes'] or digest.hexdigest() != end['raw_sha256']:
        raise RuntimeError('Streamed raw digest/byte count differs from immutable end marker')
    decompressed_hash = hashlib.sha256()
    decompressed_bytes = 0
    with gzip.open(archive_path, 'rb') as source:
        for block in iter(lambda: source.read(8 * 1024 * 1024), b''):
            decompressed_hash.update(block)
            decompressed_bytes += len(block)
    if decompressed_bytes != end['raw_bytes'] or decompressed_hash.hexdigest() != end['raw_sha256']:
        raise RuntimeError('Compressed archive failed streaming round-trip verification')
    result = {'run': args.run_id, 'campaign_id': CAMPAIGN_ID,
              'capture_policy': ready.get('capture_policy', 'all_supported'),
              'created_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'raw_path': str(raw), 'archive_path': str(archive_path),
              'raw_sha256': end['raw_sha256'], 'raw_bytes': end['raw_bytes'],
              'compressed_sha256': sha256(archive_path), 'compressed_bytes': archive_path.stat().st_size,
              'streamed_source_bytes': count, 'roundtrip_bytes': decompressed_bytes,
              'roundtrip_sha256': decompressed_hash.hexdigest(),
              'start_offset': start['offset'], 'end_offset': end['offset'],
              'collector_stopped': True, 'raw_volume_uuid': RAW_UUID,
              'archive_volume_uuid': ROOT_UUID, 'host_verification_required_before_raw_removal': True}
    manifest_path.write_text(json.dumps(result, indent=2) + '\n')
    share_with_lab_account(run_base, archive_path, manifest_path)
    print(json.dumps(result))


def remove_verified_raw(args):
    configure_campaign(args)
    safe_run_id(args.run_id)
    run_base = ARCHIVE_BASE / args.run_id
    ready = json.loads((run_base / 'ready.json').read_text())
    end = json.loads((run_base / 'end.json').read_text())
    manifest = json.loads((run_base / 'archive-manifest.json').read_text())
    raw = Path(ready['raw_path'])
    removal_path = run_base / 'raw-removal.json'
    if removal_path.exists():
        raise RuntimeError('Refusing to overwrite raw-removal receipt')
    if (fs(RAW_MOUNT)['uuid'] != RAW_UUID or run(['systemctl', 'is-active', UNIT], check=False).strip() != 'inactive'
            or pid() != 0 or established_peers()):
        raise RuntimeError('Raw volume identity or closed-writer guard failed')
    if raw.parent != RAW_MOUNT or raw.is_symlink() or not raw.is_file():
        raise RuntimeError('Refusing to remove a raw path outside this campaign mount')
    if (args.verified_host_raw_sha256 != end.get('raw_sha256')
            or args.verified_host_gzip_sha256 != manifest.get('compressed_sha256')):
        raise RuntimeError('Host hash receipt parameters do not match the immutable guest archive')
    if (manifest.get('raw_sha256') != end.get('raw_sha256')
            or manifest.get('raw_bytes') != end.get('raw_bytes')
            or sha256(raw) != end.get('raw_sha256') or raw.stat().st_size != end.get('raw_bytes')
            or sha256(Path(manifest['archive_path'])) != manifest.get('compressed_sha256')):
        raise RuntimeError('Guest raw or compressed archive no longer matches its recorded digest')
    raw.unlink()
    result = {'run': args.run_id, 'campaign_id': CAMPAIGN_ID,
              'removed_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'raw_path': str(raw), 'raw_sha256': end['raw_sha256'], 'raw_bytes': end['raw_bytes'],
              'compressed_sha256': manifest['compressed_sha256'],
              'host_verification_hashes_match': True, 'collector_writer_closed': True,
              'raw_volume_uuid': RAW_UUID}
    removal_path.write_text(json.dumps(result, indent=2) + '\n')
    share_with_lab_account(run_base, removal_path)
    print(json.dumps(result))


parser = argparse.ArgumentParser(description=__doc__)
sub = parser.add_subparsers(dest='command', required=True)
prepare_parser = sub.add_parser('prepare')
prepare_parser.add_argument('--campaign-id', default=DEFAULT_CAMPAIGN_ID)
prepare_parser.add_argument('--run-id', required=True)
prepare_parser.add_argument('--mode', choices=('authoritative', 'recursive'), required=True)
prepare_parser.add_argument('--qps', type=int, required=True)
prepare_parser.add_argument('--duration', type=int, choices=(60, 300), required=True)
prepare_parser.add_argument('--forecast-raw-bytes', type=int, required=True)
prepare_parser.add_argument('--raw-budget-bytes', type=int, default=12 * GIB)
prepare_parser.add_argument('--capture-policy', choices=('all_supported', 'client_only'),
                            default='all_supported')
finish_parser = sub.add_parser('finish')
finish_parser.add_argument('--campaign-id', default=DEFAULT_CAMPAIGN_ID)
finish_parser.add_argument('--run-id', required=True)
finish_parser.add_argument('--sender-stopped', action='store_true')
monitor_parser = sub.add_parser('monitor')
monitor_parser.add_argument('--campaign-id', default=DEFAULT_CAMPAIGN_ID)
monitor_parser.add_argument('--run-id', required=True)
monitor_parser.add_argument('--seconds', type=int, default=375)
archive_parser = sub.add_parser('archive')
archive_parser.add_argument('--campaign-id', default=DEFAULT_CAMPAIGN_ID)
archive_parser.add_argument('--run-id', required=True)
remove_parser = sub.add_parser('remove-verified-raw')
remove_parser.add_argument('--campaign-id', default=DEFAULT_CAMPAIGN_ID)
remove_parser.add_argument('--run-id', required=True)
remove_parser.add_argument('--verified-host-raw-sha256', required=True)
remove_parser.add_argument('--verified-host-gzip-sha256', required=True)
arguments = parser.parse_args()
if arguments.command == 'prepare':
    prepare(arguments)
elif arguments.command == 'monitor':
    monitor(arguments)
elif arguments.command == 'archive':
    archive(arguments)
elif arguments.command == 'remove-verified-raw':
    remove_verified_raw(arguments)
else:
    finish(arguments)
