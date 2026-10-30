"""Install the isolated TTL-zero recursive fixture and per-mode nft packet counters."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import socket
import struct
import subprocess
import time

DEFAULT_CAMPAIGN_ID = 'parser-ramp-20261030'
CAMPAIGN_ID = DEFAULT_CAMPAIGN_ID
BASE = Path(f'/home/dnstaplab/{DEFAULT_CAMPAIGN_ID}/upstream')
NAMES = BASE / 'rec-names.txt'
CONFIG = BASE / 'dnsmasq.conf'
RULES = BASE / 'dnstap-performance.nft'
TABLE = 'dnstap_performance'
EXPECTED_NAMES_SHA256 = '9B918CC7DA1155583B1E032741CCCE8B4665C26103DBC394FDF49A9BFF885BA6'
WINDOWS = '172.27.215.163'
UBUNTU = '172.27.215.60'


def call(argv, *, check=True, input_text=None):
    return subprocess.run(argv, check=check, input=input_text, capture_output=True, text=True)


def sha(path):
    return hashlib.file_digest(Path(path).open('rb'), 'sha256').hexdigest().upper()


def current_sysctls():
    return {key: int(call(['sysctl', '-n', key]).stdout.strip())
            for key in ('net.core.rmem_default', 'net.core.rmem_max')}


def set_sysctls(values):
    call(['sysctl', '-w', f'net.core.rmem_default={values["net.core.rmem_default"]}'])
    call(['sysctl', '-w', f'net.core.rmem_max={values["net.core.rmem_max"]}'])


def socket_buffer_evidence():
    text = call(['ss', '-uanm', 'src', UBUNTU, 'sport', '=', ':53']).stdout
    match = re.search(r'\brb(\d+)', text)
    return {'text': text, 'receive_buffer_bytes': int(match.group(1)) if match else None}


def open_socket(address, port, timeout=3):
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(timeout)
    sock.bind((UBUNTU, 0))
    return sock


def encode_name(name):
    return b''.join(bytes([len(label)]) + label.encode('ascii') for label in name.rstrip('.').split('.')) + b'\0'


def skip_name(data, offset):
    while True:
        length = data[offset]
        if length & 0xC0 == 0xC0:
            return offset + 2
        offset += 1
        if length == 0:
            return offset
        offset += length


def query_proof(qname):
    ident = 0x5A19
    query = struct.pack('!HHHHHH', ident, 0x0100, 1, 0, 0, 0) + encode_name(qname) + struct.pack('!HH', 1, 1)
    with open_socket(UBUNTU, 53) as sock:
        sock.sendto(query, (UBUNTU, 53))
        response, _ = sock.recvfrom(4096)
    if len(response) < 12:
        raise RuntimeError('DNS fixture response header is truncated')
    rid, flags, qd, an, ns, ar = struct.unpack('!HHHHHH', response[:12])
    if rid != ident or not flags & 0x8000 or flags & 0x000F or qd != 1 or an < 1:
        raise RuntimeError('DNS fixture response does not contain a successful answer')
    pos = skip_name(response, 12) + 4
    rr = []
    for _ in range(an):
        pos = skip_name(response, pos)
        rtype, rclass, ttl, rdlength = struct.unpack('!HHIH', response[pos:pos+10])
        pos += 10
        rdata = response[pos:pos+rdlength]
        pos += rdlength
        rr.append({'type': rtype, 'class': rclass, 'ttl': ttl,
                   'address': socket.inet_ntoa(rdata) if rtype == 1 and len(rdata) == 4 else None})
    if not any(item == {'type': 1, 'class': 1, 'ttl': 0, 'address': '192.0.2.53'} for item in rr):
        raise RuntimeError('Fixture answer does not match the expected TTL-zero A record')
    return {'qname': qname, 'response_bytes': len(response), 'rcode': flags & 0xF,
            'aa': bool(flags & 0x0400), 'answers': rr, 'dnsmasq_source_address': UBUNTU,
            'proof_scope': 'Single local control query before nft stage counters; not a workload sample.'}


def counters():
    tree = json.loads(call(['nft', '-j', 'list', 'table', 'inet', TABLE]).stdout)
    result = {}
    for item in tree.get('nftables', []):
        rule = item.get('rule', {})
        comment = rule.get('comment')
        if not isinstance(comment, str):
            continue
        for expr in rule.get('expr', []):
            if isinstance(expr, dict) and isinstance(expr.get('counter'), dict):
                result[comment] = int(expr['counter']['packets'])
    return result


def configure_campaign(args):
    global BASE, CAMPAIGN_ID, NAMES, CONFIG, RULES
    if not re.fullmatch(r'[A-Za-z0-9_-]{1,64}', args.campaign_id):
        raise SystemExit('Unsafe campaign ID')
    CAMPAIGN_ID = args.campaign_id
    BASE = Path('/home/dnstaplab') / CAMPAIGN_ID / 'upstream'
    NAMES = BASE / 'rec-names.txt'
    CONFIG = BASE / 'dnsmasq.conf'
    RULES = BASE / 'dnstap-performance.nft'


def prepare(args):
    configure_campaign(args)
    if Path('/etc/hostname').read_text().strip() != 'dnstap-ubuntu2604':
        raise RuntimeError('Unexpected Ubuntu guest')
    BASE.mkdir(mode=0o700, parents=True, exist_ok=True)
    if not NAMES.exists() and not args.resume:
        source_names = Path(args.names_source)
        if not source_names.is_file() or sha(source_names) != EXPECTED_NAMES_SHA256:
            raise RuntimeError('Source recursive workload names do not match preserved source hash')
        shutil.copyfile(source_names, NAMES)
        NAMES.chmod(0o600)
    unit = 'dnstap-performance-upstream'
    upstream_active = call(['systemctl', 'is-active', unit], check=False).stdout.strip() == 'active'
    if call(['systemctl', 'is-active', 'dnscollector'], check=False).stdout.strip() != 'inactive':
        raise RuntimeError('Collector must be stopped while installing the recursive fixture')
    if not args.resume and upstream_active:
        raise RuntimeError('Historical upstream unit name is already active')
    if not args.resume and call(['pgrep', '-a', 'dnsmasq'], check=False).stdout.strip():
        raise RuntimeError('Unowned dnsmasq process is already present')
    if call(['nft', 'list', 'table', 'inet', TABLE], check=False).returncode == 0:
        raise RuntimeError('Refusing to replace existing nft table')
    if (not args.resume and call(['ss', '-lnut'], check=True).stdout.find(f'{UBUNTU}:53') >= 0):
        raise RuntimeError('Ubuntu upstream address port 53 is already occupied')
    if not NAMES.is_file() or sha(NAMES) != EXPECTED_NAMES_SHA256:
        raise RuntimeError('Transferred recursive workload names do not match preserved source hash')
    names = NAMES.read_text(encoding='utf-8').splitlines()
    if len(names) != 5000 or any(not name.endswith('.rec.stress.test') for name in names):
        raise RuntimeError('Expected exactly 5000 rec.stress.test workload names')
    if (RULES.exists() or (BASE / 'upstream-ready.json').exists()
            or (not args.resume and CONFIG.exists())):
        raise RuntimeError('Task upstream evidence/config path already exists')
    if not args.resume:
        (BASE / 'names-source.sha256').write_text(EXPECTED_NAMES_SHA256 + '  rec-names.txt\n')
    config = ('port=53\nlisten-address=172.27.215.60\nbind-interfaces\nno-resolv\nno-hosts\n'
              'cache-size=0\nlocal-ttl=0\nauth-ttl=0\n'
              'auth-server=ns.rec.stress.test,172.27.215.60\nauth-zone=rec.stress.test\n'
              'user=nobody\ngroup=nogroup\nlog-facility=-\n')
    config += ''.join(f'host-record={name},192.0.2.53,0\n' for name in names)
    if args.resume:
        if not CONFIG.is_file() or CONFIG.read_text(encoding='utf-8') != config or not upstream_active:
            raise RuntimeError('Partial upstream state does not match the exact task fixture; preserve and inspect')
        exec_start = call(['systemctl', 'show', unit, '-p', 'ExecStart', '--no-pager']).stdout
        if str(CONFIG) not in exec_start:
            raise RuntimeError('Active upstream service does not point to the task-owned config')
    else:
        CONFIG.write_text(config)
    config_test = call(['/usr/sbin/dnsmasq', '--test', f'--conf-file={CONFIG}'])
    if not args.resume:
        (BASE / 'dnsmasq-config-test.txt').write_text(config_test.stdout + config_test.stderr)
        service_start = call(['systemd-run', f'--unit={unit}', '--property=Restart=no',
                              '--', '/usr/sbin/dnsmasq', '--keep-in-foreground', f'--conf-file={CONFIG}'])
        (BASE / 'upstream-start.txt').write_text(service_start.stdout + service_start.stderr)
    sockets = ''
    for _ in range(100):
        sockets = call(['ss', '-lnutp']).stdout
        if f'{UBUNTU}:53' in sockets and 'dnsmasq' in sockets:
            break
        time.sleep(0.1)
    if (call(['systemctl', 'is-active', unit], check=False).stdout.strip() != 'active'
            or f'{UBUNTU}:53' not in sockets or 'dnsmasq' not in sockets):
        raise RuntimeError('dnsmasq is not bound to the isolated Ubuntu port 53 address')
    proof = query_proof(names[0])
    (BASE / 'local-answer-proof.json').write_text(json.dumps(proof, indent=2) + '\n')
    rules = f'''table inet {TABLE} {{
 chain input {{
  type filter hook input priority filter; policy accept;
  ip daddr {UBUNTU} ip saddr {WINDOWS} udp dport 53 counter accept comment "upstream_queries_udp"
  ip daddr {UBUNTU} ip saddr {WINDOWS} tcp dport 53 counter accept comment "upstream_queries_tcp"
  ip daddr {UBUNTU} udp dport 53 counter drop comment "upstream_other_udp_dropped"
  ip daddr {UBUNTU} tcp dport 53 counter drop comment "upstream_other_tcp_dropped"
 }}
 chain output {{
  type filter hook output priority filter; policy accept;
  ip saddr {UBUNTU} ip daddr {WINDOWS} udp sport 53 counter comment "upstream_responses_udp"
  ip saddr {UBUNTU} ip daddr {WINDOWS} tcp sport 53 counter comment "upstream_responses_tcp"
 }}
}}
'''
    call(['nft', '--check', '-f', '-'], input_text=rules)
    call(['nft', '-f', '-'], input_text=rules)
    (BASE / 'dnstap-performance.nft').write_text(rules)
    counts = counters()
    expected = ('upstream_queries_udp', 'upstream_queries_tcp', 'upstream_responses_udp',
                'upstream_responses_tcp', 'upstream_other_udp_dropped', 'upstream_other_tcp_dropped')
    if any(key not in counts or counts[key] != 0 for key in expected):
        raise RuntimeError('New nft stage counters did not start at zero')
    backup_dir = BASE / 'backup'
    backup_dir.mkdir(mode=0o700, exist_ok=True)
    backup_path = backup_dir / 'rmem-original.json'
    original = current_sysctls()
    historical = {'net.core.rmem_default': 212992, 'net.core.rmem_max': 4194304}
    if backup_path.exists():
        raise RuntimeError('Refusing to overwrite a socket-buffer baseline receipt')
    backup_path.write_text(json.dumps({'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                                       'values': original}, indent=2) + '\n')
    applied = {'net.core.rmem_default': max(original['net.core.rmem_default'], 8 * 1024 * 1024),
               'net.core.rmem_max': max(original['net.core.rmem_max'], 16 * 1024 * 1024)}
    pid_before = int(call(['systemctl', 'show', unit, '-p', 'MainPID', '--value']).stdout.strip())
    before_restore = None
    try:
        call(['sysctl', '-w', f'net.core.rmem_max={applied["net.core.rmem_max"]}'])
        call(['sysctl', '-w', f'net.core.rmem_default={applied["net.core.rmem_default"]}'])
        call(['systemctl', 'restart', unit])
        for _ in range(50):
            before_restore = socket_buffer_evidence()
            if (before_restore['receive_buffer_bytes'] is not None
                    and before_restore['receive_buffer_bytes'] >= 8 * 1024 * 1024):
                break
            time.sleep(0.1)
        else:
            raise RuntimeError('Owned DNS socket did not acquire historical 8 MiB receive buffer')
    finally:
        set_sysctls(original)
    restored = current_sysctls()
    after_restore = socket_buffer_evidence()
    if (restored != original or after_restore['receive_buffer_bytes'] is None
            or after_restore['receive_buffer_bytes'] < 8 * 1024 * 1024
            or call(['systemctl', 'is-active', unit], check=False).stdout.strip() != 'active'):
        raise RuntimeError('Sysctl defaults did not restore cleanly or socket buffer did not persist')
    buffer_receipt = {'created_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                      'original_values': original, 'historical_values': historical,
                      'temporary_values': applied, 'restored_values': restored,
                      'socket_before_global_restore': before_restore,
                      'socket_after_global_restore': after_restore,
                      'upstream_pid_before_restart': pid_before,
                      'upstream_pid_after_restart': int(call(['systemctl', 'show', unit, '-p', 'MainPID', '--value']).stdout.strip()),
                      'global_defaults_restored_before_load': True}
    (BASE / 'buffer-adjustment.json').write_text(json.dumps(buffer_receipt, indent=2) + '\n')
    output = {'campaign_id': CAMPAIGN_ID, 'capture_policy': args.capture_policy,
              'upstream_ready': True, 'address': UBUNTU, 'port': 53, 'transport': ['udp', 'tcp'],
              'unit': unit, 'service_active': True, 'collector_active': False,
              'udp_socket_receive_buffer_bytes': after_restore['receive_buffer_bytes'],
              'sysctl_restored_values': restored,
              'names_file': str(NAMES), 'names_sha256': sha(NAMES), 'names_count': len(names),
              'zone': 'rec.stress.test', 'answer_a': '192.0.2.53', 'answer_ttl': 0,
              'cache_size': 0, 'dnsmasq_config_sha256': sha(CONFIG),
              'dnsmasq_binary_version': call(['/usr/sbin/dnsmasq', '--version']).stdout.splitlines()[0],
              'local_answer_proof': proof, 'nft_table': f'inet {TABLE}',
              'nft_counter_baseline': counts,
              'counter_scope': 'Windows resolver traffic only for query counters and replies to that same host; other DNS packets to Ubuntu port 53 are dropped and counted.',
              'ttl_zero_caveat': 'Cache behavior is measured from nft query/response packet deltas; TTL zero does not imply one upstream query per client request.',
              'prometheus_receiver_counters': None,
              'created_utc': datetime.datetime.now(datetime.timezone.utc).isoformat()}
    path = BASE / 'upstream-ready.json'
    path.write_text(json.dumps(output, indent=2) + '\n')
    print(json.dumps(output))


parser = argparse.ArgumentParser(description=__doc__)
sub = parser.add_subparsers(dest='command', required=True)
prepare_parser = sub.add_parser('prepare')
prepare_parser.add_argument('--campaign-id', default=DEFAULT_CAMPAIGN_ID)
prepare_parser.add_argument('--names-source', default='/home/dnstaplab/parser-ramp-20261030/upstream/rec-names.txt')
prepare_parser.add_argument('--capture-policy', choices=('all_supported', 'client_only'),
                            default='all_supported')
prepare_parser.add_argument('--resume', action='store_true')
arguments = parser.parse_args()
if arguments.command == 'prepare':
    prepare(arguments)
