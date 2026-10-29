"""Verify a transferred gzip evidence archive against both source hashes."""
import argparse
import datetime
import gzip
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('archive', type=Path)
parser.add_argument('--raw-sha256', required=True)
parser.add_argument('--compressed-sha256', required=True)
parser.add_argument('--raw-size', required=True, type=int)
parser.add_argument('--output', required=True, type=Path)
args = parser.parse_args()
with args.archive.open('rb') as archive_stream:
    compressed = hashlib.file_digest(archive_stream, 'sha256').hexdigest()
digest = hashlib.sha256()
size = 0
with gzip.open(args.archive, 'rb') as stream:
    while chunk := stream.read(8*1024*1024):
        digest.update(chunk)
        size += len(chunk)
raw = digest.hexdigest()
if raw != args.raw_sha256.lower():
    raise RuntimeError('Decompressed SHA-256 mismatch')
if compressed != args.compressed_sha256.lower():
    raise RuntimeError('Compressed SHA-256 mismatch')
if size != args.raw_size:
    raise RuntimeError('Decompressed size mismatch')
receipt = {'archive': str(args.archive.resolve()),
           'verified_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
           'raw_sha256': raw, 'compressed_sha256': compressed,
           'raw_bytes': size, 'compressed_bytes': args.archive.stat().st_size,
           'gzip_integrity_verified': True, 'source_hashes_match': True}
args.output.write_text(json.dumps(receipt, indent=2)+'\n')
print(json.dumps(receipt))
