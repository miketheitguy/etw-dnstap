"""Archive a stopped receiver's owned run once, preserving source hashes.

Call only after the Windows owner confirms final drain and the end marker.
Host verification must finish before any separate removal of the raw file.
"""
import argparse
import datetime
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))  # lab_config lives next to this script
import lab_config  # noqa: E402

parser = argparse.ArgumentParser()
parser.add_argument('run')
parser.add_argument('--campaign-id', help='required when the raw log is on the temporary data disk (/mnt/...)')
parser.add_argument('--remove-verified-raw', action='store_true')
parser.add_argument('--verified-host-raw-sha256')
parser.add_argument('--verified-host-gzip-sha256')
args = parser.parse_args()
if not re.fullmatch(r'[A-Za-z0-9_-]{1,72}', args.run):
    raise SystemExit('Unsafe run ID')
base = Path(os.environ['DNSTAP_PERF_BASE']).resolve()
raw = Path(os.environ['DNSTAP_PERF_LOG'])
if base.parent != Path(lab_config.get('ubuntu_vm.home')) or not re.fullmatch(r'performance(?:-[a-z0-9-]+)?', base.name):
    raise SystemExit('Unexpected lab evidence directory')
allowed_parent = raw.parent == Path('/var/log/dnscollector')
if raw.parent.parent == Path('/mnt'):
    if not args.campaign_id or not re.fullmatch(r'[A-Za-z0-9_-]{1,64}', args.campaign_id):
        raise SystemExit('--campaign-id is required for a raw log on the temporary data disk')
    storage = json.loads((base/'temporary-data-disk.json').read_text())
    mounted_uuid = subprocess.check_output(['findmnt', '-n', '-o', 'UUID', '--target', str(raw.parent)], text=True).strip()
    allowed_parent = (base.name == f'performance-{args.campaign_id}'
                      and raw.parent.is_mount()
                      and mounted_uuid == storage['filesystem_uuid']
                      and storage['mountpoint'] == str(raw.parent))
if not allowed_parent or not re.fullmatch(r'performance(?:-[a-z0-9-]+)?\.jsonl', raw.name) or raw.is_symlink():
    raise SystemExit('Unexpected receiver log path')
state = subprocess.run(['systemctl', 'is-active', 'dnscollector'], capture_output=True, text=True, check=False).stdout.strip()  # non-zero exit means inactive
if state != 'inactive':
    raise SystemExit('Receiver must be stopped outside the measured window')
start = json.loads((base/f'{args.run}-start.json').read_text())
end = json.loads((base/f'{args.run}-end.json').read_text())
before = raw.stat()
if not 0 <= start['offset'] <= end['offset'] <= before.st_size:
    raise SystemExit('Marker offsets do not fit this raw file epoch')
archive = base/f'{args.run}-performance.jsonl.gz'
manifest = base/f'{args.run}-archive-source.json'
if args.remove_verified_raw:
    saved = json.loads(manifest.read_text())
    removal = base/f'{args.run}-raw-removal.json'
    if removal.exists():
        raise SystemExit('Refusing to overwrite a prior raw-removal receipt')
    if args.verified_host_raw_sha256 != saved['raw_sha256'] or args.verified_host_gzip_sha256 != saved['compressed_sha256']:
        raise SystemExit('Host verification hashes do not match the source manifest')
    with raw.open('rb') as stream:
        current = hashlib.file_digest(stream, 'sha256').hexdigest()
    if current != saved['raw_sha256'] or before.st_size != saved['raw_bytes']:
        raise SystemExit('Owned raw file changed after host verification')
    raw.unlink()
    receipt = {'run': args.run, 'removed_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
               'raw_path': str(raw), 'raw_sha256': current, 'raw_bytes': before.st_size,
               'host_verification_hashes_match': True}
    removal.write_text(json.dumps(receipt, indent=2)+'\n')
    print(json.dumps(receipt))
    raise SystemExit()
if archive.exists() or manifest.exists():
    raise SystemExit('Refusing to overwrite prior archive evidence')
digest = hashlib.sha256()
with raw.open('rb') as source, archive.open('xb') as destination:
    with gzip.GzipFile(filename=archive.name, fileobj=destination, mode='wb', compresslevel=1, mtime=0) as compressed:
        while block := source.read(8*1024*1024):
            digest.update(block)
            compressed.write(block)
after = raw.stat()
if (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
    raise RuntimeError('Raw receiver file changed during compression; retain evidence and investigate')
with archive.open('rb') as stream:
    compressed_sha = hashlib.file_digest(stream, 'sha256').hexdigest()
receipt = {'run': args.run, 'created_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
           'raw_path': str(raw), 'archive_path': str(archive),
           'raw_sha256': digest.hexdigest(), 'compressed_sha256': compressed_sha,
           'raw_bytes': before.st_size, 'compressed_bytes': archive.stat().st_size,
           'start_offset': start['offset'], 'end_offset': end['offset']}
manifest.write_text(json.dumps(receipt, indent=2)+'\n')
print(json.dumps(receipt))
