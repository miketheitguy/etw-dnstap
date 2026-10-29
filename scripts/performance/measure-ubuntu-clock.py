"""Bound Ubuntu-minus-host UTC offset over an established SSH connection."""
import argparse
import datetime
import json
from pathlib import Path
import shlex
import subprocess
import time

parser = argparse.ArgumentParser()
parser.add_argument('--config', default='target/lab-ubuntu/ssh_config')
parser.add_argument('--output', required=True, type=Path)
args = parser.parse_args()
code = 'import sys,time\nfor line in sys.stdin:\n print(time.time_ns(),flush=True)'
process = subprocess.Popen(['ssh.exe', '-T', '-F', args.config, 'dnstap-ubuntu',
                            'python3 -u -c '+shlex.quote(code)],
                           stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
samples = []
try:
    for _ in range(8):
        before = time.time_ns()
        process.stdin.write('clock\n')
        process.stdin.flush()
        remote = int(process.stdout.readline().strip())
        after = time.time_ns()
        samples.append({'host_before_ns': before, 'ubuntu_ns': remote, 'host_after_ns': after,
                        'offset_lower_ms': (remote-after)/1e6,
                        'offset_upper_ms': (remote-before)/1e6})
finally:
    process.stdin.close()
    process.wait(timeout=5)
result = {'measured_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
          'method': 'Remote time sampled between host send/read UTC over an established SSH connection; assumes negligible clock drift during exchange.',
          'offset_lower_ms': max(s['offset_lower_ms'] for s in samples),
          'offset_upper_ms': min(s['offset_upper_ms'] for s in samples), 'samples': samples}
args.output.write_text(json.dumps(result, indent=2)+'\n')
print(json.dumps(result))
