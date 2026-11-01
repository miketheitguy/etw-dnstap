#!/usr/bin/env bash
# Prepare the Ubuntu receiver/upstream fixture for a performance run (lab VM only).
#
# Run on the Ubuntu guest as root from the directory that holds the copied lab files:
#   sudo ./setup-ubuntu-workload.sh
# Required next to this script: lab.json, lab_config.py, monitor-ubuntu-workload.py and
# $DNSTAP_PERF_BASE/rec-names.txt (5000 names ending in .rec.stress.test).
#
# Security: everything root executes or loads (the monitor script, the dnsmasq
# config and the nft ruleset) is installed into the root-owned directory
# LIB_DIR (default /usr/local/lib/etw-dnstap-lab, dirs 0755, files 0644/0755)
# first. Root never runs or loads files from the user-writable home directory.
#
# Idempotency: the script refuses to run twice (existing backup, drop-in, log or
# running units abort it) so original collector settings are never overwritten.
# Undo with restore-ubuntu-workload.sh.
set -euo pipefail

if [[ ${EUID} -ne 0 ]]; then
    echo 'This script must run as root (use sudo).' >&2
    exit 1
fi

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
config_file=${ETW_DNSTAP_LAB_CONFIG:-$here/lab.json}
[[ -r $config_file ]] || { echo "Lab config not found: $config_file (copy scripts/performance/lab.json next to this script)" >&2; exit 1; }

# Read DATA ONLY from lab.json (no code from the user-writable directory is executed).
lab() {
    python3 -c 'import json,sys
node=json.load(open(sys.argv[1],encoding="utf-8-sig"))
for part in sys.argv[2].split("."): node=node[part]
print(node)' "$config_file" "$1"
}
valid_ip() { [[ $1 =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]]; }

expected_host=$(lab ubuntu_vm.hostname)
receiver_ip=$(lab network.receiver_ip)
dns_ip=$(lab network.dns_ip)
lib_dir=${LIB_DIR:-$(lab ubuntu_vm.lab_lib_dir)}
rec_zone=$(lab dns.rec_zone)
valid_ip "$receiver_ip" && valid_ip "$dns_ip" || { echo 'Invalid IP address in lab.json' >&2; exit 1; }
[[ $rec_zone =~ ^[a-z0-9.-]+$ ]] || { echo 'Invalid zone name in lab.json' >&2; exit 1; }

[[ "$(hostname)" == "$expected_host" ]] || { echo "Wrong guest: expected $expected_host" >&2; exit 1; }
base=${DNSTAP_PERF_BASE:-$(lab ubuntu_vm.home)/performance}
log=${DNSTAP_PERF_LOG:-/var/log/dnscollector/performance.jsonl}
export DNSTAP_PERF_BASE="$base" DNSTAP_PERF_LOG="$log"
dropin=/etc/systemd/system/dnscollector.service.d/performance.conf

# Idempotent guards: refuse to overwrite earlier state.
[[ ! -e $base/backup/config.yml ]] || { echo 'Backup already exists; refuse overwrite' >&2; exit 1; }
[[ ! -e $dropin ]] || { echo "$dropin already exists; run restore-ubuntu-workload.sh first" >&2; exit 1; }
[[ ! -e $log ]] || { echo "$log already exists; refuse overwrite" >&2; exit 1; }
for unit in dnstap-performance-upstream dnstap-performance-monitor; do
    if systemctl is-active --quiet "$unit"; then echo "$unit is already running" >&2; exit 1; fi
done
for required in "$base/rec-names.txt" "$here/monitor-ubuntu-workload.py" "$here/lab_config.py" /etc/dnscollector/config.yml; do
    [[ -r $required ]] || { echo "Missing required file: $required" >&2; exit 1; }
done

install -d -m 0755 -o root -g root "$lib_dir" /var/log/dnscollector /etc/systemd/system/dnscollector.service.d
install -d -m 0700 -o root -g root "$base/backup"

[[ "$(df --output=avail -B1 /var/log/dnscollector | tail -1)" -gt 27000000000 ]] || { echo 'Less than 27 GB free on /var/log/dnscollector' >&2; exit 1; }
cp -a /etc/dnscollector/config.yml "$base/backup/config.yml"
systemctl cat dnscollector > "$base/backup/dnscollector-unit.txt"
cp /proc/net/snmp "$base/backup/snmp.txt"
df -B1 /var/log/dnscollector > "$base/backup/disk.txt"

# Root-owned copies of everything root will execute or load.
install -m 0755 -o root -g root "$here/monitor-ubuntu-workload.py" "$lib_dir/monitor-ubuntu-workload.py"
# lab.json + lab_config.py: the lib-dir scripts read their constants from these root-owned copies.
install -m 0644 -o root -g root "$config_file" "$lib_dir/lab.json"
install -m 0644 -o root -g root "$here/lab_config.py" "$lib_dir/lab_config.py"
if [[ -r $here/tune-upstream-buffer.py ]]; then
    install -m 0755 -o root -g root "$here/tune-upstream-buffer.py" "$lib_dir/tune-upstream-buffer.py"
fi

# Collector telemetry + upstream dnsmasq config (written into the root-owned dir).
LIB_DIR=$lib_dir RECEIVER_IP=$receiver_ip REC_ZONE=$rec_zone python3 - <<'PY'
import os
from pathlib import Path

lib = Path(os.environ['LIB_DIR'])
base = Path(os.environ['DNSTAP_PERF_BASE'])
receiver, zone = os.environ['RECEIVER_IP'], os.environ['REC_ZONE']
p = Path('/etc/dnscollector/config.yml')
text = p.read_text()
patched = text.replace('global:\n', 'global:\n  worker:\n    interval-monitor: 1\n  telemetry:\n    enabled: true\n    web-listen: "127.0.0.1:9165"\n    web-path: "/metrics"\n', 1)
if patched == text:
    raise SystemExit('collector config has no top-level "global:" section to patch')
p.write_text(patched)
names = (base / 'rec-names.txt').read_text().splitlines()
assert len(names) == 5000 and all(n.endswith('.' + zone) for n in names), 'rec-names.txt must hold 5000 names in ' + zone
config = f'''port=53
listen-address={receiver}
bind-interfaces
no-resolv
no-hosts
cache-size=0
local-ttl=0
auth-ttl=0
auth-server=ns.{zone},{receiver}
auth-zone={zone}
user=nobody
group=nogroup
log-facility=-
'''
config += ''.join(f'host-record={n},192.0.2.53,0\n' for n in names)
(lib / 'dnsmasq.conf').write_text(config)
PY
chmod 0644 "$lib_dir/dnsmasq.conf"
# Evidence copy for the campaign directory (install replaces, never follows, an existing destination).
install -m 0644 "$lib_dir/dnsmasq.conf" "$base/dnsmasq.conf"

cat > "$dropin" <<EOF
[Service]
StandardOutput=append:$log
StandardError=append:$log.errors.log
EOF
cat > "$lib_dir/performance-filter.nft" <<EOF
table inet dnstap_performance {
 chain input {
  type filter hook input priority filter; policy accept;
  ip daddr $receiver_ip udp dport 53 ip saddr $dns_ip counter accept comment "upstream_queries"
  ip daddr $receiver_ip tcp dport 53 ip saddr $dns_ip counter accept
  ip daddr $receiver_ip udp dport 53 counter drop
  ip daddr $receiver_ip tcp dport 53 counter drop
 }
 chain output {
  type filter hook output priority filter; policy accept;
  ip saddr $receiver_ip udp sport 53 ip daddr $dns_ip counter comment "upstream_responses"
 }
}
EOF
chmod 0644 "$lib_dir/performance-filter.nft"
install -m 0644 "$lib_dir/performance-filter.nft" "$base/performance-filter.nft"
nft --check -f "$lib_dir/performance-filter.nft"
dnsmasq --test --conf-file="$lib_dir/dnsmasq.conf"
nft -f "$lib_dir/performance-filter.nft"
systemctl daemon-reload
systemctl restart dnscollector
systemd-run --unit=dnstap-performance-upstream --property=Restart=no -- /usr/sbin/dnsmasq --keep-in-foreground --conf-file="$lib_dir/dnsmasq.conf"
sleep 2
systemctl is-active dnscollector dnstap-performance-upstream
curl -fsS http://127.0.0.1:9165/metrics > "$base/initial-metrics.txt"
systemd-run --unit=dnstap-performance-monitor --property=Restart=no -- /usr/bin/env DNSTAP_PERF_BASE="$base" DNSTAP_PERF_LOG="$log" /usr/bin/python3 "$lib_dir/monitor-ubuntu-workload.py"
ss -lntup
