"""Temporarily enlarge the owned lab upstream's UDP receive socket.

Run only between benchmark windows. Original guest sysctls are restored by the
fixture cleanup, and immediately if applying or verifying the change fails.
"""
import argparse
import datetime
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
sys.path.insert(0, str(Path(__file__).resolve().parent))  # lab_config lives next to this script
import lab_config  # noqa: E402

parser = argparse.ArgumentParser()
parser.add_argument('--restore', action='store_true')
parser.add_argument('--retry', action='store_true')
args = parser.parse_args()
BASE = Path(os.environ.get('DNSTAP_PERF_BASE') or Path(lab_config.get('ubuntu_vm.home')) / 'performance')
BACKUP = BASE / 'backup' / 'rmem-original.json'
KEYS = ('net.core.rmem_default', 'net.core.rmem_max')
if subprocess.check_output(['hostname']).decode().strip() != lab_config.get('ubuntu_vm.hostname'):
    raise SystemExit('Wrong guest identity')

def read_values():
    return {key: int(subprocess.check_output(['sysctl', '-n', key])) for key in KEYS}

def write(key, value):
    subprocess.run(['sysctl', '-w', f'{key}={int(value)}'], check=True)

def restore():
    saved = json.loads(BACKUP.read_text())
    if set(saved['values']) != set(KEYS):
        raise RuntimeError('Unexpected sysctl keys in baseline receipt')
    # Lower the default before lowering its ceiling.
    for key in KEYS:
        write(key, saved['values'][key])
    if read_values() != saved['values']:
        raise RuntimeError('Restored sysctls do not match baseline')
    return saved

if args.restore:
    if BACKUP.exists():
        restore()
        print(json.dumps({'restored': read_values()}))
    else:
        print(json.dumps({'restore': 'No buffer adjustment receipt exists'}))
    raise SystemExit()

if BACKUP.exists():
    receipt = json.loads(BACKUP.read_text())
    original = receipt['values']
    if not args.retry or read_values() != original:
        raise SystemExit('Retry requires the preserved baseline and already-restored original values')
else:
    original = read_values()
    receipt = {'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'values': original}
    BACKUP.write_text(json.dumps(receipt, indent=2)+'\n')
try:
    write('net.core.rmem_max', max(original['net.core.rmem_max'], 16*1024*1024))
    write('net.core.rmem_default', max(original['net.core.rmem_default'], 8*1024*1024))
    subprocess.run(['systemctl', 'restart', 'dnstap-performance-upstream'], check=True)
    subprocess.run(['systemctl', 'is-active', 'dnstap-performance-upstream'], check=True)
    ready = False
    for _ in range(50):
        sockets = subprocess.check_output(['ss', '-uanm', f"src {lab_config.get('network.receiver_ip')} sport = :53"]).decode()
        match = re.search(r'\brb(\d+)', sockets)
        if match and int(match.group(1)) >= 8*1024*1024:
            ready = True
            break
        time.sleep(.1)
    if not ready:
        raise RuntimeError('Owned DNS socket did not acquire an 8 MiB receive buffer')
    receipt.update({'applied_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    'applied_values': read_values(), 'socket_evidence': sockets,
                    'upstream_pid': int(subprocess.check_output(['systemctl', 'show', 'dnstap-performance-upstream', '-p', 'MainPID', '--value']))})
    (BASE / 'buffer-adjustment.json').write_text(json.dumps(receipt, indent=2)+'\n')
    print(json.dumps(receipt))
except BaseException:
    restore()
    subprocess.run(['systemctl', 'restart', 'dnstap-performance-upstream'], check=False)
    raise
