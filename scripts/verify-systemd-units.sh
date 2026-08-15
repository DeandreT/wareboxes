#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v systemd-analyze >/dev/null 2>&1; then
  echo "systemd-analyze is required to verify deployment units" >&2
  exit 127
fi

unit_root="$(mktemp -d)"
cleanup() {
  rm -r "$unit_root"
}
trap cleanup EXIT

install -d \
  "$unit_root/etc/systemd/system" \
  "$unit_root/etc/wareboxes" \
  "$unit_root/usr/bin" \
  "$unit_root/usr/local/sbin" \
  "$unit_root/var/backups/wareboxes" \
  "$unit_root/var/cache/wareboxes"
install -m 0600 /dev/null "$unit_root/etc/wareboxes/backup.env"
install -m 0755 /usr/bin/true "$unit_root/usr/bin/true"
install -m 0755 \
  deploy/wareboxes-backup \
  deploy/wareboxes-archive-commands \
  deploy/wareboxes-restore-drill \
  "$unit_root/usr/local/sbin/"
install -m 0644 \
  deploy/wareboxes-backup.service \
  deploy/wareboxes-backup.timer \
  deploy/wareboxes-command-archive.service \
  deploy/wareboxes-command-archive.timer \
  deploy/wareboxes-restore-drill.service \
  deploy/wareboxes-restore-drill.timer \
  "$unit_root/etc/systemd/system/"

cat > "$unit_root/etc/systemd/system/docker.service" <<'EOF'
[Unit]
Description=Verification stub for Docker
DefaultDependencies=no
[Service]
Type=oneshot
ExecStart=/usr/bin/true
RemainAfterExit=yes
EOF
for unit in network-online.target timers.target sysinit.target basic.target shutdown.target; do
  printf '[Unit]\nDescription=Verification stub for %s\nDefaultDependencies=no\n' "$unit" \
    > "$unit_root/etc/systemd/system/$unit"
done

systemd-analyze verify \
  --recursive-errors=no \
  --root="$unit_root" \
  wareboxes-backup.service \
  wareboxes-backup.timer \
  wareboxes-command-archive.service \
  wareboxes-command-archive.timer \
  wareboxes-restore-drill.service \
  wareboxes-restore-drill.timer
