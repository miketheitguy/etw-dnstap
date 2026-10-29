"""Initialize only the unique, blank disk added for this authorized campaign."""
import datetime
import difflib
import hashlib
import json
import os
from pathlib import Path
import subprocess

base = Path('/home/dnstaplab/performance-pooled-vm-20261029-0942')
mount = Path('/mnt/dnstap-pooled-vm-20261029-0942')
expected_os_wwn = '0x60022480fcfd8f402fe294d098aa536d'
expected_vm = 'd5ca1bde-da5c-4c08-9625-e845c1443b0b'
run = lambda *args: subprocess.check_output(args, text=True).strip()
if run('hostname') != 'dnstap-ubuntu2604':
    raise SystemExit('Wrong guest')
if subprocess.run(['systemctl', 'is-active', 'dnscollector'], capture_output=True, text=True).stdout.strip() != 'inactive':
    raise SystemExit('Collector must be stopped')
if subprocess.run(['systemctl', 'is-active', 'dnstap-performance-monitor'], capture_output=True, text=True).stdout.strip() != 'inactive':
    raise SystemExit('Monitor must be stopped')
receipt_path = base / 'temporary-data-disk.json'
if receipt_path.exists() or mount.exists():
    raise SystemExit('Storage receipt or mount directory already exists')
host = json.loads((base / 'temporary-data-disk-host.json').read_text(encoding='utf-8-sig'))
if (host['vm_guid'] != expected_vm or host['virtual_size_bytes'] != 64*1024**3
        or host['controller_location'] != 2 or host['controller_number'] != 0
        or host['vhdx_path'] != r'D:\VMs\DNSTap-Ubuntu2604\pooled-vm-20261029-0942-data.vhdx'):
    raise SystemExit('Host attachment receipt does not match the approved disk')
before = json.loads((base / 'storage-block-before.json').read_text(encoding='utf-8-sig'))
after = json.loads(run('lsblk', '-J', '-b', '-o', 'NAME,KNAME,PATH,SIZE,TYPE,FSTYPE,MOUNTPOINTS,SERIAL,WWN,UUID,HCTL,MAJ:MIN'))
(base / 'storage-block-after-attach.json').write_text(json.dumps(after, indent=2)+'\n')
old_disks = [d for d in before['blockdevices'] if d['type'] == 'disk']
new_disks = [d for d in after['blockdevices'] if d['type'] == 'disk' and d['path'] not in {p['path'] for p in old_disks}]
old_os = [d for d in old_disks if d.get('wwn') == expected_os_wwn]
if len(old_os) != 1 or len(new_disks) != 1:
    raise SystemExit('Expected exactly one known OS disk and one newly attached disk')
disk = new_disks[0]
os_hctl = old_os[0]['hctl']
if (disk['size'] != 64*1024**3 or disk.get('fstype') or disk.get('children')
        or any(disk.get('mountpoints', [])) or not disk.get('serial') or not disk.get('wwn')
        or disk['wwn'] in {d.get('wwn') for d in old_disks}
        or disk['serial'] in {d.get('serial') for d in old_disks}
        or disk.get('hctl') != os_hctl.rsplit(':', 1)[0]+':2'):
    raise SystemExit('New disk identity, HCTL, size or blank-state check failed')
device = Path('/dev/disk/by-id') / ('wwn-' + disk['wwn'])
if device.resolve() != Path(disk['path']).resolve():
    raise SystemExit('Persistent disk identity does not resolve to the new device')
signature = run('wipefs', '--no-act', str(device))
if signature:
    raise SystemExit('The new disk has existing signatures; refuse formatting')
started = datetime.datetime.now(datetime.timezone.utc).isoformat()
command = ['mkfs.ext4', '-m', '0', '-L', 'dnstap-pooled-data', '-E', 'lazy_itable_init=0,lazy_journal_init=0', str(device)]
formatted = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True, text=True, check=True)
(base / 'storage-mkfs.txt').write_text(formatted.stdout+formatted.stderr)
checked = subprocess.run(['e2fsck', '-f', '-n', str(device)], stdin=subprocess.DEVNULL, capture_output=True, text=True, check=True)
(base / 'storage-fsck.txt').write_text(checked.stdout+checked.stderr)
filesystem_uuid = run('blkid', '-s', 'UUID', '-o', 'value', str(device))
mount.mkdir(mode=0o755)
options = 'nodev,nosuid,noexec,relatime,discard,commit=30'
subprocess.run(['mount', '-o', options, str(device), str(mount)], check=True)
mounted = json.loads(run('findmnt', '-J', '-o', 'SOURCE,TARGET,UUID,OPTIONS,FSTYPE,MAJ:MIN', '--target', str(mount)))['filesystems'][0]
if mounted['target'] != str(mount) or mounted['uuid'] != filesystem_uuid or mounted['fstype'] != 'ext4':
    raise SystemExit('Mounted filesystem identity differs from the freshly initialized disk')
config = Path('/etc/dnscollector/config.yml')
dropin = Path('/etc/systemd/system/dnscollector.service.d/performance.conf')
old = dropin.read_text()
old_log = '/var/log/dnscollector/performance-pooled-vm-20261029-0942.jsonl'
new_log = str(mount / 'performance-pooled-vm-20261029-0942.jsonl')
expected = f'StandardOutput=append:{old_log}'
if old.splitlines().count(expected) != 1:
    raise SystemExit('Collector output override does not match the owned root-disk fixture')
before_hash = hashlib.sha256(config.read_bytes()).hexdigest()
(base / 'pre-data-disk-config.yml').write_bytes(config.read_bytes())
(base / 'pre-data-disk-performance.conf').write_text(old)
new = old.replace(expected+'\n', f'StandardOutput=append:{new_log}\n', 1)
diff = ''.join(difflib.unified_diff(old.splitlines(True), new.splitlines(True), fromfile='before/performance.conf', tofile='after/performance.conf'))
(base / 'collector-data-path.diff').write_text(diff)
dropin.write_text(new)
subprocess.run(['systemctl', 'daemon-reload'], check=True)
after_hash = hashlib.sha256(config.read_bytes()).hexdigest()
if before_hash != after_hash:
    raise SystemExit('Collector YAML changed unexpectedly')
receipt = dict(created_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
    initialization_started_utc=started, vm_guid=expected_vm, host_attachment=host,
    vhdx_path=host['vhdx_path'], device_path=disk['path'], persistent_device_path=str(device),
    device_serial=disk['serial'], device_wwn=disk['wwn'], device_hctl=disk['hctl'],
    major_minor=disk['maj:min'], filesystem_uuid=filesystem_uuid, mountpoint=str(mount),
    mount=mounted, mkfs_command=command, mkfs_exit_code=formatted.returncode,
    fsck_exit_code=checked.returncode, lazy_initialization_disabled=True,
    raw_path=new_log, prior_raw_path=old_log, archive_base=str(base),
    collector_yaml_sha256_before=before_hash, collector_yaml_sha256_after=after_hash,
    collector_output_diff='collector-data-path.diff', cpu_ram_network_changed=False,
    raw_cap_bytes=48*1024**3, min_free_bytes=3*1024**3,
    raw_volume_available_bytes=os.statvfs(mount).f_bavail*os.statvfs(mount).f_frsize,
    archive_volume_available_bytes=os.statvfs(base).f_bavail*os.statvfs(base).f_frsize)
with receipt_path.open('x') as out:
    out.write(json.dumps(receipt, indent=2)+'\n')
print(json.dumps(receipt, indent=2))
