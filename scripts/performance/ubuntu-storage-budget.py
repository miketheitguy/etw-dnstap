"""Read live identities and validate the approved separate-volume budget."""
import argparse
import datetime
import json
import os
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))  # lab_config lives next to this script
import lab_config  # noqa: E402

parser = argparse.ArgumentParser()
parser.add_argument('--campaign-id', required=True, help='campaign whose archive base is performance-<id>')
parser.add_argument('--archive-filesystem-uuid', required=True, help='UUID of the (root) filesystem that holds the archives')
parser.add_argument('--forecast-raw-bytes', type=int, required=True)
parser.add_argument('--raw-budget-bytes', type=int, default=48*1024**3)
args = parser.parse_args()
if not re.fullmatch(r'[A-Za-z0-9_-]{1,64}', args.campaign_id):
    raise SystemExit('--campaign-id must be 1-64 letters, digits, _ or -')
base = Path(lab_config.get('ubuntu_vm.home')) / f'performance-{args.campaign_id}'
saved = json.loads((base/'temporary-data-disk.json').read_text())
mount = Path(saved['mountpoint'])
def filesystem(path):
    data = json.loads(subprocess.check_output(['findmnt', '-J', '-o', 'SOURCE,TARGET,UUID,FSTYPE,OPTIONS,MAJ:MIN', '--target', str(path)], text=True))['filesystems'][0]
    stat = os.statvfs(path)
    return dict(mountpoint=data['target'], filesystem_uuid=data['uuid'],
                device_path=data['source'], major_minor=data['maj:min'],
                available_bytes=stat.f_bavail*stat.f_frsize, mount_options=data['options'])
raw, archive = filesystem(mount), filesystem(base)
device = json.loads(subprocess.check_output(['lsblk', '-J', '-d', '-o', 'PATH,SERIAL,WWN,MAJ:MIN', saved['device_path']], text=True))['blockdevices'][0]
if (not mount.is_mount() or raw['filesystem_uuid'] != saved['filesystem_uuid']
        or raw['major_minor'] != saved['major_minor']
        or device['serial'] != saved['device_serial'] or device['wwn'] != saved['device_wwn']
        or raw['major_minor'] == archive['major_minor']
        or archive['filesystem_uuid'] != args.archive_filesystem_uuid):
    raise SystemExit('Live raw/archive volume identities do not match the approved layout')
for option in ('discard', 'commit=30'):
    if option not in raw['mount_options'].split(',') or option not in archive['mount_options'].split(','):
        raise SystemExit('Data mount must retain the original discard/commit policy')
if not 0 < args.forecast_raw_bytes <= args.raw_budget_bytes <= 48*1024**3:
    raise SystemExit('Raw forecast or cap exceeds approved bounds')
floor, safety = 3*1024**3, 1024**3
reserve = max((args.forecast_raw_bytes+3)//4, 1024**3)
raw['required_bytes'] = args.forecast_raw_bytes+floor+safety
archive['required_bytes'] = reserve+floor+safety
raw.update(device_serial=device['serial'], device_wwn=device['wwn'])
fits = raw['available_bytes'] >= raw['required_bytes'] and archive['available_bytes'] >= archive['required_bytes']
if not fits:
    raise SystemExit('Separate-volume forecast and safety reserves do not fit')
print(json.dumps(dict(measured_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
    storage_protocol='separate_raw_data_disk_and_root_archives', raw_volume=raw,
    archive_volume=archive, forecast_raw_bytes=args.forecast_raw_bytes,
    raw_budget_bytes=args.raw_budget_bytes, archive_compression_reserve_bytes=reserve,
    min_free_bytes=floor, safety_bytes=safety, separate_volume_reserve_fits=True,
    archive_double_footprint_fits=False, storage_receipt='ubuntu/temporary-data-disk.json',
    collector_output_diff='ubuntu/collector-data-path.diff'), indent=2))
