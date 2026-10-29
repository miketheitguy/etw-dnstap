#!/bin/sh
set -eu
test "$(hostname)" = dnstap-ubuntu2604
base=${DNSTAP_PERF_BASE:-/home/dnstaplab/performance}
export DNSTAP_PERF_BASE="$base"
test -s "$base/backup/config.yml"
systemctl stop dnstap-performance-monitor.service 2>/dev/null || true
systemctl stop dnstap-performance-upstream.service 2>/dev/null || true
if test -e "$base/backup/rmem-original.json"; then
    python3 "$base/tune-upstream-buffer.py" --restore
fi
if nft list table inet dnstap_performance >/dev/null 2>&1; then nft delete table inet dnstap_performance; fi
cp -a "$base/backup/config.yml" /etc/dnscollector/config.yml
rm -f /etc/systemd/system/dnscollector.service.d/performance.conf
systemctl daemon-reload
systemctl restart dnscollector
systemctl is-active dnscollector
systemctl show dnscollector -p StandardOutput -p StandardError
sha256sum /etc/dnscollector/config.yml "$base/backup/config.yml"
