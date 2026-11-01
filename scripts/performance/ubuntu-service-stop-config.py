"""Back up and atomically toggle only pipelines[name=tap].dnstap.reset-conn."""
import argparse
import copy
import datetime
import hashlib
import json
import os
from pathlib import Path
import pwd
import stat
import tempfile

import yaml

CONFIG = Path('/etc/dnscollector/config.yml')
DEFAULT_CAMPAIGN = 'service-stop-20261030-0633'
BASE = Path('/home/dnstaplab') / DEFAULT_CAMPAIGN
BACKUP = BASE / 'backup' / 'config.original.yml'
BACKUP_RECEIPT = BASE / 'backup' / 'config-original-receipt.json'
CHANGE_RECEIPT = BASE / 'config-reset-conn-false-receipt.json'
BASELINE_SHA256 = '5d71afe6f46d8f81802ae592a97c8be2582891b52c5624405c6ef64120065935'


def require_campaign(args) -> None:
    if args.campaign_id != DEFAULT_CAMPAIGN:
        raise RuntimeError('This helper is scoped to its fixed campaign directory')


def sha_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def target_dnstap(document: dict) -> dict:
    pipelines = document.get('pipelines')
    if not isinstance(pipelines, list):
        raise RuntimeError('Expected pipelines to be a YAML sequence')
    matches = [item for item in pipelines if isinstance(item, dict) and item.get('name') == 'tap']
    if len(matches) != 1:
        raise RuntimeError('Expected exactly one pipeline named tap')
    dnstap = matches[0].get('dnstap')
    if not isinstance(dnstap, dict):
        raise RuntimeError('pipelines[name=tap].dnstap is not a mapping')
    return dnstap


def config_stat(path: Path) -> dict:
    info = path.stat(follow_symlinks=False)
    if not stat.S_ISREG(info.st_mode) or path.is_symlink():
        raise RuntimeError('Collector configuration must be a regular, non-symlink file')
    return {'uid': info.st_uid, 'gid': info.st_gid, 'mode': stat.S_IMODE(info.st_mode),
            'bytes': info.st_size, 'mtime_ns': info.st_mtime_ns}


def ensure_campaign_dir() -> None:
    account = pwd.getpwnam('dnstaplab')
    if BASE.is_symlink():
        raise RuntimeError('Campaign evidence directory must not be a symlink')
    BASE.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chown(BASE, account.pw_uid, account.pw_gid)
    os.chmod(BASE, 0o700)
    backup_dir = BASE / 'backup'
    if backup_dir.is_symlink():
        raise RuntimeError('Campaign backup directory must not be a symlink')
    backup_dir.mkdir(mode=0o700, exist_ok=True)
    os.chown(backup_dir, account.pw_uid, account.pw_gid)
    os.chmod(backup_dir, 0o700)


def write_exclusive(path: Path, data: bytes, mode: int = 0o600) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    fd = os.open(path, flags, mode)
    with os.fdopen(fd, 'wb') as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def backup(args) -> None:
    require_campaign(args)
    ensure_campaign_dir()
    original = CONFIG.read_bytes()
    digest = sha_bytes(original)
    if digest != args.expected_original_sha256.lower():
        raise RuntimeError(f'Current config SHA-256 {digest} does not match the specified original')
    document = yaml.safe_load(original)
    dnstap = target_dnstap(document)
    if 'reset-conn' in dnstap:
        raise RuntimeError('Baseline must keep reset-conn omitted so the pinned default true applies')
    metadata = config_stat(CONFIG)
    if BACKUP.exists() or BACKUP.is_symlink() or BACKUP_RECEIPT.exists() or BACKUP_RECEIPT.is_symlink():
        raise RuntimeError('Refusing to overwrite an existing campaign config backup')
    write_exclusive(BACKUP, original)
    account = pwd.getpwnam('dnstaplab')
    os.chown(BACKUP, account.pw_uid, account.pw_gid)
    os.chmod(BACKUP, 0o600)
    receipt = {'campaign_id': args.campaign_id, 'created_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
               'config_path': str(CONFIG), 'backup_path': str(BACKUP), 'original_sha256': digest,
               'original_file': metadata, 'reset_conn_config_key_present': False,
               'reset_conn_effective_value': True,
               'effective_value_source': 'pinned collector-collectors.go ResetConn default true'}
    encoded = (json.dumps(receipt, indent=2) + '\n').encode()
    write_exclusive(BACKUP_RECEIPT, encoded)
    os.chown(BACKUP_RECEIPT, account.pw_uid, account.pw_gid)
    print(json.dumps(receipt))


def insert_reset_false(original: bytes) -> bytes:
    text = original.decode('utf-8')
    before = yaml.safe_load(text)
    dnstap = target_dnstap(before)
    if 'reset-conn' in dnstap:
        raise RuntimeError('reset-conn must be absent in the source config before this one-key edit')
    lines = text.splitlines(keepends=True)
    tap_items = [index for index, line in enumerate(lines) if line.rstrip('\r\n') == '  - name: tap']
    tap_maps = [index for index, line in enumerate(lines) if line.rstrip('\r\n') == '    dnstap:']
    if len(tap_items) != 1 or len(tap_maps) != 1 or tap_maps[0] <= tap_items[0]:
        raise RuntimeError('Could not identify a unique exact tap/dnstap YAML stanza')
    item_end = next((index for index in range(tap_items[0] + 1, len(lines))
                     if lines[index].startswith('  - name: ') or lines[index].startswith('  routing-policy:')),
                    len(lines))
    if tap_maps[0] >= item_end:
        raise RuntimeError('dnstap stanza is outside the tap pipeline')
    newline = '\r\n' if lines[tap_maps[0]].endswith('\r\n') else '\n'
    insertion = '      reset-conn: false' + newline
    insert_offset = sum(len(line.encode('utf-8')) for line in lines[:tap_maps[0] + 1])
    changed = original[:insert_offset] + insertion.encode('utf-8') + original[insert_offset:]
    after = yaml.safe_load(changed)
    expected = copy.deepcopy(before)
    target_dnstap(expected)['reset-conn'] = False
    if after != expected:
        raise RuntimeError('YAML semantic comparison found a change beyond reset-conn=false')
    if changed != original[:insert_offset] + insertion.encode('utf-8') + original[insert_offset:]:
        raise RuntimeError('Config edit changed bytes outside the inserted reset-conn line')
    return changed


def atomic_replace(path: Path, content: bytes, metadata: dict) -> None:
    fd, temp_name = tempfile.mkstemp(prefix='.config.yml.service-stop-', dir=path.parent)
    temp = Path(temp_name)
    try:
        os.fchown(fd, metadata['uid'], metadata['gid'])
        os.fchmod(fd, metadata['mode'])
        with os.fdopen(fd, 'wb') as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp, path)
        dir_fd = os.open(path.parent, os.O_RDONLY | getattr(os, 'O_DIRECTORY', 0))
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    except BaseException:
        try:
            os.close(fd)
        except OSError:
            pass
        temp.unlink(missing_ok=True)
        raise


def apply_false(args) -> None:
    require_campaign(args)
    if not BACKUP.is_file() or BACKUP.is_symlink() or not BACKUP_RECEIPT.is_file():
        raise RuntimeError('Protected original config backup is missing')
    original = CONFIG.read_bytes()
    baseline_hash = sha_bytes(original)
    backup_bytes = BACKUP.read_bytes()
    if (baseline_hash != args.expected_original_sha256.lower()
            or sha_bytes(backup_bytes) != baseline_hash or backup_bytes != original):
        raise RuntimeError('Current config and protected backup do not match the expected original')
    backup_receipt = json.loads(BACKUP_RECEIPT.read_text())
    if (backup_receipt.get('campaign_id') != args.campaign_id
            or backup_receipt.get('original_sha256') != baseline_hash):
        raise RuntimeError('Protected config backup receipt is inconsistent')
    if CHANGE_RECEIPT.exists() or CHANGE_RECEIPT.is_symlink():
        raise RuntimeError('Refusing to overwrite an existing reset-conn change receipt')
    metadata = config_stat(CONFIG)
    changed = insert_reset_false(original)
    expected_hash = args.expected_changed_sha256.lower()
    actual_hash = sha_bytes(changed)
    if expected_hash != 'auto' and actual_hash != expected_hash:
        raise RuntimeError(f'One-key config change hashes to {actual_hash}, not the supplied expected hash')
    atomic_replace(CONFIG, changed, metadata)
    after_stat = config_stat(CONFIG)
    after_bytes = CONFIG.read_bytes()
    if (sha_bytes(after_bytes) != actual_hash or after_bytes != changed
            or after_stat['uid'] != metadata['uid'] or after_stat['gid'] != metadata['gid']
            or after_stat['mode'] != metadata['mode']):
        raise RuntimeError('Atomic config replacement failed byte/hash/owner/mode verification')
    receipt = {'campaign_id': args.campaign_id, 'changed_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
               'config_path': str(CONFIG), 'original_sha256': baseline_hash,
               'changed_sha256': actual_hash, 'original_file': metadata, 'changed_file': after_stat,
               'changed_yaml_path': ['pipelines', 'name=tap', 'dnstap', 'reset-conn'],
               'old_effective_value': True, 'old_key_was_omitted': True,
               'new_explicit_value': False, 'atomic_replace': True,
               'semantic_diff_only_reset_conn': True}
    write_exclusive(CHANGE_RECEIPT, (json.dumps(receipt, indent=2) + '\n').encode())
    account = pwd.getpwnam('dnstaplab')
    os.chown(CHANGE_RECEIPT, account.pw_uid, account.pw_gid)
    print(json.dumps(receipt))


def rollback(args) -> None:
    require_campaign(args)
    if (not BACKUP.is_file() or BACKUP.is_symlink()
            or not BACKUP_RECEIPT.is_file() or BACKUP_RECEIPT.is_symlink()):
        raise RuntimeError('Protected original config backup is missing')
    receipt = json.loads(BACKUP_RECEIPT.read_text())
    current = CONFIG.read_bytes()
    current_hash = sha_bytes(current)
    backup = BACKUP.read_bytes()
    original_hash = sha_bytes(backup)
    if (not isinstance(receipt, dict) or receipt.get('campaign_id') != args.campaign_id
            or receipt.get('original_sha256') != original_hash
            or original_hash != args.expected_original_sha256.lower()):
        raise RuntimeError('Protected original config backup identity/hash mismatch')
    original_metadata = receipt.get('original_file')
    if (not isinstance(original_metadata, dict)
            or any(type(original_metadata.get(key)) is not int or original_metadata[key] < 0
                   for key in ('uid', 'gid', 'mode', 'bytes'))
            or original_metadata['mode'] > 0o7777
            or original_metadata['bytes'] != len(backup)
            or receipt.get('config_path') != str(CONFIG)
            or receipt.get('backup_path') != str(BACKUP)):
        raise RuntimeError('Protected original config backup metadata receipt is inconsistent')
    # CONFIG can already be replaced when a crash interrupts CHANGE_RECEIPT.
    # Derive the sole permitted edited state from the verified original bytes.
    expected_changed = insert_reset_false(backup)
    if current != expected_changed or current_hash != sha_bytes(expected_changed):
        raise RuntimeError('Current config is not the exact campaign reset-conn=false edit')
    if CHANGE_RECEIPT.is_symlink():
        raise RuntimeError('Change receipt must not be a symlink')
    changed_receipt = None
    if CHANGE_RECEIPT.exists():
        if not CHANGE_RECEIPT.is_file():
            raise RuntimeError('Change receipt must be a regular file')
        try:
            changed_receipt = json.loads(CHANGE_RECEIPT.read_text())
        except (json.JSONDecodeError, UnicodeDecodeError):
            # Missing or incomplete publication is recoverable only because
            # pinned backup and exact changed bytes independently prove identity.
            pass
        else:
            if (not isinstance(changed_receipt, dict)
                    or changed_receipt.get('campaign_id') != args.campaign_id
                    or changed_receipt.get('changed_sha256') != current_hash
                    or changed_receipt.get('original_sha256') != original_hash):
                raise RuntimeError('Change receipt contradicts the verified campaign edit')
    metadata = config_stat(CONFIG)
    if any(metadata[key] != original_metadata[key] for key in ('uid', 'gid', 'mode')):
        raise RuntimeError('Current config owner/mode differs from the protected original')
    atomic_replace(CONFIG, backup, metadata)
    if CONFIG.read_bytes() != backup or sha_bytes(CONFIG.read_bytes()) != original_hash:
        raise RuntimeError('Original config rollback failed byte/hash verification')
    rollback_receipt = {'campaign_id': args.campaign_id,
                        'restored_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                        'config_path': str(CONFIG), 'restored_original_sha256': original_hash,
                        'prior_changed_sha256': current_hash, 'atomic_replace': True}
    path = BASE / 'config-reset-conn-rollback-receipt.json'
    write_exclusive(path, (json.dumps(rollback_receipt, indent=2) + '\n').encode())
    account = pwd.getpwnam('dnstaplab')
    os.chown(path, account.pw_uid, account.pw_gid)
    print(json.dumps(rollback_receipt))


def verify(args) -> None:
    require_campaign(args)
    data = CONFIG.read_bytes()
    digest = sha_bytes(data)
    if digest != args.expected_sha256.lower():
        raise RuntimeError(f'Config SHA-256 {digest} did not match expected')
    dnstap = target_dnstap(yaml.safe_load(data))
    desired = args.reset_conn == 'false'
    if desired:
        if dnstap.get('reset-conn') is not False:
            raise RuntimeError('reset-conn is not explicitly false')
    elif dnstap.get('reset-conn', True) is not True:
        raise RuntimeError('reset-conn effective value is not true')
    print(json.dumps({'campaign_id': args.campaign_id, 'config_path': str(CONFIG),
                      'config_sha256': digest, 'reset_conn_explicit': 'reset-conn' in dnstap,
                      'reset_conn_effective': dnstap.get('reset-conn', True),
                      'verified_utc': datetime.datetime.now(datetime.timezone.utc).isoformat()}))


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--campaign-id', default=DEFAULT_CAMPAIGN)
sub = parser.add_subparsers(dest='command', required=True)
backup_parser = sub.add_parser('backup')
backup_parser.add_argument('--expected-original-sha256', default=BASELINE_SHA256)
apply_parser = sub.add_parser('apply-false')
apply_parser.add_argument('--expected-original-sha256', default=BASELINE_SHA256)
apply_parser.add_argument('--expected-changed-sha256', default='auto')
verify_parser = sub.add_parser('verify')
verify_parser.add_argument('--expected-sha256', required=True)
verify_parser.add_argument('--reset-conn', choices=('true', 'false'), required=True)
rollback_parser = sub.add_parser('rollback')
rollback_parser.add_argument('--expected-original-sha256', default=BASELINE_SHA256)
args = parser.parse_args()
if args.command == 'backup':
    backup(args)
elif args.command == 'apply-false':
    apply_false(args)
elif args.command == 'rollback':
    rollback(args)
else:
    verify(args)
