"""Unmount the exact temporary data filesystem after actual idle/FD checks."""
import datetime
import json
from pathlib import Path
import subprocess

base = Path('/home/dnstaplab/performance-pooled-vm-20261029-0942')
saved = json.loads((base/'temporary-data-disk.json').read_text())
mount = Path('/mnt/dnstap-pooled-vm-20261029-0942')
receipt = base/'temporary-data-disk-unmounted.json'
if subprocess.check_output(['hostname'], text=True).strip() != 'dnstap-ubuntu2604' or receipt.exists():
    raise SystemExit('Wrong guest or prior unmount receipt')
states = {unit: subprocess.run(['systemctl', 'is-active', unit], capture_output=True, text=True).stdout.strip()
          for unit in ('dnscollector', 'dnstap-performance-monitor')}
if any(state != 'inactive' for state in states.values()) or Path(saved['raw_path']).exists():
    raise SystemExit('Collector/monitor must be stopped and verified raw file removed')
before = json.loads(subprocess.check_output(['findmnt', '-J', '-o', 'SOURCE,TARGET,UUID,FSTYPE,OPTIONS,MAJ:MIN', '--target', str(mount)], text=True))['filesystems'][0]
if (not mount.is_mount() or before['target'] != str(mount)
        or before['uuid'] != saved['filesystem_uuid'] or before['maj:min'] != saved['major_minor']):
    raise SystemExit('Temporary mount identity changed')
opened = []
for process in Path('/proc').glob('[0-9]*'):
    for link in [process/'cwd', *list((process/'fd').glob('*'))]:
        try:
            target = str(link.readlink())
        except FileNotFoundError:
            continue
        except PermissionError as exc:
            raise SystemExit(f'Cannot verify open descriptors: {link}') from exc
        if target == str(mount) or target.startswith(str(mount)+'/'):
            opened.append(dict(pid=int(process.name), descriptor=link.name, target=target))
if opened:
    raise SystemExit('The temporary filesystem still has open descriptors: '+json.dumps(opened))
started = datetime.datetime.now(datetime.timezone.utc).isoformat()
result = subprocess.run(['umount', str(mount)], capture_output=True, text=True, check=True)
remaining = subprocess.run(['findmnt', '-r', '-n', '-S', 'UUID='+saved['filesystem_uuid']], capture_output=True, text=True)
if mount.is_mount() or remaining.stdout.strip():
    raise SystemExit('Temporary filesystem remains mounted')
proof = dict(unmounted=True, unmounted_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
    unmount_started_utc=started, vm_guid=saved['vm_guid'], vhdx_path=saved['vhdx_path'],
    filesystem_uuid=saved['filesystem_uuid'], mountpoint=str(mount), mount_before=before,
    states=states, raw_file_absent=True, open_fd_matches=opened,
    umount_exit_code=result.returncode, no_remaining_mount_for_uuid=True,
    vhdx_should_be_retained=True)
with receipt.open('x') as out:
    out.write(json.dumps(proof, indent=2)+'\n')
print(json.dumps(proof, indent=2))
