#!/usr/bin/env bash
# Undo setup-ubuntu-workload.sh: stop the lab units, remove the nft table and the
# collector drop-in, and restore the saved collector config. Run as root.
#
# Idempotent: stopping absent units and deleting an absent table are no-ops.
# Root-owned copies under LIB_DIR are removed only after the units are stopped.
set -euo pipefail

if [[ ${EUID} -ne 0 ]]; then
    echo 'This script must run as root (use sudo).' >&2
    exit 1
fi

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
config_file=${ETW_DNSTAP_LAB_CONFIG:-$here/lab.json}
[[ -r $config_file ]] || { echo "Lab config not found: $config_file" >&2; exit 1; }
lab() {
    python3 -c 'import json,sys
node=json.load(open(sys.argv[1],encoding="utf-8-sig"))
for part in sys.argv[2].split("."): node=node[part]
print(node)' "$config_file" "$1"
}

expected_host=$(lab ubuntu_vm.hostname)
[[ "$(hostname)" == "$expected_host" ]] || { echo "Wrong guest: expected $expected_host" >&2; exit 1; }
lib_dir=${LIB_DIR:-$(lab ubuntu_vm.lab_lib_dir)}
base=${DNSTAP_PERF_BASE:-$(lab ubuntu_vm.home)/performance}
export DNSTAP_PERF_BASE="$base"
[[ -s $base/backup/config.yml ]] || { echo "No saved collector config at $base/backup/config.yml; nothing to restore" >&2; exit 1; }

systemctl stop dnstap-performance-monitor.service 2>/dev/null || true
systemctl stop dnstap-performance-upstream.service 2>/dev/null || true
if [[ -e $base/backup/rmem-original.json ]]; then
    # Runs the root-owned copy if present, otherwise refuses to execute user-writable code.
    [[ -x $lib_dir/tune-upstream-buffer.py ]] || { echo "Install tune-upstream-buffer.py into $lib_dir (root-owned) to restore the socket buffer" >&2; exit 1; }
    python3 "$lib_dir/tune-upstream-buffer.py" --restore
fi
if nft list table inet dnstap_performance >/dev/null 2>&1; then nft delete table inet dnstap_performance; fi
cp -a "$base/backup/config.yml" /etc/dnscollector/config.yml
rm -f /etc/systemd/system/dnscollector.service.d/performance.conf
systemctl daemon-reload
systemctl restart dnscollector
systemctl is-active dnscollector
systemctl show dnscollector -p StandardOutput -p StandardError
sha256sum /etc/dnscollector/config.yml "$base/backup/config.yml"
