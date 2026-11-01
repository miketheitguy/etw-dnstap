"""Run a bounded circular tcpdump capture for one service-stop A/B stage."""
import argparse
import datetime
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import pwd
import re
import signal
import subprocess
import sys
import time
sys.path.insert(0, str(Path(__file__).resolve().parent))  # lab_config lives next to this script
import lab_config  # noqa: E402

HOST = lab_config.get('ubuntu_vm.hostname')
DEFAULT_CAMPAIGN = 'service-stop-20261030-0633'
BASE = Path('/home/dnstaplab') / DEFAULT_CAMPAIGN
TCPDUMP = Path('/usr/bin/tcpdump')
MAX_RING_BYTES = 4 * 16 * 1024 * 1024
CAPTURE_SETS = ('tail', 'server-stream')


def utc() -> str:
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def safe_run_id(value: str) -> None:
    if not re.fullmatch(r'[A-Za-z0-9_-]{1,64}', value):
        raise SystemExit('Unsafe run ID')


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def share_tree(path: Path) -> None:
    account = pwd.getpwnam('dnstaplab')
    for item in [path, *path.iterdir()]:
        if item.is_file():
            os.chown(item, account.pw_uid, account.pw_gid)
            os.chmod(item, 0o600)
        elif item.is_dir():
            os.chown(item, account.pw_uid, account.pw_gid)
            os.chmod(item, 0o700)


def parse_stats(text: str) -> dict:
    patterns = {
        'packets_captured': r'(\d+) packets captured',
        'packets_received_by_filter': r'(\d+) packets received by filter',
        'packets_dropped_by_kernel': r'(\d+) packets dropped by kernel',
    }
    result = {key: None for key in patterns}
    for key, pattern in patterns.items():
        match = re.search(pattern, text)
        if match:
            result[key] = int(match.group(1))
    result['stats_complete'] = all(value is not None for value in result.values())
    return result


def worker(run_dir: Path, capture_set: str, client_ip: str, collector_ip: str) -> int:
    client_ip = str(ipaddress.IPv4Address(client_ip))
    collector_ip = str(ipaddress.IPv4Address(collector_ip))
    pcap_dir = run_dir / 'pcap'
    prefix = pcap_dir / f'{capture_set}.pcap'
    stats_path = pcap_dir / f'tcpdump-{capture_set}-stderr.log'
    process_path = pcap_dir / f'tcpdump-{capture_set}-process.json'
    stop_path = pcap_dir / f'tcpdump-{capture_set}-stop.json'
    endpoints = f'host {client_ip} and host {collector_ip} and tcp port 6000'
    if capture_set == 'tail':
        bpf = endpoints
    elif capture_set == 'server-stream':
        payload = (f'src host {collector_ip} and '
                   'ip[2:2] > (((ip[0] & 0x0f) << 2) + ((tcp[12] & 0xf0) >> 2))')
        close_flags = 'tcp[tcpflags] & (tcp-syn|tcp-fin|tcp-rst) != 0'
        bpf = f'{endpoints} and (({payload}) or ({close_flags}))'
    else:
        raise RuntimeError('Unknown task-local capture set')
    command = [str(TCPDUMP), '-i', 'eth0', '-n', '-s', '0', '-B', '8192', '-U',
               '-C', '16', '-W', '4', '-Z', 'root', '-w', str(prefix), bpf]
    with stats_path.open('xb') as stats_stream:
        capture = subprocess.Popen(command, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.DEVNULL, stderr=stats_stream)
        process = {'capture_set': capture_set, 'pid': capture.pid,
                   'started_utc': utc(), 'command': command,
                   'interface': 'eth0', 'link_type_expected': 'EN10MB (Ethernet)',
                   'snaplen': 0, 'kernel_buffer_kib': 8192,
                   'ring_file_limit': 4, 'ring_file_size_mib': 16,
                   'bpf': bpf, 'client_ip': client_ip, 'collector_ip': collector_ip,
                   'collector_port': 6000}
        process_path.write_text(json.dumps(process, indent=2) + '\n')
        exit_code = capture.wait()
    stats_text = stats_path.read_text(errors='replace')
    files = []
    for item in sorted(pcap_dir.glob(f'{capture_set}.pcap*')):
        if item.is_file() and item.name not in ('tcpdump-process.json', 'tcpdump-stop.json'):
            files.append({'name': item.name, 'bytes': item.stat().st_size,
                          'sha256': sha256(item)})
    result = {'run_dir': str(run_dir), 'capture_set': capture_set,
              'stopped_utc': utc(), 'pid': capture.pid,
              'exit_code': exit_code, 'signal': 'SIGINT', 'tcpdump_stats_text': stats_text,
              'tcpdump_stats': parse_stats(stats_text), 'ring_files': files,
              'ring_total_bytes': sum(item['bytes'] for item in files),
              'ring_limit_bytes': MAX_RING_BYTES,
              'ring_within_limit': sum(item['bytes'] for item in files) <= MAX_RING_BYTES + 4096,
              'capture_clean_exit': exit_code == 0 and parse_stats(stats_text)['stats_complete']}
    stop_path.write_text(json.dumps(result, indent=2) + '\n')
    return exit_code


def start(args) -> None:
    if args.campaign_id != DEFAULT_CAMPAIGN:
        raise RuntimeError('This pcap helper is scoped to its fixed campaign directory')
    if subprocess.run(['hostname'], check=True, capture_output=True, text=True).stdout.strip() != HOST:
        raise RuntimeError('Unexpected Ubuntu receiver hostname')
    if not TCPDUMP.is_file():
        raise RuntimeError('tcpdump is unavailable at its verified path')
    ipaddress.IPv4Address(args.client_ip)
    ipaddress.IPv4Address(args.collector_ip)
    safe_run_id(args.run_id)
    run_dir = BASE / args.run_id
    if run_dir.is_symlink() or not run_dir.is_dir():
        raise RuntimeError('Stage directory must already exist and must not be a symlink')
    pcap_dir = run_dir / 'pcap'
    if pcap_dir.exists() or pcap_dir.is_symlink():
        raise RuntimeError('Refusing to reuse an existing pcap evidence directory')
    pcap_dir.mkdir(mode=0o700)
    start_path = pcap_dir / 'tcpdump-start.json'
    processes = {}
    worker_procs = {}
    try:
        for capture_set in CAPTURE_SETS:
            worker_command = [sys.executable, str(Path(__file__).resolve()), '_worker',
                              str(run_dir), capture_set, args.client_ip, args.collector_ip]
            proc = subprocess.Popen(worker_command, stdin=subprocess.DEVNULL,
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                    start_new_session=True)
            worker_procs[capture_set] = proc
            process_path = pcap_dir / f'tcpdump-{capture_set}-process.json'
            stats_path = pcap_dir / f'tcpdump-{capture_set}-stderr.log'
            deadline = time.monotonic() + 15
            process = None
            while time.monotonic() < deadline:
                if proc.poll() is not None:
                    raise RuntimeError(f'{capture_set} tcpdump worker exited before start: {proc.returncode}')
                if process_path.is_file():
                    process = json.loads(process_path.read_text())
                    stats_text = stats_path.read_text(errors='replace') if stats_path.exists() else ''
                    if 'listening on eth0' in stats_text:
                        break
                time.sleep(0.1)
            stats_text = stats_path.read_text(errors='replace') if stats_path.exists() else ''
            if not process or 'listening on eth0' not in stats_text:
                stop_owned_capture(process_path, pcap_dir / f'{capture_set}.pcap', proc)
                raise RuntimeError(f'{capture_set} tcpdump did not publish its listening state')
            processes[capture_set] = {'worker_pid': proc.pid, **process}
    except BaseException:
        for capture_set, worker_proc in worker_procs.items():
            process_path = pcap_dir / f'tcpdump-{capture_set}-process.json'
            stop_owned_capture(process_path, pcap_dir / f'{capture_set}.pcap', worker_proc)
        raise
    receipt = {'campaign_id': args.campaign_id, 'run': args.run_id,
               'capture_started_before_sender_connection': True,
               'all_capture_sets_listening': len(processes) == len(CAPTURE_SETS),
               'captures': {capture_set: {'capture_started_utc': process['started_utc'],
                   'worker_pid': process['worker_pid'], 'tcpdump_pid': process['pid'],
                   'command': process['command'], 'interface': process['interface'],
                   'link_type_expected': process['link_type_expected'], 'snaplen': process['snaplen'],
                   'kernel_buffer_kib': process['kernel_buffer_kib'],
                   'ring_file_limit': process['ring_file_limit'],
                   'ring_file_size_mib': process['ring_file_size_mib'], 'bpf': process['bpf'],
                   'client_ip': process['client_ip'], 'collector_ip': process['collector_ip'],
                   'collector_port': process['collector_port']}
                   for capture_set, process in processes.items()}}
    start_path.write_text(json.dumps(receipt, indent=2) + '\n')
    share_tree(pcap_dir)
    print(json.dumps(receipt))


def stop_owned_capture(process_path: Path, prefix: Path, worker_proc=None) -> None:
    process_found = False
    if process_path.is_file():
        process = json.loads(process_path.read_text())
        pid = process.get('pid')
        if isinstance(pid, int) and not isinstance(pid, bool) and pid > 1:
            command_path = Path(f'/proc/{pid}/cmdline')
            if command_path.is_file():
                command_line = command_path.read_bytes().replace(b'\0', b' ').decode(errors='replace')
                if 'tcpdump' in command_line and str(prefix) in command_line:
                    process_found = True
                    try:
                        os.kill(pid, signal.SIGINT)
                    except ProcessLookupError:
                        pass
    if not process_found and worker_proc is not None and worker_proc.poll() is None:
        try:
            os.killpg(worker_proc.pid, signal.SIGINT)
        except ProcessLookupError:
            pass
    if worker_proc is not None:
        try:
            worker_proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(worker_proc.pid, signal.SIGINT)
            except ProcessLookupError:
                pass
            worker_proc.wait(timeout=5)


def stop(args) -> None:
    if args.campaign_id != DEFAULT_CAMPAIGN:
        raise RuntimeError('This pcap helper is scoped to its fixed campaign directory')
    safe_run_id(args.run_id)
    pcap_dir = BASE / args.run_id / 'pcap'
    start_path = pcap_dir / 'tcpdump-start.json'
    if not start_path.is_file():
        raise RuntimeError('Capture start receipt is missing')
    start_receipt = json.loads(start_path.read_text())
    receipts = {}
    failures = []
    for capture_set in CAPTURE_SETS:
        process_path = pcap_dir / f'tcpdump-{capture_set}-process.json'
        stop_path = pcap_dir / f'tcpdump-{capture_set}-stop.json'
        prefix = pcap_dir / f'{capture_set}.pcap'
        if not process_path.is_file() or stop_path.exists():
            failures.append(f'{capture_set}: missing process receipt or already finalized')
            continue
        process = json.loads(process_path.read_text())
        pid = process.get('pid')
        if isinstance(pid, bool) or not isinstance(pid, int) or pid <= 1:
            failures.append(f'{capture_set}: invalid tcpdump PID')
            continue
        command_path = Path(f'/proc/{pid}/cmdline')
        if command_path.is_file():
            command_line = command_path.read_bytes().replace(b'\0', b' ').decode(errors='replace')
            if 'tcpdump' not in command_line or str(prefix) not in command_line:
                failures.append(f'{capture_set}: refusing to signal an unbound PID')
                continue
            try:
                os.kill(pid, signal.SIGINT)
            except ProcessLookupError:
                pass
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline and not stop_path.is_file():
            time.sleep(0.1)
        if not stop_path.is_file():
            failures.append(f'{capture_set}: no flushed stop receipt within 30 seconds')
            continue
        worker_receipt = json.loads(stop_path.read_text())
        if worker_receipt.get('pid') != pid or worker_receipt.get('capture_set') != capture_set:
            failures.append(f'{capture_set}: stop receipt PID/capture-set mismatch')
            continue
        worker_receipt['campaign_id'] = args.campaign_id
        worker_receipt['run'] = args.run_id
        worker_receipt['worker_stop_receipt_sha256'] = sha256(stop_path)
        stop_path.write_text(json.dumps(worker_receipt, indent=2) + '\n')
        receipts[capture_set] = worker_receipt
    if failures:
        share_tree(pcap_dir)
        raise RuntimeError('; '.join(failures))
    receipt = {'campaign_id': args.campaign_id, 'run': args.run_id,
               'stopped_utc': utc(), 'stop_signal': 'SIGINT',
               'start_receipt_sha256': sha256(start_path), 'capture_sets': receipts}
    stop_path = pcap_dir / 'tcpdump-stop.json'
    stop_path.write_text(json.dumps(receipt, indent=2) + '\n')
    share_tree(pcap_dir)
    print(json.dumps(receipt))


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--campaign-id', default=DEFAULT_CAMPAIGN)
sub = parser.add_subparsers(dest='command', required=True)
start_parser = sub.add_parser('start')
start_parser.add_argument('--run-id', required=True)
start_parser.add_argument('--client-ip', required=True)
start_parser.add_argument('--collector-ip', default=lab_config.get('network.receiver_ip'))
stop_parser = sub.add_parser('stop')
stop_parser.add_argument('--run-id', required=True)
worker_parser = sub.add_parser('_worker')
worker_parser.add_argument('run_dir')
worker_parser.add_argument('capture_set', choices=CAPTURE_SETS)
worker_parser.add_argument('client_ip')
worker_parser.add_argument('collector_ip')
args = parser.parse_args()
if args.command == 'start':
    start(args)
elif args.command == 'stop':
    stop(args)
else:
    raise SystemExit(worker(Path(args.run_dir), args.capture_set, args.client_ip, args.collector_ip))
