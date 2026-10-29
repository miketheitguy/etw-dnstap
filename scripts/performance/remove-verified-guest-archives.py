"""Remove only this campaign's guest duplicates after host verification.

Copy the existing host verification and exact record-count manifests into BASE
as RUN-host-verification.json and RUN-archive-counts.json first. Defaults to a
read-only plan. Historical campaigns and protected backups are never traversed.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import re

parser = argparse.ArgumentParser()
parser.add_argument('runs', nargs='+')
parser.add_argument('--delete', action='store_true')
args = parser.parse_args()
base = Path(os.environ['DNSTAP_PERF_BASE']).resolve()
if base.parent != Path('/home/dnstaplab') or not re.fullmatch(r'performance-pooled-vm-20261029-0942', base.name):
    raise SystemExit('Removal is restricted to the authorized current campaign')
plans = []
for run in args.runs:
    if not re.fullmatch(r'pooled-[A-Za-z0-9_-]{1,64}', run):
        raise SystemExit('Unsafe or unrelated run ID')
    archive = base / f'{run}-performance.jsonl.gz'
    receipt_path = base / f'{run}-guest-archive-removal.json'
    if receipt_path.exists() or archive.is_symlink() or archive.resolve().parent != base:
        raise SystemExit('Prior removal receipt, symlink, or unexpected archive path')
    source = json.loads((base / f'{run}-archive-source.json').read_text())
    host = json.loads((base / f'{run}-host-verification.json').read_text())
    counts_path = base / f'{run}-archive-counts.json'
    counts = json.loads(counts_path.read_text())
    if host.get('gzip_integrity_verified') is not True or host.get('source_hashes_match') is not True:
        raise SystemExit('Host CRC/hash verification is not explicit')
    if any(host.get(key) != source.get(key) for key in ('raw_sha256', 'compressed_sha256', 'raw_bytes', 'compressed_bytes')):
        raise SystemExit('Host and source archive proofs disagree')
    if (host['archive'].replace('\\', '/').rsplit('/', 1)[-1] != archive.name
            or counts['archive'].replace('\\', '/').rsplit('/', 1)[-1] != archive.name
            or type(counts.get('all_json_records')) is not int
            or counts['all_json_records'] < 0):
        raise SystemExit('Host archive/count manifest does not identify this run')
    with archive.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    if digest != host['compressed_sha256'] or archive.stat().st_size != host['compressed_bytes']:
        raise SystemExit('Guest duplicate changed after host verification')
    plans.append((archive, receipt_path, {
        'run': run, 'guest_archive_path': str(archive), 'compressed_sha256': digest,
        'compressed_bytes': archive.stat().st_size, 'retained_host_archive': host['archive'],
        'host_raw_sha256': host['raw_sha256'], 'host_gzip_crc_and_hash_verified': True,
        'host_exact_json_records': counts['all_json_records'],
        'host_count_manifest_sha256': hashlib.sha256(counts_path.read_bytes()).hexdigest(),
    }))
for archive, receipt_path, receipt in plans:
    if args.delete:
        archive.unlink()
        receipt['removed_utc'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        with receipt_path.open('x') as out:
            out.write(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps({'delete': args.delete, **receipt}))
