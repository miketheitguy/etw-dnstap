#!/bin/sh
set -eu
# Run from ~/performance with sudo. Owns only this fixture and the saved collector settings.
test "$(hostname)" = dnstap-ubuntu2604
base=${DNSTAP_PERF_BASE:-/home/dnstaplab/performance}
log=${DNSTAP_PERF_LOG:-/var/log/dnscollector/performance.jsonl}
export DNSTAP_PERF_BASE="$base" DNSTAP_PERF_LOG="$log"
mkdir -p "$base/backup" /var/log/dnscollector /etc/systemd/system/dnscollector.service.d
chmod 0700 "$base/backup"
test ! -e "$base/backup/config.yml" || { echo 'Backup already exists; refuse overwrite'; exit 1; }
test ! -e /etc/systemd/system/dnscollector.service.d/performance.conf
test ! -e "$log"
cp -a /etc/dnscollector/config.yml "$base/backup/config.yml"
systemctl cat dnscollector > "$base/backup/dnscollector-unit.txt"
cp /proc/net/snmp "$base/backup/snmp.txt"
df -B1 /var/log/dnscollector > "$base/backup/disk.txt"
test "$(df --output=avail -B1 /var/log/dnscollector | tail -1)" -gt 27000000000
python3 - <<'PY'
from pathlib import Path
import os
base = Path(os.environ['DNSTAP_PERF_BASE'])
p=Path('/etc/dnscollector/config.yml')
s=p.read_text().replace('global:\n','global:\n  worker:\n    interval-monitor: 1\n  telemetry:\n    enabled: true\n    web-listen: "127.0.0.1:9165"\n    web-path: "/metrics"\n',1)
p.write_text(s)
names=(base/'rec-names.txt').read_text().splitlines()
assert len(names)==5000 and all(n.endswith('.rec.stress.test') for n in names)
config='''port=53
listen-address=172.27.215.60
bind-interfaces
no-resolv
no-hosts
cache-size=0
local-ttl=0
auth-ttl=0
auth-server=ns.rec.stress.test,172.27.215.60
auth-zone=rec.stress.test
user=nobody
group=nogroup
log-facility=-
'''
config+=''.join(f'host-record={n},192.0.2.53,0\n' for n in names)
(base/'dnsmasq.conf').write_text(config)
PY
cat > /etc/systemd/system/dnscollector.service.d/performance.conf <<EOF
[Service]
StandardOutput=append:$log
StandardError=append:$log.errors.log
EOF
cat > "$base/performance-filter.nft" <<'EOF'
table inet dnstap_performance {
 chain input {
  type filter hook input priority filter; policy accept;
  ip daddr 172.27.215.60 udp dport 53 ip saddr 172.27.215.163 counter accept comment "upstream_queries"
  ip daddr 172.27.215.60 tcp dport 53 ip saddr 172.27.215.163 counter accept
  ip daddr 172.27.215.60 udp dport 53 counter drop
  ip daddr 172.27.215.60 tcp dport 53 counter drop
 }
 chain output {
  type filter hook output priority filter; policy accept;
  ip saddr 172.27.215.60 udp sport 53 ip daddr 172.27.215.163 counter comment "upstream_responses"
 }
}
EOF
nft --check -f "$base/performance-filter.nft"
nft -f "$base/performance-filter.nft"
dnsmasq --test --conf-file="$base/dnsmasq.conf"
systemctl daemon-reload
systemctl restart dnscollector
systemd-run --unit=dnstap-performance-upstream --property=Restart=no -- /usr/sbin/dnsmasq --keep-in-foreground --conf-file="$base/dnsmasq.conf"
sleep 2
systemctl is-active dnscollector dnstap-performance-upstream
curl -fsS http://127.0.0.1:9165/metrics > "$base/initial-metrics.txt"
systemd-run --unit=dnstap-performance-monitor --property=Restart=no -- /usr/bin/env DNSTAP_PERF_BASE="$base" DNSTAP_PERF_LOG="$log" /usr/bin/python3 "$base/monitor-ubuntu-workload.py"
ss -lntup
