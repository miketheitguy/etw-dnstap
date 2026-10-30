"""Verify a transferred parser-ramp gzip and stream its exact raw JSONL into a fresh file."""
import argparse
import datetime
import gzip
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--archive', required=True, type=Path)
parser.add_argument('--manifest', required=True, type=Path)
parser.add_argument('--output-raw', required=True, type=Path)
parser.add_argument('--receipt', required=True, type=Path)
args = parser.parse_args()
manifest = json.loads(args.manifest.read_text(encoding='utf-8-sig'))
if args.output_raw.exists() or args.receipt.exists():
    raise SystemExit('Refusing to overwrite raw output or verification receipt')
compressed_hash = hashlib.file_digest(args.archive.open('rb'), 'sha256').hexdigest()
if compressed_hash != manifest.get('compressed_sha256'):
    raise SystemExit('Transferred gzip SHA-256 differs from guest archive manifest')
raw_hash = hashlib.sha256()
raw_bytes = 0
with gzip.open(args.archive, 'rb') as source, args.output_raw.open('xb') as output:
    for block in iter(lambda: source.read(8 * 1024 * 1024), b''):
        raw_hash.update(block)
        raw_bytes += len(block)
        output.write(block)
if raw_bytes != manifest.get('raw_bytes') or raw_hash.hexdigest() != manifest.get('raw_sha256'):
    raise SystemExit('Decompressed bytes or SHA-256 differs from the recorded raw source')
receipt = {'run': manifest.get('run'), 'verified_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
           'archive_path': str(args.archive), 'archive_sha256': compressed_hash,
           'archive_bytes': args.archive.stat().st_size, 'raw_path': str(args.output_raw),
           'raw_sha256': raw_hash.hexdigest(), 'raw_bytes': raw_bytes,
           'gzip_integrity_verified': True, 'source_hashes_match': True}
args.receipt.write_text(json.dumps(receipt, indent=2) + '\n', encoding='utf-8')
print(json.dumps(receipt))
